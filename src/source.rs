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
use crate::obs::{self, Ptr};
use crate::render::{Gpu, PLAIN_STYLE, Shader, View};
use crate::styles::{self, Frame, Prop, Style};

const ID: &CStr = c"theater_puppet";
const NAME: &CStr = c"\u{1F3AD} Theater Puppet";

const S_FILE: &CStr = c"file";
const S_WIDTH: &CStr = c"width";
const S_HEIGHT: &CStr = c"height";
const S_ZOOM: &CStr = c"zoom";
const S_CENTRE: &CStr = c"centre";
const S_YAW: &CStr = c"yaw";
const S_ARMS: &CStr = c"arms_down";
const S_IDLE: &CStr = c"idle";
const S_SPRINGS: &CStr = c"springs";

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
    running: Mutex<[Option<Running>; SLOTS]>,
    /// Bumped by each new file, so a slow load that finishes late is dropped
    generation: Arc<AtomicU64>,
    loaded: Arc<Mutex<Option<(u64, Result<Model, String>)>>>,
    gpu: Mutex<Option<Gpu>>,
    /// Its skeleton, springs and posed vertices (motion.rs)
    rig: Mutex<Option<Rig>>,
    motion: Mutex<Motion>,
    /// The compiled shader, and which stack (id#version, …) it's from
    shader: Mutex<Option<(String, Shader)>>,
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
        let m = Motion::default();
        (api.obs_data_set_default_double)(settings, S_ARMS.as_ptr(), m.arms_down as f64);
        (api.obs_data_set_default_double)(settings, S_IDLE.as_ptr(), m.idle as f64);
        (api.obs_data_set_default_bool)(settings, S_SPRINGS.as_ptr(), m.springs);
    }
}

/// Every slot's settings for every style: given its script's defaults, then
/// read back
fn read_style_settings(settings: Ptr) -> HashMap<(usize, String), Map> {
    let api = obs::api();
    let mut out = HashMap::new();
    for style in styles::all() {
        for slot in 0..SLOTS {
            let mut map = Map::new();
            for prop in &style.props {
                let Some(key) = prop.key() else { continue };
                let name = setting_key(slot, &style.id, key);
                let default = style.defaults.get(key);
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
                            Dynamic::from(get_string(settings, &name))
                        }
                        Prop::Info { .. } => continue,
                    }
                };
                map.insert(key.into(), value);
            }
            out.insert((slot, style.id.clone()), map);
        }
    }
    out
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
                    add_prop(group, slot, &s.id, i, prop);
                }
                let title = cstring(&format!("{} {}", s.name, slot + 1));
                let g = (api.obs_properties_add_group)(props, group_name(slot, &s.id).as_ptr(), title.as_ptr(), obs::OBS_GROUP_NORMAL, group);
                (api.obs_property_set_visible)(g, s.id == current[slot]);
            }
        }
        let folder = cstring(&format!("Your own styles (.rhai) go in {}", crate::config_dir().join("styles").display()));
        (api.obs_properties_add_text)(props, c"styles_info".as_ptr(), folder.as_ptr(), obs::OBS_TEXT_INFO);

        (api.obs_properties_add_int)(props, S_WIDTH.as_ptr(), c"Width".as_ptr(), 16, 8192, 1);
        (api.obs_properties_add_int)(props, S_HEIGHT.as_ptr(), c"Height".as_ptr(), 16, 8192, 1);
        (api.obs_properties_add_float_slider)(props, S_ZOOM.as_ptr(), c"Zoom".as_ptr(), 0.2, 8.0, 0.05);
        (api.obs_properties_add_float_slider)(props, S_CENTRE.as_ptr(), c"Centre (feet to head)".as_ptr(), -1.0, 1.0, 0.01);
        (api.obs_properties_add_float_slider)(props, S_YAW.as_ptr(), c"Turn".as_ptr(), -180.0, 180.0, 1.0);
        (api.obs_properties_add_float_slider)(props, S_ARMS.as_ptr(), c"Arms down (degrees from the T-pose)".as_ptr(), 0.0, 85.0, 1.0);
        (api.obs_properties_add_float_slider)(props, S_IDLE.as_ptr(), c"Idle motion".as_ptr(), 0.0, 1.0, 0.01);
        (api.obs_properties_add_bool)(props, S_SPRINGS.as_ptr(), c"Spring bones (hair, skirt…)".as_ptr());
        props
    }
}

unsafe fn add_prop(group: Ptr, slot: usize, style: &str, index: usize, prop: &Prop) {
    let api = obs::api();
    unsafe {
        match prop {
            Prop::Slider { key, label, min, max, step } => {
                (api.obs_properties_add_float_slider)(group, setting_key(slot, style, key).as_ptr(), cstring(label).as_ptr(), *min, *max, *step);
            }
            Prop::IntSlider { key, label, min, max, step } => {
                (api.obs_properties_add_int_slider)(group, setting_key(slot, style, key).as_ptr(), cstring(label).as_ptr(), *min as i32, *max as i32, *step as i32);
            }
            Prop::Toggle { key, label } => {
                (api.obs_properties_add_bool)(group, setting_key(slot, style, key).as_ptr(), cstring(label).as_ptr());
            }
            Prop::Color { key, label } => {
                (api.obs_properties_add_color_alpha)(group, setting_key(slot, style, key).as_ptr(), cstring(label).as_ptr());
            }
            Prop::Choice { key, label, options } => {
                let list = (api.obs_properties_add_list)(group, setting_key(slot, style, key).as_ptr(), cstring(label).as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
                for (l, v) in options {
                    (api.obs_property_list_add_string)(list, cstring(l).as_ptr(), cstring(v).as_ptr());
                }
            }
            Prop::Info { label } => {
                (api.obs_properties_add_text)(group, setting_key(slot, style, &format!("info{index}")).as_ptr(), cstring(label).as_ptr(), obs::OBS_TEXT_INFO);
            }
        }
    }
}

/// A Style list changed: each slot shows its own style's settings only
unsafe extern "C" fn style_changed(props: Ptr, _prop: Ptr, settings: Ptr) -> bool {
    let api = obs::api();
    let all = styles::all();
    for slot in 0..SLOTS {
        let current = get_string(settings, &slot_setting(slot));
        for s in &all {
            unsafe {
                let g = (api.obs_properties_get)(props, group_name(slot, &s.id).as_ptr());
                if !g.is_null() {
                    (api.obs_property_set_visible)(g, s.id == current);
                }
            }
        }
    }
    true
}

unsafe extern "C" fn create(settings: Ptr, source: Ptr) -> Ptr {
    let parts: crate::parts::Shared = Default::default();
    crate::parts::add_source(source, parts.clone());
    let puppet = Box::new(Puppet {
        styles_read: AtomicU64::new(0),
        source,
        parts,
        width: AtomicU32::new(1080),
        height: AtomicU32::new(1080),
        view: Mutex::new(View { zoom: 1.0, height: 0.0, yaw: 0.0 }),
        file: Mutex::new(String::new()),
        slots: Mutex::new(Default::default()),
        style_settings: Mutex::new(HashMap::new()),
        running: Mutex::new(Default::default()),
        generation: Arc::new(AtomicU64::new(0)),
        loaded: Arc::new(Mutex::new(None)),
        gpu: Mutex::new(None),
        rig: Mutex::new(None),
        motion: Mutex::new(Motion::default()),
        shader: Mutex::new(None),
    });
    let data = Box::into_raw(puppet) as Ptr;
    unsafe { update(data, settings) };
    data
}

unsafe extern "C" fn destroy(data: Ptr) {
    let puppet = unsafe { Box::from_raw(data as *mut Puppet) };
    crate::parts::remove_source(puppet.source);
    puppet.generation.fetch_add(1, Ordering::SeqCst);
    let gpu = puppet.gpu.lock().unwrap().take();
    let shader = puppet.shader.lock().unwrap().take();
    if gpu.is_some() || shader.is_some() {
        let api = obs::api();
        unsafe { (api.obs_enter_graphics)() };
        if let Some(g) = gpu {
            g.destroy();
        }
        if let Some((_, s)) = shader {
            s.destroy();
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
        };
        *puppet.motion.lock().unwrap() = Motion {
            arms_down: (api.obs_data_get_double)(settings, S_ARMS.as_ptr()) as f32,
            idle: (api.obs_data_get_double)(settings, S_IDLE.as_ptr()) as f32,
            yaw: 0.0,
            springs: (api.obs_data_get_bool)(settings, S_SPRINGS.as_ptr()),
        };
    }
    *puppet.slots.lock().unwrap() = std::array::from_fn(|slot| get_string(settings, &slot_setting(slot)));
    puppet.styles_read.store(styles::generation(), Ordering::SeqCst);
    *puppet.style_settings.lock().unwrap() = read_style_settings(settings);

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
                    "[Puppet] 📦 Loaded {file}: {} meshes, {} materials, {} textures in {} ms",
                    m.primitives.len(),
                    m.materials.len(),
                    m.images.len(),
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

/// Runs each slot's tick: what it sets this frame
unsafe extern "C" fn video_tick(data: Ptr, dt: f32) {
    let puppet = unsafe { &*(data as *const Puppet) };
    crate::snapshot::poll();
    // The avatar's pose and springs, skinned (the render uploads them)
    if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
        let mut motion = *puppet.motion.lock().unwrap();
        motion.yaw = puppet.view.lock().unwrap().yaw;
        rig.step(dt, motion);
    }
    // A style file changed: its settings' defaults may have too
    let generation = styles::generation();
    if puppet.styles_read.swap(generation, Ordering::SeqCst) != generation {
        let api = obs::api();
        unsafe {
            let settings = (api.obs_source_get_settings)(puppet.source);
            if !settings.is_null() {
                *puppet.style_settings.lock().unwrap() = read_style_settings(settings);
                (api.obs_data_release)(settings);
            }
        }
    }
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
    // A newly loaded file replaces what's on the GPU
    if let Some((generation, result)) = puppet.loaded.lock().unwrap().take() {
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
    let Some(g) = gpu.as_ref() else { return };
    // This frame's pose
    if let Some(rig) = puppet.rig.lock().unwrap().as_mut() {
        if rig.dirty {
            g.update_vertices(&rig.points, &rig.normals);
            rig.dirty = false;
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
    if let Some((_, s)) = shader.as_ref() {
        let view = *puppet.view.lock().unwrap();
        let (width, height) = (puppet.width.load(Ordering::Relaxed), puppet.height.load(Ordering::Relaxed));
        g.draw(width, height, view, s, &frame);
        // The body parts map, for Theater Scene
        if let Some(pixels) = g.parts(width, height, view, s) {
            let map = crate::parts::PartMap { pixels, width, height };
            if crate::snapshot::parts_wanted() {
                crate::parts::save_png(&map);
            }
            *puppet.parts.lock().unwrap() = Some(map);
        }
    }
}
