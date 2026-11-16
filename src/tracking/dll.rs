//! Other programs' libraries (SteamVR's, the Leap's), loaded at run time
//! from where those programs keep them: Puppet builds and runs without them.

use std::ffi::{CStr, c_char, c_void};
use std::path::Path;

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// The library's own folder is searched for what it needs itself
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;

#[derive(Clone, Copy)]
pub struct Library(*mut c_void);

// A module handle: good on any thread
unsafe impl Send for Library {}

impl Library {
    /// The first of `paths` that is there and loads
    pub fn first(paths: &[std::path::PathBuf]) -> Option<(Library, std::path::PathBuf)> {
        paths.iter().find_map(|p| Some((Library::load(p)?, p.clone())))
    }

    pub fn load(path: &Path) -> Option<Library> {
        if !path.is_file() {
            return None;
        }
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
        let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        (!module.is_null()).then_some(Library(module))
    }

    /// `name` as the function pointer type `T`
    pub fn function<T>(&self, name: &CStr) -> Option<T> {
        let p = unsafe { GetProcAddress(self.0, name.as_ptr()) };
        (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
    }
}
