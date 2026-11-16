//! "Tracking…" in the shared 🎭 Theater menu (theater-control-panel's
//! docs/shared-menu.md), and its window: OBS's own properties dialog, on a
//! private source ("theater_puppet_tracking") whose settings are the
//! tracking settings (config.rs). Which inputs are on, the phones'
//! addresses, and which input drives which part of the avatar.
//!
//! The menu item goes through Theater Scene's Qt shim (trust_qt.dll, shipped
//! next to this DLL, as Theater Reader does); without it, OBS's Tools menu.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};

use super::config::{self, Config};
use super::{Part, Pick, Source};
use crate::obs::{self, Ptr};

const ID: &CStr = c"theater_puppet_tracking";
const TITLE: &CStr = c"Theater Puppet: Tracking";
const MENU_TITLE: &CStr = c"\u{1F3AF} Tracking\u{2026}";
const OBS_SOURCE_CAP_DISABLED: u32 = 1 << 10;
const OBS_TASK_UI: u32 = 0;

/// The private source the dialog edits (made the first time it opens)
static SOURCE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `name` from the loaded DLL `lib`, as the function pointer type `T`
fn function<T>(lib: &str, name: &CStr) -> Option<T> {
    let module = unsafe { GetModuleHandleW(wide(lib).as_ptr()) };
    if module.is_null() {
        return None;
    }
    let p = unsafe { GetProcAddress(module, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}

fn key(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn part_key(part: Part) -> CString {
    key(&format!("assign_{part:?}"))
}

fn pick_value(pick: Pick) -> String {
    match pick {
        Pick::Auto => "auto".into(),
        Pick::Off => "off".into(),
        Pick::Only(s) => format!("{s:?}"),
    }
}

fn pick_from(value: &str) -> Pick {
    match value {
        "off" => Pick::Off,
        v => Source::ALL.into_iter().find(|s| format!("{s:?}") == v).map(Pick::Only).unwrap_or(Pick::Auto),
    }
}

type SetString = unsafe extern "C" fn(Ptr, *const c_char, *const c_char);
type SetBool = unsafe extern "C" fn(Ptr, *const c_char, bool);
type SetDouble = unsafe extern "C" fn(Ptr, *const c_char, f64);

/// The settings as the source's OBS data
fn to_data(config: &Config, data: Ptr) {
    let (Some(set_string), Some(set_bool), Some(set_double)) = (
        function::<SetString>("obs.dll", c"obs_data_set_string"),
        function::<SetBool>("obs.dll", c"obs_data_set_bool"),
        function::<SetDouble>("obs.dll", c"obs_data_set_double"),
    ) else {
        return;
    };
    let string = |k: &CStr, v: &str| unsafe { set_string(data, k.as_ptr(), key(v).as_ptr()) };
    let boolean = |k: &CStr, v: bool| unsafe { set_bool(data, k.as_ptr(), v) };
    let double = |k: &CStr, v: f32| unsafe { set_double(data, k.as_ptr(), v as f64) };
    boolean(c"ifm_on", config.ifacialmocap.enabled);
    string(c"ifm_ip", &config.ifacialmocap.ip);
    boolean(c"meow_on", config.meowface.enabled);
    string(c"meow_ip", &config.meowface.ip);
    boolean(c"steamvr", config.steamvr);
    boolean(c"leap", config.leap);
    boolean(c"leap_on_head", config.leap_on_head);
    double(c"leap_height", config.leap_height);
    boolean(c"mediapipe", config.mediapipe);
    string(c"camera", &config.camera);
    string(c"python", &config.python);
    boolean(c"mirror", config.mirror);
    double(c"head_strength", config.head_strength);
    double(c"smoothing", config.smoothing);
    double(c"user_height", config.user_height);
    for part in Part::ALL {
        string(&part_key(part), &pick_value(config.assign.get(part)));
    }
}

fn get_string(data: Ptr, k: &CStr) -> String {
    let p = unsafe { (obs::api().obs_data_get_string)(data, k.as_ptr()) };
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// The source's OBS data as the settings
fn from_data(data: Ptr) -> Config {
    let api = obs::api();
    let boolean = |k: &CStr| unsafe { (api.obs_data_get_bool)(data, k.as_ptr()) };
    let double = |k: &CStr| unsafe { (api.obs_data_get_double)(data, k.as_ptr()) as f32 };
    let mut c = config::get();
    c.ifacialmocap.enabled = boolean(c"ifm_on");
    c.ifacialmocap.ip = get_string(data, c"ifm_ip").trim().to_string();
    c.meowface.enabled = boolean(c"meow_on");
    c.meowface.ip = get_string(data, c"meow_ip").trim().to_string();
    c.steamvr = boolean(c"steamvr");
    c.leap = boolean(c"leap");
    c.leap_on_head = boolean(c"leap_on_head");
    c.leap_height = double(c"leap_height");
    c.mediapipe = boolean(c"mediapipe");
    c.camera = get_string(data, c"camera");
    c.python = get_string(data, c"python").trim().trim_matches('"').to_string();
    c.mirror = boolean(c"mirror");
    c.head_strength = double(c"head_strength");
    c.smoothing = double(c"smoothing");
    c.user_height = double(c"user_height");
    for part in Part::ALL {
        c.assign.set(part, pick_from(&get_string(data, &part_key(part))));
    }
    c
}

unsafe extern "C" fn get_name(_: Ptr) -> *const c_char {
    TITLE.as_ptr()
}

unsafe extern "C" fn create(settings: Ptr, _source: Ptr) -> Ptr {
    unsafe { update(std::ptr::null_mut(), settings) };
    1 as Ptr
}

unsafe extern "C" fn destroy(_: Ptr) {}

unsafe extern "C" fn size(_: Ptr) -> u32 {
    0
}

unsafe extern "C" fn get_defaults(settings: Ptr) {
    let api = obs::api();
    let d = Config::default();
    unsafe {
        (api.obs_data_set_default_double)(settings, c"head_strength".as_ptr(), d.head_strength as f64);
        (api.obs_data_set_default_double)(settings, c"smoothing".as_ptr(), d.smoothing as f64);
        (api.obs_data_set_default_double)(settings, c"user_height".as_ptr(), d.user_height as f64);
        for part in Part::ALL {
            (api.obs_data_set_default_string)(settings, part_key(part).as_ptr(), c"auto".as_ptr());
        }
    }
}

/// The dialog's change: the tracking settings (saved, inputs restarted)
unsafe extern "C" fn update(_: Ptr, settings: Ptr) {
    config::set(from_data(settings));
}

unsafe extern "C" fn render(_: Ptr, _: Ptr) {}

unsafe extern "C" fn get_properties(_: Ptr) -> Ptr {
    let api = obs::api();
    unsafe {
        let props = (api.obs_properties_create)();
        let info = |name: &CStr, text: &str| {
            (api.obs_properties_add_text)(props, name.as_ptr(), key(text).as_ptr(), obs::OBS_TEXT_INFO);
        };
        // What each input says now
        let statuses = super::statuses();
        let status = |s: Source| {
            statuses.iter().find(|(src, _, _)| *src == s).map(|(_, ago, text)| format!("{text} ({:.0} s ago)", ago.as_secs_f32())).unwrap_or_else(|| "off".into())
        };
        info(c"i_inputs", "Inputs: turn on what you use. Reopen this window to refresh what each one says.");
        (api.obs_properties_add_bool)(props, c"ifm_on".as_ptr(), key(&format!("iFacialMocap (iPhone): {}", status(Source::IFacialMocap))).as_ptr());
        (api.obs_properties_add_text)(props, c"ifm_ip".as_ptr(), c"iFacialMocap: the phone's address (the app shows it)".as_ptr(), 0);
        (api.obs_properties_add_bool)(props, c"meow_on".as_ptr(), key(&format!("MeowFace (Android): {}", status(Source::MeowFace))).as_ptr());
        (api.obs_properties_add_text)(props, c"meow_ip".as_ptr(), c"MeowFace: the phone's address (the app shows it)".as_ptr(), 0);
        (api.obs_properties_add_bool)(props, c"steamvr".as_ptr(), key(&format!("SteamVR (headset, controllers, trackers): {}", status(Source::SteamVr))).as_ptr());
        (api.obs_properties_add_bool)(props, c"leap".as_ptr(), key(&format!("Leap Motion (hands): {}", status(Source::Leap))).as_ptr());
        (api.obs_properties_add_bool)(props, c"leap_on_head".as_ptr(), c"The Leap is on the headset, not on the desk".as_ptr());
        (api.obs_properties_add_float_slider)(props, c"leap_height".as_ptr(), c"Leap: hands higher (+) or lower (-), metres".as_ptr(), -0.6, 0.6, 0.01);
        (api.obs_properties_add_bool)(props, c"mediapipe".as_ptr(), key(&format!("MediaPipe (webcam: body and face): {}", status(Source::MediaPipe))).as_ptr());

        // MediaPipe tracks from a camera: any on the PC (its helper opens it)
        let list = (api.obs_properties_add_list)(props, c"camera".as_ptr(), c"MediaPipe: camera".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
        (api.obs_property_list_add_string)(list, c"(none)".as_ptr(), c"".as_ptr());
        for (name, id) in cameras() {
            (api.obs_property_list_add_string)(list, key(&name).as_ptr(), key(&id).as_ptr());
        }

        (api.obs_properties_add_text)(props, c"python".as_ptr(), c"MediaPipe: python.exe with mediapipe and opencv installed (empty: the one on the PATH)".as_ptr(), 0);

        info(c"i_assign", "What tracks what: for each part of the avatar, the input that drives it. Auto takes whichever of its inputs sent last.");
        for part in Part::ALL {
            let list = (api.obs_properties_add_list)(props, part_key(part).as_ptr(), key(part.name()).as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
            (api.obs_property_list_add_string)(list, c"Auto".as_ptr(), c"auto".as_ptr());
            for s in part.inputs() {
                (api.obs_property_list_add_string)(list, key(s.name()).as_ptr(), key(&format!("{s:?}")).as_ptr());
            }
            (api.obs_property_list_add_string)(list, c"Off".as_ptr(), c"off".as_ptr());
        }

        info(c"i_feel", "How it feels:");
        (api.obs_properties_add_bool)(props, c"mirror".as_ptr(), c"Mirror (your left moves the avatar's right)".as_ptr());
        (api.obs_properties_add_float_slider)(props, c"head_strength".as_ptr(), c"Head turn strength".as_ptr(), 0.0, 1.5, 0.05);
        (api.obs_properties_add_float_slider)(props, c"smoothing".as_ptr(), c"Smoothing (0 = raw, 1 = smooth but a little late)".as_ptr(), 0.0, 1.0, 0.05);
        (api.obs_properties_add_float_slider)(props, c"user_height".as_ptr(), c"Your height (metres)".as_ptr(), 1.0, 2.2, 0.01);
        info(c"i_avatar", "Each Theater Puppet source follows this while its \"Follow tracking\" is on.");
        props
    }
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

/// The dialog's source type. Call from obs_module_load.
pub fn register() {
    static INFO: obs::SourceInfo = obs::SourceInfo {
        id: ID.as_ptr(),
        type_: obs::OBS_SOURCE_TYPE_INPUT,
        // Not in the Add Source list: it's only this dialog
        output_flags: obs::OBS_SOURCE_VIDEO | OBS_SOURCE_CAP_DISABLED,
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        get_width: Some(size),
        get_height: Some(size),
        get_defaults: Some(get_defaults),
        get_properties: Some(get_properties),
        update: Some(update),
        activate: None,
        deactivate: None,
        show: None,
        hide: None,
        video_tick: None,
        video_render: Some(render),
    };
    unsafe { (obs::api().obs_register_source_s)(&INFO, std::mem::size_of::<obs::SourceInfo>()) };
}

/// The menu item: OBS's properties dialog on the tracking source. UI thread.
unsafe extern "C" fn open(_: *mut c_void) {
    type DataCreate = unsafe extern "C" fn() -> Ptr;
    type CreatePrivate = unsafe extern "C" fn(*const c_char, *const c_char, Ptr) -> Ptr;
    type OpenProperties = unsafe extern "C" fn(Ptr);
    let mut source = SOURCE.load(Ordering::SeqCst);
    if source.is_null() {
        let (Some(data_create), Some(create_private)) =
            (function::<DataCreate>("obs.dll", c"obs_data_create"), function::<CreatePrivate>("obs.dll", c"obs_source_create_private"))
        else {
            return crate::log("[Puppet] ⚠️ Tracking window: libobs can't make its source");
        };
        unsafe {
            let data = data_create();
            to_data(&config::get(), data);
            source = create_private(ID.as_ptr(), TITLE.as_ptr(), data);
            (obs::api().obs_data_release)(data);
        }
        SOURCE.store(source, Ordering::SeqCst);
    }
    match function::<OpenProperties>("obs-frontend-api.dll", c"obs_frontend_open_source_properties") {
        Some(open) if !source.is_null() => unsafe { open(source) },
        _ => crate::log("[Puppet] ⚠️ Tracking window: OBS can't open it"),
    }
}

type MenuCb = unsafe extern "C" fn(*mut c_void);

/// Adds "Tracking…" to the 🎭 Theater menu once OBS's main window is up.
pub fn setup() {
    let _ = std::thread::Builder::new().name("theater-puppet-menu".into()).spawn(|| {
        if let Some(main_window) = function::<unsafe extern "C" fn() -> *mut c_void>("obs-frontend-api.dll", c"obs_frontend_get_main_window") {
            for _ in 0..150 {
                if !unsafe { main_window() }.is_null() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        type QueueTask = unsafe extern "C" fn(u32, MenuCb, *mut c_void, bool);
        match function::<QueueTask>("obs.dll", c"obs_queue_task") {
            Some(queue) => unsafe { queue(OBS_TASK_UI, add_item, std::ptr::null_mut(), false) },
            None => unsafe { add_item(std::ptr::null_mut()) },
        }
    });
}

/// On OBS's UI thread: the 🎭 Theater menu, else OBS's Tools menu
unsafe extern "C" fn add_item(_: *mut c_void) {
    if in_theater_menu() {
        return crate::log("[Puppet] 🎭 Added 'Tracking…' to the Theater menu");
    }
    type AddTools = unsafe extern "C" fn(*const c_char, MenuCb, *mut c_void);
    match function::<AddTools>("obs-frontend-api.dll", c"obs_frontend_add_tools_menu_item") {
        Some(add) => {
            unsafe { add(TITLE.as_ptr(), open, std::ptr::null_mut()) };
            crate::log("[Puppet] 🔧 No Qt shim (trust_qt.dll): 'Theater Puppet: Tracking' is in OBS's Tools menu");
        }
        None => crate::log("[Puppet] ⚠️ Couldn't add the Tracking menu item"),
    }
}

/// Its group and item in the shared menu, through the shim next to this DLL
fn in_theater_menu() -> bool {
    let Some(dir) = crate::data_dir().and_then(|d| Some(d.parent()?.join("bin").join("64bit"))) else { return false };
    let path = dir.join("trust_qt.dll");
    if !path.is_file() {
        return false;
    }
    let shim = unsafe { LoadLibraryW(wide(&path.to_string_lossy()).as_ptr()) };
    if shim.is_null() {
        return false;
    }
    let get = |name: &CStr| unsafe { GetProcAddress(shim, name.as_ptr()) };
    let (start, add) = (get(c"trust_ui_menu_start_group"), get(c"trust_ui_menu_add_item"));
    if start.is_null() || add.is_null() {
        return false;
    }
    type StartGroup = unsafe extern "C" fn(*const c_char) -> bool;
    type AddItem = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, MenuCb, *mut c_void) -> bool;
    let (start, add): (StartGroup, AddItem) = unsafe { (std::mem::transmute(start), std::mem::transmute(add)) };
    let id = key(crate::PLUGIN_ID);
    let icon = key(&crate::data_dir().map(|d| d.join("icon.png").to_string_lossy().into_owned()).unwrap_or_default());
    unsafe { start(id.as_ptr()) && add(id.as_ptr(), icon.as_ptr(), MENU_TITLE.as_ptr(), open, std::ptr::null_mut()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_survive_the_dialog() {
        for pick in [Pick::Auto, Pick::Off, Pick::Only(Source::Leap), Pick::Only(Source::IFacialMocap)] {
            assert_eq!(pick_from(&pick_value(pick)), pick);
        }
        assert_eq!(pick_from("nonsense"), Pick::Auto);
    }
}
