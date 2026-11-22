//! What the Puppet Tracking dock (theater-scripts/puppet_tracking_dock.rhai,
//! one of Theater Scene's Rhai docks) asks Puppet, through OBS's procedure
//! handler:
//!
//! `theater_puppet_tracking(in string args, out string json)`, `args` being
//! JSON with an `action`:
//! - `get` (or none): the state below;
//! - `set`: the settings given (`leap_height`, `leap_forward`, metres;
//!   `telemetry_scale`);
//! - `leap_centre`: the Leap's hands where they are now become hands resting
//!   in front of the avatar (sets `leap_height` and `leap_forward`);
//! - `telemetry_calibrate` (`now`: at once, else when both triggers are
//!   pressed), `telemetry_reset`: VR telemetry's T-pose calibration;
//! - `mediapipe_recentre`, `ifacialmocap_recentre`: where the head is now
//!   becomes the head at rest;
//! - `mediapipe_restart`: MediaPipe's helper started again.
//!
//! - `set_config` {values}: any tracking settings (config.rs's names, as
//!   tracking.json has them; an object, like `ifacialmocap`, keeps what
//!   `values` leaves out);
//! - `assign` {part, pick}: which input drives a part (`Arms`, `HeadTurn`…;
//!   `auto`, `off`, or an input: `IFacialMocap`, `Desk`…).
//!
//! It answers `{"leap", "leap_height", "leap_forward", "leap_status",
//! "leap_hands", "mediapipe", "mediapipe_status", "done", "config",
//! "statuses", "parts"}`, and with `lists: true` also `cameras` and
//! `controllers` ([name, id] each; asking OBS for cameras takes a moment).

use std::ffi::CStr;

use serde_json::{Value, json};

use super::{Source, config, leap, mediapipe, telemetry};
use crate::obs::{self, Ptr};
use crate::parts::{Calldata, get_string, set_string};

const DECL: &CStr = c"void theater_puppet_tracking(in string args, out string json)";

/// Registers the procedure. Call from obs_module_load.
pub fn register() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if !handler.is_null() {
            (api.proc_handler_add)(handler, DECL.as_ptr(), tracking_proc, std::ptr::null_mut());
        }
    }
}

unsafe extern "C" fn tracking_proc(_data: Ptr, cd: *mut Calldata) {
    let args = get_string(cd, c"args");
    let json = std::panic::catch_unwind(|| answer(&args)).unwrap_or_else(|_| "{}".into());
    set_string(cd, c"json", &json);
}

/// What the dock asked (`args`), done; the state after it, as JSON
pub fn answer(args: &str) -> String {
    let args: Value = serde_json::from_str(args).unwrap_or(json!({}));
    let number = |k: &str| args.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
    let done = match args.get("action").and_then(|a| a.as_str()).unwrap_or("get") {
        "set" => {
            let mut c = config::get();
            if let Some(v) = number("leap_height") {
                c.leap_height = v.clamp(-0.6, 0.6);
            }
            if let Some(v) = number("leap_forward") {
                c.leap_forward = v.clamp(-0.6, 0.6);
            }
            if let Some(v) = number("telemetry_scale") {
                c.telemetry_scale = v.clamp(0.2, 5.0);
            }
            config::set(c);
            true
        }
        "leap_centre" => leap::centre(),
        // Windows Firewall: check again, or let OBS in (Windows' administrator prompt)
        "firewall_check" => {
            super::firewall::check_now();
            true
        }
        "firewall_allow" => super::firewall::allow(),
        "network_new_code" => {
            let mut c = config::get();
            c.network.code = super::network::new_code();
            config::set(c);
            true
        }
        "mediapipe_recentre" => {
            mediapipe::recentre();
            true
        }
        "ifacialmocap_recentre" => {
            super::ifacialmocap::recentre();
            true
        }
        "telemetry_calibrate" => {
            telemetry::calibrate(args.get("now").and_then(|n| n.as_bool()).unwrap_or(false));
            true
        }
        "telemetry_reset" => {
            telemetry::reset_calibration();
            true
        }
        "mediapipe_restart" => {
            mediapipe::restart(&config::get());
            true
        }
        "set_config" => match args.get("values").and_then(|v| v.as_object()) {
            Some(values) => {
                let mut now = serde_json::to_value(config::get()).unwrap_or(json!({}));
                merge(&mut now, values);
                match serde_json::from_value::<config::Config>(now) {
                    Ok(mut c) => {
                        for phone in [&mut c.ifacialmocap, &mut c.telemetry] {
                            phone.ip = phone.ip.trim().to_string();
                        }
                        // Input from another computer: a pairing code once it's on
                        if c.network.enabled && c.network.code.trim().is_empty() {
                            c.network.code = super::network::new_code();
                        }
                        config::set(c);
                        true
                    }
                    Err(_) => false,
                }
            }
            None => false,
        },
        "assign" => {
            let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            match super::Part::ALL.into_iter().find(|p| format!("{p:?}") == text("part")) {
                Some(part) => {
                    let pick = match text("pick").as_str() {
                        "off" => super::Pick::Off,
                        v => Source::ALL.into_iter().find(|s| format!("{s:?}") == v).map(super::Pick::Only).unwrap_or(super::Pick::Auto),
                    };
                    let mut c = config::get();
                    c.assign.set(part, pick);
                    config::set(c);
                    true
                }
                None => false,
            }
        }
        _ => true,
    };
    let c = config::get();
    let statuses = super::statuses();
    // An input receiving what General gives to another: said at the end of its line
    let noted = |s: Source, text: String| match super::sidelined(s, &c.assign) {
        Some(note) if text.starts_with("receiving") => format!("{text}, {note}"),
        _ => text,
    };
    let status = |s: Source, on: bool| {
        if !on {
            return "off (turn it on in the Configuration window)".to_string();
        }
        noted(s, statuses.iter().find(|(src, _, _)| *src == s).map(|(_, _, t)| t.clone()).unwrap_or_else(|| "starting".into()))
    };
    let pick_value = |p: super::Pick| match p {
        super::Pick::Auto => "auto".to_string(),
        super::Pick::Off => "off".to_string(),
        super::Pick::Only(s) => format!("{s:?}"),
    };
    let parts: Vec<Value> = super::Part::ALL
        .into_iter()
        .map(|p| {
            json!({
                "key": format!("{p:?}"),
                "name": p.name(),
                "pick": pick_value(c.assign.get(p)),
                "inputs": p.inputs().iter().map(|s| json!([format!("{s:?}"), s.name()])).collect::<Vec<_>>(),
            })
        })
        .collect();
    let all_statuses: serde_json::Map<String, Value> = Source::ALL
        .into_iter()
        .map(|s| (format!("{s:?}"), json!(noted(s, statuses.iter().find(|(src, _, _)| *src == s).map(|(_, _, t)| t.clone()).unwrap_or_else(|| "off".into())))))
        .collect();
    let mut out = json!({
        "config": serde_json::to_value(&c).unwrap_or(json!({})),
        "statuses": all_statuses,
        "parts": parts,
        "leap": c.leap,
        "leap_height": c.leap_height,
        "leap_forward": c.leap_forward,
        "leap_status": status(Source::Leap, c.leap),
        "leap_hands": leap::hands_seen(),
        "telemetry": c.telemetry.enabled,
        "telemetry_status": status(Source::Telemetry, c.telemetry.enabled),
        "telemetry_calibration": telemetry::calibration_state(),
        "telemetry_scale": c.telemetry_scale,
        "ifacialmocap": c.ifacialmocap.enabled,
        "ifacialmocap_status": status(Source::IFacialMocap, c.ifacialmocap.enabled),
        "mediapipe": c.mediapipe,
        "mediapipe_status": status(Source::MediaPipe, c.mediapipe),
        "firewall": super::firewall::status(),
        "network_status": if c.network.enabled { super::network::status() } else { "off".to_string() },
        "network_addresses": super::network::addresses(),
        "done": done,
    });
    if args.get("lists").and_then(|v| v.as_bool()).unwrap_or(false) {
        out["cameras"] = json!(super::menu::cameras().into_iter().map(|(n, id)| json!([n, id])).collect::<Vec<_>>());
        out["controllers"] = json!(super::gamepad::devices().into_iter().map(|(n, id)| json!([n, id])).collect::<Vec<_>>());
    }
    out.to_string()
}

/// `values` into `into`: an object in both is merged (what `values` leaves out stays)
fn merge(into: &mut Value, values: &serde_json::Map<String, Value>) {
    for (k, v) in values {
        match (into.get_mut(k), v) {
            (Some(Value::Object(old)), Value::Object(new)) => {
                let mut o = Value::Object(std::mem::take(old));
                merge(&mut o, new);
                into[k] = o;
            }
            _ => into[k] = v.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_docks_parse() {
        // Theater Scene runs them; a syntax error would only show in OBS
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("theater-scripts");
        let mut n = 0;
        for f in std::fs::read_dir(&dir).unwrap().flatten().filter(|f| f.path().extension().is_some_and(|e| e == "rhai")) {
            let src = std::fs::read_to_string(f.path()).unwrap();
            if let Err(e) = rhai::Engine::new().compile(&src) {
                panic!("{}: {e}", f.path().display());
            }
            n += 1;
        }
        // (The Configuration window; the animation dock is gone)
        assert!(n >= 1);
    }

    #[test]
    fn the_configuration_window_changes_settings() {
        // A nested one keeps what isn't given
        let mut v = json!({ "ifacialmocap": { "enabled": false, "ip": "1.2.3.4", "port": 49983 }, "mirror": false });
        merge(&mut v, json!({ "ifacialmocap": { "enabled": true }, "mirror": true }).as_object().unwrap());
        assert_eq!(v, json!({ "ifacialmocap": { "enabled": true, "ip": "1.2.3.4", "port": 49983 }, "mirror": true }));
        // Through the procedure: Arms on the desk, the iPhone's address trimmed
        let r: Value = serde_json::from_str(&answer(r#"{"action":"assign","part":"Arms","pick":"Desk"}"#)).unwrap();
        assert_eq!(r["done"], json!(true));
        assert_eq!(config::get().assign.arms, super::super::Pick::Only(Source::Desk));
        answer(r#"{"action":"set_config","values":{"ifacialmocap":{"ip":" 192.168.1.20 "}}}"#);
        assert_eq!(config::get().ifacialmocap.ip, "192.168.1.20");
        let parts = r["parts"].as_array().unwrap();
        assert!(parts.iter().any(|p| p["key"] == "Arms" && p["pick"] == "Desk"));
        assert!(r["statuses"].get("Desk").is_some());
    }
}
