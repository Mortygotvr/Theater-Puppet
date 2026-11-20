//! Theater Puppet Sender: run it on the computer you play on, when OBS (and
//! Theater Puppet) runs on another. It reads this computer's keyboard, mouse
//! and controller and sends them to Puppet (its tracking/network.rs) about
//! 60 times a second. Only which keys are down *now* is sent: nothing typed
//! is kept.
//!
//! The first time it asks for the OBS computer's address and the pairing code
//! (Puppet → Configuration → 🌐 Network shows both) and keeps them in
//! theater-puppet-sender.json (Linux: ~/.config/theater-puppet-sender.json;
//! Windows: beside it). Or:
//!   theater-puppet-sender <address> <code> [--port 47620] [--no-keyboard] [--no-controller]

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

mod state;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::Input;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::Input;

use state::Message;

const PORT: u16 = 47620;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    address: String,
    port: u16,
    code: String,
    keyboard: bool,
    controller: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { address: String::new(), port: PORT, code: String::new(), keyboard: true, controller: true }
    }
}

fn settings_file() -> PathBuf {
    if cfg!(windows) {
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default().join("theater-puppet-sender.json")
    } else {
        let config = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
        let _ = std::fs::create_dir_all(&config);
        config.join("theater-puppet-sender.json")
    }
}

fn ask(question: &str) -> String {
    print!("{question} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "another computer".into())
}

fn main() {
    println!("Theater Puppet Sender: this computer's keyboard, mouse and controller, to Theater Puppet on the OBS computer.");
    let file = settings_file();
    let mut s: Settings = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    // The command line, over what's kept
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut plain = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                s.port = args.get(i).and_then(|p| p.parse().ok()).unwrap_or(s.port);
            }
            "--no-keyboard" => s.keyboard = false,
            "--no-controller" => s.controller = false,
            "--keyboard" => s.keyboard = true,
            "--controller" => s.controller = true,
            "-h" | "--help" => {
                println!("theater-puppet-sender <address> <code> [--port {PORT}] [--no-keyboard] [--no-controller]");
                return;
            }
            a => plain.push(a.to_string()),
        }
        i += 1;
    }
    if let Some(a) = plain.first() {
        s.address = a.clone();
    }
    if let Some(c) = plain.get(1) {
        s.code = c.clone();
    }
    if s.address.is_empty() {
        s.address = ask("The OBS computer's address (Puppet → Configuration → Network shows it, like 192.168.1.20):");
    }
    if s.code.is_empty() {
        s.code = ask("The pairing code (Puppet → Configuration → Network):");
    }
    let _ = std::fs::write(&file, serde_json::to_string_pretty(&s).unwrap_or_default());
    let name = computer_name();

    let mut input = Input::start(s.keyboard, s.controller);
    let what: Vec<&str> = [(s.keyboard, "the keyboard & mouse"), (s.controller, "the controller")].iter().filter(|(on, _)| *on).map(|(_, w)| *w).collect();
    println!("Sending {} to {}:{} as \"{name}\" (Ctrl+C to stop). Settings: {}", what.join(" and "), s.address, s.port, file.display());
    // Give the devices a moment to be found, then say what was
    std::thread::sleep(Duration::from_millis(600));
    for line in input.report() {
        println!("  {line}");
    }

    loop {
        match send(&s, &name, &mut input) {
            Ok(()) => println!("Puppet closed the connection; trying again…"),
            Err(e) if e.contains("wrong pairing code") => {
                println!("Puppet says the pairing code is wrong.");
                s.code = ask("The pairing code (Puppet → Configuration → Network):");
                let _ = std::fs::write(&file, serde_json::to_string_pretty(&s).unwrap_or_default());
                continue;
            }
            Err(e) => println!("Can't reach Puppet at {}:{} ({e}); trying again… (Is Network on in Puppet's Configuration? Is the port open in the OBS computer's firewall?)", s.address, s.port),
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// One connection: the code, then the states until it breaks
fn send(s: &Settings, name: &str, input: &mut Input) -> Result<(), String> {
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(s.address.as_str(), s.port)).map_err(|e| e.to_string())?.next().ok_or("no such address")?;
    let stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let (mut ws, _) = tungstenite::client(format!("ws://{}:{}", s.address, s.port), stream).map_err(|e| e.to_string())?;
    let hello = Message { hello: Some(s.code.clone()), name: Some(name.to_string()), ..Default::default() };
    ws.send(tungstenite::Message::text(serde_json::to_string(&hello).unwrap_or_default())).map_err(|e| e.to_string())?;
    let answer = ws.read().map_err(|e| e.to_string())?;
    let answer = answer.to_text().unwrap_or("").to_string();
    if !answer.contains("\"ok\":true") {
        return Err(answer);
    }
    println!("Connected to Puppet: sending.");
    let mut said = Instant::now();
    let mut seen = Vec::new();
    loop {
        let (desk, pad) = input.read();
        let m = Message { desk, pad, ..Default::default() };
        ws.send(tungstenite::Message::text(serde_json::to_string(&m).unwrap_or_default())).map_err(|e| e.to_string())?;
        // What's read here, when it changes (a device plugged in or out)
        if said.elapsed() > Duration::from_secs(3) {
            said = Instant::now();
            let now = input.report();
            if now != seen {
                for line in &now {
                    println!("  {line}");
                }
                seen = now;
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
