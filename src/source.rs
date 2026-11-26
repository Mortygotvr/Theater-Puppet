//! The "🎭 Theater Puppet" source: pick a GLB and its styles, it's drawn.
//! The file loads on a thread of its own; the next frame uploads it to the
//! GPU. Styles stack in slots (styles.rs); each slot's tick runs every frame
//! (video_tick), and the shader is rebuilt when a slot or a style file
//! changes.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Map};

use crate::model::{self, Model};
use crate::motion::{Motion, Rig};
use crate::springs::{Adjust, KINDS, Tuning};
use crate::obs::{self, Ptr};
use crate::filters::{self, Filter};
use crate::render::{FilterPass, Gpu, PLAIN_STYLE, Shader, View};
use crate::styles::{self, Frame, Prop, Style, Value};

const ID: &CStr = c"theater_puppet";
const NAME: &CStr = c"\u{1F3AD} Theater Puppet";

const S_FILE: &CStr = c"file";
/// GLBs drawn with the avatar in its picture, copying its pose ("Also
/// draw" 1, 2…: as many as added): how many, and each one's file, Show and
/// group (of a group, only one shows: remote.rs). Slot 0 is the avatar.
/// The controller it holds (Arms on the Controller input): a GLB of your own,
/// "" for the built-in one (controller.rs)
const S_CONTROLLER_MODEL: &CStr = c"controller_model";
/// The most there can be
const MAX_EXTRAS: usize = 32;

fn extra_key(n: usize) -> CString {
    cstring(&format!("extra{n}"))
}

/// Slot `slot`'s Show (0: the avatar's)
fn shown_key(slot: usize) -> CString {
    if slot == 0 { cstring("shown") } else { cstring(&format!("extra{slot}.shown")) }
}

fn group_key(n: usize) -> CString {
    cstring(&format!("extra{n}.group"))
}

/// Also draw `n`'s follow settings: (copy movement, from, copy rotation, from, copy shape keys)
fn follow_keys(n: usize) -> [CString; 5] {
    ["copy_move", "move_from", "copy_rotate", "rotate_from", "copy_shapes"].map(|k| cstring(&format!("extra{n}.{k}")))
}

/// Also draw `n`'s follow settings, read
unsafe fn follow_of(settings: Ptr, n: usize) -> crate::motion::FollowSettings {
    let api = obs::api();
    let [mv, mf, rt, rf, sh] = follow_keys(n);
    unsafe {
        crate::motion::FollowSettings {
            move_on: (api.obs_data_get_bool)(settings, mv.as_ptr()),
            move_from: get_string(settings, &mf),
            rotate_on: (api.obs_data_get_bool)(settings, rt.as_ptr()),
            rotate_from: get_string(settings, &rf),
            shapes: (api.obs_data_get_bool)(settings, sh.as_ptr()),
            clip: get_string(settings, &extra_clip_key(n)),
            adjust: read_adjust_for(settings, &extra_prefix(n)),
        }
    }
}

/// An Also draw GLB's own looping clip ("" none)
fn extra_clip_key(n: usize) -> CString {
    cstring(&format!("extra{n}.animation"))
}

/// How many Also draw GLBs a source has: its count, or (a source from
/// before the Add button) its last of extra1–3 with a file
fn extra_count(settings: Ptr) -> usize {
    (1..=MAX_EXTRAS).rev().find(|&n| !get_string(settings, &extra_key(n)).is_empty()).unwrap_or(0)
}
const S_WIDTH: &CStr = c"width";
const S_HEIGHT: &CStr = c"height";
/// The source the canvas's size (OBS's Base resolution); off: width and height
const S_CANVAS: &CStr = c"canvas_size";

/// libobs's obs_video_info, up to the canvas's size (the rest left alone)
#[repr(C)]
struct VideoInfo {
    graphics_module: *const std::ffi::c_char,
    fps_num: u32,
    fps_den: u32,
    base_width: u32,
    base_height: u32,
    rest: [u8; 128],
}

/// OBS's canvas: its width and height
fn canvas() -> Option<(u32, u32)> {
    type GetVideoInfo = unsafe extern "C" fn(*mut VideoInfo) -> bool;
    let get = crate::tracking::obs_function::<GetVideoInfo>(c"obs_get_video_info")?;
    let mut info = VideoInfo { graphics_module: std::ptr::null(), fps_num: 0, fps_den: 0, base_width: 0, base_height: 0, rest: [0; 128] };
    (unsafe { get(&mut info) } && info.base_width > 0 && info.base_height > 0).then_some((info.base_width, info.base_height))
}
const S_ZOOM: &CStr = c"zoom";
const S_CENTRE: &CStr = c"centre";
pub(crate) const S_YAW: &CStr = c"yaw";
// "perspective" (the default) or "orthographic"
const S_PROJECTION: &CStr = c"camera_projection";
/// Where the avatar stands (metres): right, up, towards the camera
pub(crate) const S_POSITION: [&CStr; 3] = [c"position_x", c"position_y", c"position_z"];
// The light (optional): its own direction, colour and strength, or the one
// Puppet always had
const S_LIGHT_ON: &CStr = c"light_custom";
const S_LIGHT_TURN: &CStr = c"light_turn";
const S_LIGHT_HEIGHT: &CStr = c"light_height";
const S_LIGHT_COLOR: &CStr = c"light_color";
const S_LIGHT_STRENGTH: &CStr = c"light_strength";
/// The avatar's own clip looping in place of the idle motion ("" = none)
pub(crate) const S_ANIMATION: &CStr = c"animation";
/// A folder of animation GLBs (Theater Rig's conversions): every avatar plays them (retarget.rs)
const S_ANIM_FOLDER: &CStr = c"animation_folder";
const S_ARMS: &CStr = c"arms_down";
const S_IDLE: &CStr = c"idle";
const S_SPRINGS: &CStr = c"springs";
/// The spring bones' colliders drawn over the picture (to see them)
const S_SHOW_COLLIDERS: &CStr = c"show_colliders";
const S_TRACKED: &CStr = c"tracked";
/// The controller it holds moved from its place on the chest (metres)
const S_CONTROLLER_UP: &CStr = c"controller_up";
const S_CONTROLLER_FORWARD: &CStr = c"controller_forward";
/// The hands on it moved from its grips: up, apart (each side; metres)
const S_HAND_UP: &CStr = c"controller_hand_up";
const S_HAND_APART: &CStr = c"controller_hand_apart";
const S_HAND_ROLL: &CStr = c"controller_hand_roll";
/// A reflection: an OBS source's picture on the avatar (render.rs), how much,
/// flat or 360, how the 360 one is turned (degrees)
const S_REFLECTION_SOURCE: &CStr = c"reflection_source";
const S_REFLECTION_STRENGTH: &CStr = c"reflection_strength";
const S_REFLECTION_MAPPING: &CStr = c"reflection_mapping";
const S_REFLECTION_TURN: &CStr = c"reflection_turn";
// "light" (the default: it lights the avatar) or "mirror"
const S_REFLECTION_USE: &CStr = c"reflection_use";

/// The reflection's settings
#[derive(Clone, Default)]
struct ReflectionSettings {
    source: String,
    strength: f32,
    flat: bool,
    turn: f32,
    light: bool,
}

/// Every source's and scene's name, for the reflection's list
fn source_names() -> Vec<String> {
    unsafe extern "C" fn add(list: Ptr, source: Ptr) -> bool {
        let list = unsafe { &mut *(list as *mut Vec<String>) };
        let p = unsafe { (obs::api().obs_source_get_name)(source) };
        if !p.is_null() {
            list.push(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
        }
        true
    }
    let mut list: Vec<String> = Vec::new();
    unsafe {
        (obs::api().obs_enum_scenes)(add, &mut list as *mut Vec<String> as Ptr);
        (obs::api().obs_enum_sources)(add, &mut list as *mut Vec<String> as Ptr);
    }
    list.sort();
    list.dedup();
    list
}
const S_HAND_FORWARD: &CStr = c"controller_hand_forward";
const S_THUMB_LIFT: &CStr = c"controller_thumb_lift";
// At a desk (Arms on the Keyboard & mouse input): its GLB, what the keys
// show, the hands, a left-handed user, where it is
const S_DESK_MODEL: &CStr = c"desk_model";
const S_DESK_KEYS: &CStr = c"desk_keys";
const S_DESK_HANDS: &CStr = c"desk_hands";
const S_DESK_LEFT: &CStr = c"desk_left_handed";
const S_DESK_UP: &CStr = c"desk_up";
const S_DESK_FORWARD: &CStr = c"desk_forward";
const S_DESK_LEFT_UP: &CStr = c"desk_left_hand_up";
const S_DESK_RIGHT_UP: &CStr = c"desk_right_hand_up";
const S_DESK_LEFT_FORWARD: &CStr = c"desk_left_hand_forward";
const S_DESK_RIGHT_FORWARD: &CStr = c"desk_right_hand_forward";
const S_DESK_HAND_TILT: &CStr = c"desk_hand_tilt";
const S_DESK_CURL: &CStr = c"desk_curl";
const S_DESK_MOUSE_FINGERS: &CStr = c"desk_mouse_fingers";

/// Colour slots (Theater Rig's Color<N>) a source can set
const COLOR_SLOTS: u32 = 100;

/// Colour slot N's settings: recoloured or not, and its colour (OBS's 0xAABBGGRR)
fn color_on_key(slot: u32) -> CString {
    cstring(&format!("color{slot}.on"))
}

fn color_key(slot: u32) -> CString {
    cstring(&format!("color{slot}"))
}

/// OBS's colour (0xAABBGGRR) as sRGB 0–1, and back (opaque)
fn obs_to_rgb(c: i64) -> [f32; 3] {
    [0, 8, 16].map(|s| ((c >> s) & 0xFF) as f32 / 255.0)
}

fn rgb_to_obs(rgb: [f32; 3]) -> i64 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as i64;
    0xFF00_0000 | (byte(rgb[2]) << 16) | (byte(rgb[1]) << 8) | byte(rgb[0])
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn obs_colours_go_both_ways() {
        // OBS: 0xAABBGGRR
        assert_eq!(obs_to_rgb(0xFF0080FF), [1.0, 128.0 / 255.0, 0.0]);
        assert_eq!(rgb_to_obs([1.0, 128.0 / 255.0, 0.0]), 0xFF0080FF);
    }

    #[test]
    fn the_avatar_s_slots_are_listed() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let m = model::load(std::path::Path::new(&path)).unwrap();
        println!("colour slots: {:?}", color_slots(&[&m]));
    }
}

/// The colour slots the avatar (and the GLBs drawn with it) have, each with
/// the colour it shows now (its first material's)
fn color_slots(models: &[&Model]) -> Vec<(u32, [f32; 3])> {
    let mut out: Vec<(u32, [f32; 3])> = Vec::new();
    for m in models.iter().flat_map(|m| m.materials.iter()) {
        if let Some(slot) = m.color_slot.filter(|s| *s < COLOR_SLOTS) {
            if !out.iter().any(|(s, _)| *s == slot) {
                out.push((slot, [m.color[0], m.color[1], m.color[2]]));
            }
        }
    }
    out.sort_by_key(|(s, _)| *s);
    out
}

/// Style slots: one (a style per source; the slot code stays, its setting
/// keeps its name, style1, so saved sources keep theirs)
const SLOTS: usize = 1;
/// A new source's first style
const DEFAULT_STYLE: &str = "toon";

/// Slot `slot`'s setting (style1…)
fn slot_setting(slot: usize) -> CString {
    cstring(&format!("style{}", slot + 1))
}

/// A style's setting in slot `slot` (1.toon.bands): each slot has its own
fn setting_key(slot: usize, style: &str, key: &str) -> CString {
    cstring(&format!("{}.{style}.{key}", slot + 1))
}

fn group_name(slot: usize, style: &str) -> CString {
    cstring(&format!("group_{}_{style}", slot + 1))
}

/// Filter slots, applied in order on the whole picture
const FILTER_SLOTS: usize = 4;

fn filter_slot_setting(slot: usize) -> CString {
    cstring(&format!("filter{}", slot + 1))
}

/// A filter's setting in filter slot `slot` (f1.theater_toon_filter.bands)
fn filter_key(slot: usize, filter: &str, key: &str) -> CString {
    cstring(&format!("f{}.{filter}.{key}", slot + 1))
}

fn filter_group(slot: usize, filter: &str) -> CString {
    cstring(&format!("fgroup_{}_{filter}", slot + 1))
}

/// A filter running in one slot: its `this`, and its values this frame
struct RunningFilter {
    filter: Arc<Filter>,
    this: Dynamic,
    values: Vec<(String, Value)>,
    failed: bool,
}

/// A style running in one slot: its `this`, and what its last tick asked for
struct Running {
    style: Arc<Style>,
    this: Dynamic,
    frame: Frame,
    failed: bool,
}

struct Puppet {
    width: AtomicU32,
    height: AtomicU32,
    view: Mutex<View>,
    file: Mutex<String>,
    /// The animation folder, and its files as last loaded (with their times)
    anim_folder: Mutex<String>,
    anim_stamp: Mutex<Vec<(String, u64)>>,
    /// Each slot's style id ("" = empty)
    slots: Mutex<[String; SLOTS]>,
    /// Each slot's settings, by (slot, style id) (read in update, and again
    /// when a style file changes: its defaults may have)
    style_settings: Mutex<HashMap<(usize, String), Map>>,
    /// styles::generation() when they were read
    styles_read: AtomicU64,
    /// OBS's source, for its settings
    source: Ptr,
    /// Its body parts map, for Theater Scene (parts.rs)
    parts: crate::parts::Shared,
    /// Its points and depth for Scene's 2D effects (points.rs), and its bones now
    space: crate::points::Shared,
    bones: Mutex<Vec<(String, glam::Vec3)>>,
    /// What Control Panel and Scene ask of it, and what it tells them (remote.rs)
    remote: crate::remote::Shared,
    /// GLB prefabs on screen (prefab.rs): stepped by video_tick, drawn by video_render
    prefabs: Mutex<Vec<crate::prefab::Prefab>>,
    /// Colour slots given a colour (sRGB, 0–1)
    colors: Mutex<HashMap<u32, [f32; 3]>>,
    /// The colours as they show this frame (glides, the rainbow: colorfx.rs)
    colors_shown: Mutex<HashMap<u32, crate::colorfx::Look>>,
    /// Its colliders shown (the setting), and where they are this frame
    show_colliders: std::sync::atomic::AtomicBool,
    capsules: Mutex<Vec<(glam::Vec3, glam::Vec3, f32, f32)>>,
    running: Mutex<[Option<Running>; SLOTS]>,
    /// Bumped by each new file, so a slow load that finishes late is dropped
    generation: Arc<AtomicU64>,
    loaded: Arc<Mutex<Option<(u64, Result<Model, String>)>>>,
    /// The GLBs drawn with it: their files, the latest load (bumped like
    /// `generation`), what's loaded, and whether the GPU and rig have them yet
    /// Each Also draw GLB: (slot, file, when it was written)
    extra_files: Mutex<Vec<(usize, String, u64)>>,
    /// When the avatar's file was written (as last loaded)
    file_written: Mutex<u64>,
    /// Seconds since its Camera filter was looked for (camera::ensure)
    camera_checked: Mutex<f32>,
    extra_generation: Arc<AtomicU64>,
    extra_loaded: Arc<Mutex<Option<(u64, Vec<(usize, Arc<Model>)>)>>>,
    /// Each loaded extra with its slot (1, 2…)
    extras: Mutex<Vec<(usize, Arc<Model>)>>,
    /// Each Also draw slot's follow settings
    extra_follow: Mutex<Vec<(usize, crate::motion::FollowSettings)>>,
    /// The controller's file, its latest load (bumped like `generation`),
    /// and the model loaded
    prop_file: Mutex<String>,
    prop_generation: Arc<AtomicU64>,
    prop_loaded: Arc<Mutex<Option<(u64, Option<Arc<Model>>)>>>,
    prop_model: Mutex<Option<Arc<Model>>>,
    /// The controller's file and the desk's (the Arms setting picks one)
    prop_files: Mutex<(String, String)>,
    /// Its reflection's settings
    reflection: Mutex<ReflectionSettings>,
    gpu: Mutex<Option<Gpu>>,
    /// Its skeleton, springs and posed vertices (motion.rs)
    rig: Mutex<Option<Rig>>,
    motion: Mutex<Motion>,
    /// The compiled shader, and which stack (id#version, …) it's from
    shader: Mutex<Option<(String, Shader)>>,
    /// Each filter slot's filter id ("" = empty), settings, running filter,
    /// and its compiled effect (id#version, effect, wants gbuf)
    filter_slots: Mutex<[String; FILTER_SLOTS]>,
    filter_settings: Mutex<HashMap<(usize, String), Map>>,
    filters_read: AtomicU64,
    running_filters: Mutex<[Option<RunningFilter>; FILTER_SLOTS]>,
    filter_effects: Mutex<[Option<(String, Ptr, bool)>; FILTER_SLOTS]>,
}

pub fn register() {
    static INFO: obs::SourceInfo = obs::SourceInfo {
        id: ID.as_ptr(),
        type_: obs::OBS_SOURCE_TYPE_INPUT,
        output_flags: obs::OBS_SOURCE_VIDEO | obs::OBS_SOURCE_CUSTOM_DRAW,
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        get_width: Some(get_width),
        get_height: Some(get_height),
        get_defaults: Some(get_defaults),
        get_properties: Some(get_properties),
        update: Some(update),
        activate: None,
        deactivate: None,
        show: None,
        hide: None,
        video_tick: Some(video_tick),
        video_render: Some(video_render),
    };
    unsafe { (obs::api().obs_register_source_s)(&INFO, std::mem::size_of::<obs::SourceInfo>()) };
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// 0xAARRGGBB (scripts) <-> 0xAABBGGRR (OBS's colour properties)
fn swap_red_blue(c: i64) -> i64 {
    (c & 0xFF00_FF00) | ((c >> 16) & 0xFF) | ((c & 0xFF) << 16)
}

unsafe extern "C" fn get_name(_: Ptr) -> *const c_char {
    NAME.as_ptr()
}

unsafe extern "C" fn get_defaults(settings: Ptr) {
    let api = obs::api();
    let first = cstring(DEFAULT_STYLE);
    unsafe {
        (api.obs_data_set_default_string)(settings, slot_setting(0).as_ptr(), first.as_ptr());
        (api.obs_data_set_default_bool)(settings, S_CANVAS.as_ptr(), true);
        // Also draw GLBs copy everything by default (as they always did)
        for n in 1..=MAX_EXTRAS {
            let [mv, _, rt, _, sh] = follow_keys(n);
            for k in [mv, rt, sh] {
                (api.obs_data_set_default_bool)(settings, k.as_ptr(), true);
            }
        }
        (api.obs_data_set_default_int)(settings, S_WIDTH.as_ptr(), 1080);
        (api.obs_data_set_default_int)(settings, S_HEIGHT.as_ptr(), 1080);
        (api.obs_data_set_default_double)(settings, S_ZOOM.as_ptr(), 1.0);
        // The light's own settings start as the light Puppet always had
        (api.obs_data_set_default_double)(settings, S_LIGHT_TURN.as_ptr(), -22.0);
        (api.obs_data_set_default_double)(settings, S_LIGHT_HEIGHT.as_ptr(), 29.0);
        (api.obs_data_set_default_int)(settings, S_LIGHT_COLOR.as_ptr(), 0xFFFF_FFFF);
        (api.obs_data_set_default_double)(settings, S_LIGHT_STRENGTH.as_ptr(), 1.0);
        (api.obs_data_set_default_double)(settings, S_CENTRE.as_ptr(), 0.0);
        (api.obs_data_set_default_double)(settings, S_YAW.as_ptr(), 0.0);
        for slot in 0..=MAX_EXTRAS {
            (api.obs_data_set_default_bool)(settings, shown_key(slot).as_ptr(), true);
        }
        for key in S_POSITION {
            (api.obs_data_set_default_double)(settings, key.as_ptr(), 0.0);
        }
        let m = Motion::default();
        (api.obs_data_set_default_double)(settings, S_ARMS.as_ptr(), m.arms_down as f64);
        (api.obs_data_set_default_double)(settings, S_IDLE.as_ptr(), m.idle as f64);
        (api.obs_data_set_default_bool)(settings, S_SPRINGS.as_ptr(), m.springs);
        (api.obs_data_set_default_bool)(settings, S_TRACKED.as_ptr(), m.tracked);
        (api.obs_data_set_default_double)(settings, S_HAND_APART.as_ptr(), m.hand_offset[1] as f64);
        (api.obs_data_set_default_string)(settings, S_DESK_KEYS.as_ptr(), c"area".as_ptr());
        (api.obs_data_set_default_string)(settings, S_DESK_HANDS.as_ptr(), c"gaming".as_ptr());
        // The keyboard & mouse as the user tuned it live
        for (key, v) in [
            (S_DESK_UP, 0.38),
            (S_DESK_FORWARD, -0.01),
            (S_DESK_LEFT_UP, -0.052),
            (S_DESK_LEFT_FORWARD, -0.06),
            (S_DESK_RIGHT_UP, -0.045),
            (S_DESK_RIGHT_FORWARD, 0.0),
            (S_DESK_HAND_TILT, -29.0),
            (S_DESK_CURL, -0.54),
            (S_DESK_MOUSE_FINGERS, 0.0),
        ] {
            (api.obs_data_set_default_double)(settings, key.as_ptr(), v);
        }
        // The springs' adjustments: the avatar's, and each Also draw GLB's
        for prefix in std::iter::once(String::new()).chain((1..=MAX_EXTRAS).map(extra_prefix)) {
            for kind in 0..KINDS.len() {
                for what in TUNING {
                    (api.obs_data_set_default_double)(settings, tune_key_for(&prefix, kind, what).as_ptr(), 1.0);
                }
            }
            (api.obs_data_set_default_double)(settings, colliders_key_for(&prefix).as_ptr(), 1.0);
        }
    }
}

/// The spring tuning each kind gets (springs::Tuning's fields), with their labels
const TUNING: [&str; 3] = ["stiffness", "gravity", "drag"];

/// Its settings' prefix: "" for the avatar, "extra<n>." for an Also draw GLB
fn extra_prefix(n: usize) -> String {
    format!("extra{n}.")
}

/// A spring kind's tuning setting for the avatar ("") or an Also draw GLB
/// ("extra2."): extra2.tune.hair.stiffness
fn tune_key_for(prefix: &str, kind: usize, what: &str) -> CString {
    cstring(&format!("{prefix}tune.{}.{what}", KINDS[kind]))
}

fn colliders_key_for(prefix: &str) -> CString {
    cstring(&format!("{prefix}tune.colliders"))
}

/// The spring tuning in `settings`: the avatar's
fn read_adjust(settings: Ptr) -> Adjust {
    read_adjust_for(settings, "")
}

/// The spring tuning of the avatar ("") or of an Also draw GLB ("extra2.")
fn read_adjust_for(settings: Ptr, prefix: &str) -> Adjust {
    let api = obs::api();
    let get = |key: &CStr| unsafe { (api.obs_data_get_double)(settings, key.as_ptr()) as f32 };
    let mut adjust = Adjust::default();
    for (kind, t) in adjust.kinds.iter_mut().enumerate() {
        *t = Tuning { stiffness: get(&tune_key_for(prefix, kind, "stiffness")), gravity: get(&tune_key_for(prefix, kind, "gravity")), drag: get(&tune_key_for(prefix, kind, "drag")) };
    }
    adjust.colliders = get(&colliders_key_for(prefix));
    adjust
}

/// A spring box's sliders: each of `kinds` (its stiffness, gravity, damping),
/// then the colliders' thickness, for the avatar ("") or an Also draw GLB
unsafe fn spring_sliders(group: Ptr, prefix: &str, kinds: &[usize]) {
    let api = obs::api();
    for &kind in kinds {
        let name = KINDS[kind];
        let title = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
        for (what, label, max) in [("stiffness", "stiffness", 4.0), ("gravity", "gravity", 4.0), ("drag", "damping", 2.0)] {
            unsafe { (api.obs_properties_add_float_slider)(group, tune_key_for(prefix, kind, what).as_ptr(), cstring(&format!("{title}: {label}")).as_ptr(), 0.0, max, 0.05) };
        }
    }
    unsafe { (api.obs_properties_add_float_slider)(group, colliders_key_for(prefix).as_ptr(), c"Colliders: thickness".as_ptr(), 0.0, 3.0, 0.05) };
}

/// Every slot's settings for every style: given its script's defaults, then
/// read back
fn read_style_settings(settings: Ptr) -> HashMap<(usize, String), Map> {
    let mut out = HashMap::new();
    for style in styles::all() {
        for slot in 0..SLOTS {
            out.insert((slot, style.id.clone()), read_props(settings, &style.props, &style.defaults, |k| setting_key(slot, &style.id, k)));
        }
    }
    out
}

/// Every filter slot's settings for every filter, the same way
fn read_filter_settings(settings: Ptr) -> HashMap<(usize, String), Map> {
    let mut out = HashMap::new();
    for filter in filters::all() {
        for slot in 0..FILTER_SLOTS {
            out.insert((slot, filter.id.clone()), read_props(settings, &filter.props, &filter.defaults, |k| filter_key(slot, &filter.id, k)));
        }
    }
    out
}

/// A script's settings (its `props`, under `key(name)`): given its
/// defaults, then read back as its `this.settings`
fn read_props(settings: Ptr, props: &[Prop], defaults: &Map, key: impl Fn(&str) -> CString) -> Map {
    let api = obs::api();
    let mut map = Map::new();
    for prop in props {
        let Some(k) = prop.key() else { continue };
        let name = key(k);
        let default = defaults.get(k);
        let value = unsafe {
            match prop {
                Prop::Slider { .. } => {
                    let d = default.and_then(|v| v.as_float().ok().or_else(|| v.as_int().ok().map(|i| i as f64))).unwrap_or(0.0);
                    (api.obs_data_set_default_double)(settings, name.as_ptr(), d);
                    Dynamic::from_float((api.obs_data_get_double)(settings, name.as_ptr()))
                }
                Prop::IntSlider { .. } => {
                    let d = default.and_then(|v| v.as_int().ok().or_else(|| v.as_float().ok().map(|f| f as i64))).unwrap_or(0);
                    (api.obs_data_set_default_int)(settings, name.as_ptr(), d);
                    Dynamic::from_int((api.obs_data_get_int)(settings, name.as_ptr()))
                }
                Prop::Toggle { .. } => {
                    (api.obs_data_set_default_bool)(settings, name.as_ptr(), default.and_then(|v| v.as_bool().ok()).unwrap_or(false));
                    Dynamic::from_bool((api.obs_data_get_bool)(settings, name.as_ptr()))
                }
                Prop::Color { .. } => {
                    let d = default.and_then(|v| v.as_int().ok()).unwrap_or(0xFFFF_FFFF);
                    (api.obs_data_set_default_int)(settings, name.as_ptr(), swap_red_blue(d));
                    Dynamic::from_int(swap_red_blue((api.obs_data_get_int)(settings, name.as_ptr())))
                }
                Prop::Choice { .. } => {
                    let d = cstring(&default.map(|v| v.to_string()).unwrap_or_default());
                    (api.obs_data_set_default_string)(settings, name.as_ptr(), d.as_ptr());
                    // A number when it is one (Scene's filters choose numbers: [label, 2])
                    let s = get_string(settings, &name);
                    s.parse::<i64>()
                        .map(Dynamic::from_int)
                        .or_else(|_| s.parse::<f64>().map(Dynamic::from_float))
                        .unwrap_or_else(|_| Dynamic::from(s))
                }
                Prop::Info { .. } => continue,
            }
        };
        map.insert(k.into(), value);
    }
    map
}

/// The source a properties window is of (set_param in get_properties)
unsafe fn properties_source(props: Ptr) -> Ptr {
    unsafe { (obs::api().obs_properties_get_param)(props) }
}

/// Also draw `n`'s box in the window
fn extra_box(n: usize) -> CString {
    cstring(&format!("extra{n}.group_box"))
}

/// How many Also draw boxes the window shows
unsafe fn shown_boxes(props: Ptr) -> usize {
    let api = obs::api();
    (1..=MAX_EXTRAS)
        .take_while(|&n| {
            let p = unsafe { (api.obs_properties_get)(props, extra_box(n).as_ptr()) };
            !p.is_null() && unsafe { (api.obs_property_visible)(p) }
        })
        .count()
}

/// "Add a GLB": the next box shown, to pick its GLB in. (OBS refreshes the
/// window's widgets after a button, it doesn't build the window again: the
/// boxes are all there, hidden past the last GLB.) An empty one is gone the
/// next time the window opens
/// "🔄 Reload GLBs": the avatar, its Also draw GLBs and its Animation folder
/// read again (a GLB exported again to the same file). Nothing is watched:
/// this is how a change on disk comes in
unsafe extern "C" fn reload_glbs(_props: Ptr, _button: Ptr, data: Ptr) -> bool {
    if data.is_null() {
        return false;
    }
    let puppet = unsafe { &*(data as *const Puppet) };
    // Forgotten: the settings, run again, load them all
    puppet.file.lock().unwrap().clear();
    puppet.extra_files.lock().unwrap().clear();
    puppet.anim_stamp.lock().unwrap().clear();
    crate::log("[Puppet] 🔄 Reloading the GLBs");
    let api = obs::api();
    unsafe {
        let settings = (api.obs_data_create)();
        if !settings.is_null() {
            (api.obs_source_update)(puppet.source, settings);
            (api.obs_data_release)(settings);
        }
    }
    false
}

unsafe extern "C" fn add_extra(props: Ptr, _button: Ptr, _data: Ptr) -> bool {
    let api = obs::api();
    unsafe {
        let n = shown_boxes(props) + 1;
        let p = (api.obs_properties_get)(props, extra_box(n).as_ptr());
        if p.is_null() {
            return false;
        }
        (api.obs_property_set_visible)(p, true);
        if n >= MAX_EXTRAS {
            let add = (api.obs_properties_get)(props, c"extra.add".as_ptr());
            if !add.is_null() {
                (api.obs_property_set_visible)(add, false);
            }
        }
    }
    true
}

/// "Remove" on Also draw `n` (the button's value): the ones after it move up one
unsafe extern "C" fn remove_extra(props: Ptr, _button: Ptr, data: Ptr) -> bool {
    let api = obs::api();
    let n = data as usize;
    unsafe {
        let source = properties_source(props);
        let settings = if source.is_null() { std::ptr::null_mut() } else { (api.obs_source_get_settings)(source) };
        if settings.is_null() {
            return false;
        }
        let count = shown_boxes(props).max(extra_count(settings));
        if (1..=count).contains(&n) {
            for m in n..count {
                let file = get_string(settings, &extra_key(m + 1));
                let group = get_string(settings, &group_key(m + 1));
                (api.obs_data_set_string)(settings, extra_key(m).as_ptr(), cstring(&file).as_ptr());
                (api.obs_data_set_string)(settings, group_key(m).as_ptr(), cstring(&group).as_ptr());
                (api.obs_data_set_bool)(settings, shown_key(m).as_ptr(), (api.obs_data_get_bool)(settings, shown_key(m + 1).as_ptr()));
                let (to, from) = (follow_keys(m), follow_keys(m + 1));
                for k in [0, 2, 4] {
                    (api.obs_data_set_bool)(settings, to[k].as_ptr(), (api.obs_data_get_bool)(settings, from[k].as_ptr()));
                }
                for k in [1, 3] {
                    (api.obs_data_set_string)(settings, to[k].as_ptr(), cstring(&get_string(settings, &from[k])).as_ptr());
                }
            }
            for key in [extra_key(count), group_key(count), shown_key(count)].into_iter().chain(follow_keys(count)) {
                (api.obs_data_erase)(settings, key.as_ptr());
            }
            let last = (api.obs_properties_get)(props, extra_box(count).as_ptr());
            if !last.is_null() {
                (api.obs_property_set_visible)(last, false);
            }
            let add = (api.obs_properties_get)(props, c"extra.add".as_ptr());
            if !add.is_null() {
                (api.obs_property_set_visible)(add, true);
            }
        }
        (api.obs_data_release)(settings);
        (api.obs_source_update)(source, std::ptr::null_mut());
    }
    true
}

unsafe extern "C" fn get_properties(data: Ptr) -> Ptr {
    let api = obs::api();
    let current: [String; SLOTS] = if data.is_null() {
        std::array::from_fn(|i| if i == 0 { DEFAULT_STYLE.to_string() } else { String::new() })
    } else {
        unsafe { &*(data as *const Puppet) }.slots.lock().unwrap().clone()
    };
    let all = styles::all();
    unsafe {
        let props = (api.obs_properties_create)();
        (api.obs_properties_add_path)(props, S_FILE.as_ptr(), c"Avatar (GLB from Theater Rig)".as_ptr(), obs::OBS_PATH_FILE, c"glTF avatar (*.glb *.gltf)".as_ptr(), std::ptr::null());
        (api.obs_properties_add_bool)(props, shown_key(0).as_ptr(), c"Show the avatar".as_ptr());
        // A GLB exported again to the same file: read again (nothing is watched)
        (api.obs_properties_add_button2)(props, c"reload".as_ptr(), c"\u{1F504} Reload GLBs (after exporting again)".as_ptr(), reload_glbs, data);
        // 🎬 The avatar's pose: its own animation, its arms, the idle motion
        let pose = (api.obs_properties_create)();
        // Its animations: the avatar's default (in its GLB) and the Animation folder's
        let clips: Vec<String> = if data.is_null() { Vec::new() } else { (&*(data as *const Puppet)).remote.lock().unwrap().animation.clips.clone() };
        let list = (api.obs_properties_add_list)(pose, S_ANIMATION.as_ptr(), c"Animation (loops)".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
        (api.obs_property_list_add_string)(list, c"(the avatar's default, else the idle motion)".as_ptr(), c"".as_ptr());
        let idle = cstring(crate::animation::IDLE_ONLY);
        (api.obs_property_list_add_string)(list, c"(the idle motion only)".as_ptr(), idle.as_ptr());
        for clip in &clips {
            let c = cstring(clip);
            (api.obs_property_list_add_string)(list, c.as_ptr(), c.as_ptr());
        }
        (api.obs_properties_add_path)(pose, S_ANIM_FOLDER.as_ptr(), c"Animation folder (Theater Rig's animation GLBs, Mixamo DAE \"With Skin\": any avatar plays them)".as_ptr(), obs::OBS_PATH_DIRECTORY, std::ptr::null(), std::ptr::null());
        (api.obs_properties_add_float_slider)(pose, S_ARMS.as_ptr(), c"Arms down (degrees from the T-pose)".as_ptr(), 0.0, 85.0, 1.0);
        (api.obs_properties_add_float_slider)(pose, S_IDLE.as_ptr(), c"Idle motion".as_ptr(), 0.0, 1.0, 0.01);
        (api.obs_properties_add_group)(props, c"pose_box".as_ptr(), c"🎬 Pose and animation".as_ptr(), obs::OBS_GROUP_NORMAL, pose);
        // 🎥 The camera: the picture's size, the lens, where it looks from
        let camera = (api.obs_properties_create)();
        (api.obs_properties_add_bool)(camera, S_CANVAS.as_ptr(), c"Canvas size (as OBS's canvas; off: the width and height below)".as_ptr());
        (api.obs_properties_add_int)(camera, S_WIDTH.as_ptr(), c"Width".as_ptr(), 16, 8192, 1);
        (api.obs_properties_add_int)(camera, S_HEIGHT.as_ptr(), c"Height".as_ptr(), 16, 8192, 1);
        // (Far out: a big prop or scene beside the avatar fits)
        (api.obs_properties_add_float_slider)(camera, S_ZOOM.as_ptr(), c"Zoom".as_ptr(), 0.05, 50.0, 0.01);
        let projection = (api.obs_properties_add_list)(camera, S_PROJECTION.as_ptr(), c"Camera".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
        (api.obs_property_list_add_string)(projection, c"Perspective".as_ptr(), c"perspective".as_ptr());
        (api.obs_property_list_add_string)(projection, c"Orthographic (no perspective)".as_ptr(), c"orthographic".as_ptr());
        (api.obs_properties_add_float_slider)(camera, S_CENTRE.as_ptr(), c"Centre (feet to head)".as_ptr(), -1.0, 1.0, 0.01);
        (api.obs_properties_add_float_slider)(camera, S_YAW.as_ptr(), c"Turn (the camera goes around the avatar)".as_ptr(), -180.0, 180.0, 1.0);
        for (key, label, range) in [
            (S_POSITION[0], c"Position: left / right (m)", 3.0),
            (S_POSITION[1], c"Position: down / up (m)", 2.0),
            (S_POSITION[2], c"Position: back / forward (m)", 3.0),
        ] {
            (api.obs_properties_add_float_slider)(camera, key.as_ptr(), label.as_ptr(), -range, range, 0.01);
        }
        (api.obs_properties_add_group)(props, c"camera_box".as_ptr(), c"🎥 Camera".as_ptr(), obs::OBS_GROUP_NORMAL, camera);
        // The light (optional)
        let light = (api.obs_properties_create)();
        (api.obs_properties_add_bool)(light, S_LIGHT_ON.as_ptr(), c"Its own light (off: from the front, upper left, white)".as_ptr());
        (api.obs_properties_add_float_slider)(light, S_LIGHT_TURN.as_ptr(), c"From: around (degrees; 0 the camera's side, + the picture's right)".as_ptr(), -180.0, 180.0, 1.0);
        (api.obs_properties_add_float_slider)(light, S_LIGHT_HEIGHT.as_ptr(), c"From: height (degrees; 90 straight above)".as_ptr(), -90.0, 90.0, 1.0);
        (api.obs_properties_add_color)(light, S_LIGHT_COLOR.as_ptr(), c"Colour".as_ptr());
        (api.obs_properties_add_float_slider)(light, S_LIGHT_STRENGTH.as_ptr(), c"Strength".as_ptr(), 0.0, 4.0, 0.05);
        (api.obs_properties_add_group)(props, c"light".as_ptr(), c"💡 Light".as_ptr(), obs::OBS_GROUP_NORMAL, light);
        // 🌅 The sky: another source's picture lighting the avatar, or reflected on it
        if !data.is_null() {
            let puppet = &*(data as *const Puppet);
            // A reflection: another source's picture on the avatar
            let own = get_string_of(puppet.source);
            let group = (api.obs_properties_create)();
            let list = (api.obs_properties_add_list)(group, S_REFLECTION_SOURCE.as_ptr(), c"Source (the sky: a picture, 360 or flat)".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
            (api.obs_property_list_add_string)(list, c"(none)".as_ptr(), c"".as_ptr());
            for name in source_names().into_iter().filter(|n| *n != own) {
                let n = cstring(&name);
                (api.obs_property_list_add_string)(list, n.as_ptr(), n.as_ptr());
            }
            let using = (api.obs_properties_add_list)(group, S_REFLECTION_USE.as_ptr(), c"Shows as".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
            (api.obs_property_list_add_string)(using, c"Light (the picture lights the avatar, as Blender's world: no gloss)".as_ptr(), c"light".as_ptr());
            (api.obs_property_list_add_string)(using, c"Mirror (reflected on it)".as_ptr(), c"mirror".as_ptr());
            (api.obs_properties_add_float_slider)(group, S_REFLECTION_STRENGTH.as_ptr(), c"Strength".as_ptr(), 0.0, 3.0, 0.01);
            let mapping = (api.obs_properties_add_list)(group, S_REFLECTION_MAPPING.as_ptr(), c"The picture is".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
            (api.obs_property_list_add_string)(mapping, c"360 (equirectangular)".as_ptr(), c"360".as_ptr());
            (api.obs_property_list_add_string)(mapping, c"Flat (a mirror ball facing the camera)".as_ptr(), c"flat".as_ptr());
            (api.obs_properties_add_float_slider)(group, S_REFLECTION_TURN.as_ptr(), c"360: turn (degrees)".as_ptr(), -180.0, 180.0, 1.0);
            (api.obs_properties_add_group)(props, c"reflection".as_ptr(), c"🌅 Sky (lights the avatar, or reflected on it)".as_ptr(), obs::OBS_GROUP_NORMAL, group);
        }
        // 🎨 The look: colour slots, styles (3D), then filters (the picture)
        (api.obs_properties_add_text)(props, c"look_heading".as_ptr(), c"🎨 Look: colours, styles, filters".as_ptr(), obs::OBS_TEXT_INFO);
        // Colour slots (Theater Rig's Color<N>): the avatar's and its extras'
        if !data.is_null() {
            let puppet = &*(data as *const Puppet);
            let slots = puppet.rig.lock().unwrap().as_ref().map(|r| {
                let models: Vec<&Model> = std::iter::once(&*r.model).chain(r.followers.iter().map(|f| &*f.model)).collect();
                color_slots(&models)
            });
            if let Some(slots) = slots.filter(|s| !s.is_empty()) {
                // Each picker starts at the colour the slot shows now
                let settings = (api.obs_source_get_settings)(puppet.source);
                let group = (api.obs_properties_create)();
                (api.obs_properties_add_text)(
                    group,
                    c"colors_info".as_ptr(),
                    c"Theater Rig's colour slots: every part named Color<N> takes slot N's colour, its shadow the same colour, darker.".as_ptr(),
                    obs::OBS_TEXT_INFO,
                );
                for (slot, rgb) in slots {
                    if !settings.is_null() {
                        (api.obs_data_set_default_int)(settings, color_key(slot).as_ptr(), rgb_to_obs(rgb));
                    }
                    (api.obs_properties_add_bool)(group, color_on_key(slot).as_ptr(), cstring(&format!("Recolour slot {slot}")).as_ptr());
                    (api.obs_properties_add_color)(group, color_key(slot).as_ptr(), cstring(&format!("Colour {slot}")).as_ptr());
                }
                if !settings.is_null() {
                    (api.obs_data_release)(settings);
                }
                (api.obs_properties_add_group)(props, c"colors".as_ptr(), c"Colours".as_ptr(), obs::OBS_GROUP_NORMAL, group);
            }
        }
        (api.obs_properties_add_text)(
            props,
            c"styles_order".as_ptr(),
            c"The style shades the avatar (and its Also draw GLBs) in 3D, before the filters. (none): the colours with a soft light.".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        for slot in 0..SLOTS {
            let label = if SLOTS == 1 { cstring("Style") } else { cstring(&format!("Style {}", slot + 1)) };
            let list = (api.obs_properties_add_list)(props, slot_setting(slot).as_ptr(), label.as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
            (api.obs_property_list_add_string)(list, c"(none)".as_ptr(), c"".as_ptr());
            for s in &all {
                let (name, id) = (cstring(&s.name), cstring(&s.id));
                (api.obs_property_list_add_string)(list, name.as_ptr(), id.as_ptr());
            }
            (api.obs_property_set_modified_callback)(list, style_changed);
            // The slot's settings for each style, shown while it's the one picked
            for s in &all {
                let group = (api.obs_properties_create)();
                for (i, prop) in s.props.iter().enumerate() {
                    add_prop(group, &|k| setting_key(slot, &s.id, k), i, prop);
                }
                let title = if SLOTS == 1 { cstring(&s.name) } else { cstring(&format!("{} {}", s.name, slot + 1)) };
                let g = (api.obs_properties_add_group)(props, group_name(slot, &s.id).as_ptr(), title.as_ptr(), obs::OBS_GROUP_NORMAL, group);
                (api.obs_property_set_visible)(g, s.id == current[slot]);
            }
        }
        let folder = cstring(&format!("Your own styles (.rhai) go in {}", crate::config_dir().join("styles").display()));
        (api.obs_properties_add_text)(props, c"styles_info".as_ptr(), folder.as_ptr(), obs::OBS_TEXT_INFO);

        // Filters: on the whole avatar as one picture, after the styles
        let current_filters: [String; FILTER_SLOTS] =
            if data.is_null() { Default::default() } else { (&*(data as *const Puppet)).filter_slots.lock().unwrap().clone() };
        let all_filters = filters::all();
        (api.obs_properties_add_text)(
            props,
            c"filters_order".as_ptr(),
            c"Filters work on the whole avatar as one picture, after the styles, top to bottom: Theater Scene's Rhai filters (the same files).".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        for slot in 0..FILTER_SLOTS {
            let label = cstring(&format!("Filter {}", slot + 1));
            let list = (api.obs_properties_add_list)(props, filter_slot_setting(slot).as_ptr(), label.as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
            (api.obs_property_list_add_string)(list, c"(none)".as_ptr(), c"".as_ptr());
            for f in &all_filters {
                let (name, id) = (cstring(&f.name), cstring(&f.id));
                (api.obs_property_list_add_string)(list, name.as_ptr(), id.as_ptr());
            }
            (api.obs_property_set_modified_callback)(list, style_changed);
            for f in &all_filters {
                let group = (api.obs_properties_create)();
                for (i, prop) in f.props.iter().enumerate() {
                    add_prop(group, &|k| filter_key(slot, &f.id, k), i, prop);
                }
                let title = cstring(&format!("{} (filter {})", f.name, slot + 1));
                let g = (api.obs_properties_add_group)(props, filter_group(slot, &f.id).as_ptr(), title.as_ptr(), obs::OBS_GROUP_NORMAL, group);
                (api.obs_property_set_visible)(g, f.id == current_filters[slot]);
            }
        }
        let folder = cstring(&format!("Your own filters (.rhai) go in {} (Theater Scene's are listed too)", crate::config_dir().join("filters").display()));
        (api.obs_properties_add_text)(props, c"filters_info".as_ptr(), folder.as_ptr(), obs::OBS_TEXT_INFO);

        // The springs' tuning: the kinds the avatar has (each Also draw GLB's are in its box)
        let kinds: Vec<usize> = if data.is_null() {
            Vec::new()
        } else {
            (&*(data as *const Puppet)).rig.lock().unwrap().as_ref().map(|r| r.spring_kinds()).unwrap_or_default()
        };
        let group = (api.obs_properties_create)();
        (api.obs_properties_add_bool)(group, S_SPRINGS.as_ptr(), c"Spring bones (hair, skirt…)".as_ptr());
        (api.obs_properties_add_bool)(group, S_SHOW_COLLIDERS.as_ptr(), c"Show colliders (orange: where the hair stops; drawn on the picture)".as_ptr());
        (api.obs_properties_add_text)(
            group,
            c"tune_info".as_ptr(),
            c"The avatar's: multipliers on what its file says (1 = as made). Stiffness: how fast it springs back. Gravity: how much it hangs. Damping: how quickly a swing dies down. An Also draw GLB's are in its own box.".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        if kinds.is_empty() {
            (api.obs_properties_add_text)(group, c"tune_none".as_ptr(), c"The avatar has no spring bones (Theater Rig: Rename Springs, or Make Cloth Bones).".as_ptr(), obs::OBS_TEXT_INFO);
        } else {
            spring_sliders(group, "", &kinds);
        }
        (api.obs_properties_add_group)(props, c"spring_tuning".as_ptr(), c"🪢 Spring bones".as_ptr(), obs::OBS_GROUP_NORMAL, group);
        // 📦 The GLBs drawn with it, last (the longest)
        (api.obs_properties_add_text)(props, c"extras_heading".as_ptr(), c"📦 Also draw: GLBs drawn with the avatar (props, outfits, a head…)".as_ptr(), obs::OBS_TEXT_INFO);
        // The Also draw GLBs: each one's file, Show, group, and a button to remove it
        if !data.is_null() {
            let puppet = &*(data as *const Puppet);
            // The buttons find the source through the properties
            (api.obs_properties_set_param)(props, puppet.source, std::ptr::null_mut());
            let settings = (api.obs_source_get_settings)(puppet.source);
            let count = if settings.is_null() { 0 } else { extra_count(settings) };
            if !settings.is_null() {
                (api.obs_data_release)(settings);
            }
            // The avatar's bones, for "from" (its skeleton's joints)
            let bones: Vec<String> = puppet.rig.lock().unwrap().as_ref().map(|r| {
                let mut names: Vec<String> = Vec::new();
                for j in r.model.skins.iter().flat_map(|s| s.joints.iter()) {
                    let name = r.model.nodes[*j].name.clone();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                names
            }).unwrap_or_default();
            // Every box, hidden past the last GLB (Add shows the next)
            for n in 1..=MAX_EXTRAS {
                let group = (api.obs_properties_create)();
                (api.obs_properties_add_path)(group, extra_key(n).as_ptr(), c"GLB".as_ptr(), obs::OBS_PATH_FILE, c"glTF (*.glb *.gltf)".as_ptr(), std::ptr::null());
                (api.obs_properties_add_bool)(group, shown_key(n).as_ptr(), c"Show".as_ptr());
                (api.obs_properties_add_text)(group, group_key(n).as_ptr(), c"Group (only one GLB of a group shows; empty: none)".as_ptr(), obs::OBS_TEXT_DEFAULT);
                // What it copies from the avatar (all off: it doesn't move)
                let [mv, mf, rt, rf, sh] = follow_keys(n);
                for (check, label, from) in [(&mv, c"Copy movement", &mf), (&rt, c"Copy rotation", &rf)] {
                    (api.obs_properties_add_bool)(group, check.as_ptr(), label.as_ptr());
                    let list = (api.obs_properties_add_list)(group, from.as_ptr(), c"  from".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
                    (api.obs_property_list_add_string)(list, c"The armature (each bone from the avatar's of the same name)".as_ptr(), c"".as_ptr());
                    for b in &bones {
                        let name = cstring(b);
                        (api.obs_property_list_add_string)(list, name.as_ptr(), name.as_ptr());
                    }
                }
                (api.obs_properties_add_bool)(group, sh.as_ptr(), c"Copy shape keys".as_ptr());
                // Its own animations (made in Blender, in this GLB), one looping
                let clips: Vec<String> = puppet.extras.lock().unwrap().iter().find(|(slot, _)| *slot == n).map(|(_, m)| m.clips.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
                let list = (api.obs_properties_add_list)(group, extra_clip_key(n).as_ptr(), c"Animation (loops; this GLB's own)".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
                (api.obs_property_list_add_string)(list, c"(none)".as_ptr(), c"".as_ptr());
                for clip in &clips {
                    let c = cstring(clip);
                    (api.obs_property_list_add_string)(list, c.as_ptr(), c.as_ptr());
                }
                (api.obs_property_set_visible)(list, !clips.is_empty());
                // Its spring bones: the kinds it has, its own adjustments
                let spring_kinds: Vec<usize> = {
                    let place = puppet.extras.lock().unwrap().iter().position(|(slot, _)| *slot == n);
                    let rig = puppet.rig.lock().unwrap();
                    place.and_then(|i| rig.as_ref().and_then(|r| r.followers.get(i)).map(|f| f.spring_kinds())).unwrap_or_default()
                };
                if !spring_kinds.is_empty() {
                    (api.obs_properties_add_text)(group, cstring(&format!("extra{n}.springs_info")).as_ptr(), c"🪢 Its spring bones (1 = as made):".as_ptr(), obs::OBS_TEXT_INFO);
                    spring_sliders(group, &extra_prefix(n), &spring_kinds);
                }
                (api.obs_properties_add_button2)(group, cstring(&format!("extra{n}.remove")).as_ptr(), c"Remove".as_ptr(), remove_extra, n as Ptr);
                let b = (api.obs_properties_add_group)(props, extra_box(n).as_ptr(), cstring(&format!("Also draw {n}")).as_ptr(), obs::OBS_GROUP_NORMAL, group);
                (api.obs_property_set_visible)(b, n <= count);
            }
            let add = (api.obs_properties_add_button2)(props, c"extra.add".as_ptr(), c"\u{2795} Add a GLB".as_ptr(), add_extra, std::ptr::null_mut());
            (api.obs_property_set_visible)(add, count < MAX_EXTRAS);
        }
        props
    }
}

/// One property of a script, under `key(name)`
unsafe fn add_prop(group: Ptr, key: &dyn Fn(&str) -> CString, index: usize, prop: &Prop) {
    let api = obs::api();
    unsafe {
        match prop {
            Prop::Slider { key: k, label, min, max, step } => {
                (api.obs_properties_add_float_slider)(group, key(k).as_ptr(), cstring(label).as_ptr(), *min, *max, *step);
            }
            Prop::IntSlider { key: k, label, min, max, step } => {
                (api.obs_properties_add_int_slider)(group, key(k).as_ptr(), cstring(label).as_ptr(), *min as i32, *max as i32, *step as i32);
            }
            Prop::Toggle { key: k, label } => {
                (api.obs_properties_add_bool)(group, key(k).as_ptr(), cstring(label).as_ptr());
            }
            Prop::Color { key: k, label } => {
                (api.obs_properties_add_color_alpha)(group, key(k).as_ptr(), cstring(label).as_ptr());
            }
            Prop::Choice { key: k, label, options } => {
                let list = (api.obs_properties_add_list)(group, key(k).as_ptr(), cstring(label).as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
                for (l, v) in options {
                    (api.obs_property_list_add_string)(list, cstring(l).as_ptr(), cstring(v).as_ptr());
                }
            }
            Prop::Info { label } => {
                (api.obs_properties_add_text)(group, key(&format!("info{index}")).as_ptr(), cstring(label).as_ptr(), obs::OBS_TEXT_INFO);
            }
        }
    }
}

/// A Style or Filter list changed: each slot shows its own script's settings only
unsafe extern "C" fn style_changed(props: Ptr, _prop: Ptr, settings: Ptr) -> bool {
    let api = obs::api();
    let show = |group: CString, visible: bool| unsafe {
        let g = (api.obs_properties_get)(props, group.as_ptr());
        if !g.is_null() {
            (api.obs_property_set_visible)(g, visible);
        }
    };
    let styles = styles::all();
    for slot in 0..SLOTS {
        let current = get_string(settings, &slot_setting(slot));
        for s in &styles {
            show(group_name(slot, &s.id), s.id == current);
        }
    }
    let filters = filters::all();
    for slot in 0..FILTER_SLOTS {
        let current = get_string(settings, &filter_slot_setting(slot));
        for f in &filters {
            show(filter_group(slot, &f.id), f.id == current);
        }
    }
    true
}

unsafe extern "C" fn create(settings: Ptr, source: Ptr) -> Ptr {
    let parts: crate::parts::Shared = Default::default();
    crate::parts::add_source(source, parts.clone());
    let space: crate::points::Shared = Default::default();
    crate::points::add_source(source, space.clone());
    let remote: crate::remote::Shared = Default::default();
    crate::remote::add_source(source, remote.clone());
    let puppet = Box::new(Puppet {
        styles_read: AtomicU64::new(0),
        source,
        parts,
        space,
        bones: Mutex::new(Vec::new()),
        remote,
        prefabs: Mutex::new(Vec::new()),
        colors: Mutex::new(HashMap::new()),
        colors_shown: Mutex::new(HashMap::new()),
        show_colliders: std::sync::atomic::AtomicBool::new(false),
        capsules: Mutex::new(Vec::new()),
        width: AtomicU32::new(1080),
        height: AtomicU32::new(1080),
        view: Mutex::new(View { zoom: 1.0, height: 0.0, yaw: 0.0, orbit: 0.0, orthographic: false, position: glam::Vec3::ZERO, light: View::LIGHT, light_color: [1.0; 3] }),
        file: Mutex::new(String::new()),
        anim_folder: Mutex::new(String::new()),
        anim_stamp: Mutex::new(Vec::new()),
        slots: Mutex::new(Default::default()),
        style_settings: Mutex::new(HashMap::new()),
        running: Mutex::new(Default::default()),
        generation: Arc::new(AtomicU64::new(0)),
        loaded: Arc::new(Mutex::new(None)),
        extra_files: Mutex::new(Vec::new()),
        file_written: Mutex::new(0),
        camera_checked: Mutex::new(1.0),
        extra_generation: Arc::new(AtomicU64::new(0)),
        extra_loaded: Arc::new(Mutex::new(None)),
        extras: Mutex::new(Vec::new()),
        extra_follow: Mutex::new(Vec::new()),
        prop_file: Mutex::new(String::new()),
        prop_generation: Arc::new(AtomicU64::new(0)),
        prop_loaded: Arc::new(Mutex::new(None)),
        prop_model: Mutex::new(None),
        prop_files: Mutex::new((String::new(), String::new())),
        reflection: Mutex::new(ReflectionSettings::default()),
        gpu: Mutex::new(None),
        rig: Mutex::new(None),
        motion: Mutex::new(Motion::default()),
        shader: Mutex::new(None),
        filter_slots: Mutex::new(Default::default()),
        filter_settings: Mutex::new(HashMap::new()),
        filters_read: AtomicU64::new(0),
        running_filters: Mutex::new(Default::default()),
        filter_effects: Mutex::new(Default::default()),
    });
    let data = Box::into_raw(puppet) as Ptr;
    unsafe { update(data, settings) };
    data
}

unsafe extern "C" fn destroy(data: Ptr) {
    let puppet = unsafe { Box::from_raw(data as *mut Puppet) };
    crate::tracking::config::set_own(puppet.source as usize, None);
    crate::parts::remove_source(puppet.source);
    crate::points::remove_source(puppet.source);
    crate::remote::remove_source(puppet.source);
    puppet.generation.fetch_add(1, Ordering::SeqCst);
    let gpu = puppet.gpu.lock().unwrap().take();
    let shader = puppet.shader.lock().unwrap().take();
    let effects: Vec<Ptr> = puppet.filter_effects.lock().unwrap().iter_mut().filter_map(|e| e.take().map(|(_, p, _)| p)).collect();
    if gpu.is_some() || shader.is_some() || !effects.is_empty() {
        let api = obs::api();
        unsafe { (api.obs_enter_graphics)() };
        if let Some(g) = gpu {
            g.destroy();
        }
        if let Some((_, s)) = shader {
            s.destroy();
        }
        for e in effects {
            crate::render::destroy_effect(e);
        }
        unsafe { (api.obs_leave_graphics)() };
    }
}

unsafe extern "C" fn get_width(data: Ptr) -> u32 {
    unsafe { &*(data as *const Puppet) }.width.load(Ordering::Relaxed)
}

unsafe extern "C" fn get_height(data: Ptr) -> u32 {
    unsafe { &*(data as *const Puppet) }.height.load(Ordering::Relaxed)
}

fn api_of() -> &'static obs::Api {
    obs::api()
}

/// A source's name
fn get_string_of(source: Ptr) -> String {
    let p = unsafe { (obs::api().obs_source_get_name)(source) };
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn get_string(settings: Ptr, name: &CStr) -> String {
    let p = unsafe { (obs::api().obs_data_get_string)(settings, name.as_ptr()) };
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

unsafe extern "C" fn update(data: Ptr, settings: Ptr) {
    let puppet = unsafe { &*(data as *const Puppet) };
    let api = obs::api();
    unsafe {
        // The canvas's size, or the width and height given
        let (w, h) = match canvas().filter(|_| (api.obs_data_get_bool)(settings, S_CANVAS.as_ptr())) {
            Some(size) => size,
            None => ((api.obs_data_get_int)(settings, S_WIDTH.as_ptr()).clamp(16, 8192) as u32, (api.obs_data_get_int)(settings, S_HEIGHT.as_ptr()).clamp(16, 8192) as u32),
        };
        puppet.width.store(w.clamp(16, 8192), Ordering::Relaxed);
        puppet.height.store(h.clamp(16, 8192), Ordering::Relaxed);
        *puppet.view.lock().unwrap() = View {
            zoom: (api.obs_data_get_double)(settings, S_ZOOM.as_ptr()) as f32,
            height: (api.obs_data_get_double)(settings, S_CENTRE.as_ptr()) as f32,
            // (Turn takes the camera around the avatar; Puppet Place turns the body)
            yaw: 0.0,
            orbit: (api.obs_data_get_double)(settings, S_YAW.as_ptr()) as f32,
            orthographic: CStr::from_ptr((api.obs_data_get_string)(settings, S_PROJECTION.as_ptr())).to_bytes() == b"orthographic",
            position: glam::Vec3::from_array(S_POSITION.map(|k| (api.obs_data_get_double)(settings, k.as_ptr()) as f32)),
            light: if (api.obs_data_get_bool)(settings, S_LIGHT_ON.as_ptr()) {
                View::light_from((api.obs_data_get_double)(settings, S_LIGHT_TURN.as_ptr()) as f32, (api.obs_data_get_double)(settings, S_LIGHT_HEIGHT.as_ptr()) as f32)
            } else {
                View::LIGHT
            },
            light_color: if (api.obs_data_get_bool)(settings, S_LIGHT_ON.as_ptr()) {
                let k = (api.obs_data_get_double)(settings, S_LIGHT_STRENGTH.as_ptr()) as f32;
                obs_to_rgb((api.obs_data_get_int)(settings, S_LIGHT_COLOR.as_ptr())).map(|c| c * k)
            } else {
                [1.0; 3]
            },
        };
        *puppet.motion.lock().unwrap() = Motion {
            arms_down: (api.obs_data_get_double)(settings, S_ARMS.as_ptr()) as f32,
            idle: (api.obs_data_get_double)(settings, S_IDLE.as_ptr()) as f32,
            yaw: 0.0,
            springs: (api.obs_data_get_bool)(settings, S_SPRINGS.as_ptr()),
            adjust: read_adjust(settings),
            tracked: (api.obs_data_get_bool)(settings, S_TRACKED.as_ptr()),
            controller_offset: [(api.obs_data_get_double)(settings, S_CONTROLLER_UP.as_ptr()).max(0.0) as f32, (api.obs_data_get_double)(settings, S_CONTROLLER_FORWARD.as_ptr()) as f32],
            hand_offset: [(api.obs_data_get_double)(settings, S_HAND_UP.as_ptr()) as f32, (api.obs_data_get_double)(settings, S_HAND_APART.as_ptr()) as f32],
            hand_roll: (api.obs_data_get_double)(settings, S_HAND_ROLL.as_ptr()) as f32,
            hand_forward: (api.obs_data_get_double)(settings, S_HAND_FORWARD.as_ptr()) as f32,
            thumb_lift: (api.obs_data_get_double)(settings, S_THUMB_LIFT.as_ptr()) as f32,
            desk_look: crate::desk::Look {
                detail: match get_string(settings, S_DESK_KEYS).as_str() {
                    "exact" => crate::desk::KeyDetail::Exact,
                    "motion" => crate::desk::KeyDetail::Motion,
                    _ => crate::desk::KeyDetail::Area,
                },
                hands: match get_string(settings, S_DESK_HANDS).as_str() {
                    "switching" => crate::desk::HandMode::Switching,
                    "typing" => crate::desk::HandMode::Typing,
                    _ => crate::desk::HandMode::Gaming,
                },
                left_handed: (api.obs_data_get_bool)(settings, S_DESK_LEFT.as_ptr()),
                hand_up: [S_DESK_LEFT_UP, S_DESK_RIGHT_UP].map(|k| (api.obs_data_get_double)(settings, k.as_ptr()) as f32),
                hand_forward: [S_DESK_LEFT_FORWARD, S_DESK_RIGHT_FORWARD].map(|k| (api.obs_data_get_double)(settings, k.as_ptr()) as f32),
                hand_tilt: (api.obs_data_get_double)(settings, S_DESK_HAND_TILT.as_ptr()) as f32,
                curl: (api.obs_data_get_double)(settings, S_DESK_CURL.as_ptr()) as f32,
                mouse_fingers: (api.obs_data_get_double)(settings, S_DESK_MOUSE_FINGERS.as_ptr()) as f32,
            },
            desk_offset: [(api.obs_data_get_double)(settings, S_DESK_UP.as_ptr()) as f32, (api.obs_data_get_double)(settings, S_DESK_FORWARD.as_ptr()) as f32],
            own_assign: crate::tracking::config::OwnAssign(crate::tracking::Part::ALL.map(|p| crate::tracking::config::pick_from(&get_string(settings, &cstring(&crate::tracking::config::OwnAssign::key(p)))))),
        };
    }
    // Its own tracking picks: MediaPipe tracks what any Puppet wants from it
    let own = puppet.motion.lock().unwrap().own_assign;
    if crate::tracking::config::set_own(puppet.source as usize, Some(own)) {
        let c = crate::tracking::config::get();
        if c.mediapipe {
            crate::tracking::mediapipe::restart(&c);
        }
    }
    puppet.show_colliders.store(unsafe { (api.obs_data_get_bool)(settings, S_SHOW_COLLIDERS.as_ptr()) }, Ordering::Relaxed);
    puppet.remote.lock().unwrap().animation.loop_clip = get_string(settings, S_ANIMATION);
    *puppet.reflection.lock().unwrap() = ReflectionSettings {
        source: get_string(settings, S_REFLECTION_SOURCE),
        strength: unsafe { (api.obs_data_get_double)(settings, S_REFLECTION_STRENGTH.as_ptr()) } as f32,
        flat: get_string(settings, S_REFLECTION_MAPPING) == "flat",
        turn: unsafe { (api.obs_data_get_double)(settings, S_REFLECTION_TURN.as_ptr()) } as f32,
        light: get_string(settings, S_REFLECTION_USE) != "mirror",
    };
    *puppet.colors.lock().unwrap() = (0..COLOR_SLOTS)
        .filter(|&s| unsafe { (api.obs_data_get_bool)(settings, color_on_key(s).as_ptr()) })
        .map(|s| (s, obs_to_rgb(unsafe { (api.obs_data_get_int)(settings, color_key(s).as_ptr()) })))
        .collect();
    *puppet.slots.lock().unwrap() = std::array::from_fn(|slot| get_string(settings, &slot_setting(slot)));
    puppet.styles_read.store(styles::generation(), Ordering::SeqCst);
    *puppet.style_settings.lock().unwrap() = read_style_settings(settings);
    *puppet.filter_slots.lock().unwrap() = std::array::from_fn(|slot| get_string(settings, &filter_slot_setting(slot)));
    puppet.filters_read.store(filters::generation(), Ordering::SeqCst);
    *puppet.filter_settings.lock().unwrap() = read_filter_settings(settings);

    // Which GLBs show, and the extras' groups
    {
        let mut r = puppet.remote.lock().unwrap();
        let count = extra_count(settings);
        r.shown = (0..=count).map(|s| unsafe { (api.obs_data_get_bool)(settings, shown_key(s).as_ptr()) }).collect();
        r.groups = (0..=count).map(|s| if s == 0 { String::new() } else { get_string(settings, &group_key(s)).trim().to_string() }).collect();
        r.update_hidden();
    }

    // The controller it holds and the desk it uses: the built-in ones unless it has its own
    let file = |key: &CStr, built_in: &str| {
        let own = get_string(settings, key);
        if own.trim().is_empty() {
            crate::data_dir().map(|d| d.join("controller").join(built_in).to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            own
        }
    };
    *puppet.prop_files.lock().unwrap() = (file(S_CONTROLLER_MODEL, "gamepad.glb"), file(S_DESK_MODEL, "desk.glb"));
    load_prop(puppet);

    // The GLBs drawn with it: loaded again when their list changes
    let extra_files: Vec<(usize, String, u64)> = (1..=extra_count(settings)).map(|n| (n, get_string(settings, &extra_key(n)))).filter(|(_, f)| !f.is_empty()).map(|(n, f)| { let w = written(&f); (n, f, w) }).collect();
    *puppet.extra_follow.lock().unwrap() = (1..=extra_count(settings)).map(|n| (n, unsafe { follow_of(settings, n) })).collect();
    let mut current_extras = puppet.extra_files.lock().unwrap();
    if *current_extras != extra_files {
        *current_extras = extra_files.clone();
        let generation = puppet.extra_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (loaded, latest) = (puppet.extra_loaded.clone(), puppet.extra_generation.clone());
        std::thread::Builder::new()
            .name("theater-puppet-load-extras".into())
            .spawn(move || {
                let models: Vec<(usize, Arc<Model>)> = extra_files
                    .iter()
                    .filter_map(|(slot, file, _)| match model::load(&PathBuf::from(file)) {
                        Ok(m) => {
                            crate::log(&format!("[Puppet] 📦 Also drawn {slot}: {file}: {} meshes, {} materials", m.primitives.len(), m.materials.len()));
                            Some((*slot, Arc::new(m)))
                        }
                        Err(e) => {
                            crate::log(&format!("[Puppet] ⚠️ {e}"));
                            None
                        }
                    })
                    .collect();
                if latest.load(Ordering::SeqCst) == generation {
                    *loaded.lock().unwrap() = Some((generation, models));
                }
            })
            .ok();
    }
    drop(current_extras);

    let file = get_string(settings, S_FILE);
    let folder = get_string(settings, S_ANIM_FOLDER).trim().to_string();
    // Its own folder's files and the library's (Configuration → Animations)
    let mut stamp = crate::retarget::folder_stamp(&folder);
    stamp.extend(crate::retarget::folder_stamp(&crate::retarget::library().to_string_lossy()));
    let file_written = written(&file);
    {
        let mut current = puppet.file.lock().unwrap();
        let mut current_folder = puppet.anim_folder.lock().unwrap();
        let mut current_stamp = puppet.anim_stamp.lock().unwrap();
        let mut current_written = puppet.file_written.lock().unwrap();
        if *current == file && *current_folder == folder && *current_stamp == stamp && *current_written == file_written {
            return;
        }
        *current = file;
        *current_written = file_written;
        *current_folder = folder;
        *current_stamp = stamp;
    }
    load_avatar(puppet);
}

/// When `path` was written (milliseconds; 0: no file)
fn written(path: &str) -> u64 {
    if path.trim().is_empty() {
        return 0;
    }
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// Runs a Puppet source's settings again (an animation added to the library: read)
pub(crate) fn rerun_settings(source: Ptr) {
    let api = obs::api();
    unsafe {
        let data = (api.obs_data_create)();
        if !data.is_null() {
            (api.obs_source_update)(source, data);
            (api.obs_data_release)(data);
        }
    }
}

/// Loads the avatar (puppet.file) in the background, with the animation
/// folder's clips put on it
fn load_avatar(puppet: &Puppet) {
    let file = puppet.file.lock().unwrap().clone();
    let folder = puppet.anim_folder.lock().unwrap().clone();
    let generation = puppet.generation.fetch_add(1, Ordering::SeqCst) + 1;
    if file.is_empty() {
        // No avatar: an empty one, so its Also draw GLBs (props, parts) still draw
        *puppet.loaded.lock().unwrap() = Some((generation, Ok(Model::empty())));
        return;
    }
    let (loaded, latest) = (puppet.loaded.clone(), puppet.generation.clone());
    std::thread::Builder::new()
        .name("theater-puppet-load".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let mut result = model::load(&PathBuf::from(&file));
            // The library's clips (Configuration → Animations) and its own folder's, put on this avatar
            if let Ok(m) = &mut result {
                let library = crate::retarget::library();
                let mut folders = vec![library.clone()];
                if !folder.is_empty() && std::path::Path::new(&folder) != library {
                    folders.push(PathBuf::from(&folder));
                }
                for dir in folders.into_iter().filter(|d| d.is_dir()) {
                    let (clips, notes) = crate::retarget::folder_onto(&dir, m);
                    for n in notes {
                        crate::log(&format!("[Puppet] ⚠️ Animations: {n}"));
                    }
                    if !clips.is_empty() {
                        crate::log(&format!("[Puppet] 🎞 Animations from {}: {} clip(s) put on the avatar", dir.display(), clips.len()));
                    }
                    m.clips.extend(clips);
                }
            }
            match &result {
                Ok(m) => crate::log(&format!(
                    "[Puppet] 📦 Loaded {file}: {} meshes, {} materials, {} textures, animations [{}] in {} ms",
                    m.primitives.len(),
                    m.materials.len(),
                    m.images.len(),
                    m.clips.iter().map(|c| format!("{} {:.1}s", c.name, c.duration)).collect::<Vec<_>>().join(", "),
                    started.elapsed().as_millis()
                )),
                Err(e) => crate::log(&format!("[Puppet] ⚠️ {e}")),
            }
            if latest.load(Ordering::SeqCst) == generation {
                *loaded.lock().unwrap() = Some((generation, result));
            }
        })
        .ok();
}

/// Runs each filter slot's tick: its shader's values this frame
fn tick_filters(puppet: &Puppet, dt: f32) {
    let slots = puppet.filter_slots.lock().unwrap().clone();
    let all = filters::all();
    let mut running = puppet.running_filters.lock().unwrap();
    for (slot, id) in slots.iter().enumerate() {
        let Some(filter) = all.iter().find(|f| !id.is_empty() && f.id == *id).cloned() else {
            running[slot] = None;
            continue;
        };
        // A new filter, or its file reloaded: a fresh `this`
        if running[slot].as_ref().is_none_or(|r| !Arc::ptr_eq(&r.filter, &filter)) {
            running[slot] = Some(RunningFilter { filter: filter.clone(), this: Dynamic::from_map(Map::new()), values: Vec::new(), failed: false });
        }
        let r = running[slot].as_mut().unwrap();
        let settings = puppet.filter_settings.lock().unwrap().get(&(slot, filter.id.clone())).cloned().unwrap_or_default();
        if let Some(mut this) = r.this.write_lock::<Map>() {
            let time = this.get("time").and_then(|t| t.as_float().ok()).unwrap_or(0.0) + dt as f64;
            this.insert("settings".into(), Dynamic::from_map(settings));
            this.insert("time".into(), Dynamic::from_float(time));
            this.insert("width".into(), Dynamic::from_int(puppet.width.load(Ordering::Relaxed) as i64));
            this.insert("height".into(), Dynamic::from_int(puppet.height.load(Ordering::Relaxed) as i64));
        }
        match filter.tick(&mut r.this, dt) {
            Ok(values) => {
                r.values = values;
                r.failed = false;
            }
            Err(e) => {
                if !r.failed {
                    crate::log(&format!("[Puppet] ⚠️ {e}"));
                }
                r.failed = true;
            }
        }
    }
}

/// This frame's filter chain: each slot's effect (compiled when its filter
/// or file changes; one that doesn't compile is left out) and values.
/// Call on the graphics thread.
fn filter_chain(puppet: &Puppet) -> Vec<FilterPass> {
    let running = puppet.running_filters.lock().unwrap();
    let mut effects = puppet.filter_effects.lock().unwrap();
    let mut chain = Vec::new();
    for slot in 0..FILTER_SLOTS {
        let Some(r) = running[slot].as_ref() else {
            if let Some((_, old, _)) = effects[slot].take() {
                crate::render::destroy_effect(old);
            }
            continue;
        };
        let key = format!("{}#{}", r.filter.id, r.filter.version);
        if effects[slot].as_ref().is_none_or(|(k, _, _)| *k != key) {
            if let Some((_, old, _)) = effects[slot].take() {
                crate::render::destroy_effect(old);
            }
            match crate::render::compile_filter(&r.filter.shader, &r.filter.id) {
                Ok((effect, wants_gbuf)) => effects[slot] = Some((key, effect, wants_gbuf)),
                Err(e) => {
                    crate::log(&format!("[Puppet] ⚠️ Filter {}'s shader: {e}", r.filter.id));
                    // Not tried again until the file changes
                    effects[slot] = Some((key, std::ptr::null_mut(), false));
                }
            }
        }
        if let Some((_, effect, wants_gbuf)) = effects[slot] {
            if !effect.is_null() {
                chain.push(FilterPass { effect, values: r.values.clone(), premultiplied: r.filter.premultiplied, wants_gbuf });
            }
        }
    }
    chain
}

/// Loads the prop the Arms setting asks for (the controller, or the desk
/// while Arms are on the Keyboard & mouse input), when it isn't the one loaded
fn load_prop(puppet: &Puppet) {
    // (This Puppet's own Arms pick, else every Puppet's)
    let own = puppet.motion.lock().unwrap().own_assign;
    let at_desk = own.over(&crate::tracking::config::get().assign).arms == crate::tracking::Pick::Only(crate::tracking::Source::Desk);
    let prop_file = {
        let files = puppet.prop_files.lock().unwrap();
        if at_desk { files.1.clone() } else { files.0.clone() }
    };
    let mut current_prop = puppet.prop_file.lock().unwrap();
    if prop_file.is_empty() || *current_prop == prop_file {
        return;
    }
    {
        *current_prop = prop_file.clone();
        let generation = puppet.prop_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (loaded, latest) = (puppet.prop_loaded.clone(), puppet.prop_generation.clone());
        std::thread::Builder::new()
            .name("theater-puppet-load-controller".into())
            .spawn(move || {
                let model = match model::load(&PathBuf::from(&prop_file)) {
                    Ok(m) => Some(Arc::new(m)),
                    Err(e) => {
                        crate::log(&format!("[Puppet] ⚠️ The controller: {e}"));
                        None
                    }
                };
                if latest.load(Ordering::SeqCst) == generation {
                    *loaded.lock().unwrap() = Some((generation, model));
                }
            })
            .ok();
    }
}

/// Hands what Control Panel and Scene asked (remote.rs) to the rig, and
/// tells them what the Puppet has. The Show settings to write (a shape-key
/// animation ended hiding its GLB), once the locks are let go
fn sync_remote(puppet: &Puppet, rig: &mut Rig) -> Option<Vec<bool>> {
    let mut r = puppet.remote.lock().unwrap();
    let model = rig.model.clone();
    rig.player.sync(&model, &mut r.animation);
    rig.drive_shapes(std::mem::take(&mut r.shapes));
    let mut shown = None;
    for (layer, fade) in std::mem::take(&mut rig.hide) {
        let now = shown.unwrap_or_else(|| r.shown.clone());
        shown = Some(crate::remote::showing(&now, &r.groups, r.slot(layer), false));
        r.pending_fade = r.pending_fade.max(fade);
    }
    // Its GLBs (the avatar first) and colour slots, when they changed
    let models: Vec<&Model> = std::iter::once(&*rig.model).chain(rig.followers.iter().map(|f| &*f.model)).collect();
    if r.glbs.len() != models.len() || r.glbs.iter().zip(&models).any(|(g, m)| g.name != m.name) {
        r.glbs = models
            .iter()
            .zip(std::iter::once(&rig.model).chain(rig.followers.iter().map(|f| &f.model)))
            .map(|(m, arc)| crate::remote::GlbInfo {
                model: Some(arc.clone()),
                name: m.name.clone(),
                keys: m.morph_names.clone(),
                materials: m.materials.iter().map(|x| x.name.clone()).collect(),
                tiles: m.tiles.clone(),
            })
            .collect();
        // A new avatar: the tiles of the old one go
        r.tiles.clear();
        r.slot_colors = color_slots(&models);
        r.slots = r.slot_colors.iter().map(|(s, _)| *s).collect();
    }
    shown
}

/// Writes the GLB slots' Show settings of a Puppet source
pub(crate) fn set_shown(source: Ptr, shown: Vec<bool>) {
    let api = obs::api();
    unsafe {
        let data = (api.obs_data_create)();
        if data.is_null() {
            return;
        }
        for (slot, on) in shown.into_iter().enumerate() {
            (api.obs_data_set_bool)(data, shown_key(slot).as_ptr(), on);
        }
        (api.obs_source_update)(source, data);
        (api.obs_data_release)(data);
    }
}

/// Sets one text setting of a Puppet source (as its properties would)
pub(crate) fn set_string_setting(source: Ptr, key: &CStr, value: &str) {
    let api = obs::api();
    unsafe {
        let data = (api.obs_data_create)();
        if data.is_null() {
            return;
        }
        (api.obs_data_set_string)(data, key.as_ptr(), cstring(value).as_ptr());
        (api.obs_source_update)(source, data);
        (api.obs_data_release)(data);
    }
}

/// A Puppet source's tracking settings, edited in the Puppet Configuration
/// window (theater-scripts/puppet_configuration.rhai) rather than in its
/// properties: (name, 'd' a number, 'b' on / off, 's' text)
pub(crate) const AVATAR_SETTINGS: &[(&str, char)] = &[
    ("tracked", 'b'),
    // Its own tracking assignment, part by part ("" the same as every Puppet's)
    ("assign.Expressions", 's'),
    ("assign.HeadTurn", 's'),
    ("assign.HeadPosition", 's'),
    ("assign.Arms", 's'),
    ("assign.Fingers", 's'),
    ("assign.LowerBody", 's'),
    // The Animation folder (Scene sets it when a redeem it imports brings animations)
    ("animation_folder", 's'),
    ("controller_model", 's'),
    ("controller_up", 'd'),
    ("controller_forward", 'd'),
    ("controller_hand_up", 'd'),
    ("controller_hand_apart", 'd'),
    ("controller_hand_roll", 'd'),
    ("controller_hand_forward", 'd'),
    ("controller_thumb_lift", 'd'),
    ("desk_model", 's'),
    ("desk_keys", 's'),
    ("desk_hands", 's'),
    ("desk_left_handed", 'b'),
    ("desk_up", 'd'),
    ("desk_forward", 'd'),
    ("desk_left_hand_up", 'd'),
    ("desk_left_hand_forward", 'd'),
    ("desk_right_hand_up", 'd'),
    ("desk_right_hand_forward", 'd'),
    ("desk_hand_tilt", 'd'),
    ("desk_curl", 'd'),
    ("desk_mouse_fingers", 'd'),
];

/// Those settings of a Puppet source, as they are (defaults included)
pub(crate) fn avatar_settings(source: Ptr) -> serde_json::Value {
    let api = obs::api();
    let mut out = serde_json::Map::new();
    unsafe {
        let settings = (api.obs_source_get_settings)(source);
        if settings.is_null() {
            return serde_json::Value::Object(out);
        }
        for &(name, kind) in AVATAR_SETTINGS {
            let key = cstring(name);
            let v = match kind {
                'b' => serde_json::json!((api.obs_data_get_bool)(settings, key.as_ptr())),
                's' => serde_json::json!(get_string(settings, &key)),
                _ => serde_json::json!((api.obs_data_get_double)(settings, key.as_ptr())),
            };
            out.insert(name.to_string(), v);
        }
        (api.obs_data_release)(settings);
    }
    serde_json::Value::Object(out)
}

/// Changes some of those settings of a Puppet source (others are left alone)
pub(crate) fn set_avatar_settings(source: Ptr, values: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let api = obs::api();
    unsafe {
        let data = (api.obs_data_create)();
        if data.is_null() {
            return Err("OBS couldn't make settings".into());
        }
        for (name, v) in values {
            let Some(&(_, kind)) = AVATAR_SETTINGS.iter().find(|(n, _)| n == name) else {
                (api.obs_data_release)(data);
                return Err(format!("no setting \"{name}\""));
            };
            let key = cstring(name);
            match kind {
                'b' => (api.obs_data_set_bool)(data, key.as_ptr(), v.as_bool().unwrap_or(false)),
                's' => (api.obs_data_set_string)(data, key.as_ptr(), cstring(v.as_str().unwrap_or("").trim()).as_ptr()),
                _ => (api.obs_data_set_double)(data, key.as_ptr(), v.as_f64().unwrap_or(0.0)),
            }
        }
        (api.obs_source_update)(source, data);
        (api.obs_data_release)(data);
    }
    Ok(())
}

/// The view with what Puppet Place filters on it add (place.rs)
fn placed(puppet: &Puppet) -> View {
    let mut view = *puppet.view.lock().unwrap();
    let o = crate::place::offset(puppet.source);
    view.yaw += o.turn;
    view.position += o.position;
    // The Puppet Camera filters: the camera, and the model's spin
    crate::camera::apply(&mut view, crate::camera::camera(puppet.source));
    view
}

/// Runs each slot's tick: what it sets this frame
unsafe extern "C" fn video_tick(data: Ptr, dt: f32) {
    let puppet = unsafe { &*(data as *const Puppet) };
    crate::snapshot::poll();
    // Its Camera filter: there at all times (added when missing; looked at once a second)
    {
        let mut since = puppet.camera_checked.lock().unwrap();
        *since += dt;
        if *since >= 1.0 {
            *since = 0.0;
            crate::camera::ensure(puppet.source);
            // The microphone's source: listened to once it's there
            crate::lipsync::ensure();
        }
    }
    // The GLB prefabs: new ones from the procedure, each a frame on, the ended ones gone
    {
        let (new, clear) = {
            let mut r = puppet.remote.lock().unwrap();
            (std::mem::take(&mut r.prefabs), std::mem::take(&mut r.prefab_clear))
        };
        let mut prefabs = puppet.prefabs.lock().unwrap();
        if clear {
            prefabs.clear();
        }
        prefabs.retain_mut(|p| p.step(dt));
        prefabs.extend(new);
        let over = prefabs.len().saturating_sub(crate::prefab::MAX);
        prefabs.drain(..over);
    }
    // Arms moved between the controller and the desk: the other GLB
    load_prop(puppet);
    // The avatar's pose and springs, skinned (the render uploads them)
    let mut shown = None;
    if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
        let started = std::time::Instant::now();
        let mut motion = *puppet.motion.lock().unwrap();
        motion.yaw = placed(puppet).yaw;
        shown = sync_remote(puppet, rig);
        {
            let follow = puppet.extra_follow.lock().unwrap();
            rig.follow = puppet.extras.lock().unwrap().iter().map(|(slot, _)| follow.iter().find(|(s, _)| s == slot).map(|(_, f)| f.clone()).unwrap_or_default()).collect();
        }
        puppet.remote.lock().unwrap().step_fade(dt);
        rig.step(dt, motion);
        // Its bones where they are now (projected where it's drawn: points.rs)
        *puppet.bones.lock().unwrap() = rig.points();
        // Its colliders where they are now, to draw (when shown)
        *puppet.capsules.lock().unwrap() = if puppet.show_colliders.load(Ordering::Relaxed) { rig.capsules() } else { Vec::new() };
        crate::timing::tick(started.elapsed());
    }
    // The colour slots as they show: glides and the rainbow, a frame on
    {
        // The properties' colours, and the redeems' over them
        let mut set = puppet.colors.lock().unwrap().clone();
        let mut r = puppet.remote.lock().unwrap();
        set.extend(r.color_over.iter().map(|(s, c)| (*s, *c)));
        let slots = r.slot_colors.clone();
        let shown = r.color_fx.step(dt, &set, &slots);
        drop(r);
        *puppet.colors_shown.lock().unwrap() = shown;
    }
    // The GLB prefabs posed: those with the avatar's bones following its pose of this frame
    {
        let rig = puppet.rig.lock().unwrap();
        let main = rig.as_ref().map(|r| r.main());
        for p in puppet.prefabs.lock().unwrap().iter_mut() {
            p.pose_now(main.as_ref());
        }
    }
    if let Some(shown) = shown {
        set_shown(puppet.source, shown);
    }
    // Timed commands (remote.rs) whose time is up
    crate::remote::run_due(puppet.source, &puppet.remote);
    // A style or filter file changed: its settings' defaults may have too
    let generation = styles::generation();
    let filters_generation = filters::generation();
    let styles_changed = puppet.styles_read.swap(generation, Ordering::SeqCst) != generation;
    let filters_changed = puppet.filters_read.swap(filters_generation, Ordering::SeqCst) != filters_generation;
    if styles_changed || filters_changed {
        let api = obs::api();
        unsafe {
            let settings = (api.obs_source_get_settings)(puppet.source);
            if !settings.is_null() {
                if styles_changed {
                    *puppet.style_settings.lock().unwrap() = read_style_settings(settings);
                }
                if filters_changed {
                    *puppet.filter_settings.lock().unwrap() = read_filter_settings(settings);
                }
                (api.obs_data_release)(settings);
            }
        }
    }
    tick_filters(puppet, dt);
    let slots = puppet.slots.lock().unwrap().clone();
    let all = styles::all();
    let mut running = puppet.running.lock().unwrap();
    for (slot, id) in slots.iter().enumerate() {
        let Some(style) = all.iter().find(|s| !id.is_empty() && s.id == *id).cloned() else {
            running[slot] = None;
            continue;
        };
        // A new style, or its file reloaded: a fresh `this`
        if running[slot].as_ref().is_none_or(|r| !Arc::ptr_eq(&r.style, &style)) {
            running[slot] = Some(Running { style: style.clone(), this: Dynamic::from_map(Map::new()), frame: Frame::default(), failed: false });
        }
        let r = running[slot].as_mut().unwrap();
        let settings = puppet.style_settings.lock().unwrap().get(&(slot, style.id.clone())).cloned().unwrap_or_default();
        if let Some(mut this) = r.this.write_lock::<Map>() {
            let time = this.get("time").and_then(|t| t.as_float().ok()).unwrap_or(0.0) + dt as f64;
            this.insert("settings".into(), Dynamic::from_map(settings));
            this.insert("time".into(), Dynamic::from_float(time));
            this.insert("width".into(), Dynamic::from_int(puppet.width.load(Ordering::Relaxed) as i64));
            this.insert("height".into(), Dynamic::from_int(puppet.height.load(Ordering::Relaxed) as i64));
        }
        match style.tick(&mut r.this, dt) {
            Ok(frame) => {
                r.frame = frame;
                r.failed = false;
            }
            Err(e) => {
                if !r.failed {
                    crate::log(&format!("[Puppet] ⚠️ {e}"));
                }
                r.failed = true;
            }
        }
    }
}

/// The stack's shader code, its key (rebuilt when it changes), and this
/// frame's values with each slot's names prefixed
fn stack(running: &[Option<Running>; SLOTS]) -> (String, String, Frame) {
    let layers: Vec<(usize, &Running)> = running.iter().enumerate().filter_map(|(i, r)| r.as_ref().map(|r| (i, r))).collect();
    if layers.is_empty() {
        return ("plain".into(), PLAIN_STYLE.into(), Frame::default());
    }
    let key = layers.iter().map(|(i, r)| format!("{i}:{}#{}", r.style.id, r.style.version)).collect::<Vec<_>>().join(",");
    let code = styles::compose(&layers.iter().map(|(i, r)| (*i, r.style.shader.as_str())).collect::<Vec<_>>());
    let mut frame = Frame::default();
    for (i, r) in &layers {
        let prefix = styles::slot_prefix(*i);
        frame.uniforms.extend(r.frame.uniforms.iter().map(|(k, v)| (format!("{prefix}{k}"), v.clone())));
        // The last slot with an outline draws it
        if r.frame.outline_width > 0.0 {
            frame.outline_width = r.frame.outline_width;
            frame.outline_color = r.frame.outline_color;
            frame.outline_jitter = r.frame.outline_jitter;
            frame.outline_boil = r.frame.outline_boil;
        }
        if r.frame.lines.width > 0.0 {
            frame.lines = r.frame.lines.clone();
        }
    }
    (key, code, frame)
}

unsafe extern "C" fn video_render(data: Ptr, _effect: Ptr) {
    let puppet = unsafe { &*(data as *const Puppet) };
    // Inside a Puppet's reflection source: not drawn (no endless mirror)
    if crate::render::IN_REFLECTION.with(|f| f.get()) {
        return;
    }
    let mut gpu = puppet.gpu.lock().unwrap();
    // The GLBs drawn with it, newly loaded
    let mut extras_changed = false;
    if let Some((generation, models)) = puppet.extra_loaded.lock().unwrap().take() {
        if generation == puppet.extra_generation.load(Ordering::SeqCst) {
            *puppet.extras.lock().unwrap() = models;
            extras_changed = true;
        }
    }
    // The controller, newly loaded
    if let Some((generation, model)) = puppet.prop_loaded.lock().unwrap().take() {
        if generation == puppet.prop_generation.load(Ordering::SeqCst) {
            *puppet.prop_model.lock().unwrap() = model;
            extras_changed = true;
        }
    }
    // A newly loaded file replaces what's on the GPU
    if let Some((generation, result)) = puppet.loaded.lock().unwrap().take() {
        extras_changed = true;
        if generation == puppet.generation.load(Ordering::SeqCst) {
            if let Some(old) = gpu.take() {
                old.destroy();
            }
            match result.and_then(|m| Gpu::upload(&m).map(|g| (g, m))) {
                Ok((g, m)) => {
                    *gpu = Some(g);
                    // Its skeleton and springs, moved from video_tick on
                    *puppet.rig.lock().unwrap() = Some(Rig::new(Arc::new(m)));
                }
                Err(e) => crate::log(&format!("[Puppet] ⚠️ {e}")),
            }
        }
    }
    // The GPU and the rig take the GLBs drawn with it (a new avatar too)
    if extras_changed {
        let loaded = puppet.extras.lock().unwrap().clone();
        let extras: Vec<Arc<Model>> = loaded.iter().map(|(_, m)| m.clone()).collect();
        {
            let mut r = puppet.remote.lock().unwrap();
            r.layer_slot = std::iter::once(0).chain(loaded.iter().map(|(s, _)| *s)).collect();
            r.update_hidden();
        }
        // The controller drawn last, after the extras
        let prop = puppet.prop_model.lock().unwrap().clone();
        if let Some(g) = gpu.as_mut() {
            let all: Vec<Arc<Model>> = extras.iter().cloned().chain(prop.clone()).collect();
            g.set_extras(&all, extras.len());
            *g.prop_layer.lock().unwrap() = prop.as_ref().map(|_| extras.len() + 1);
        }
        if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
            rig.set_extras(&extras);
            rig.set_prop(prop);
        }
    }
    // The GLB prefabs: their meshes (a new one's made, a gone one's freed), posed
    if let Some(g) = gpu.as_mut() {
        let mut prefabs = puppet.prefabs.lock().unwrap();
        let now: Vec<(u64, String, Arc<Model>, f32)> = prefabs.iter().map(|p| (p.id, p.key.clone(), p.model.clone(), p.fade)).collect();
        g.sync_prefabs(&now);
        for p in prefabs.iter_mut().filter(|p| p.dirty) {
            g.update_prefab(p.id, &p.points, &p.normals);
            p.dirty = false;
        }
    }
    let Some(g) = gpu.as_ref() else { return };
    let timing = crate::timing::begin_render();
    // The controller's layer and how much it's held (set below)
    let mut prop_fade: Option<(usize, f32)> = None;
    // This frame's pose
    if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
        if rig.dirty {
            g.update_vertices(0, &rig.points, &rig.normals);
            rig.dirty = false;
        }
        for (i, f) in rig.followers.iter_mut().enumerate() {
            if f.dirty {
                g.update_vertices(i + 1, &f.points, &f.normals);
                f.dirty = false;
            }
        }
        let layer = rig.followers.len() + 1;
        if let Some(p) = rig.prop.as_mut() {
            if p.dirty {
                g.update_vertices(layer, &p.points, &p.normals);
                p.dirty = false;
            }
            prop_fade = Some((layer, p.weight));
        }
        // The tiles showing: each material's texture placement
        let (tiles, tile_fade) = {
            let r = puppet.remote.lock().unwrap();
            (r.tiles.clone(), r.tile_fade.clone())
        };
        let models: Vec<Arc<Model>> = std::iter::once(rig.model.clone()).chain(rig.followers.iter().map(|f| f.model.clone())).collect();
        g.set_uvs(|layer, material| {
            let seconds = tile_fade.iter().find(|(k, _)| *k == (layer, material)).map(|(_, s)| *s).unwrap_or(0.0);
            let base = models.get(layer).and_then(|m| m.materials.get(material)).map(|m| m.uv);
            (base.and_then(|base| crate::remote::tiled(base, &tiles, layer, material)), seconds)
        });
    }

    let (key, code, frame) = stack(&puppet.running.lock().unwrap());
    let mut shader = puppet.shader.lock().unwrap();
    if shader.as_ref().is_none_or(|(k, _)| *k != key) {
        if let Some((_, old)) = shader.take() {
            old.destroy();
        }
        let compiled = Shader::compile(&code).or_else(|e| {
            // A stack that doesn't compile draws plain, until its files are fixed
            crate::log(&format!("[Puppet] ⚠️ The styles' shader ({key}): {e}"));
            Shader::compile(PLAIN_STYLE)
        });
        match compiled {
            Ok(s) => *shader = Some((key, s)),
            Err(e) => crate::log(&format!("[Puppet] ⚠️ The plain shader didn't compile: {e}")),
        }
    }
    *g.colors.lock().unwrap() = puppet.colors_shown.lock().unwrap().clone();
    *g.capsules.lock().unwrap() = puppet.capsules.lock().unwrap().clone();
    {
        let mut fade = puppet.remote.lock().unwrap().fade.clone();
        // The controller: shown as much as it's held
        if let Some((layer, weight)) = prop_fade {
            if fade.len() <= layer {
                fade.resize(layer + 1, 1.0);
            }
            fade[layer] = weight;
        }
        *g.fade.lock().unwrap() = fade;
    }
    if let Some((_, s)) = shader.as_ref() {
        let view = placed(puppet);
        let (width, height) = (puppet.width.load(Ordering::Relaxed), puppet.height.load(Ordering::Relaxed));
        let filters = filter_chain(puppet);
        // The reflection's picture: its source drawn now
        let r = puppet.reflection.lock().unwrap().clone();
        let reflection = (!r.source.is_empty() && r.strength > 0.0)
            .then(|| {
                let name = cstring(&r.source);
                let source = unsafe { (api_of().obs_get_source_by_name)(name.as_ptr()) };
                if source.is_null() {
                    return None;
                }
                let texture = g.render_source(source);
                unsafe { (api_of().obs_source_release)(source) };
                texture.map(|texture| crate::render::Reflection { texture, strength: r.strength, flat: r.flat, turn: r.turn.to_radians(), light: r.light })
            })
            .flatten();
        g.draw(width, height, view, s, &frame, &filters, reflection);
        // Its points and depth, for Scene's 2D effects (points.rs)
        {
            let bones = puppet.bones.lock().unwrap().clone();
            let at: Vec<glam::Vec3> = bones.iter().map(|(_, p)| *p).collect();
            let projected = g.project(width, height, view, &at);
            let wanted = puppet.space.lock().unwrap().depth_wanted();
            let depth = if wanted { g.depth(width, height, view, s, &frame).map_or(0, |t| t as usize) } else { 0 };
            let mut space = puppet.space.lock().unwrap();
            space.width = width;
            space.height = height;
            space.points = bones.into_iter().map(|(n, _)| n).zip(projected).collect();
            space.depth = depth;
        }
        // The body parts map, for Theater Scene
        if let Some(pixels) = g.parts(width, height, view, s) {
            let map = crate::parts::PartMap { pixels, width, height };
            if crate::snapshot::parts_wanted() {
                crate::parts::save_png(&map);
            }
            *puppet.parts.lock().unwrap() = Some(map);
        }
    }
    crate::timing::end_render(timing);
}
