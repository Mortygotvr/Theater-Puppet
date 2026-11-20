//! Input from another computer: the keyboard, mouse and controller of the
//! computer you play on, when OBS runs on another one. Theater Puppet Sender
//! (src/bin/sender.rs) reads them there, with the same code Puppet uses
//! here (desk.rs, gamepad.rs), and sends them to this WebSocket server.
//!
//! - Puppet listens on every network of this computer, on `port` (47620).
//! - A sender says the pairing code first (Configuration → Network); a wrong
//!   one is turned away. One sender at a time: a new one takes over.
//! - Then it sends, about 60 times a second, `{"desk": Desk?, "pad": Pad?}`
//!   (either, or both): what's down and moved there now. Only which keys
//!   are down *now*: nothing typed is kept, here or there.
//! - While its states keep coming, desk::latest() and gamepad::latest() are
//!   them, not this computer's (the Keyboard & mouse and Controller inputs
//!   must be on, as for this computer's).

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::desk::Desk;
use super::gamepad::Pad;

pub const PORT: u16 = 47620;

/// The input's settings (tracking.json)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub port: u16,
    /// The pairing code a sender must give (made when it's turned on)
    pub code: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, port: PORT, code: String::new() }
    }
}

/// What a sender sends
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    /// The first message: the pairing code, and the computer's name
    pub hello: Option<String>,
    pub name: Option<String>,
    pub desk: Option<Desk>,
    pub pad: Option<Pad>,
}

/// How long a state from the network stays the avatar's
const FRESH: Duration = Duration::from_millis(500);

static DESK: Mutex<Option<(Desk, Instant)>> = Mutex::new(None);
static PAD: Mutex<Option<(Pad, Instant)>> = Mutex::new(None);
/// The sender now: its name and address, and since when
static SENDER: Mutex<Option<(String, SocketAddr, Instant)>> = Mutex::new(None);
static STATUS: Mutex<String> = Mutex::new(String::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Which connection is the sender's (a new one takes over)
static CONNECTION: AtomicU64 = AtomicU64::new(0);

/// The keyboard and mouse from the network, while they keep coming
pub fn desk() -> Option<Desk> {
    DESK.lock().unwrap().filter(|(_, at)| at.elapsed() < FRESH).map(|(d, _)| d)
}

/// The controller from the network, while it keeps coming
pub fn pad() -> Option<Pad> {
    PAD.lock().unwrap().filter(|(_, at)| at.elapsed() < FRESH).map(|(p, _)| p)
}

/// The Configuration window's line: listening, who's sending, what
pub fn status() -> String {
    let base = STATUS.lock().unwrap().clone();
    match SENDER.lock().unwrap().as_ref() {
        Some((name, addr, _)) => {
            let what: Vec<&str> = [(desk().is_some(), "keyboard & mouse"), (pad().is_some(), "controller")].into_iter().filter(|(on, _)| *on).map(|(_, w)| w).collect();
            let what = if what.is_empty() { "nothing yet".to_string() } else { what.join(" and ") };
            format!("{name} ({}) connected: receiving {what}", addr.ip())
        }
        None => base,
    }
}

/// A new pairing code: six digits
pub fn new_code() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("{:06}", h.finish() % 1_000_000)
}

/// The addresses of this computer a sender can use (IPv4, not loopback)
pub fn addresses() -> Vec<String> {
    // The address the system would send from to the outside (no packet is sent)
    let mut out = Vec::new();
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if s.connect("192.0.2.1:9").is_ok() {
            if let Ok(a) = s.local_addr() {
                out.push(a.ip().to_string());
            }
        }
    }
    out
}

pub fn restart(settings: &Settings) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    *DESK.lock().unwrap() = None;
    *PAD.lock().unwrap() = None;
    *SENDER.lock().unwrap() = None;
    if !settings.enabled {
        *STATUS.lock().unwrap() = "off".into();
        return;
    }
    let settings = settings.clone();
    let _ = std::thread::Builder::new().name("theater-puppet-network".into()).spawn(move || serve(settings, generation));
}

fn mine(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}

fn serve(settings: Settings, generation: u64) {
    let listener = match TcpListener::bind(("0.0.0.0", settings.port)) {
        Ok(l) => l,
        Err(e) => {
            *STATUS.lock().unwrap() = format!("can't listen on port {}: {e}", settings.port);
            crate::log(&format!("[Puppet] 🌐 Network input: can't listen on port {}: {e}", settings.port));
            return;
        }
    };
    let _ = listener.set_nonblocking(true);
    *STATUS.lock().unwrap() = format!("waiting for a sender on port {}", settings.port);
    crate::log(&format!("[Puppet] 🌐 Network input: listening on port {}", settings.port));
    while mine(generation) {
        match listener.accept() {
            Ok((stream, addr)) => {
                let code = settings.code.clone();
                let _ = std::thread::Builder::new().name("theater-puppet-network-sender".into()).spawn(move || connection(stream, addr, code, generation));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// One sender: its code, then its states until it goes (or another takes over)
fn connection(stream: TcpStream, addr: SocketAddr, code: String, generation: u64) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(mut ws) = tungstenite::accept(stream) else { return };
    // The pairing code first
    let hello: Message = match ws.read() {
        Ok(tungstenite::Message::Text(t)) => serde_json::from_str(t.as_str()).unwrap_or_default(),
        _ => return,
    };
    if hello.hello.as_deref().map(str::trim) != Some(code.trim()) || code.trim().is_empty() {
        crate::log(&format!("[Puppet] 🌐 Network input: {} gave a wrong pairing code", addr.ip()));
        let _ = ws.send(tungstenite::Message::text(r#"{"ok":false,"error":"wrong pairing code"}"#));
        let _ = ws.close(None);
        return;
    }
    let _ = ws.send(tungstenite::Message::text(r#"{"ok":true}"#));
    let me = CONNECTION.fetch_add(1, Ordering::SeqCst) + 1;
    let name = hello.name.unwrap_or_else(|| "a computer".into());
    crate::log(&format!("[Puppet] 🌐 Network input: {name} ({}) connected", addr.ip()));
    *SENDER.lock().unwrap() = Some((name.clone(), addr, Instant::now()));
    while mine(generation) && CONNECTION.load(Ordering::SeqCst) == me {
        match ws.read() {
            Ok(tungstenite::Message::Text(t)) => {
                if let Ok(m) = serde_json::from_str::<Message>(t.as_str()) {
                    let now = Instant::now();
                    if let Some(d) = m.desk {
                        *DESK.lock().unwrap() = Some((d, now));
                    }
                    if let Some(p) = m.pad {
                        *PAD.lock().unwrap() = Some((p, now));
                    }
                }
            }
            Ok(tungstenite::Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    if CONNECTION.load(Ordering::SeqCst) == me {
        *SENDER.lock().unwrap() = None;
        crate::log(&format!("[Puppet] 🌐 Network input: {name} ({}) left", addr.ip()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_with_the_code_drives_the_desk_and_pad() {
        let port = 47699;
        restart(&Settings { enabled: true, port, code: "123456".into() });
        std::thread::sleep(Duration::from_millis(300));
        // A wrong code: turned away, nothing taken
        let (mut bad, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();
        bad.send(tungstenite::Message::text(r#"{"hello":"000000"}"#)).unwrap();
        let answer = bad.read().unwrap();
        assert!(answer.to_text().unwrap().contains("wrong"), "{answer:?}");
        // The right one: its states are the avatar's
        let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();
        ws.send(tungstenite::Message::text(r#"{"hello":"123456","name":"Gaming PC"}"#)).unwrap();
        assert!(ws.read().unwrap().to_text().unwrap().contains("true"));
        let mut desk = Desk { mouse: [0.01, -0.02], buttons: 1, ..Default::default() };
        desk.press(0x57);
        let pad = Pad { buttons: super::super::gamepad::button::A, left_stick: [0.5, 0.0], ..Default::default() };
        let m = Message { desk: Some(desk), pad: Some(pad), ..Default::default() };
        ws.send(tungstenite::Message::text(serde_json::to_string(&m).unwrap())).unwrap();
        let start = Instant::now();
        while (super::desk().is_none() || super::pad().is_none()) && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(super::desk(), Some(desk));
        assert_eq!(super::pad(), Some(pad));
        assert!(status().contains("Gaming PC"), "{}", status());
        // They go stale when it stops
        std::thread::sleep(FRESH + Duration::from_millis(100));
        assert_eq!(super::desk(), None);
        restart(&Settings::default());
    }
}
