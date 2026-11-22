//! Windows Firewall and the inputs that receive from the network
//! (iFacialMocap, VR telemetry, input from another computer): whether OBS is let in, and a
//! way to let it in that asks Windows' permission (an administrator prompt).
//!
//! Windows allows each program by itself: another app being allowed doesn't let
//! OBS in, and a closed firewall prompt leaves a Block rule for OBS without a
//! word. The check (PowerShell, a couple of seconds, in the background) reads
//! OBS's inbound rules and the networks' kinds; letting OBS in turns its Block
//! rules off and adds an Allow rule for home (Private) networks: not Public
//! ones (a café's Wi-Fi), whose kind is said instead.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// What the check found
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// OBS's enabled inbound rules' profiles ("Private", "Any", "Private, Public"…)
    pub allow: Vec<String>,
    pub block: Vec<String>,
    /// The networks connected: (name, kind: Private, Public, DomainAuthenticated)
    pub networks: Vec<(String, String)>,
}

/// Whether a rule's profiles cover a network kind
fn covers(profiles: &str, kind: &str) -> bool {
    let kind = if kind.starts_with("Domain") { "Domain" } else { kind };
    profiles.contains("Any") || profiles.split(',').any(|p| p.trim().eq_ignore_ascii_case(kind))
}

/// The verdict: let in (on every network connected), and what to say
pub fn verdict(f: &Found) -> (&'static str, String) {
    if f.networks.is_empty() {
        return ("unknown", "Windows Firewall: no network connected".into());
    }
    let blocked: Vec<&(String, String)> = f.networks.iter().filter(|(_, k)| f.block.iter().any(|p| covers(p, k))).collect();
    if !blocked.is_empty() {
        return ("blocked", format!("Windows Firewall blocks OBS on {} (a Block rule: a firewall prompt closed or refused). Let OBS in with the button.", names(&blocked)));
    }
    let shut: Vec<&(String, String)> = f.networks.iter().filter(|(_, k)| !f.allow.iter().any(|p| covers(p, k))).collect();
    if shut.is_empty() {
        let kinds: Vec<String> = f.networks.iter().map(|(n, k)| format!("{n}: {k}")).collect();
        return ("allowed", format!("Windows Firewall lets OBS in ({})", kinds.join(", ")));
    }
    let public: Vec<&(String, String)> = shut.iter().copied().filter(|(_, k)| k == "Public").collect();
    if !public.is_empty() && f.allow.iter().any(|p| covers(p, "Private")) {
        return ("public", format!("Windows treats {} as a Public network, where OBS isn't let in: set it to Private (Settings → Network & internet → that network → Private network), as a home network is", names(&public)));
    }
    ("not_allowed", format!("Windows Firewall doesn't let OBS in on {}: phones and other computers can't reach Puppet. Let OBS in with the button.", names(&shut)))
}

fn names(nets: &[&(String, String)]) -> String {
    nets.iter().map(|(n, k)| format!("{n} ({k})")).collect::<Vec<_>>().join(", ")
}

/// The last check (when, what), whether one is running, and the last request's outcome
#[derive(Default)]
struct State {
    checked: Option<(Instant, Result<Found, String>)>,
    checking: bool,
    asked: Option<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// A check this old is done again (when the window asks)
const FRESH: Duration = Duration::from_secs(60);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(s.get_or_insert_with(State::default))
}

/// For the Configuration window: {state, text, can_fix}; a check starts in
/// the background when there's none fresh
pub fn status() -> Value {
    if !cfg!(windows) {
        return json!({ "state": "unsupported", "text": "", "can_fix": false });
    }
    let (stale, out) = with(|s| {
        let stale = !s.checking && s.checked.as_ref().is_none_or(|(at, _)| at.elapsed() > FRESH);
        let out = match &s.checked {
            None => json!({ "state": "checking", "text": "Windows Firewall: checking…", "can_fix": false }),
            Some((_, Err(e))) => json!({ "state": "unknown", "text": format!("Windows Firewall: couldn't check ({e})"), "can_fix": true }),
            Some((_, Ok(f))) => {
                let (state, text) = verdict(f);
                json!({ "state": state, "text": text, "can_fix": matches!(state, "blocked" | "not_allowed") })
            }
        };
        (stale, out)
    });
    if stale {
        check_now();
    }
    let mut out = out;
    if let Some(asked) = with(|s| s.asked.clone()) {
        out["asked"] = json!(asked);
    }
    out
}

/// Checks again, in the background
pub fn check_now() {
    if with(|s| std::mem::replace(&mut s.checking, true)) {
        return;
    }
    let _ = std::thread::Builder::new().name("theater-puppet-firewall".into()).spawn(|| {
        let result = run_check();
        with(|s| {
            s.checked = Some((Instant::now(), result));
            s.checking = false;
        });
    });
}

/// OBS itself (this program)
fn obs_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn check_script(program: &str) -> String {
    format!(
        "$p = {}; $rules = @(Get-NetFirewallApplicationFilter | Where-Object {{ [Environment]::ExpandEnvironmentVariables($_.Program) -eq $p }} | Get-NetFirewallRule | Where-Object {{ $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }}); \
         @{{ allow = @($rules | Where-Object {{ $_.Action -eq 'Allow' }} | ForEach-Object {{ $_.Profile.ToString() }}); block = @($rules | Where-Object {{ $_.Action -eq 'Block' }} | ForEach-Object {{ $_.Profile.ToString() }}); \
         nets = @(Get-NetConnectionProfile | ForEach-Object {{ @{{ name = $_.Name; category = $_.NetworkCategory.ToString() }} }}) }} | ConvertTo-Json -Compress -Depth 4",
        quoted(program)
    )
}

/// The fix, run with Windows' permission: OBS's inbound Block rules off, an
/// Allow rule for home (Private, and work-domain) networks, once
fn allow_script(program: &str) -> String {
    format!(
        "$p = {}; $mine = 'OBS Studio (Theater Puppet)'; \
         Get-NetFirewallApplicationFilter | Where-Object {{ [Environment]::ExpandEnvironmentVariables($_.Program) -eq $p }} | Get-NetFirewallRule | Where-Object {{ $_.Direction -eq 'Inbound' -and $_.Action -eq 'Block' -and $_.Enabled -eq 'True' }} | Disable-NetFirewallRule; \
         if (-not (Get-NetFirewallRule -DisplayName $mine -ErrorAction SilentlyContinue)) {{ New-NetFirewallRule -DisplayName $mine -Description 'Theater Puppet: phone and network input (iFacialMocap, Network)' -Direction Inbound -Action Allow -Program $p -Profile Private, Domain | Out-Null }}",
        quoted(program)
    )
}

/// PowerShell's -EncodedCommand: the script as base64 of UTF-16LE (no quoting to get wrong)
fn encoded(script: &str) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The check's JSON as what it found
pub fn parse(text: &str) -> Result<Found, String> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("unreadable answer: {e}"))?;
    let list = |k: &str| -> Vec<String> {
        match v.get(k) {
            Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    };
    let nets = match v.get("nets") {
        Some(Value::Array(a)) => a.clone(),
        Some(o @ Value::Object(_)) => vec![o.clone()],
        _ => Vec::new(),
    };
    let networks = nets
        .iter()
        .map(|n| (n.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string(), n.get("category").and_then(|x| x.as_str()).unwrap_or("?").to_string()))
        .collect();
    Ok(Found { allow: list("allow"), block: list("block"), networks })
}

#[cfg(windows)]
fn run_check() -> Result<Found, String> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded(&check_script(&obs_path()))])
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| format!("PowerShell: {e}"))?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(windows))]
fn run_check() -> Result<Found, String> {
    Err("only on Windows".into())
}

/// Lets OBS in: asks Windows' permission (its administrator prompt), then
/// checks again. Answers at once; the prompt is Windows'
pub fn allow() -> bool {
    #[cfg(windows)]
    {
        let script = allow_script(&obs_path());
        let _ = std::thread::Builder::new().name("theater-puppet-firewall-allow".into()).spawn(move || {
            #[link(name = "shell32")]
            unsafe extern "system" {
                fn ShellExecuteW(hwnd: *mut std::ffi::c_void, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
            }
            let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
            let params = format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand {}", encoded(&script));
            let r = unsafe { ShellExecuteW(std::ptr::null_mut(), wide("runas").as_ptr(), wide("powershell.exe").as_ptr(), wide(&params).as_ptr(), std::ptr::null(), 0) };
            // Above 32: started (the user said yes); else declined or failed
            let said = if r > 32 { "Windows' permission given: letting OBS in…" } else { "Windows' permission was declined: OBS isn't let in" };
            with(|s| s.asked = Some(said.to_string()));
            crate::log(&format!("[Puppet] 🛡 Firewall: {said}"));
            if r > 32 {
                // The fix runs by itself: checked again once it has
                for wait in [3, 6] {
                    std::thread::sleep(Duration::from_secs(wait));
                    check_now();
                }
            }
        });
        true
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(allow: &[&str], block: &[&str], nets: &[(&str, &str)]) -> Found {
        Found { allow: allow.iter().map(|s| s.to_string()).collect(), block: block.iter().map(|s| s.to_string()).collect(), networks: nets.iter().map(|(n, k)| (n.to_string(), k.to_string())).collect() }
    }

    #[test]
    fn the_verdict_says_what_stops_obs() {
        assert_eq!(verdict(&found(&["Private"], &[], &[("Home", "Private")])).0, "allowed");
        assert_eq!(verdict(&found(&["Any"], &[], &[("Home", "Public")])).0, "allowed");
        assert_eq!(verdict(&found(&["Private, Public"], &[], &[("Home", "Public")])).0, "allowed");
        // A closed prompt's Block rule wins over an Allow one
        assert_eq!(verdict(&found(&["Private"], &["Private"], &[("Home", "Private")])).0, "blocked");
        // Allowed at home, the network called Public: said, not "blocked"
        let (state, text) = verdict(&found(&["Private"], &[], &[("Home", "Public")]));
        assert_eq!(state, "public");
        assert!(text.contains("Home"), "{text}");
        assert_eq!(verdict(&found(&[], &[], &[("Home", "Private")])).0, "not_allowed");
        assert_eq!(verdict(&found(&["Domain"], &[], &[("Work", "DomainAuthenticated")])).0, "allowed");
    }

    #[test]
    fn the_check_s_answer_is_read() {
        // As PowerShell's ConvertTo-Json writes it (one network: an object, not a list)
        let f = parse(r#"{"allow":["Private","Any"],"nets":{"category":"Private","name":"mortimer"},"block":[]}"#).unwrap();
        assert_eq!(f, found(&["Private", "Any"], &[], &[("mortimer", "Private")]));
        let f = parse(r#"{"allow":"Private","nets":[{"category":"Public","name":"Cafe"},{"category":"Private","name":"Tailscale"}],"block":[]}"#).unwrap();
        assert_eq!(f.allow, vec!["Private".to_string()]);
        assert_eq!(f.networks.len(), 2);
    }

    /// The real check, on this computer (cargo test -- --ignored)
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn the_check_runs_here() {
        let f = run_check().unwrap();
        println!("found: {f:?}
verdict: {:?}", verdict(&f));
        assert!(!f.networks.is_empty());
    }

    #[test]
    fn scripts_are_encoded_for_powershell() {
        // "ab" as UTF-16LE (61 00 62 00), base64
        assert_eq!(encoded("ab"), "YQBiAA==");
        assert!(check_script("C:\\O'BS\\obs64.exe").contains("'C:\\O''BS\\obs64.exe'"));
        assert!(allow_script("C:\\obs64.exe").contains("-Profile Private, Domain"));
    }
}
