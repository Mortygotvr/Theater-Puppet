//! libobs's functions Theater Puppet uses, found in obs.dll at run time (no
//! import libraries), and the C structures and constants they take.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

pub type Ptr = *mut c_void;

/// The start of libobs's obs_source_info, up to video_render: OBS copies
/// only the size it's given (obs_register_source_s), the rest stays zero.
#[repr(C)]
pub struct SourceInfo {
    pub id: *const c_char,
    pub type_: u32,
    pub output_flags: u32,
    pub get_name: Option<unsafe extern "C" fn(Ptr) -> *const c_char>,
    pub create: Option<unsafe extern "C" fn(Ptr, Ptr) -> Ptr>,
    pub destroy: Option<unsafe extern "C" fn(Ptr)>,
    pub get_width: Option<unsafe extern "C" fn(Ptr) -> u32>,
    pub get_height: Option<unsafe extern "C" fn(Ptr) -> u32>,
    pub get_defaults: Option<unsafe extern "C" fn(Ptr)>,
    pub get_properties: Option<unsafe extern "C" fn(Ptr) -> Ptr>,
    pub update: Option<unsafe extern "C" fn(Ptr, Ptr)>,
    pub activate: Option<unsafe extern "C" fn(Ptr)>,
    pub deactivate: Option<unsafe extern "C" fn(Ptr)>,
    pub show: Option<unsafe extern "C" fn(Ptr)>,
    pub hide: Option<unsafe extern "C" fn(Ptr)>,
    pub video_tick: Option<unsafe extern "C" fn(Ptr, f32)>,
    pub video_render: Option<unsafe extern "C" fn(Ptr, Ptr)>,
}
unsafe impl Sync for SourceInfo {}

pub const OBS_SOURCE_TYPE_INPUT: u32 = 0;
pub const OBS_SOURCE_TYPE_FILTER: u32 = 1;
pub const OBS_SOURCE_VIDEO: u32 = 1 << 0;
pub const OBS_SOURCE_CUSTOM_DRAW: u32 = 1 << 3;
pub const OBS_PATH_FILE: u32 = 0;
pub const OBS_COMBO_TYPE_LIST: u32 = 2;
pub const OBS_COMBO_FORMAT_STRING: u32 = 3;
pub const OBS_TEXT_INFO: u32 = 3;
pub const OBS_GROUP_NORMAL: u32 = 1;

/// A property's "modified" callback: true redraws the properties
pub type ModifiedCallback = unsafe extern "C" fn(Ptr, Ptr, Ptr) -> bool;
pub const OBS_EFFECT_DEFAULT: u32 = 0;

/// libobs's gs_vb_data (vec3 is 16 bytes: an SSE register)
#[repr(C)]
pub struct VbData {
    pub num: usize,
    pub points: *mut [f32; 4],
    pub normals: *mut [f32; 4],
    pub tangents: *mut [f32; 4],
    pub colors: *mut u32,
    pub num_tex: usize,
    pub tvarray: *mut TVertArray,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TVertArray {
    pub width: usize,
    pub array: *mut c_void,
}

/// libobs's gs_effect_param_info
#[repr(C)]
pub struct EffectParamInfo {
    pub name: *const c_char,
    pub type_: u32,
}

/// gs_shader_param_type's texture
pub const GS_SHADER_PARAM_TEXTURE: u32 = 12;

pub const GS_RGBA: u32 = 3;
pub const GS_RGBA16F: u32 = 9;
pub const GS_ZS_NONE: u32 = 0;
pub const GS_Z24_S8: u32 = 2;
pub const GS_UNSIGNED_LONG: u32 = 1;
/// A buffer updated after creation (gs_vertexbuffer_flush)
pub const GS_DYNAMIC: u32 = 1 << 1;
pub const GS_TRIS: u32 = 3;
pub const GS_CLEAR_COLOR: u32 = 1 << 0;
pub const GS_CLEAR_DEPTH: u32 = 1 << 1;
pub const GS_LESS: u32 = 1;
pub const GS_NEITHER: u32 = 2;
pub const GS_BLEND_ONE: u32 = 1;
pub const GS_BLEND_SRCALPHA: u32 = 4;
pub const GS_BLEND_INVSRCALPHA: u32 = 5;

macro_rules! api {
    ($($name:ident: fn($($arg:ty),* $(,)?) $(-> $ret:ty)?;)*) => {
        pub struct Api { $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)* }
        // Only function pointers into obs.dll, which outlives the plugin
        unsafe impl Send for Api {}
        unsafe impl Sync for Api {}
        fn load() -> Result<Api, &'static str> {
            Ok(Api { $($name: function(concat!(stringify!($name), "\0")).ok_or(stringify!($name))?,)* })
        }
    };
}

api! {
    obs_register_source_s: fn(*const SourceInfo, usize);
    obs_enter_graphics: fn();
    obs_leave_graphics: fn();
    obs_get_base_effect: fn(u32) -> Ptr;
    obs_data_get_string: fn(Ptr, *const c_char) -> *const c_char;
    obs_data_get_int: fn(Ptr, *const c_char) -> i64;
    obs_data_get_double: fn(Ptr, *const c_char) -> f64;
    obs_data_set_default_int: fn(Ptr, *const c_char, i64);
    obs_data_set_default_double: fn(Ptr, *const c_char, f64);
    obs_properties_create: fn() -> Ptr;
    obs_properties_add_path: fn(Ptr, *const c_char, *const c_char, u32, *const c_char, *const c_char) -> Ptr;
    obs_properties_add_int: fn(Ptr, *const c_char, *const c_char, i32, i32, i32) -> Ptr;
    obs_properties_add_float_slider: fn(Ptr, *const c_char, *const c_char, f64, f64, f64) -> Ptr;
    obs_properties_add_int_slider: fn(Ptr, *const c_char, *const c_char, i32, i32, i32) -> Ptr;
    obs_properties_add_bool: fn(Ptr, *const c_char, *const c_char) -> Ptr;
    obs_properties_add_color_alpha: fn(Ptr, *const c_char, *const c_char) -> Ptr;
    obs_properties_add_text: fn(Ptr, *const c_char, *const c_char, u32) -> Ptr;
    obs_properties_add_list: fn(Ptr, *const c_char, *const c_char, u32, u32) -> Ptr;
    obs_properties_add_group: fn(Ptr, *const c_char, *const c_char, u32, Ptr) -> Ptr;
    obs_properties_get: fn(Ptr, *const c_char) -> Ptr;
    obs_property_list_add_string: fn(Ptr, *const c_char, *const c_char) -> usize;
    obs_property_set_visible: fn(Ptr, bool);
    obs_property_set_modified_callback: fn(Ptr, ModifiedCallback);
    obs_data_get_bool: fn(Ptr, *const c_char) -> bool;
    obs_source_get_settings: fn(Ptr) -> Ptr;
    obs_source_get_name: fn(Ptr) -> *const c_char;
    obs_get_proc_handler: fn() -> Ptr;
    proc_handler_add: fn(Ptr, *const c_char, crate::parts::ProcFn, Ptr);
    calldata_get_string: fn(Ptr, *const c_char, *mut *const c_char) -> bool;
    calldata_get_data: fn(Ptr, *const c_char, *mut c_void, usize) -> bool;
    calldata_set_data: fn(Ptr, *const c_char, *const c_void, usize);
    obs_data_release: fn(Ptr);
    obs_data_create: fn() -> Ptr;
    obs_data_set_double: fn(Ptr, *const c_char, f64);
    obs_data_set_string: fn(Ptr, *const c_char, *const c_char);
    obs_source_update: fn(Ptr, Ptr);
    obs_source_enabled: fn(Ptr) -> bool;
    obs_filter_get_parent: fn(Ptr) -> Ptr;
    obs_source_skip_video_filter: fn(Ptr);
    obs_data_set_default_bool: fn(Ptr, *const c_char, bool);
    obs_data_set_default_string: fn(Ptr, *const c_char, *const c_char);
    gs_stagesurface_create: fn(u32, u32, u32) -> Ptr;
    gs_stagesurface_destroy: fn(Ptr);
    gs_stage_texture: fn(Ptr, Ptr);
    gs_stagesurface_map: fn(Ptr, *mut *mut u8, *mut u32) -> bool;
    gs_stagesurface_unmap: fn(Ptr);
    gs_effect_get_num_params: fn(Ptr) -> usize;
    gs_effect_get_param_by_idx: fn(Ptr, usize) -> Ptr;
    gs_effect_get_param_info: fn(Ptr, *mut EffectParamInfo);
    gs_effect_get_val_size: fn(Ptr) -> usize;
    gs_effect_set_val: fn(Ptr, *const c_void, usize);
    gs_effect_set_vec2: fn(Ptr, *const [f32; 2]);
    gs_effect_set_vec3: fn(Ptr, *const [f32; 4]);
    bmalloc: fn(usize) -> *mut c_void;
    bfree: fn(*mut c_void);
    gs_vertexbuffer_create: fn(*mut VbData, u32) -> Ptr;
    gs_vertexbuffer_destroy: fn(Ptr);
    gs_vertexbuffer_get_data: fn(Ptr) -> *mut VbData;
    gs_vertexbuffer_flush: fn(Ptr);
    gs_indexbuffer_create: fn(u32, *mut c_void, usize, u32) -> Ptr;
    gs_indexbuffer_destroy: fn(Ptr);
    gs_load_vertexbuffer: fn(Ptr);
    gs_load_indexbuffer: fn(Ptr);
    gs_draw: fn(u32, u32, u32);
    gs_draw_sprite: fn(Ptr, u32, u32, u32);
    gs_texture_create: fn(u32, u32, u32, u32, *const *const u8, u32) -> Ptr;
    gs_texture_destroy: fn(Ptr);
    gs_effect_create: fn(*const c_char, *const c_char, *mut *mut c_char) -> Ptr;
    gs_effect_destroy: fn(Ptr);
    gs_effect_get_param_by_name: fn(Ptr, *const c_char) -> Ptr;
    gs_effect_set_texture: fn(Ptr, Ptr);
    gs_effect_set_float: fn(Ptr, f32);
    gs_effect_set_vec4: fn(Ptr, *const [f32; 4]);
    gs_effect_set_matrix4: fn(Ptr, *const [f32; 16]);
    gs_effect_loop: fn(Ptr, *const c_char) -> bool;
    gs_texrender_create: fn(u32, u32) -> Ptr;
    gs_texrender_destroy: fn(Ptr);
    gs_texrender_reset: fn(Ptr);
    gs_texrender_begin: fn(Ptr, u32, u32) -> bool;
    gs_texrender_end: fn(Ptr);
    gs_texrender_get_texture: fn(Ptr) -> Ptr;
    gs_clear: fn(u32, *const [f32; 4], f32, u8);
    gs_ortho: fn(f32, f32, f32, f32, f32, f32);
    gs_enable_depth_test: fn(bool);
    gs_depth_function: fn(u32);
    gs_get_cull_mode: fn() -> u32;
    gs_set_cull_mode: fn(u32);
    gs_enable_blending: fn(bool);
    gs_blend_function: fn(u32, u32);
    gs_blend_function_separate: fn(u32, u32, u32, u32);
    gs_blend_state_push: fn();
    gs_blend_state_pop: fn();
}

static API: OnceLock<Api> = OnceLock::new();

/// Finds every function; the name of the first missing one otherwise.
pub fn init() -> Result<(), &'static str> {
    if API.get().is_none() {
        let _ = API.set(load()?);
    }
    Ok(())
}

pub fn api() -> &'static Api {
    API.get().expect("obs::init first")
}

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

#[cfg(not(target_os = "windows"))]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// `name` (nul-terminated) from libobs, as the function pointer type `T`
fn function<T>(name: &str) -> Option<T> {
    let name = CStr::from_bytes_with_nul(name.as_bytes()).ok()?;
    #[cfg(target_os = "windows")]
    let p = {
        let lib: Vec<u16> = "obs.dll".encode_utf16().chain(std::iter::once(0)).collect();
        let module = unsafe { GetModuleHandleW(lib.as_ptr()) };
        if module.is_null() {
            return None;
        }
        unsafe { GetProcAddress(module, name.as_ptr()) }
    };
    #[cfg(not(target_os = "windows"))]
    let p = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
    // SAFETY: T is the function pointer type of the export `name`.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}
