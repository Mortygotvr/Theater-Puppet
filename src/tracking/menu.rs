//! Helpers the tracking inputs share with OBS: a function of a loaded DLL
//! (libobs's, the frontend's), and the PC's cameras.
//!
//! (The Tracking… window that was here, OBS's properties dialog on a private
//! source, is gone: the Puppet Configuration window has every option.)

use std::ffi::{CStr, c_char, c_void};

use crate::obs::{self, Ptr};

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `name` from the loaded DLL `lib`, as the function pointer type `T`
pub(super) fn function<T>(lib: &str, name: &CStr) -> Option<T> {
    let module = unsafe { GetModuleHandleW(wide(lib).as_ptr()) };
    if module.is_null() {
        return None;
    }
    let p = unsafe { GetProcAddress(module, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}

/// Every camera on the PC, as OBS's own Video Capture Device source lists
/// them: (name, device id)
pub(super) fn cameras() -> Vec<(String, String)> {
    type GetProps = unsafe extern "C" fn(*const c_char) -> Ptr;
    type Count = unsafe extern "C" fn(Ptr) -> usize;
    type Item = unsafe extern "C" fn(Ptr, usize) -> *const c_char;
    type Destroy = unsafe extern "C" fn(Ptr);
    let (Some(get_props), Some(count), Some(item_name), Some(item_string), Some(destroy)) = (
        function::<GetProps>("obs.dll", c"obs_get_source_properties"),
        function::<Count>("obs.dll", c"obs_property_list_item_count"),
        function::<Item>("obs.dll", c"obs_property_list_item_name"),
        function::<Item>("obs.dll", c"obs_property_list_item_string"),
        function::<Destroy>("obs.dll", c"obs_properties_destroy"),
    ) else {
        return Vec::new();
    };
    let text = |p: *const c_char| if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
    let mut out = Vec::new();
    unsafe {
        let props = get_props(c"dshow_input".as_ptr());
        if props.is_null() {
            return out;
        }
        let devices = (obs::api().obs_properties_get)(props, c"video_device_id".as_ptr());
        if !devices.is_null() {
            for i in 0..count(devices) {
                let (name, id) = (text(item_name(devices, i)), text(item_string(devices, i)));
                if !id.is_empty() {
                    out.push((name, id));
                }
            }
        }
        destroy(props);
    }
    out
}
