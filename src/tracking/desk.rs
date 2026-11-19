//! The keyboard and mouse, for the avatar at a desk (desk.rs places its
//! hands): which keys are down now, the mouse's buttons, and how the mouse
//! moved (as a small offset that settles back to the middle of its pad).
//!
//! - Keys and mouse buttons: polled with GetAsyncKeyState, as OBS reads its
//!   own hotkeys. Nothing is registered, so nothing is taken from OBS or the
//!   game. Only which keys are down *now* is kept: never what was typed.
//! - Mouse movement: a low-level mouse hook (each movement: where it goes
//!   from where the cursor was), which works while OBS isn't in front.
//!   Not Raw Input: a program has one Raw Input mouse target, and other
//!   plugins' windows (egui's) take it, so Puppet's never heard a thing.
//!   The hook's thread wakes on each movement and answers at once: the
//!   system waits for it before moving the cursor.

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

    /// MSLLHOOKSTRUCT
    #[repr(C)]
    pub struct MouseHook {
        pub pt: [i32; 2],
        pub data: u32,
        pub flags: u32,
        pub time: u32,
        pub extra: usize,
    }

    pub const WH_MOUSE_LL: i32 = 14;
    pub const WM_MOUSEMOVE: usize = 0x0200;
    pub const PM_REMOVE: u32 = 1;
    pub const QS_ALLINPUT: u32 = 0x04FF;
    pub const MODULE_FROM_ADDRESS: u32 = 0x4 | 0x2;

    pub type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn GetAsyncKeyState(vk: i32) -> i16;
        pub fn GetCursorPos(pt: *mut [i32; 2]) -> i32;
        /// The hook gives physical pixels; GetCursorPos does too only on a
        /// DPI-aware thread (scaled otherwise: 1.75× apart on a 175% display)
        pub fn SetThreadDpiAwarenessContext(context: isize) -> isize;
        pub fn SetWindowsHookExW(id: i32, proc_: HookProc, module: *mut c_void, thread: u32) -> *mut c_void;
        pub fn UnhookWindowsHookEx(hook: *mut c_void) -> i32;
        pub fn CallNextHookEx(hook: *mut c_void, code: i32, wparam: usize, lparam: isize) -> isize;
        pub fn PeekMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
        pub fn DispatchMessageW(msg: *const Msg) -> isize;
        pub fn MsgWaitForMultipleObjects(count: u32, handles: *const *mut c_void, all: i32, ms: u32, wake: u32) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
    }
}

/// The first movement since the start was logged
#[cfg(windows)]
static FIRST_SEEN: AtomicBool = AtomicBool::new(false);
/// The mouse's movement since the reader last took it (the hook adds to it)
#[cfg(windows)]
static MOVED: Mutex<[i32; 2]> = Mutex::new([0, 0]);

/// Each movement: from where the cursor is to where it's going (asked
/// before the cursor moves, so a game that puts it back still counts)
#[cfg(windows)]
unsafe extern "system" fn on_mouse(code: i32, wparam: usize, lparam: isize) -> isize {
    use win::*;
    if code >= 0 && wparam == WM_MOUSEMOVE && lparam != 0 {
        let info = unsafe { &*(lparam as *const MouseHook) };
        let mut now = [0i32; 2];
        if unsafe { GetCursorPos(&mut now) } != 0 {
            if !FIRST_SEEN.swap(true, Ordering::Relaxed) {
                crate::log(&format!("[Puppet] ⌨ First mouse movement: going to {:?} from {:?} (the same pixels, or the mouse jumps)", info.pt, now));
            }
            if let Ok(mut m) = MOVED.try_lock() {
                m[0] += info.pt[0] - now[0];
                m[1] += info.pt[1] - now[1];
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
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
    // Per-monitor DPI-aware, so the cursor's position is in the hook's pixels
    const PER_MONITOR_AWARE_V2: isize = -4;
    unsafe { SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) };
    // The hook, in this DLL, on this thread (which keeps answering it)
    let mut module = std::ptr::null_mut();
    unsafe { GetModuleHandleExW(MODULE_FROM_ADDRESS, on_mouse as *const u16, &mut module) };
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, on_mouse, module, 0) };
    let hooked = !hook.is_null();
    crate::log(&format!("[Puppet] ⌨ Keyboard & mouse: reading{}", if hooked { " (mouse hook)" } else { " (no mouse hook: the mouse won't move)" }));
    super::set_status(Source::Desk, if hooked { "reading the keyboard and mouse" } else { "reading the keyboard (the mouse can't be read)" });
    *MOVED.lock().unwrap() = [0, 0];
    FIRST_SEEN.store(false, Ordering::Relaxed);

    let mut desk = Desk { mouse_idle: 10.0, keys_idle: 10.0, ..Default::default() };
    let mut last = Instant::now();
    while mine() {
        // Wait for the next mouse event (answered at once) or the next poll
        let wait = Duration::from_millis(8).saturating_sub(last.elapsed()).as_millis() as u32;
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait, QS_ALLINPUT) };
        let mut msg = Msg::default();
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe { DispatchMessageW(&msg) };
        }
        if last.elapsed() < Duration::from_millis(8) {
            continue;
        }
        let dt = last.elapsed().as_secs_f32();
        last = Instant::now();
        let delta = std::mem::take(&mut *MOVED.lock().unwrap());
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
    }
    if hooked {
        unsafe { UnhookWindowsHookEx(hook) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moves the real cursor a little and back: run by hand
    /// (cargo test --release mouse_hook -- --ignored)
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn the_mouse_hook_hears_the_mouse() {
        #[repr(C)]
        struct Input {
            kind: u32,
            _pad: u32,
            dx: i32,
            dy: i32,
            data: u32,
            flags: u32,
            time: u32,
            _pad2: u32,
            extra: usize,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
            fn SetProcessDpiAwarenessContext(context: isize) -> i32;
        }
        // As OBS is (per-monitor DPI-aware v2)
        unsafe { SetProcessDpiAwarenessContext(-4) };
        let nudge = |dx: i32| {
            let i = Input { kind: 0, _pad: 0, dx, dy: 0, data: 0, flags: 0x0001, time: 0, _pad2: 0, extra: 0 };
            assert_eq!(unsafe { SendInput(1, &i, std::mem::size_of::<Input>() as i32) }, 1);
        };
        restart(&Settings { enabled: true });
        std::thread::sleep(Duration::from_millis(300));
        for _ in 0..10 {
            nudge(20);
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(30));
        let moved_right = latest().unwrap().mouse[0];
        for _ in 0..10 {
            nudge(-20);
            std::thread::sleep(Duration::from_millis(5));
        }
        restart(&Settings { enabled: false });
        println!("the avatar's mouse moved {moved_right} m right");
        // 10 nudges of 20 (a little more with pointer acceleration): millimetres,
        // not the pad's edge (a jump between pixel scales would be)
        assert!(moved_right > 0.002 && moved_right < 0.03, "{moved_right}");
    }

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
