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
/// GLBs drawn with the avatar in its picture, copying its pose
const S_EXTRAS: [&CStr; 3] = [c"extra1", c"extra2", c"extra3"];
/// Each GLB slot shown (0 the avatar, 1–3 its extras), and the extras' groups
/// (of a group, only one shows: remote.rs)
const S_SHOWN: [&CStr; 4] = [c"shown", c"extra1.shown", c"extra2.shown", c"extra3.shown"];
const S_GROUP: [&CStr; 3] = [c"extra1.group", c"extra2.group", c"extra3.group"];
const S_WIDTH: &CStr = c"width";
const S_HEIGHT: &CStr = c"height";
const S_ZOOM: &CStr = c"zoom";
const S_CENTRE: &CStr = c"centre";
pub(crate) const S_YAW: &CStr = c"yaw";
/// Where the avatar stands (metres): right, up, towards the camera
pub(crate) const S_POSITION: [&CStr; 3] = [c"position_x", c"position_y", c"position_z"];
/// The avatar's own clip looping in place of the idle motion ("" = none)
pub(crate) const S_ANIMATION: &CStr = c"animation";
const S_ARMS: &CStr = c"arms_down";
const S_IDLE: &CStr = c"idle";
const S_SPRINGS: &CStr = c"springs";
const S_TRACKED: &CStr = c"tracked";

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

/// Style slots, applied in order
const SLOTS: usize = 4;
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
    /// What Control Panel and Scene ask of it, and what it tells them (remote.rs)
    remote: crate::remote::Shared,
    /// Colour slots given a colour (sRGB, 0–1)
    colors: Mutex<HashMap<u32, [f32; 3]>>,
    running: Mutex<[Option<Running>; SLOTS]>,
    /// Bumped by each new file, so a slow load that finishes late is dropped
    generation: Arc<AtomicU64>,
    loaded: Arc<Mutex<Option<(u64, Result<Model, String>)>>>,
    /// The GLBs drawn with it: their files, the latest load (bumped like
    /// `generation`), what's loaded, and whether the GPU and rig have them yet
    extra_files: Mutex<Vec<(usize, String)>>,
    extra_generation: Arc<AtomicU64>,
    extra_loaded: Arc<Mutex<Option<(u64, Vec<(usize, Arc<Model>)>)>>>,
    /// Each loaded extra with its slot (1–3)
    extras: Mutex<Vec<(usize, Arc<Model>)>>,
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
        (api.obs_data_set_default_int)(settings, S_WIDTH.as_ptr(), 1080);
        (api.obs_data_set_default_int)(settings, S_HEIGHT.as_ptr(), 1080);
        (api.obs_data_set_default_double)(settings, S_ZOOM.as_ptr(), 1.0);
        (api.obs_data_set_default_double)(settings, S_CENTRE.as_ptr(), 0.0);
        (api.obs_data_set_default_double)(settings, S_YAW.as_ptr(), 0.0);
        for key in S_SHOWN {
            (api.obs_data_set_default_bool)(settings, key.as_ptr(), true);
        }
        for key in S_POSITION {
            (api.obs_data_set_default_double)(settings, key.as_ptr(), 0.0);
        }
        let m = Motion::default();
        (api.obs_data_set_default_double)(settings, S_ARMS.as_ptr(), m.arms_down as f64);
        (api.obs_data_set_default_double)(settings, S_IDLE.as_ptr(), m.idle as f64);
        (api.obs_data_set_default_bool)(settings, S_SPRINGS.as_ptr(), m.springs);
        (api.obs_data_set_default_bool)(settings, S_TRACKED.as_ptr(), m.tracked);
        for kind in 0..KINDS.len() {
            for what in TUNING {
                (api.obs_data_set_default_double)(settings, tune_key(kind, what).as_ptr(), 1.0);
            }
        }
        (api.obs_data_set_default_double)(settings, S_COLLIDERS.as_ptr(), 1.0);
    }
}

/// The spring tuning each kind gets (springs::Tuning's fields), with their labels
const TUNING: [&str; 3] = ["stiffness", "gravity", "drag"];
const S_COLLIDERS: &CStr = c"tune.colliders";

/// A spring kind's tuning setting (tune.hair.stiffness)
fn tune_key(kind: usize, what: &str) -> CString {
    cstring(&format!("tune.{}.{what}", KINDS[kind]))
}

/// The spring tuning in `settings`
fn read_adjust(settings: Ptr) -> Adjust {
    let api = obs::api();
    let get = |key: &CStr| unsafe { (api.obs_data_get_double)(settings, key.as_ptr()) as f32 };
    let mut adjust = Adjust::default();
    for (kind, t) in adjust.kinds.iter_mut().enumerate() {
        *t = Tuning { stiffness: get(&tune_key(kind, "stiffness")), gravity: get(&tune_key(kind, "gravity")), drag: get(&tune_key(kind, "drag")) };
    }
    adjust.colliders = get(S_COLLIDERS);
    adjust
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
        for (i, key) in S_EXTRAS.iter().enumerate() {
            let label = cstring(&format!("Also draw {} (a GLB copying the avatar's pose)", i + 1));
            (api.obs_properties_add_path)(props, key.as_ptr(), label.as_ptr(), obs::OBS_PATH_FILE, c"glTF (*.glb *.gltf)".as_ptr(), std::ptr::null());
            (api.obs_properties_add_bool)(props, S_SHOWN[i + 1].as_ptr(), cstring(&format!("Show {}", i + 1)).as_ptr());
            (api.obs_properties_add_text)(props, S_GROUP[i].as_ptr(), cstring(&format!("Group of {} (only one GLB of a group shows; empty: none)", i + 1)).as_ptr(), obs::OBS_TEXT_DEFAULT);
        }
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
            c"Styles apply top to bottom, each on what the ones above made. Empty slots do nothing; with none, the colours are drawn plain.".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        for slot in 0..SLOTS {
            let label = cstring(&format!("Style {}", slot + 1));
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
                let title = cstring(&format!("{} {}", s.name, slot + 1));
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

        (api.obs_properties_add_int)(props, S_WIDTH.as_ptr(), c"Width".as_ptr(), 16, 8192, 1);
        (api.obs_properties_add_int)(props, S_HEIGHT.as_ptr(), c"Height".as_ptr(), 16, 8192, 1);
        (api.obs_properties_add_float_slider)(props, S_ZOOM.as_ptr(), c"Zoom".as_ptr(), 0.2, 8.0, 0.05);
        (api.obs_properties_add_float_slider)(props, S_CENTRE.as_ptr(), c"Centre (feet to head)".as_ptr(), -1.0, 1.0, 0.01);
        (api.obs_properties_add_float_slider)(props, S_YAW.as_ptr(), c"Turn".as_ptr(), -180.0, 180.0, 1.0);
        for (key, label, range) in [
            (S_POSITION[0], c"Position: left / right (m)", 3.0),
            (S_POSITION[1], c"Position: down / up (m)", 2.0),
            (S_POSITION[2], c"Position: back / forward (m)", 3.0),
        ] {
            (api.obs_properties_add_float_slider)(props, key.as_ptr(), label.as_ptr(), -range, range, 0.01);
        }
        // The avatar's own animations (made in Blender, in its GLB)
        let clips: Vec<String> = if data.is_null() { Vec::new() } else { (&*(data as *const Puppet)).remote.lock().unwrap().animation.clips.clone() };
        let list = (api.obs_properties_add_list)(props, S_ANIMATION.as_ptr(), c"Animation (loops; made in Blender, in the GLB)".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
        (api.obs_property_list_add_string)(list, c"(none: idle motion)".as_ptr(), c"".as_ptr());
        for clip in &clips {
            let c = cstring(clip);
            (api.obs_property_list_add_string)(list, c.as_ptr(), c.as_ptr());
        }
        (api.obs_properties_add_float_slider)(props, S_ARMS.as_ptr(), c"Arms down (degrees from the T-pose)".as_ptr(), 0.0, 85.0, 1.0);
        (api.obs_properties_add_float_slider)(props, S_IDLE.as_ptr(), c"Idle motion".as_ptr(), 0.0, 1.0, 0.01);
        (api.obs_properties_add_bool)(props, S_SPRINGS.as_ptr(), c"Spring bones (hair, skirt…)".as_ptr());
        (api.obs_properties_add_bool)(props, S_TRACKED.as_ptr(), c"Follow tracking (Theater menu > Tracking)".as_ptr());

        // The springs' tuning: the avatar's own kinds (all, before it's loaded)
        let kinds: Vec<usize> = if data.is_null() {
            Vec::new()
        } else {
            (&*(data as *const Puppet)).rig.lock().unwrap().as_ref().map(|r| r.spring_kinds()).unwrap_or_default()
        };
        let kinds = if kinds.is_empty() { (0..KINDS.len()).collect() } else { kinds };
        let group = (api.obs_properties_create)();
        (api.obs_properties_add_text)(
            group,
            c"tune_info".as_ptr(),
            c"Multipliers on what the avatar's file says (1 = as made). Stiffness: how fast it springs back. Gravity: how much it hangs. Damping: how quickly a swing dies down.".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        for kind in kinds {
            let name = KINDS[kind];
            let title = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
            for (what, label, max) in [("stiffness", "stiffness", 4.0), ("gravity", "gravity", 4.0), ("drag", "damping", 2.0)] {
                (api.obs_properties_add_float_slider)(group, tune_key(kind, what).as_ptr(), cstring(&format!("{title}: {label}")).as_ptr(), 0.0, max, 0.05);
            }
        }
        (api.obs_properties_add_float_slider)(group, S_COLLIDERS.as_ptr(), c"Colliders: thickness".as_ptr(), 0.0, 3.0, 0.05);
        (api.obs_properties_add_group)(props, c"spring_tuning".as_ptr(), c"Spring bones: adjust".as_ptr(), obs::OBS_GROUP_NORMAL, group);
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
    let remote: crate::remote::Shared = Default::default();
    crate::remote::add_source(source, remote.clone());
    let puppet = Box::new(Puppet {
        styles_read: AtomicU64::new(0),
        source,
        parts,
        remote,
        colors: Mutex::new(HashMap::new()),
        width: AtomicU32::new(1080),
        height: AtomicU32::new(1080),
        view: Mutex::new(View { zoom: 1.0, height: 0.0, yaw: 0.0, position: glam::Vec3::ZERO }),
        file: Mutex::new(String::new()),
        slots: Mutex::new(Default::default()),
        style_settings: Mutex::new(HashMap::new()),
        running: Mutex::new(Default::default()),
        generation: Arc::new(AtomicU64::new(0)),
        loaded: Arc::new(Mutex::new(None)),
        extra_files: Mutex::new(Vec::new()),
        extra_generation: Arc::new(AtomicU64::new(0)),
        extra_loaded: Arc::new(Mutex::new(None)),
        extras: Mutex::new(Vec::new()),
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
    crate::parts::remove_source(puppet.source);
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

fn get_string(settings: Ptr, name: &CStr) -> String {
    let p = unsafe { (obs::api().obs_data_get_string)(settings, name.as_ptr()) };
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

unsafe extern "C" fn update(data: Ptr, settings: Ptr) {
    let puppet = unsafe { &*(data as *const Puppet) };
    let api = obs::api();
    unsafe {
        puppet.width.store((api.obs_data_get_int)(settings, S_WIDTH.as_ptr()).clamp(16, 8192) as u32, Ordering::Relaxed);
        puppet.height.store((api.obs_data_get_int)(settings, S_HEIGHT.as_ptr()).clamp(16, 8192) as u32, Ordering::Relaxed);
        *puppet.view.lock().unwrap() = View {
            zoom: (api.obs_data_get_double)(settings, S_ZOOM.as_ptr()) as f32,
            height: (api.obs_data_get_double)(settings, S_CENTRE.as_ptr()) as f32,
            yaw: (api.obs_data_get_double)(settings, S_YAW.as_ptr()) as f32,
            position: glam::Vec3::from_array(S_POSITION.map(|k| (api.obs_data_get_double)(settings, k.as_ptr()) as f32)),
        };
        *puppet.motion.lock().unwrap() = Motion {
            arms_down: (api.obs_data_get_double)(settings, S_ARMS.as_ptr()) as f32,
            idle: (api.obs_data_get_double)(settings, S_IDLE.as_ptr()) as f32,
            yaw: 0.0,
            springs: (api.obs_data_get_bool)(settings, S_SPRINGS.as_ptr()),
            adjust: read_adjust(settings),
            tracked: (api.obs_data_get_bool)(settings, S_TRACKED.as_ptr()),
        };
    }
    puppet.remote.lock().unwrap().animation.loop_clip = get_string(settings, S_ANIMATION);
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
        r.shown = S_SHOWN.map(|k| unsafe { (api.obs_data_get_bool)(settings, k.as_ptr()) });
        r.groups = std::array::from_fn(|s| if s == 0 { String::new() } else { get_string(settings, S_GROUP[s - 1]).trim().to_string() });
        r.update_hidden();
    }

    // The GLBs drawn with it: loaded again when their list changes
    let extra_files: Vec<(usize, String)> = S_EXTRAS.iter().enumerate().map(|(i, k)| (i + 1, get_string(settings, k))).filter(|(_, f)| !f.is_empty()).collect();
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
                    .filter_map(|(slot, file)| match model::load(&PathBuf::from(file)) {
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
    let mut current = puppet.file.lock().unwrap();
    if *current == file {
        return;
    }
    *current = file.clone();
    let generation = puppet.generation.fetch_add(1, Ordering::SeqCst) + 1;
    if file.is_empty() {
        *puppet.loaded.lock().unwrap() = None;
        return;
    }
    let (loaded, latest) = (puppet.loaded.clone(), puppet.generation.clone());
    std::thread::Builder::new()
        .name("theater-puppet-load".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let result = model::load(&PathBuf::from(&file));
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

/// Hands what Control Panel and Scene asked (remote.rs) to the rig, and
/// tells them what the Puppet has. The Show settings to write (a shape-key
/// animation ended hiding its GLB), once the locks are let go
fn sync_remote(puppet: &Puppet, rig: &mut Rig) -> Option<[bool; crate::remote::SLOTS]> {
    let mut r = puppet.remote.lock().unwrap();
    let model = rig.model.clone();
    rig.player.sync(&model, &mut r.animation);
    rig.drive_shapes(std::mem::take(&mut r.shapes));
    let mut shown = None;
    for layer in std::mem::take(&mut rig.hide) {
        let now = shown.unwrap_or(r.shown);
        shown = Some(crate::remote::showing(now, &r.groups, r.slot(layer), false));
    }
    // Its GLBs (the avatar first) and colour slots, when they changed
    let models: Vec<&Model> = std::iter::once(&*rig.model).chain(rig.followers.iter().map(|f| &*f.model)).collect();
    if r.glbs.len() != models.len() || r.glbs.iter().zip(&models).any(|(g, m)| g.name != m.name) {
        r.glbs = models.iter().map(|m| crate::remote::GlbInfo { name: m.name.clone(), keys: m.morph_names.clone() }).collect();
        r.slots = color_slots(&models).into_iter().map(|(s, _)| s).collect();
    }
    shown
}

/// Writes the GLB slots' Show settings of a Puppet source
pub(crate) fn set_shown(source: Ptr, shown: [bool; crate::remote::SLOTS]) {
    let api = obs::api();
    unsafe {
        let data = (api.obs_data_create)();
        if data.is_null() {
            return;
        }
        for (key, on) in S_SHOWN.iter().zip(shown) {
            (api.obs_data_set_bool)(data, key.as_ptr(), on);
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

/// Gives colour slot `slot` of a Puppet source a colour (sRGB 0–1), or
/// gives it back its own (None)
pub(crate) fn set_color_setting(source: Ptr, slot: u32, rgb: Option<[f32; 3]>) {
    let api = obs::api();
    unsafe {
        let data = (api.obs_data_create)();
        if data.is_null() {
            return;
        }
        (api.obs_data_set_bool)(data, color_on_key(slot).as_ptr(), rgb.is_some());
        if let Some(rgb) = rgb {
            (api.obs_data_set_int)(data, color_key(slot).as_ptr(), rgb_to_obs(rgb));
        }
        (api.obs_source_update)(source, data);
        (api.obs_data_release)(data);
    }
}

/// The view with what Puppet Place filters on it add (place.rs)
fn placed(puppet: &Puppet) -> View {
    let mut view = *puppet.view.lock().unwrap();
    let o = crate::place::offset(puppet.source);
    view.yaw += o.turn;
    view.position += o.position;
    view
}

/// Runs each slot's tick: what it sets this frame
unsafe extern "C" fn video_tick(data: Ptr, dt: f32) {
    let puppet = unsafe { &*(data as *const Puppet) };
    crate::snapshot::poll();
    // The avatar's pose and springs, skinned (the render uploads them)
    let mut shown = None;
    if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
        let started = std::time::Instant::now();
        let mut motion = *puppet.motion.lock().unwrap();
        motion.yaw = placed(puppet).yaw;
        shown = sync_remote(puppet, rig);
        rig.step(dt, motion);
        crate::timing::tick(started.elapsed());
    }
    if let Some(shown) = shown {
        set_shown(puppet.source, shown);
    }
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
    let mut gpu = puppet.gpu.lock().unwrap();
    // The GLBs drawn with it, newly loaded
    let mut extras_changed = false;
    if let Some((generation, models)) = puppet.extra_loaded.lock().unwrap().take() {
        if generation == puppet.extra_generation.load(Ordering::SeqCst) {
            *puppet.extras.lock().unwrap() = models;
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
        if let Some(g) = gpu.as_mut() {
            g.set_extras(&extras);
        }
        if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
            rig.set_extras(&extras);
        }
    }
    let Some(g) = gpu.as_ref() else { return };
    let timing = crate::timing::begin_render();
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
    *g.colors.lock().unwrap() = puppet.colors.lock().unwrap().clone();
    *g.hidden.lock().unwrap() = puppet.remote.lock().unwrap().hidden.clone();
    if let Some((_, s)) = shader.as_ref() {
        let view = placed(puppet);
        let (width, height) = (puppet.width.load(Ordering::Relaxed), puppet.height.load(Ordering::Relaxed));
        let filters = filter_chain(puppet);
        g.draw(width, height, view, s, &frame, &filters);
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
