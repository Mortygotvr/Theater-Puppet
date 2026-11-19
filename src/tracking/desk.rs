//! The keyboard and mouse, for the avatar at a desk (desk.rs places its
//! hands): which keys are down now, the mouse's buttons, and how the mouse
//! moved (as a small offset that settles back to the middle of its pad).
//!
//! - Keys and mouse buttons: polled with GetAsyncKeyState, as OBS reads its
//!   own hotkeys. Nothing is registered, so nothing is taken from OBS or the
//!   game. Only which keys are down *now* is kept: never what was typed.
//! - Mouse movement: Raw Input (a message-only window of its own, receiving
//!   while OBS isn't in front), which still moves while a game holds the
//!   cursor in place.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::Source;

/// The keys and mouse now
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Desk {
    /// Virtual-key codes down (a bit each, 0–255)
    pub keys: [u64; 4],
    /// Key presses so far (a count only, for typing that shows no keys)
    pub presses: u32,
    /// The mouse moved from the middle of its pad: (to the user's right, away
    /// from the user), metres
    pub mouse: [f32; 2],
    /// Its buttons: 1 left, 2 right, 4 middle
    pub buttons: u8,
    /// Seconds since the mouse last moved or clicked, and since a key went down
    pub mouse_idle: f32,
    pub keys_idle: f32,
}

impl Desk {
    pub fn down(&self, vk: u8) -> bool {
        self.keys[(vk >> 6) as usize] & (1 << (vk & 63)) != 0
    }

    pub fn press(&mut self, vk: u8) {
        self.keys[(vk >> 6) as usize] |= 1 << (vk & 63);
    }

    pub fn any_key(&self) -> bool {
        self.keys.iter().any(|k| *k != 0)
    }
}

/// The input's settings (tracking.json)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
}

static LATEST: Mutex<Option<(Desk, Instant)>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The keyboard and mouse now; None while the input is off
pub fn latest() -> Option<Desk> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let fresh = LATEST.lock().unwrap().filter(|(_, at)| at.elapsed() < Duration::from_millis(500)).map(|(d, _)| d);
    Some(fresh.unwrap_or_default())
}

/// How far a mouse count moves the avatar's mouse (metres: an 800 DPI
/// mouse's), how far it can go from the middle, and how fast it settles back
const METRES_PER_COUNT: f32 = 0.0254 / 800.0;
const REACH: [f32; 2] = [0.07, 0.05];
const SETTLE: f32 = 0.6;

/// The mouse offset after moving `delta` counts (x right, y toward the
/// user, as Windows gives them) over `dt` seconds
pub fn moved(mouse: [f32; 2], delta: [i32; 2], dt: f32) -> [f32; 2] {
    let k = (dt / SETTLE).min(1.0);
    let x = mouse[0] * (1.0 - k) + delta[0] as f32 * METRES_PER_COUNT;
    let y = mouse[1] * (1.0 - k) - delta[1] as f32 * METRES_PER_COUNT;
    [x.clamp(-REACH[0], REACH[0]), y.clamp(-REACH[1], REACH[1])]
}

pub fn restart(settings: &Settings) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    ENABLED.store(settings.enabled, Ordering::Relaxed);
    *LATEST.lock().unwrap() = None;
    if !settings.enabled {
        return;
    }
    let _ = std::thread::Builder::new().name("theater-puppet-desk".into()).spawn(move || {
        #[cfg(windows)]
        read(&|| GENERATION.load(Ordering::SeqCst) == generation);
        #[cfg(not(windows))]
        super::set_status(Source::Desk, "only on Windows");
    });
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct RawInputDevice {
        pub usage_page: u16,
        pub usage: u16,
        pub flags: u32,
        pub target: *mut c_void,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Msg {
        pub hwnd: usize,
        pub message: u32,
        pub wparam: usize,
        pub lparam: isize,
        pub time: u32,
        pub pt: [i32; 2],
        pub private: u32,
    }

    pub const HWND_MESSAGE: isize = -3;
    pub const RIDEV_INPUTSINK: u32 = 0x100;
    pub const RIDEV_REMOVE: u32 = 0x1;
    pub const RID_INPUT: u32 = 0x1000_0003;
    pub const RIM_TYPEMOUSE: u32 = 0;
    pub const WM_INPUT: u32 = 0x00FF;
    pub const PM_REMOVE: u32 = 1;
    /// RAWINPUTHEADER's size on 64-bit Windows, and where RAWMOUSE's fields are after it
    pub const HEADER: u32 = 24;
    pub const MOUSE_FLAGS: usize = 24;
    pub const MOUSE_X: usize = 24 + 12;
    pub const MOUSE_Y: usize = 24 + 16;
    /// MOUSE_MOVE_ABSOLUTE: a tablet or remote desktop (not a movement)
    pub const MOUSE_ABSOLUTE: u16 = 1;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn GetAsyncKeyState(vk: i32) -> i16;
        pub fn CreateWindowExW(ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: isize, menu: *mut c_void, instance: *mut c_void, param: *mut c_void) -> *mut c_void;
        pub fn DestroyWindow(hwnd: *mut c_void) -> i32;
        pub fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
        pub fn GetRawInputData(input: isize, command: u32, data: *mut c_void, size: *mut u32, header: u32) -> u32;
        pub fn PeekMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
        pub fn DispatchMessageW(msg: *const Msg) -> isize;
    }
}

/// The virtual keys read: every key but the mouse buttons (1, 2, 4, 5, 6)
/// and the generic shift / ctrl / alt (0x10–0x12: their left and right
/// ones are read)
#[cfg(windows)]
fn is_key(vk: i32) -> bool {
    !matches!(vk, 0..=7 | 0x10..=0x12) && vk < 0xFF
}

#[cfg(windows)]
fn read(mine: &dyn Fn() -> bool) {
    use win::*;
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
    let mouse_dev = RawInputDevice { usage_page: 1, usage: 2, flags: RIDEV_INPUTSINK, target: hwnd };
    let raw = !hwnd.is_null() && unsafe { RegisterRawInputDevices(&mouse_dev, 1, std::mem::size_of::<RawInputDevice>() as u32) } != 0;
    crate::log(&format!("[Puppet] ⌨ Keyboard & mouse: reading{}", if raw { "" } else { " (no Raw Input: the mouse won't move)" }));
    super::set_status(Source::Desk, if raw { "reading the keyboard and mouse" } else { "reading the keyboard (the mouse can't be read)" });

    let mut desk = Desk { mouse_idle: 10.0, keys_idle: 10.0, ..Default::default() };
    let mut last = Instant::now();
    let mut buf = vec![0u8; 64];
    while mine() {
        // The mouse's movement since the last pass
        let mut delta = [0i32; 2];
        let mut msg = Msg::default();
        while raw && unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_INPUT {
                let mut size = buf.len() as u32;
                let got = unsafe { GetRawInputData(msg.lparam, RID_INPUT, buf.as_mut_ptr() as *mut c_void, &mut size, HEADER) };
                if got != u32::MAX && got as usize >= MOUSE_Y + 4 && u32::from_le_bytes(buf[0..4].try_into().unwrap()) == RIM_TYPEMOUSE {
                    let flags = u16::from_le_bytes([buf[MOUSE_FLAGS], buf[MOUSE_FLAGS + 1]]);
                    if flags & MOUSE_ABSOLUTE == 0 {
                        delta[0] += i32::from_le_bytes(buf[MOUSE_X..MOUSE_X + 4].try_into().unwrap());
                        delta[1] += i32::from_le_bytes(buf[MOUSE_Y..MOUSE_Y + 4].try_into().unwrap());
                    }
                }
            }
            unsafe { DispatchMessageW(&msg) };
        }
        let dt = last.elapsed().as_secs_f32();
        last = Instant::now();
        desk.mouse = moved(desk.mouse, delta, dt);
        // The keys and buttons down now (only that: nothing is kept)
        let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
        let buttons = (down(1) as u8) | ((down(2) as u8) << 1) | ((down(4) as u8) << 2);
        let mut keys = [0u64; 4];
        for vk in (0..0xFF).filter(|&v| is_key(v)) {
            if down(vk) {
                keys[(vk >> 6) as usize] |= 1 << (vk & 63);
            }
        }
        let new_presses = (0..4).map(|i| (keys[i] & !desk.keys[i]).count_ones()).sum::<u32>();
        desk.presses = desk.presses.wrapping_add(new_presses);
        desk.keys_idle = if new_presses > 0 { 0.0 } else { desk.keys_idle + dt };
        desk.mouse_idle = if delta != [0, 0] || buttons != 0 { 0.0 } else { desk.mouse_idle + dt };
        desk.keys = keys;
        desk.buttons = buttons;
        *LATEST.lock().unwrap() = Some((desk, Instant::now()));
        std::thread::sleep(Duration::from_millis(8));
    }
    if raw {
        let off = RawInputDevice { usage_page: 1, usage: 2, flags: RIDEV_REMOVE, target: std::ptr::null_mut() };
        unsafe { RegisterRawInputDevices(&off, 1, std::mem::size_of::<RawInputDevice>() as u32) };
    }
    if !hwnd.is_null() {
        unsafe { DestroyWindow(hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_bits() {
        let mut d = Desk::default();
        d.press(0x57);
        d.press(0xA0);
        assert!(d.down(0x57) && d.down(0xA0) && !d.down(0x41));
        assert!(d.any_key());
    }

    #[test]
    fn the_mouse_moves_and_settles_back() {
        // 800 counts right is an inch
        let m = moved([0.0, 0.0], [800, 0], 0.0);
        assert!((m[0] - 0.0254).abs() < 1e-6);
        // Toward the user (Windows' +y) is -y here
        assert!(moved([0.0, 0.0], [0, 400], 0.0)[1] < 0.0);
        // It stays on its pad
        assert_eq!(moved([0.0, 0.0], [100_000, 0], 0.0)[0], REACH[0]);
        // Still, it settles back to the middle
        let mut m = [0.05, 0.0];
        for _ in 0..200 {
            m = moved(m, [0, 0], 0.016);
        }
        assert!(m[0].abs() < 0.001);
    }
}
