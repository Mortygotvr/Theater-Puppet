//! A game controller (gamepad, the standard layout: ABXY, two sticks, d-pad,
//! bumpers, triggers), for the avatar holding one: its buttons, triggers and
//! sticks now. The rig puts the hands on the controller model and moves the
//! thumbs and index fingers (motion.rs).
//!
//! Read through, the user's choice:
//! - XInput: Windows' own, Xbox controllers (and what Steam Input turns
//!   others into, when that's visible outside the game);
//! - SDL: any controller, read directly (PlayStation, Switch Pro, 8BitDo…),
//!   with Steam's own SDL3.dll when Puppet has none of its own;
//! - Auto: XInput when it has a controller, SDL otherwise.
//! Reading a controller doesn't take it from the game: several programs can.

use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::Source;
use super::dll::Library;

/// The buttons, as bits of `Pad::buttons`
pub mod button {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LB: u32 = 1 << 4;
    pub const RB: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const LS: u32 = 1 << 8;
    pub const RS: u32 = 1 << 9;
    pub const UP: u32 = 1 << 10;
    pub const DOWN: u32 = 1 << 11;
    pub const LEFT: u32 = 1 << 12;
    pub const RIGHT: u32 = 1 << 13;
    pub const GUIDE: u32 = 1 << 14;
}

/// A controller's state (also what another computer sends, network.rs)
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pad {
    pub buttons: u32,
    /// 0–1
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// x right, y up, -1–1
    pub left_stick: [f32; 2],
    pub right_stick: [f32; 2],
}

impl Pad {
    pub fn pressed(&self, b: u32) -> bool {
        self.buttons & b != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    XInput,
    Sdl,
}

/// The controller input's settings (tracking.json)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub backend: Backend,
    /// Which controller ("" = the first found): a `devices()` id
    pub device: String,
}

/// The newest state read, and when
static LATEST: Mutex<Option<(Pad, Instant)>> = Mutex::new(None);
/// The controllers found (label, id), for the Configuration window
static DEVICES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// The input is on (the avatar holds the controller while Arms are on it)
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Bumped at each restart: a reader stops when it's no longer its own
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Held by the reader that has SDL open (a new one waits for the old to close it)
static SDL_IN_USE: Mutex<()> = Mutex::new(());

/// The controller now: its newest state, a neutral one when none is
/// connected; None while the input is off
pub fn latest() -> Option<Pad> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    // Another computer's while it sends one (network.rs), else this one's
    if let Some(p) = super::network::pad() {
        return Some(p);
    }
    let fresh = LATEST.lock().unwrap().filter(|(_, at)| at.elapsed() < Duration::from_millis(500)).map(|(p, _)| p);
    Some(fresh.unwrap_or_default())
}

/// The controllers found: (label, id)
pub fn devices() -> Vec<(String, String)> {
    DEVICES.lock().unwrap().clone()
}

// ---------------------------------------------------------------------------
// XInput

#[repr(C)]
#[derive(Default)]
struct XState {
    packet: u32,
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

type XGetState = unsafe extern "system" fn(u32, *mut XState) -> u32;

struct XInput {
    get_state: XGetState,
}

impl XInput {
    fn open() -> Option<XInput> {
        let system = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32");
        let (lib, _) = Library::first(&["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll"].map(|n| system.join(n)))?;
        Some(XInput { get_state: lib.function(c"XInputGetState")? })
    }

    /// The four slots' states (None: nothing there)
    fn read(&self) -> [Option<Pad>; 4] {
        std::array::from_fn(|slot| {
            let mut s = XState::default();
            (unsafe { (self.get_state)(slot as u32, &mut s) } == 0).then(|| from_xinput(&s))
        })
    }
}

fn from_xinput(s: &XState) -> Pad {
    const MAP: [(u16, u32); 14] = [
        (0x0001, button::UP),
        (0x0002, button::DOWN),
        (0x0004, button::LEFT),
        (0x0008, button::RIGHT),
        (0x0010, button::START),
        (0x0020, button::BACK),
        (0x0040, button::LS),
        (0x0080, button::RS),
        (0x0100, button::LB),
        (0x0200, button::RB),
        (0x1000, button::A),
        (0x2000, button::B),
        (0x4000, button::X),
        (0x8000, button::Y),
    ];
    let axis = |v: i16| (v as f32 / 32767.0).clamp(-1.0, 1.0);
    Pad {
        buttons: MAP.iter().filter(|(x, _)| s.buttons & x != 0).fold(0, |b, (_, p)| b | p),
        left_trigger: s.left_trigger as f32 / 255.0,
        right_trigger: s.right_trigger as f32 / 255.0,
        // XInput's y is up already
        left_stick: [axis(s.lx), axis(s.ly)],
        right_stick: [axis(s.rx), axis(s.ry)],
    }
}

// ---------------------------------------------------------------------------
// SDL 3

type Ptr = *mut c_void;

struct Sdl {
    init: unsafe extern "C" fn(u32) -> bool,
    quit_sub: unsafe extern "C" fn(u32),
    set_hint: unsafe extern "C" fn(*const c_char, *const c_char) -> bool,
    get_gamepads: unsafe extern "C" fn(*mut i32) -> *mut u32,
    free: unsafe extern "C" fn(*mut c_void),
    name_for_id: unsafe extern "C" fn(u32) -> *const c_char,
    open: unsafe extern "C" fn(u32) -> Ptr,
    close: unsafe extern "C" fn(Ptr),
    connected: unsafe extern "C" fn(Ptr) -> bool,
    update: unsafe extern "C" fn(),
    button: unsafe extern "C" fn(Ptr, i32) -> bool,
    axis: unsafe extern "C" fn(Ptr, i32) -> i16,
}

const SDL_INIT_GAMEPAD: u32 = 0x0000_2000;

/// Where an SDL3.dll is: Puppet's own (its data folder), else Steam's
fn sdl_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(data) = crate::data_dir() {
        out.push(data.join("SDL3.dll"));
    }
    out.push(crate::config_dir().join("SDL3.dll"));
    for base in [std::env::var("ProgramFiles(x86)").ok(), std::env::var("ProgramFiles").ok()].into_iter().flatten() {
        out.push(PathBuf::from(base).join("Steam").join("SDL3.dll"));
    }
    out
}

impl Sdl {
    fn open() -> Option<(Sdl, PathBuf)> {
        let (lib, path) = Library::first(&sdl_paths())?;
        Some((
            Sdl {
                init: lib.function(c"SDL_Init")?,
                quit_sub: lib.function(c"SDL_QuitSubSystem")?,
                set_hint: lib.function(c"SDL_SetHint")?,
                get_gamepads: lib.function(c"SDL_GetGamepads")?,
                free: lib.function(c"SDL_free")?,
                name_for_id: lib.function(c"SDL_GetGamepadNameForID")?,
                open: lib.function(c"SDL_OpenGamepad")?,
                close: lib.function(c"SDL_CloseGamepad")?,
                connected: lib.function(c"SDL_GamepadConnected")?,
                update: lib.function(c"SDL_UpdateGamepads")?,
                button: lib.function(c"SDL_GetGamepadButton")?,
                axis: lib.function(c"SDL_GetGamepadAxis")?,
            },
            path,
        ))
    }

    /// The gamepads plugged in: (id, name)
    fn list(&self) -> Vec<(u32, String)> {
        let mut n = 0;
        let ids = unsafe { (self.get_gamepads)(&mut n) };
        if ids.is_null() {
            return Vec::new();
        }
        let out = (0..n.max(0) as usize)
            .map(|i| {
                let id = unsafe { *ids.add(i) };
                let name = unsafe { (self.name_for_id)(id) };
                let name = if name.is_null() { "Controller".to_string() } else { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() };
                (id, name)
            })
            .collect();
        unsafe { (self.free)(ids as *mut c_void) };
        out
    }

    fn read(&self, pad: Ptr) -> Pad {
        // SDL's: south, east, west, north, back, guide, start, sticks, shoulders, d-pad
        const MAP: [(i32, u32); 15] = [
            (0, button::A),
            (1, button::B),
            (2, button::X),
            (3, button::Y),
            (4, button::BACK),
            (5, button::GUIDE),
            (6, button::START),
            (7, button::LS),
            (8, button::RS),
            (9, button::LB),
            (10, button::RB),
            (11, button::UP),
            (12, button::DOWN),
            (13, button::LEFT),
            (14, button::RIGHT),
        ];
        let axis = |i: i32| (unsafe { (self.axis)(pad, i) } as f32 / 32767.0).clamp(-1.0, 1.0);
        Pad {
            buttons: MAP.iter().filter(|(i, _)| unsafe { (self.button)(pad, *i) }).fold(0, |b, (_, p)| b | p),
            // SDL's y is down
            left_stick: [axis(0), -axis(1)],
            right_stick: [axis(2), -axis(3)],
            left_trigger: axis(4).max(0.0),
            right_trigger: axis(5).max(0.0),
        }
    }
}

// ---------------------------------------------------------------------------
// The reader

fn publish(pad: Pad) {
    *LATEST.lock().unwrap() = Some((pad, Instant::now()));
}

pub fn restart(settings: &Settings) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    ENABLED.store(settings.enabled, Ordering::Relaxed);
    *LATEST.lock().unwrap() = None;
    if !settings.enabled {
        return;
    }
    let settings = settings.clone();
    let _ = std::thread::Builder::new().name("theater-puppet-gamepad".into()).spawn(move || {
        let mine = || GENERATION.load(Ordering::SeqCst) == generation;
        let xinput = if settings.backend == Backend::Sdl { None } else { XInput::open() };
        let use_sdl = match settings.backend {
            Backend::Sdl => true,
            Backend::XInput => false,
            // XInput unless it has nothing: then SDL
            Backend::Auto => xinput.as_ref().is_none_or(|x| x.read().iter().all(Option::is_none)),
        };
        if use_sdl {
            read_sdl(&settings, &mine);
        } else if let Some(x) = xinput {
            read_xinput(&x, &settings, &mine);
        } else {
            super::set_status(Source::Controller, "XInput isn't there (Windows' xinput1_4.dll)");
        }
    });
}

fn read_xinput(x: &XInput, settings: &Settings, mine: &dyn Fn() -> bool) {
    crate::log("[Puppet] 🎮 Controller: reading through XInput");
    let mut told = None;
    while mine() {
        let pads = x.read();
        *DEVICES.lock().unwrap() = (0..4).filter(|&i| pads[i].is_some()).map(|i| (format!("XInput controller {}", i + 1), format!("xinput:{i}"))).collect();
        let wanted = settings.device.strip_prefix("xinput:").and_then(|n| n.parse::<usize>().ok());
        let slot = wanted.filter(|&i| i < 4 && pads[i].is_some()).or_else(|| pads.iter().position(Option::is_some));
        let status = match slot {
            Some(i) => {
                publish(pads[i].unwrap());
                format!("XInput controller {}", i + 1)
            }
            None => "no XInput controller plugged in".to_string(),
        };
        if told.as_ref() != Some(&status) {
            super::set_status(Source::Controller, status.clone());
            told = Some(status);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn read_sdl(settings: &Settings, mine: &dyn Fn() -> bool) {
    let Some((sdl, path)) = Sdl::open() else {
        super::set_status(Source::Controller, "SDL3.dll not found (Steam has one: is Steam installed?)");
        return;
    };
    // The one reader with SDL open
    let _held = SDL_IN_USE.lock().unwrap_or_else(|e| e.into_inner());
    if !mine() {
        return;
    }
    unsafe {
        // Read while OBS isn't the window in front (a game is)
        (sdl.set_hint)(c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS".as_ptr(), c"1".as_ptr());
        if !(sdl.init)(SDL_INIT_GAMEPAD) {
            super::set_status(Source::Controller, "SDL didn't start");
            return;
        }
    }
    crate::log(&format!("[Puppet] 🎮 Controller: reading through SDL ({})", path.display()));
    let mut open: Option<(u32, Ptr)> = None;
    let mut told = None;
    let mut listed = Instant::now() - Duration::from_secs(10);
    let mut list = Vec::new();
    while mine() {
        unsafe { (sdl.update)() };
        // The list again every second (plugged in, unplugged)
        if listed.elapsed() > Duration::from_secs(1) {
            list = sdl.list();
            *DEVICES.lock().unwrap() = list.iter().map(|(_, name)| (name.clone(), format!("sdl:{name}"))).collect();
            listed = Instant::now();
            let wanted = settings.device.strip_prefix("sdl:");
            let pick = list.iter().find(|(_, n)| Some(n.as_str()) == wanted).or(list.first()).map(|(id, _)| *id);
            if open.map(|(id, _)| id) != pick || open.is_some_and(|(_, p)| !unsafe { (sdl.connected)(p) }) {
                if let Some((_, p)) = open.take() {
                    unsafe { (sdl.close)(p) };
                }
                open = pick.and_then(|id| {
                    let p = unsafe { (sdl.open)(id) };
                    (!p.is_null()).then_some((id, p))
                });
            }
        }
        let status = match open {
            Some((id, p)) => {
                publish(sdl.read(p));
                list.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| "a controller".into())
            }
            None => "no controller plugged in (SDL)".to_string(),
        };
        if told.as_ref() != Some(&status) {
            super::set_status(Source::Controller, status.clone());
            told = Some(status);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    unsafe {
        if let Some((_, p)) = open {
            (sdl.close)(p);
        }
        (sdl.quit_sub)(SDL_INIT_GAMEPAD);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xinput_states_are_read() {
        let s = XState { packet: 1, buttons: 0x1000 | 0x0200 | 0x0004, left_trigger: 255, right_trigger: 0, lx: 32767, ly: -32768, rx: 0, ry: 16384 };
        let p = from_xinput(&s);
        assert!(p.pressed(button::A) && p.pressed(button::RB) && p.pressed(button::LEFT));
        assert!(!p.pressed(button::B));
        assert_eq!(p.left_trigger, 1.0);
        assert_eq!(p.left_stick, [1.0, -1.0]);
        assert!((p.right_stick[1] - 0.5).abs() < 0.01);
    }

    #[test]
    fn this_pc_has_an_xinput_and_steams_sdl() {
        // Not an assertion on other PCs: what this one has, printed
        let x = XInput::open();
        println!("XInput: {}, slots used: {:?}", x.is_some(), x.map(|x| x.read().map(|p| p.is_some())));
        println!("SDL3 dll: {:?}", Library::first(&sdl_paths()).map(|(_, p)| p));
        if let Some((sdl, path)) = Sdl::open() {
            unsafe { (sdl.set_hint)(c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS".as_ptr(), c"1".as_ptr()) };
            let started = unsafe { (sdl.init)(SDL_INIT_GAMEPAD) };
            std::thread::sleep(Duration::from_millis(300));
            unsafe { (sdl.update)() };
            println!("SDL from {}: every function found, started {started}, gamepads {:?}", path.display(), sdl.list());
            unsafe { (sdl.quit_sub)(SDL_INIT_GAMEPAD) };
        } else {
            println!("SDL: a function is missing");
        }
    }
}
