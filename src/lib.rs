//! Theater Puppet: an OBS source drawing an avatar prepared by Theater Rig
//! (a GLB in the Theater convention: Mixamo bones, spring_ chains, ARKit
//! shape keys, Color<N> slots). This first version draws it at rest.

pub mod filters;
pub mod ik;
pub mod model;
pub mod motion;
pub mod obs;
pub mod parts;
pub mod pose;
pub mod render;
pub mod snapshot;
pub mod source;
pub mod springs;
pub mod styles;
pub mod tracking;

use std::path::PathBuf;

/// The plugin's id: its folder's name
pub const PLUGIN_ID: &str = "theater-puppet";

/// The plugin's data folder (styles shipped with it): bin\64bit\..\..\data
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        unsafe extern "system" {
            fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut std::ffi::c_void) -> i32;
            fn GetModuleFileNameW(module: *mut std::ffi::c_void, file: *mut u16, size: u32) -> u32;
        }
        const FROM_ADDRESS: u32 = 0x4;
        const UNCHANGED_REFCOUNT: u32 = 0x2;
        let mut module = std::ptr::null_mut();
        let mut buf = [0u16; 1024];
        let len = unsafe {
            if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, data_dir as *const u16, &mut module) == 0 {
                return None;
            }
            GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32)
        };
        let dll = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        Some(dll.parent()?.parent()?.parent()?.join("data"))
    }
    #[cfg(not(target_os = "windows"))]
    None
}

/// Its settings' folder (your own styles): OBS's plugin_config\theater-puppet
pub fn config_dir() -> PathBuf {
    let dir = obs_user_dir().join("plugin_config").join(PLUGIN_ID);
    if !cfg!(test) {
        let _ = std::fs::create_dir_all(dir.join("styles"));
    }
    dir
}

/// OBS's own folder of the user's (its logs)
fn obs_user_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var("APPDATA").map(PathBuf::from).unwrap_or_default().join("obs-studio")
    } else {
        std::env::var("HOME").map(PathBuf::from).unwrap_or_default().join(".config").join("obs-studio")
    }
}

/// A line in OBS's logs folder, theater_puppet.log
pub fn log(msg: &str) {
    if cfg!(test) {
        return;
    }
    use std::io::Write;
    let dir = obs_user_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("theater_puppet.log")) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{secs}: {msg}");
    }
}

/// Panics otherwise vanish on Windows, where OBS has no console
fn install_panic_logger() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log(&format!("[Puppet PANIC] thread '{}': {}", std::thread::current().name().unwrap_or("<unnamed>"), info));
            default_hook(info);
        }));
    });
}

// ---------------------------------------------------------------------------
// The OBS module

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_ver() -> u32 {
    30u32 << 24
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_set_pointer(_module: *mut std::ffi::c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_name() -> *const std::ffi::c_char {
    c"Theater Puppet".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_description() -> *const std::ffi::c_char {
    c"Draws a Theater Rig avatar (GLB) as an OBS source".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_load() -> bool {
    install_panic_logger();
    if let Err(missing) = obs::init() {
        log(&format!("[Puppet] ⚠️ libobs has no {missing}: not loaded"));
        return false;
    }
    source::register();
    parts::register_procs();
    // Tracking: its settings window's source, its inputs, its menu item
    tracking::menu::register();
    tracking::start();
    tracking::menu::setup();
    log("[Puppet] 🚀 Theater Puppet loaded");
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_unload() {
    log("[Puppet] 🛑 Theater Puppet unloaded");
}

/// Keeps the DLL loaded until OBS exits: its menu item and the tracking
/// inputs' threads call into it, and may outlive obs_module_unload.
#[cfg(target_os = "windows")]
fn pin_module() {
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut *mut std::ffi::c_void) -> i32;
    }
    const PIN: u32 = 0x1;
    const FROM_ADDRESS: u32 = 0x4;
    let mut module: *mut std::ffi::c_void = std::ptr::null_mut();
    unsafe {
        GetModuleHandleExW(FROM_ADDRESS | PIN, pin_module as *const u16, &mut module);
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut std::ffi::c_void, reason: u32, _reserved: *mut std::ffi::c_void) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        pin_module();
    }
    1
}
