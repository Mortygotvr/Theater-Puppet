//! Linux: the keyboards, mice and controller, read from the kernel's input
//! devices (/dev/input/event*, evdev). Works on X11, Wayland and the console
//! alike. Nothing is grabbed: the game and the desktop still get it all.
//!
//! Reading them needs the `input` group (most distributions):
//!   sudo usermod -aG input $USER   (then log out and in)

use std::collections::HashSet;
use std::ffi::c_ulong;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::*;

unsafe extern "C" {
    fn ioctl(fd: i32, request: c_ulong, ...) -> i32;
}

/// _IOC(_IOC_READ, 'E', nr, size)
const fn ioc_read(nr: u32, size: u32) -> c_ulong {
    ((2u32 << 30) | (size << 16) | ((b'E' as u32) << 8) | nr) as c_ulong
}

/// A device's capability bits for event type `ev` (0: the types themselves)
fn bits(f: &File, ev: u16, bytes: usize) -> Vec<u8> {
    let mut buf = vec![0u8; bytes];
    unsafe { ioctl(f.as_raw_fd(), ioc_read(0x20 + ev as u32, bytes as u32), buf.as_mut_ptr()) };
    buf
}

fn has(bits: &[u8], n: u16) -> bool {
    bits.get(n as usize / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

fn name(f: &File) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { ioctl(f.as_raw_fd(), ioc_read(0x06, 256), buf.as_mut_ptr()) };
    if n <= 0 {
        return "?".into();
    }
    String::from_utf8_lossy(&buf[..(n as usize).min(256)]).trim_end_matches('\0').to_string()
}

/// An axis's range (struct input_absinfo: value, min, max, fuzz, flat, resolution)
fn range(f: &File, code: u16) -> Option<(i32, i32)> {
    let mut info = [0i32; 6];
    let r = unsafe { ioctl(f.as_raw_fd(), ioc_read(0x40 + code as u32, 24), info.as_mut_ptr()) };
    (r >= 0 && info[2] > info[1]).then_some((info[1], info[2]))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Keyboard,
    Mouse,
    Gamepad,
}

/// What a device is (None: none of those, a power button, a webcam…)
fn kind(f: &File) -> Vec<Kind> {
    let types = bits(f, 0, 4);
    let keys = if has(&types, EV_KEY) { bits(f, EV_KEY, 96) } else { vec![] };
    let rel = if has(&types, EV_REL) { bits(f, EV_REL, 2) } else { vec![] };
    let abs = if has(&types, EV_ABS) { bits(f, EV_ABS, 8) } else { vec![] };
    let mut out = Vec::new();
    if has(&keys, KEY_A) && has(&keys, KEY_SPACE) {
        out.push(Kind::Keyboard);
    }
    if has(&rel, REL_X) && has(&rel, REL_Y) && has(&keys, BTN_LEFT) {
        out.push(Kind::Mouse);
    }
    if has(&keys, BTN_SOUTH) && has(&abs, ABS_X) {
        out.push(Kind::Gamepad);
    }
    out
}

#[derive(Default)]
struct Shared {
    kbm: KeyboardMouse,
    pad: Option<Gamepad>,
    /// Which device is the controller (the first found)
    pad_device: Option<String>,
    keyboards: usize,
    mice: usize,
    open: HashSet<String>,
    found: Vec<String>,
    denied: usize,
}

pub struct Input {
    shared: Arc<Mutex<Shared>>,
    keyboard: bool,
    controller: bool,
    last: Instant,
}

impl Input {
    pub fn start(keyboard: bool, controller: bool) -> Input {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let s = shared.clone();
        std::thread::spawn(move || loop {
            scan(&s, keyboard, controller);
            std::thread::sleep(Duration::from_secs(2));
        });
        Input { shared, keyboard, controller, last: Instant::now() }
    }

    /// The desk and the controller now (None: none found, or not sent)
    pub fn read(&mut self) -> (Option<Desk>, Option<Pad>) {
        let dt = self.last.elapsed().as_secs_f32();
        self.last = Instant::now();
        let mut s = self.shared.lock().unwrap();
        let desk = (self.keyboard && s.keyboards + s.mice > 0).then(|| s.kbm.tick(dt));
        let pad = if self.controller { s.pad.as_ref().map(|g| g.pad()) } else { None };
        (desk, pad)
    }

    /// What it reads (for the console)
    pub fn report(&self) -> Vec<String> {
        let s = self.shared.lock().unwrap();
        let mut out: Vec<String> = s.found.clone();
        if s.denied > 0 && s.open.is_empty() {
            out.push(format!(
                "Can't read the input devices ({} of /dev/input): add yourself to the input group:\n    sudo usermod -aG input $USER\n  then log out and back in (or run this once with sudo to try it).",
                s.denied
            ));
        }
        if self.keyboard && s.keyboards + s.mice == 0 {
            out.push("No keyboard or mouse found yet.".into());
        }
        if self.controller && s.pad.is_none() {
            out.push("No controller found yet (plug one in: it's found within a few seconds).".into());
        }
        out
    }
}

/// The devices there now: each new one, read on its own thread
fn scan(shared: &Arc<Mutex<Shared>>, keyboard: bool, controller: bool) {
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return };
    let mut denied = 0;
    for entry in dir.flatten() {
        let path = entry.path();
        let p = path.to_string_lossy().to_string();
        if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")) || shared.lock().unwrap().open.contains(&p) {
            continue;
        }
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                denied += 1;
                continue;
            }
            Err(_) => continue,
        };
        let kinds = kind(&f);
        let mut s = shared.lock().unwrap();
        let wanted: Vec<Kind> = kinds
            .into_iter()
            .filter(|k| match k {
                Kind::Keyboard | Kind::Mouse => keyboard,
                Kind::Gamepad => controller && s.pad_device.is_none(),
            })
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let n = name(&f);
        for k in &wanted {
            match k {
                Kind::Keyboard => s.keyboards += 1,
                Kind::Mouse => s.mice += 1,
                Kind::Gamepad => {
                    let ranges: Vec<(u16, i32, i32)> = (0..0x20u16).filter_map(|c| range(&f, c).map(|(lo, hi)| (c, lo, hi))).collect();
                    s.pad = Some(Gamepad::new(&ranges));
                    s.pad_device = Some(p.clone());
                }
            }
        }
        s.found.push(format!("{}: {n} ({p})", wanted.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(" + ")));
        s.open.insert(p.clone());
        drop(s);
        let shared = shared.clone();
        std::thread::spawn(move || read(f, p, wanted, shared));
    }
    shared.lock().unwrap().denied = denied;
}

/// One device's events, until it goes
fn read(mut f: File, path: String, kinds: Vec<Kind>, shared: Arc<Mutex<Shared>>) {
    // struct input_event on 64-bit Linux: a timeval (16 bytes), type, code, value
    const SIZE: usize = 24;
    let mut buf = [0u8; SIZE * 64];
    let is_pad = kinds.contains(&Kind::Gamepad);
    let is_kbm = kinds.iter().any(|k| *k != Kind::Gamepad);
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut s = shared.lock().unwrap();
        for e in buf[..n].chunks_exact(SIZE) {
            let kind = u16::from_ne_bytes([e[16], e[17]]);
            let code = u16::from_ne_bytes([e[18], e[19]]);
            let value = i32::from_ne_bytes([e[20], e[21], e[22], e[23]]);
            if kind == EV_SYN {
                continue;
            }
            if is_pad {
                if let Some(g) = s.pad.as_mut() {
                    g.event(kind, code, value);
                }
            }
            if is_kbm {
                s.kbm.event(kind, code, value);
            }
        }
    }
    // Unplugged: forget it (a new one is found by the next scan)
    let mut s = shared.lock().unwrap();
    s.open.remove(&path);
    s.found.retain(|l| !l.ends_with(&format!("({path})")));
    for k in &kinds {
        match k {
            Kind::Keyboard => s.keyboards = s.keyboards.saturating_sub(1),
            Kind::Mouse => s.mice = s.mice.saturating_sub(1),
            Kind::Gamepad => {
                s.pad = None;
                s.pad_device = None;
            }
        }
    }
    if is_kbm {
        s.kbm.release_all();
    }
}
