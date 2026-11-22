//! The avatar on OBS's GPU: its meshes and textures (Gpu), and the shader a
//! style makes (Shader), drawn into an off-screen texture with a depth
//! buffer, which the source then draws. Everything here runs on OBS's
//! graphics thread (or inside obs_enter_graphics).

use std::ffi::{CString, c_char};
use std::sync::atomic::{AtomicBool, Ordering};

use glam::{Mat4, Vec3, Vec4};

use crate::model::{Alpha, Material, Model};
use crate::obs::{self, Ptr};
use crate::styles::{Frame, Value};

const TEMPLATE: &str = include_str!("puppet.effect");

/// With no style: soft light from the front
pub const PLAIN_STYLE: &str = "float4 style(Surface s)\n{\n\tfloat l = 0.7 + 0.3 * saturate(dot(s.normal, s.light));\n\treturn float4(s.albedo.rgb * l, s.albedo.a);\n}\n";

/// Compiles an effect. Call on the graphics thread.
fn create_effect(code: &str, name: &std::ffi::CStr) -> Result<Ptr, String> {
    let api = obs::api();
    let source = CString::new(code).map_err(|_| "the shader has a NUL character".to_string())?;
    let mut error: *mut c_char = std::ptr::null_mut();
    let effect = unsafe { (api.gs_effect_create)(source.as_ptr(), name.as_ptr(), &mut error) };
    let message = if error.is_null() {
        None
    } else {
        let s = unsafe { std::ffi::CStr::from_ptr(error) }.to_string_lossy().into_owned();
        unsafe { (api.bfree)(error as *mut _) };
        Some(s)
    };
    if effect.is_null() {
        return Err(message.unwrap_or_else(|| "unknown error".into()));
    }
    Ok(effect)
}

const INK: &str = include_str!("ink.effect");

/// Straight alpha from premultiplied: what a filter takes (OBS gives Theater
/// Scene's filters their source that way)
const UNPREMULTIPLY: &str = r#"
uniform float4x4 ViewProj;
uniform texture2d image;
sampler_state point_clamp { Filter = Point; AddressU = Clamp; AddressV = Clamp; };
struct VertData { float4 pos : POSITION; float2 uv : TEXCOORD0; };
VertData VSDefault(VertData v) { VertData o; o.pos = mul(float4(v.pos.xyz, 1.0), ViewProj); o.uv = v.uv; return o; }
float4 PSStraight(VertData v) : TARGET { float4 c = image.Sample(point_clamp, v.uv); return float4(c.rgb / max(c.a, 0.0001), c.a); }
technique Draw { pass { vertex_shader = VSDefault(v); pixel_shader = PSStraight(v); } }
"#;

/// One filter this frame: its effect, its values, how its output's blended
pub struct FilterPass {
    pub effect: Ptr,
    pub values: Vec<(String, Value)>,
    pub premultiplied: bool,
    /// Its shader declares `gbuf`: the avatar's normals and distances are drawn for it
    pub wants_gbuf: bool,
}

/// A filter's shader, compiled. Call on the graphics thread.
pub fn compile_filter(code: &str, id: &str) -> Result<(Ptr, bool), String> {
    let name = CString::new(format!("{id}.effect")).unwrap_or_default();
    let effect = create_effect(code, &name)?;
    Ok((effect, !param(effect, "gbuf").is_null()))
}

pub fn destroy_effect(effect: Ptr) {
    if !effect.is_null() {
        unsafe { (obs::api().gs_effect_destroy)(effect) };
    }
}

/// Values set by name (a missing one is skipped)
fn set_values(effect: Ptr, values: &[(String, Value)]) {
    let api = obs::api();
    for (name, value) in values {
        let p = param(effect, name);
        if p.is_null() {
            continue;
        }
        unsafe {
            match value {
                Value::Float(f) => (api.gs_effect_set_float)(p, *f),
                Value::Vec(v) if v.len() == 2 => (api.gs_effect_set_vec2)(p, &[v[0], v[1]]),
                Value::Vec(v) if v.len() == 3 => (api.gs_effect_set_vec3)(p, &[v[0], v[1], v[2], 0.0]),
                Value::Vec(v) => (api.gs_effect_set_vec4)(p, &[v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0)]),
            }
        }
    }
}

/// The ink pass's effect (ink.effect)
struct Ink {
    effect: Ptr,
}

fn param(effect: Ptr, name: &str) -> Ptr {
    let Ok(name) = CString::new(name) else { return std::ptr::null_mut() };
    unsafe { (obs::api().gs_effect_get_param_by_name)(effect, name.as_ptr()) }
}

struct Params {
    mvp: Ptr,
    image: Ptr,
    base_color: Ptr,
    shade_color: Ptr,
    use_texture: Ptr,
    alpha_cutoff: Ptr,
    blended: Ptr,
    light_dir: Ptr,
    light_color: Ptr,
    eye: Ptr,
    outline_width: Ptr,
    outline_color: Ptr,
    outline_jitter: Ptr,
    outline_boil: Ptr,
    time: Ptr,
    glb_fade: Ptr,
    tile_map: Ptr,
    uv_place: Ptr,
    tile_mix: Ptr,
    env: Ptr,
    env_strength: Ptr,
    env_flat: Ptr,
    env_turn: Ptr,
    env_light: Ptr,
    emissive: Ptr,
    emissive_tex: Ptr,
    use_emissive: Ptr,
    normal_tex: Ptr,
    use_normal: Ptr,
    normal_scale: Ptr,
    mr_tex: Ptr,
    use_mr: Ptr,
    metallic: Ptr,
    roughness: Ptr,
    coat: Ptr,
    coat_roughness: Ptr,
}

/// A reflection this frame: the picture (an OBS source drawn), how much of
/// it, flat or 360, how the 360 one is turned (radians)
#[derive(Clone, Copy, Debug)]
pub struct Reflection {
    pub texture: Ptr,
    pub strength: f32,
    pub flat: bool,
    pub turn: f32,
    /// As light (the picture lights the avatar, no gloss), or as a mirror
    pub light: bool,
}

thread_local! {
    /// A Puppet's reflection source being drawn: a Puppet inside it (a scene
    /// with itself in it) draws nothing (no endless mirror, no deadlock)
    pub static IN_REFLECTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The effect a style's code compiles into
pub struct Shader {
    effect: Ptr,
    params: Params,
    /// Every number value and its size: zeroed before each draw, so one a
    /// style doesn't give still has a value
    numeric: Vec<(Ptr, usize)>,
    /// Techniques whose unset values are logged
    logged: std::sync::Mutex<Vec<std::ffi::CString>>,
}

/// The size of a value of kind `kind` (gs_shader_param_type), None for
/// matrices and textures (always set) and what Puppet never sets
fn param_size(kind: u32) -> Option<usize> {
    match kind {
        1..=3 => Some(4),   // bool, float, int
        5 | 8 => Some(8),   // vec2, int2
        6 | 9 => Some(12),  // vec3, int3
        7 | 10 => Some(16), // vec4, int4
        _ => None,
    }
}

/// What every draw of a frame sets. OBS clears an effect's values when a
/// technique ends, and a technique's shaders get only what's set when it
/// starts: so they're set again before each draw.
struct FrameValues<'a> {
    mvp: [f32; 16],
    light: [f32; 4],
    light_color: [f32; 4],
    eye: [f32; 4],
    /// Seconds since Puppet loaded (wraps every hour, for float precision)
    time: f32,
    frame: &'a Frame,
    /// The reflection (the picture draw only)
    reflection: Option<Reflection>,
}

fn seconds() -> f32 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32() % 3600.0
}

unsafe impl Send for Shader {}

impl Shader {
    /// The template with `style_code` in it. Call on the graphics thread.
    pub fn compile(style_code: &str) -> Result<Shader, String> {
        let api = obs::api();
        let effect = create_effect(&TEMPLATE.replace("/* STYLE_CODE */", style_code), c"puppet.effect")?;
        let p = |n| param(effect, n);
        let params = Params {
            mvp: p("mvp"),
            image: p("image"),
            base_color: p("base_color"),
            shade_color: p("shade_color"),
            use_texture: p("use_texture"),
            alpha_cutoff: p("alpha_cutoff"),
            blended: p("blended"),
            light_dir: p("light_dir"),
            light_color: p("light_color"),
            eye: p("eye"),
            outline_width: p("outline_width"),
            outline_color: p("outline_color"),
            outline_jitter: p("outline_jitter"),
            outline_boil: p("outline_boil"),
            time: p("time"),
            glb_fade: p("glb_fade"),
            tile_map: p("tile_map"),
            uv_place: p("uv_place"),
            tile_mix: p("tile_mix"),
            env: p("env"),
            env_strength: p("env_strength"),
            env_flat: p("env_flat"),
            env_turn: p("env_turn"),
            env_light: p("env_light"),
            emissive: p("emissive"),
            emissive_tex: p("emissive_tex"),
            use_emissive: p("use_emissive"),
            normal_tex: p("normal_tex"),
            use_normal: p("use_normal"),
            normal_scale: p("normal_scale"),
            mr_tex: p("mr_tex"),
            use_mr: p("use_mr"),
            metallic: p("metallic"),
            roughness: p("roughness"),
            coat: p("coat"),
            coat_roughness: p("coat_roughness"),
        };
        let mut numeric = Vec::new();
        unsafe {
            for i in 0..(api.gs_effect_get_num_params)(effect) {
                let p = (api.gs_effect_get_param_by_idx)(effect, i);
                let mut info = obs::EffectParamInfo { name: std::ptr::null(), type_: 0 };
                (api.gs_effect_get_param_info)(p, &mut info);
                if let Some(size) = param_size(info.type_) {
                    numeric.push((p, size));
                }
            }
        }
        Ok(Shader { effect, params, numeric, logged: Default::default() })
    }

    /// Everything a draw needs but the material
    unsafe fn set_frame(&self, v: &FrameValues) {
        let api = obs::api();
        let p = &self.params;
        let zeros = [0u8; 16];
        unsafe {
            for (param, size) in &self.numeric {
                (api.gs_effect_set_val)(*param, zeros.as_ptr() as *const _, *size);
            }
            (api.gs_effect_set_matrix4)(p.mvp, &v.mvp);
            (api.gs_effect_set_vec4)(p.light_dir, &v.light);
            (api.gs_effect_set_vec4)(p.light_color, &v.light_color);
            (api.gs_effect_set_vec4)(p.eye, &v.eye);
            (api.gs_effect_set_float)(p.outline_width, v.frame.outline_width);
            (api.gs_effect_set_vec4)(p.outline_color, &v.frame.outline_color);
            (api.gs_effect_set_float)(p.outline_jitter, v.frame.outline_jitter);
            (api.gs_effect_set_float)(p.outline_boil, v.frame.outline_boil);
            (api.gs_effect_set_float)(p.time, v.time);
            if let Some(r) = v.reflection.filter(|r| !r.texture.is_null() && r.strength > 0.0) {
                (api.gs_effect_set_texture)(p.env, r.texture);
                (api.gs_effect_set_float)(p.env_strength, r.strength);
                (api.gs_effect_set_float)(p.env_flat, if r.flat { 1.0 } else { 0.0 });
                (api.gs_effect_set_float)(p.env_turn, r.turn);
                (api.gs_effect_set_float)(p.env_light, if r.light { 1.0 } else { 0.0 });
            }
            self.set_uniforms(v.frame);
        }
    }

    /// The style's own uniforms, from its tick
    unsafe fn set_uniforms(&self, frame: &Frame) {
        set_values(self.effect, &frame.uniforms);
    }

    /// Once per shader and technique: the values never given (D3D11 then
    /// skips the draw: "Not all shader parameters were set")
    unsafe fn log_unset(&self, technique: &std::ffi::CStr) {
        let mut logged = self.logged.lock().unwrap();
        if logged.iter().any(|t| t.as_c_str() == technique) {
            return;
        }
        logged.push(technique.to_owned());
        let api = obs::api();
        let mut unset = Vec::new();
        unsafe {
            for i in 0..(api.gs_effect_get_num_params)(self.effect) {
                let p = (api.gs_effect_get_param_by_idx)(self.effect, i);
                let mut info = obs::EffectParamInfo { name: std::ptr::null(), type_: 0 };
                (api.gs_effect_get_param_info)(p, &mut info);
                let name = if info.name.is_null() { "?".into() } else { std::ffi::CStr::from_ptr(info.name).to_string_lossy().into_owned() };
                if info.type_ != obs::GS_SHADER_PARAM_TEXTURE && (api.gs_effect_get_val_size)(p) == 0 {
                    unset.push(name);
                }
            }
        }
        if !unset.is_empty() {
            crate::log(&format!("[Puppet] ⚠️ {}: values never set (Direct3D skips the draw): {:?}", technique.to_string_lossy(), unset));
        }
    }

    pub fn destroy(self) {
        unsafe { (obs::api().gs_effect_destroy)(self.effect) };
    }
}

struct Mesh {
    vb: Ptr,
    ib: Ptr,
    count: u32,
    material: usize,
    /// Its primitive in the model (the posed vertices come per primitive)
    prim: usize,
    vertices: usize,
    /// Its model: 0 the avatar, then the GLBs drawn with it (set_extras)
    layer: usize,
    /// Its material in its own model, and its UVs as made (a tile moves them: set_uvs)
    local_material: usize,
    uvs: Vec<[f32; 2]>,
}

/// A mesh's tile crossfading: the placement before, how far in, and over how long
#[derive(Clone, Copy, Debug)]
struct TileFade {
    from: [f32; 4],
    time: f32,
    duration: f32,
}

/// For the shader: where the tile before is, from the UVs placed by `now`
/// (uv * xy + zw); None when `now` can't be undone (a scale of 0)
fn tile_map(from: [f32; 4], now: [f32; 4]) -> Option<[f32; 4]> {
    if now[0].abs() < 1e-6 || now[1].abs() < 1e-6 {
        return None;
    }
    let (ax, ay) = (from[0] / now[0], from[1] / now[1]);
    Some([ax, ay, from[2] - now[2] * ax, from[3] - now[3] * ay])
}

/// UVs placed by `st` (scale x, y, offset x, y): uv * scale + offset
fn placed_uvs(uvs: &[[f32; 2]], st: [f32; 4]) -> Vec<[f32; 2]> {
    uvs.iter().map(|[u, v]| [u * st[0] + st[2], v * st[1] + st[3]]).collect()
}

/// A model's meshes, textures and materials on the GPU, as layer `layer`
/// (its materials' and textures' places after `materials` and `textures`)
fn upload_meshes(model: &Model, layer: usize, materials: usize, textures: usize) -> (Vec<Mesh>, Vec<Ptr>, Vec<Material>) {
    let (new_textures, new_materials) = upload_looks(model, textures);
    (make_meshes(model, layer, materials), new_textures, new_materials)
}

/// A model's textures, and its materials pointing at them (from `textures` on)
fn upload_looks(model: &Model, textures: usize) -> (Vec<Ptr>, Vec<Material>) {
    let api = obs::api();
    let new_textures = model
        .images
        .iter()
        .map(|img| {
            let data = img.rgba.as_ptr();
            unsafe { (api.gs_texture_create)(img.width, img.height, obs::GS_RGBA, 1, &data, 0) }
        })
        .collect();
    let shift = |i: Option<usize>| i.map(|i| i + textures);
    let new_materials = model
        .materials
        .iter()
        .map(|m| Material { image: shift(m.image), emissive_image: shift(m.emissive_image), normal_image: shift(m.normal_image), mr_image: shift(m.mr_image), ..m.clone() })
        .collect();
    (new_textures, new_materials)
}

/// A model's meshes (its own vertex buffers), their materials from `materials` on
fn make_meshes(model: &Model, layer: usize, materials: usize) -> Vec<Mesh> {
    let api = obs::api();
    let mut meshes = Vec::new();
    for (prim, p) in model.primitives.iter().enumerate() {
        let n = p.positions.len();
        let points: Vec<[f32; 4]> = p.positions.iter().map(|v| [v[0], v[1], v[2], 0.0]).collect();
        let normals: Vec<[f32; 4]> = p.normals.iter().map(|v| [v[0], v[1], v[2], 0.0]).collect();
        let vb = unsafe {
            // libobs's gs_vbdata_create is an inline bzalloc: the same here
            let data = (api.bmalloc)(std::mem::size_of::<obs::VbData>()) as *mut obs::VbData;
            std::ptr::write_bytes(data, 0, 1);
            (*data).num = n;
            (*data).points = obs_copy(&points);
            (*data).normals = obs_copy(&normals);
            // TEXCOORD0: the UVs; TEXCOORD1–3: the body part, one channel
            // each (the Parts pass). Not one number: across a triangle
            // from torso (0) to leg (6) it would pass through 1–5
            (*data).num_tex = 4;
            let tv = obs_copy(&[obs::TVertArray { width: 2, array: std::ptr::null_mut() }, obs::TVertArray { width: 4, array: std::ptr::null_mut() }, obs::TVertArray { width: 4, array: std::ptr::null_mut() }, obs::TVertArray { width: 4, array: std::ptr::null_mut() }]);
            // Where its material's texture placement puts them
            (*tv).array = obs_copy(&placed_uvs(&p.uvs, model.materials[p.material].uv)) as *mut _;
            for group in 0..3 {
                let channels: Vec<[f32; 4]> = p.parts.iter().map(|&k| std::array::from_fn(|c| if k as usize == group * 4 + c { 1.0 } else { 0.0 })).collect();
                (*tv.add(1 + group)).array = obs_copy(&channels) as *mut _;
            }
            (*data).tvarray = tv;
            // Dynamic: re-posed every frame (update_vertices)
            (api.gs_vertexbuffer_create)(data, obs::GS_DYNAMIC)
        };
        let ib = unsafe { (api.gs_indexbuffer_create)(obs::GS_UNSIGNED_LONG, obs_copy(&p.indices) as *mut _, p.indices.len(), 0) };
        if vb.is_null() || ib.is_null() {
            continue;
        }
        meshes.push(Mesh { vb, ib, count: p.indices.len() as u32, material: p.material + materials, prim, vertices: n, layer, local_material: p.material, uvs: p.uvs.clone() });
    }
    meshes
}

/// Posed vertices into meshes' vertex buffers (each its primitive's)
fn write_vertices<'a>(meshes: impl Iterator<Item = &'a Mesh>, points: &[Vec<[f32; 4]>], normals: &[Vec<[f32; 4]>]) {
    let api = obs::api();
    for mesh in meshes {
        let (Some(p), Some(n)) = (points.get(mesh.prim), normals.get(mesh.prim)) else { continue };
        if p.len() != mesh.vertices || n.len() != mesh.vertices {
            continue;
        }
        unsafe {
            let data = (api.gs_vertexbuffer_get_data)(mesh.vb);
            if data.is_null() || (*data).num != mesh.vertices {
                continue;
            }
            std::ptr::copy_nonoverlapping(p.as_ptr(), (*data).points, mesh.vertices);
            std::ptr::copy_nonoverlapping(n.as_ptr(), (*data).normals, mesh.vertices);
            (api.gs_vertexbuffer_flush)(mesh.vb);
        }
    }
}

/// GLB prefabs on screen (prefab.rs): each GLB's textures and materials,
/// uploaded once and shared by every copy of it; each copy's own meshes, and
/// how much of it shows
#[derive(Default)]
struct PrefabGpu {
    looks: std::collections::HashMap<String, (Vec<Ptr>, Vec<Material>)>,
    shown: Vec<(u64, String, Vec<Mesh>, f32)>,
}

fn destroy_meshes(meshes: Vec<Mesh>) {
    let api = obs::api();
    for m in meshes {
        unsafe {
            (api.gs_vertexbuffer_destroy)(m.vb);
            (api.gs_indexbuffer_destroy)(m.ib);
        }
    }
}

fn destroy_textures(textures: Vec<Ptr>) {
    let api = obs::api();
    for t in textures.into_iter().filter(|t| !t.is_null()) {
        unsafe { (api.gs_texture_destroy)(t) };
    }
}

pub struct Gpu {
    meshes: Vec<Mesh>,
    textures: Vec<Ptr>,
    materials: Vec<Material>,
    /// The avatar's own (the extras' follow them)
    main_materials: usize,
    main_textures: usize,
    /// Colour slots given a colour (sRGB, 0–1), by slot: their materials
    /// show it in place of their own (every layer's)
    pub colors: std::sync::Mutex<std::collections::HashMap<u32, [f32; 3]>>,
    /// How much of each layer shows (0 not drawn, below 1 dissolving; missing: 1)
    pub fade: std::sync::Mutex<Vec<f32>>,
    /// Where a reflection source is drawn (render_source)
    env_rt: Ptr,
    /// The controller's layer (controller.rs): left out of the parts map
    pub prop_layer: std::sync::Mutex<Option<usize>>,
    /// Each mesh's texture placement now (set_uvs), and its crossfade
    uv_now: std::sync::Mutex<Vec<[f32; 4]>>,
    uv_fade: std::sync::Mutex<Vec<Option<TileFade>>>,
    /// When set_uvs last ran (its crossfades' clock)
    uv_clock: std::sync::Mutex<Option<std::time::Instant>>,
    target: Ptr,
    /// The ink pass: normals and distances, then the picture with its lines
    gbuf: Ptr,
    inked: Ptr,
    ink: Option<Ink>,
    /// The filters' two textures, used in turn, and their straight-alpha step
    chain: [Ptr; 2],
    unpremultiply: Ptr,
    /// The parts map for Theater Scene: drawn small, copied back a frame later
    parts_rt: Ptr,
    parts_stage: std::sync::Mutex<PartsStage>,
    /// What the camera frames: the avatar and the GLBs drawn with it (not
    /// the controller or desk); the avatar's own, kept for when they change
    pub min: Vec3,
    pub max: Vec3,
    main_bounds: (Vec3, Vec3),
    /// The first frame's line in the log is written
    logged: AtomicBool,
    /// GLB prefabs on screen (sync_prefabs)
    prefabs: PrefabGpu,
}

// Its pointers are GPU objects, used under OBS's graphics lock only
unsafe impl Send for Gpu {}

/// The parts map's copy back from the GPU
#[derive(Default)]
struct PartsStage {
    surface: Option<(Ptr, u32, u32)>,
    /// Staged last frame, to read this one (no wait on the GPU)
    pending: bool,
    last: Option<std::time::Instant>,
}

/// How often the parts map is redrawn
const PARTS_EVERY: std::time::Duration = std::time::Duration::from_millis(100);
/// The parts map is this many times smaller than the source
pub const PARTS_SCALE: u32 = 4;

/// A parts map read back: width x height, each byte 0 (nothing) or part + 1
pub struct PartPixels {
    pub width: u32,
    pub height: u32,
    pub ids: Vec<u8>,
}

/// Where the camera looks from, set in the source's properties
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub zoom: f32,
    /// -1 (feet) .. 1 (head): where the frame is centred
    pub height: f32,
    /// The body's turn, degrees (Puppet Place filters: its hair swings after it)
    pub yaw: f32,
    /// The camera's turn around the avatar, degrees (it looks as a body turn
    /// of the same degrees did: the old Turn)
    pub orbit: f32,
    /// Orthographic (no perspective: sizes don't shrink with distance)
    pub orthographic: bool,
    /// Where the avatar stands, metres from the middle: its right (+X, as
    /// seen), up, towards the camera
    pub position: Vec3,
    /// The light: where it comes from, as the camera sees it (towards it),
    /// and its colour times its strength
    pub light: Vec3,
    pub light_color: [f32; 3],
}

impl View {
    /// The light Puppet always had: from the front, upper left, white
    pub const LIGHT: Vec3 = Vec3::new(-0.4, 0.6, 1.0);

    /// A light from `turn` degrees around (0 the camera's side, + from the
    /// picture's right) and `height` degrees up
    pub fn light_from(turn: f32, height: f32) -> Vec3 {
        let (t, h) = (turn.to_radians(), height.to_radians());
        Vec3::new(t.sin() * h.cos(), h.sin(), t.cos() * h.cos())
    }
}

/// Copies `src` into a libobs allocation (OBS frees it with the buffer)
unsafe fn obs_copy<T: Copy>(src: &[T]) -> *mut T {
    let api = obs::api();
    let p = unsafe { (api.bmalloc)(std::mem::size_of_val(src).max(1)) } as *mut T;
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), p, src.len()) };
    p
}

/// What the camera works out for a frame
struct Camera {
    mvp: Mat4,
    /// Towards the light, in the avatar's own space
    light: Vec3,
    /// Its colour x strength
    light_color: [f32; 4],
    /// The camera, in the avatar's own space
    eye: Vec3,
}

/// The camera for a model from `min` to `max` (Gpu::camera)
fn camera(min: Vec3, max: Vec3, width: u32, height: u32, view: View) -> Camera {
    let size = max - min;
    let centre = (min + max) * 0.5;
    let fov = 30f32.to_radians();
    let tan = (fov * 0.5).tan();
    let aspect = width as f32 / height.max(1) as f32;
    // The avatar's middle on the origin; the camera goes around it on a
    // circle (Turn: a body turn of the same degrees looks the same; Puppet
    // Place's turn turns the body, motion.rs), looking at it
    let model = Mat4::from_translation(Vec3::new(-centre.x, 0.0, -centre.z));
    let visible = size.y * 1.08 / view.zoom.max(0.05);
    let target = Vec3::new(0.0, centre.y + view.height * size.y * 0.5, 0.0);
    // `visible` is the height in view at its front (its depth before its
    // middle). How far: the same all the way round, never inside the
    // avatar; closer than that, zoom narrows the lens instead, so it keeps
    // zooming. (Where the camera can come that close: as it always framed)
    let radius = 0.5 * (size.x * size.x + size.z * size.z).sqrt();
    let distance = (visible * 0.5 / tan + size.z).max(radius + 0.05);
    let fov = 2.0 * (visible * 0.5 / (distance - size.z).max(0.01)).atan();
    // The half-height in view at its middle (for orthographic)
    let half = distance * (fov * 0.5).tan();
    let around = glam::Quat::from_rotation_y(-view.orbit.to_radians());
    let eye = target + around * Vec3::Z * distance;
    // Position: where the avatar is in the picture, as the camera sees it
    // (its right, up, towards the camera): the camera's circle goes with it
    let look = Mat4::from_translation(view.position) * Mat4::look_at_rh(eye, target, Vec3::Y);
    let far = distance + size.length() * 2.0 + view.position.z.abs();
    let proj = if view.orthographic {
        // The same size as the perspective shows at the avatar's middle
        let h = half;
        let w = h * aspect;
        Mat4::orthographic_rh(-w, w, -h, h, 0.01, far)
    } else {
        Mat4::perspective_rh(fov, aspect, 0.01, far)
    };
    // The camera and the light, in the avatar's own space (the light from
    // the front, upper left as the camera sees it: it goes around with it)
    let inverse = (look * model).inverse();
    Camera {
        mvp: proj * look * model,
        light: inverse.transform_vector3(view.light.normalize_or(View::LIGHT.normalize())).normalize(),
        light_color: [view.light_color[0], view.light_color[1], view.light_color[2], 1.0],
        eye: inverse.transform_point3(Vec3::ZERO),
    }
}

impl Gpu {
    /// Uploads the model. Call on the graphics thread.
    pub fn upload(model: &Model) -> Result<Gpu, String> {
        let api = obs::api();
        let (meshes, textures, materials) = upload_meshes(model, 0, 0, 0);
        let target = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_Z24_S8) };
        if target.is_null() {
            return Err("no off-screen texture".into());
        }
        let gbuf = unsafe { (api.gs_texrender_create)(obs::GS_RGBA16F, obs::GS_Z24_S8) };
        let inked = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_ZS_NONE) };
        let parts_rt = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_Z24_S8) };
        let env_rt = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_ZS_NONE) };
        let chain = unsafe { [(api.gs_texrender_create)(obs::GS_RGBA, obs::GS_ZS_NONE), (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_ZS_NONE)] };
        let unpremultiply = create_effect(UNPREMULTIPLY, c"unpremultiply.effect").unwrap_or_else(|e| {
            crate::log(&format!("[Puppet] ⚠️ The filters' straight-alpha step: {e}"));
            std::ptr::null_mut()
        });
        let ink = match create_effect(INK, c"ink.effect") {
            Ok(effect) => Some(Ink { effect }),
            Err(e) => {
                crate::log(&format!("[Puppet] ⚠️ The ink lines' shader: {e}"));
                None
            }
        };
        Ok(Gpu {
            meshes,
            main_materials: materials.len(),
            main_textures: textures.len(),
            colors: Default::default(),
            fade: Default::default(),
            prop_layer: Default::default(),
            uv_now: Default::default(),
            uv_fade: Default::default(),
            uv_clock: Default::default(),
            textures,
            materials,
            target,
            gbuf,
            inked,
            ink,
            chain,
            unpremultiply,
            parts_rt,
            env_rt,
            parts_stage: Default::default(),
            main_bounds: (model.min, model.max),
            min: model.min,
            max: model.max,
            logged: AtomicBool::new(false),
            prefabs: PrefabGpu::default(),
        })
    }

    /// The camera: the avatar centred, `zoom` x its height in view
    fn camera(&self, width: u32, height: u32, view: View) -> Camera {
        camera(self.min, self.max, width, height, view)
    }

    /// Draws the avatar into the current target, `width` x `height`, with
    /// `shader` and the style's `frame`.
    pub fn draw(&self, width: u32, height: u32, view: View, shader: &Shader, frame: &Frame, filters: &[FilterPass], reflection: Option<Reflection>) {
        let api = obs::api();
        let cam = self.camera(width, height, view);
        unsafe {
            (api.gs_texrender_reset)(self.target);
            if (api.gs_texrender_begin)(self.target, width, height) {
                (api.gs_clear)(obs::GS_CLEAR_COLOR | obs::GS_CLEAR_DEPTH, &[0.0; 4], 1.0, 0);
                (api.gs_blend_state_push)();
                let cull = (api.gs_get_cull_mode)();
                (api.gs_set_cull_mode)(obs::GS_NEITHER);
                (api.gs_enable_depth_test)(true);
                (api.gs_depth_function)(obs::GS_LESS);
                let values = FrameValues {
                    // Transposed, as OBS does with its own ViewProj before it reaches the shader
                    mvp: cam.mvp.transpose().to_cols_array(),
                    light: [cam.light.x, cam.light.y, cam.light.z, 0.0], light_color: cam.light_color,
                    eye: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
                    time: seconds(),
                    frame,
                    reflection,
                };

                // Solid and cut-out, then the outline around them, then the blended ones
                (api.gs_enable_blending)(false);
                self.pass(shader, &values, c"Draw", |m| m.alpha != Alpha::Blend);
                if frame.outline_width > 0.0 {
                    self.pass(shader, &values, c"Outline", |m| m.alpha != Alpha::Blend);
                }
                (api.gs_enable_blending)(true);
                (api.gs_blend_function_separate)(obs::GS_BLEND_SRCALPHA, obs::GS_BLEND_INVSRCALPHA, obs::GS_BLEND_ONE, obs::GS_BLEND_INVSRCALPHA);
                self.pass(shader, &values, c"Draw", |m| m.alpha == Alpha::Blend);

                (api.gs_load_vertexbuffer)(std::ptr::null_mut());
                (api.gs_load_indexbuffer)(std::ptr::null_mut());
                (api.gs_enable_depth_test)(false);
                (api.gs_set_cull_mode)(cull);
                (api.gs_blend_state_pop)();
                (api.gs_texrender_end)(self.target);
                if !self.logged.swap(true, Ordering::Relaxed) {
                    let centre = cam.mvp * Vec4::new(0.0, (self.min.y + self.max.y) * 0.5, 0.0, 1.0);
                    crate::log(&format!(
                        "[Puppet] 🖼️ First frame: {width}x{height}, {} meshes; the avatar's middle lands at {:?} on screen (-1..1 is in view)",
                        self.meshes.len(),
                        (centre.truncate() / centre.w).to_array()
                    ));
                    // Each material's maps (what the shader gets: textures by index)
                    for m in &self.materials {
                        let has = |i: Option<usize>| i.map_or("-".to_string(), |i| format!("#{i}{}", if self.textures.get(i).is_some_and(|t| !t.is_null()) { "" } else { " (missing!)" }));
                        crate::log(&format!(
                            "[Puppet]    material {}: colour {:?} {}, emission {:?} {}, normal map {} x{}, metallic {} roughness {} {}, coat {} roughness {}",
                            m.name, m.color, has(m.image), m.emissive, has(m.emissive_image), has(m.normal_image), m.normal_scale, m.metallic, m.roughness, has(m.mr_image), m.coat, m.coat_roughness
                        ));
                    }
                }
            } else if !self.logged.swap(true, Ordering::Relaxed) {
                crate::log("[Puppet] ⚠️ Couldn't draw into its texture (gs_texrender_begin failed)");
            }

            // The result, premultiplied (blended edges were drawn over transparent black)
            let mut texture = (api.gs_texrender_get_texture)(self.target);
            if texture.is_null() {
                return;
            }
            // The avatar's normals and distances: for the ink lines, and filters asking for them
            let gbuf = if frame.lines.width > 0.0 || filters.iter().any(|f| f.wants_gbuf) {
                self.gbuffer(width, height, &cam, shader, frame)
            } else {
                None
            };
            if let (true, Some(g)) = (frame.lines.width > 0.0, gbuf) {
                if let Some(inked) = self.ink(texture, g, width, height, frame) {
                    texture = inked;
                }
            }
            // The filters, on the whole picture in turn. Each gets it straight
            // (as OBS gives Theater Scene's filters their source)
            // (Two textures in turn: each step reads the one the last wrote)
            let mut premultiplied = true;
            let mut turn = 0;
            for f in filters {
                if premultiplied {
                    if let Some(t) = self.run_effect(self.unpremultiply, &[], texture, None, width, height, turn % 2) {
                        texture = t;
                        turn += 1;
                    }
                }
                if let Some(t) = self.run_effect(f.effect, &f.values, texture, gbuf, width, height, turn % 2) {
                    texture = t;
                    turn += 1;
                    premultiplied = f.premultiplied;
                }
            }
            if crate::snapshot::wanted() {
                crate::snapshot::save(texture, width, height);
            }
            let effect = (api.obs_get_base_effect)(obs::OBS_EFFECT_DEFAULT);
            (api.gs_blend_state_push)();
            (api.gs_enable_blending)(true);
            if premultiplied {
                (api.gs_blend_function)(obs::GS_BLEND_ONE, obs::GS_BLEND_INVSRCALPHA);
            } else {
                (api.gs_blend_function)(obs::GS_BLEND_SRCALPHA, obs::GS_BLEND_INVSRCALPHA);
            }
            (api.gs_effect_set_texture)(param(effect, "image"), texture);
            while (api.gs_effect_loop)(effect, c"Draw".as_ptr()) {
                (api.gs_draw_sprite)(texture, 0, width, height);
            }
            (api.gs_blend_state_pop)();
        }
    }

    /// The GLBs drawn with the avatar, in the same picture (layers 1, 2…),
    /// in place of the ones before. Call on the graphics thread.
    /// The GLBs drawn with the avatar (its "Also draw" ones, then the
    /// controller or desk): the first `framed` of them count in the camera's
    /// framing with the avatar (a big prop beside a small avatar fits in view)
    pub fn set_extras(&mut self, models: &[std::sync::Arc<Model>], framed: usize) {
        let (mut lo, mut hi) = self.main_bounds;
        for m in models.iter().take(framed) {
            lo = lo.min(m.min);
            hi = hi.max(m.max);
        }
        (self.min, self.max) = (lo, hi);
        // Their materials in the log too, on the next frame
        self.logged.store(false, Ordering::Relaxed);
        let api = obs::api();
        let (keep, old): (Vec<Mesh>, Vec<Mesh>) = std::mem::take(&mut self.meshes).into_iter().partition(|m| m.layer == 0);
        self.meshes = keep;
        unsafe {
            for m in old {
                (api.gs_vertexbuffer_destroy)(m.vb);
                (api.gs_indexbuffer_destroy)(m.ib);
            }
            for t in self.textures.drain(self.main_textures..).filter(|t| !t.is_null()) {
                (api.gs_texture_destroy)(t);
            }
        }
        self.materials.truncate(self.main_materials);
        for (i, model) in models.iter().enumerate() {
            let (meshes, textures, materials) = upload_meshes(model, i + 1, self.materials.len(), self.textures.len());
            self.meshes.extend(meshes);
            self.textures.extend(textures);
            self.materials.extend(materials);
        }
        // Each mesh as uploaded: its material's own placement
        *self.uv_now.lock().unwrap() = self.meshes.iter().map(|m| self.materials[m.material].uv).collect();
        *self.uv_fade.lock().unwrap() = vec![None; self.meshes.len()];
    }

    /// Each mesh's texture placement: `tile(layer, its material in its model)`
    /// is (Some(placement) when a tile moves it, else its material's own; the
    /// seconds a change crossfades over). Rewritten only when it changes.
    /// Call on the graphics thread, once a frame.
    pub fn set_uvs(&self, tile: impl Fn(usize, usize) -> (Option<[f32; 4]>, f32)) {
        let api = obs::api();
        let dt = {
            let mut clock = self.uv_clock.lock().unwrap();
            let dt = clock.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0).min(0.25);
            *clock = Some(std::time::Instant::now());
            dt
        };
        let mut now = self.uv_now.lock().unwrap();
        let mut fades = self.uv_fade.lock().unwrap();
        now.resize(self.meshes.len(), crate::model::UV_IDENTITY);
        fades.resize(self.meshes.len(), None);
        for (i, mesh) in self.meshes.iter().enumerate() {
            if let Some(f) = fades[i].as_mut() {
                f.time += dt;
                if f.time >= f.duration {
                    fades[i] = None;
                }
            }
            let (st, seconds) = tile(mesh.layer, mesh.local_material);
            let st = st.unwrap_or(self.materials[mesh.material].uv);
            if now[i] == st || mesh.uvs.len() != mesh.vertices {
                continue;
            }
            // From what shows now (mid-crossfade: the new one of it)
            fades[i] = (seconds > 0.0 && tile_map(now[i], st).is_some()).then_some(TileFade { from: now[i], time: 0.0, duration: seconds });
            unsafe {
                let data = (api.gs_vertexbuffer_get_data)(mesh.vb);
                if data.is_null() || (*data).num != mesh.vertices || (*data).tvarray.is_null() || (*(*data).tvarray).array.is_null() {
                    continue;
                }
                let placed = placed_uvs(&mesh.uvs, st);
                std::ptr::copy_nonoverlapping(placed.as_ptr(), (*(*data).tvarray).array as *mut [f32; 2], mesh.vertices);
                (api.gs_vertexbuffer_flush)(mesh.vb);
            }
            now[i] = st;
        }
    }

    /// An OBS source drawn into a texture of its own size (the reflection's
    /// picture); None when it has no size or can't be drawn. A Puppet in
    /// it draws nothing (IN_REFLECTION). Call on the graphics thread.
    pub fn render_source(&self, source: Ptr) -> Option<Ptr> {
        let api = obs::api();
        unsafe {
            let (w, h) = ((api.obs_source_get_width)(source), (api.obs_source_get_height)(source));
            if w == 0 || h == 0 || self.env_rt.is_null() {
                return None;
            }
            (api.gs_texrender_reset)(self.env_rt);
            if !(api.gs_texrender_begin)(self.env_rt, w, h) {
                return None;
            }
            (api.gs_clear)(obs::GS_CLEAR_COLOR, &[0.0; 4], 1.0, 0);
            (api.gs_ortho)(0.0, w as f32, 0.0, h as f32, -100.0, 100.0);
            IN_REFLECTION.with(|f| f.set(true));
            (api.obs_source_video_render)(source);
            IN_REFLECTION.with(|f| f.set(false));
            (api.gs_texrender_end)(self.env_rt);
            let t = (api.gs_texrender_get_texture)(self.env_rt);
            (!t.is_null()).then_some(t)
        }
    }

    /// Layer `layer`'s posed vertices (pose::skin's, per primitive) into the
    /// GPU's buffers. Call on the graphics thread.
    pub fn update_vertices(&self, layer: usize, points: &[Vec<[f32; 4]>], normals: &[Vec<[f32; 4]>]) {
        write_vertices(self.meshes.iter().filter(|m| m.layer == layer), points, normals);
    }

    /// The prefabs on screen now (id, its GLB's key, its model, how much
    /// shows): a new one gets its meshes (its GLB's textures uploaded the
    /// first time), a gone one's are freed, a GLB no copy uses any more too.
    /// Call on the graphics thread.
    pub fn sync_prefabs(&mut self, now: &[(u64, String, std::sync::Arc<Model>, f32)]) {
        let pf = &mut self.prefabs;
        let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut pf.shown).into_iter().partition(|(id, ..)| now.iter().any(|(n, ..)| n == id));
        pf.shown = keep;
        for (_, _, meshes, _) in gone {
            destroy_meshes(meshes);
        }
        for (id, key, model, fade) in now {
            if let Some(s) = pf.shown.iter_mut().find(|(i, ..)| i == id) {
                s.3 = *fade;
                continue;
            }
            if !pf.looks.contains_key(key) {
                pf.looks.insert(key.clone(), upload_looks(model, 0));
            }
            pf.shown.push((*id, key.clone(), make_meshes(model, 0, 0), *fade));
        }
        let unused: Vec<String> = pf.looks.keys().filter(|k| !pf.shown.iter().any(|(_, key, ..)| key == *k)).cloned().collect();
        for k in unused {
            if let Some((textures, _)) = pf.looks.remove(&k) {
                destroy_textures(textures);
            }
        }
    }

    /// A prefab's posed vertices
    pub fn update_prefab(&self, id: u64, points: &[Vec<[f32; 4]>], normals: &[Vec<[f32; 4]>]) {
        if let Some((_, _, meshes, _)) = self.prefabs.shown.iter().find(|(i, ..)| *i == id) {
            write_vertices(meshes.iter(), points, normals);
        }
    }

    /// The parts map: the last one staged, once it's read back (a frame
    /// after it was drawn), and a new one drawn and staged every PARTS_EVERY.
    /// Call on the graphics thread, after draw.
    pub fn parts(&self, width: u32, height: u32, view: View, shader: &Shader) -> Option<PartPixels> {
        let api = obs::api();
        let mut stage = self.parts_stage.lock().unwrap();
        let mut out = None;
        unsafe {
            if stage.pending {
                stage.pending = false;
                if let Some((surface, w, h)) = stage.surface {
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let mut linesize = 0u32;
                    if (api.gs_stagesurface_map)(surface, &mut data, &mut linesize) {
                        let mut ids = vec![0u8; (w * h) as usize];
                        for y in 0..h as usize {
                            let row = data.add(y * linesize as usize);
                            for x in 0..w as usize {
                                ids[y * w as usize + x] = *row.add(x * 4);
                            }
                        }
                        (api.gs_stagesurface_unmap)(surface);
                        out = Some(PartPixels { width: w, height: h, ids });
                    }
                }
            }
            if stage.last.is_some_and(|t| t.elapsed() < PARTS_EVERY) {
                return out;
            }
            stage.last = Some(std::time::Instant::now());

            let (w, h) = ((width / PARTS_SCALE).max(16), (height / PARTS_SCALE).max(16));
            if stage.surface.is_some_and(|(_, sw, sh)| (sw, sh) != (w, h)) {
                let (old, _, _) = stage.surface.take().unwrap();
                (api.gs_stagesurface_destroy)(old);
            }
            if stage.surface.is_none() {
                let s = (api.gs_stagesurface_create)(w, h, obs::GS_RGBA);
                if s.is_null() {
                    return out;
                }
                stage.surface = Some((s, w, h));
            }
            // Same camera as the picture: the map lines up with it, smaller
            let cam = self.camera(width, height, view);
            let values = FrameValues {
                mvp: cam.mvp.transpose().to_cols_array(),
                light: [0.0; 4],
                light_color: [1.0; 4],
                eye: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
                time: seconds(),
                frame: &Frame::default(),
                reflection: None,
            };
            (api.gs_texrender_reset)(self.parts_rt);
            if !(api.gs_texrender_begin)(self.parts_rt, w, h) {
                return out;
            }
            (api.gs_clear)(obs::GS_CLEAR_COLOR | obs::GS_CLEAR_DEPTH, &[0.0; 4], 1.0, 0);
            (api.gs_blend_state_push)();
            let cull = (api.gs_get_cull_mode)();
            (api.gs_set_cull_mode)(obs::GS_NEITHER);
            (api.gs_enable_depth_test)(true);
            (api.gs_depth_function)(obs::GS_LESS);
            (api.gs_enable_blending)(false);
            // Blended bits (lashes, brows) aren't something to hit
            self.pass(shader, &values, c"Parts", |m| m.alpha != Alpha::Blend);
            (api.gs_load_vertexbuffer)(std::ptr::null_mut());
            (api.gs_load_indexbuffer)(std::ptr::null_mut());
            (api.gs_enable_depth_test)(false);
            (api.gs_set_cull_mode)(cull);
            (api.gs_blend_state_pop)();
            (api.gs_texrender_end)(self.parts_rt);
            let texture = (api.gs_texrender_get_texture)(self.parts_rt);
            if let (false, Some((surface, _, _))) = (texture.is_null(), stage.surface) {
                (api.gs_stage_texture)(surface, texture);
                stage.pending = true;
            }
        }
        out
    }

    /// Draws `input` through `effect` (a whole-picture effect: ViewProj,
    /// image, technique Draw) into chain texture `which`, with `values` (every
    /// other number value 0) and `gbuf` when it asks. Its texture, or None.
    unsafe fn run_effect(&self, effect: Ptr, values: &[(String, Value)], input: Ptr, gbuf: Option<Ptr>, width: u32, height: u32, which: usize) -> Option<Ptr> {
        let api = obs::api();
        let target = *self.chain.get(which)?;
        if effect.is_null() || target.is_null() {
            return None;
        }
        unsafe {
            (api.gs_texrender_reset)(target);
            if !(api.gs_texrender_begin)(target, width, height) {
                return None;
            }
            (api.gs_clear)(obs::GS_CLEAR_COLOR, &[0.0; 4], 1.0, 0);
            (api.gs_ortho)(0.0, width as f32, 0.0, height as f32, -100.0, 100.0);
            (api.gs_blend_state_push)();
            (api.gs_enable_blending)(false);
            // Every number value given (Direct3D skips a draw missing one), then this frame's
            let zeros = [0u8; 16];
            for i in 0..(api.gs_effect_get_num_params)(effect) {
                let p = (api.gs_effect_get_param_by_idx)(effect, i);
                let mut info = obs::EffectParamInfo { name: std::ptr::null(), type_: 0 };
                (api.gs_effect_get_param_info)(p, &mut info);
                if let Some(size) = param_size(info.type_) {
                    (api.gs_effect_set_val)(p, zeros.as_ptr() as *const _, size);
                }
            }
            set_values(effect, values);
            (api.gs_effect_set_texture)(param(effect, "image"), input);
            if let Some(g) = gbuf {
                let gp = param(effect, "gbuf");
                if !gp.is_null() {
                    (api.gs_effect_set_texture)(gp, g);
                    let has = param(effect, "has_gbuf");
                    if !has.is_null() {
                        (api.gs_effect_set_float)(has, 1.0);
                    }
                }
            }
            while (api.gs_effect_loop)(effect, c"Draw".as_ptr()) {
                (api.gs_draw_sprite)(input, 0, width, height);
            }
            (api.gs_blend_state_pop)();
            (api.gs_texrender_end)(target);
            let out = (api.gs_texrender_get_texture)(target);
            (!out.is_null()).then_some(out)
        }
    }

    /// The avatar's normals and distances, each pixel (for the ink lines and
    /// filters that ask for them). Its texture, or None when it couldn't draw.
    unsafe fn gbuffer(&self, width: u32, height: u32, cam: &Camera, shader: &Shader, frame: &Frame) -> Option<Ptr> {
        let api = obs::api();
        let values = FrameValues {
            mvp: cam.mvp.transpose().to_cols_array(),
            light: [cam.light.x, cam.light.y, cam.light.z, 0.0], light_color: cam.light_color,
            eye: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
            time: seconds(),
            frame,
            reflection: None,
        };
        unsafe {
            (api.gs_texrender_reset)(self.gbuf);
            if !(api.gs_texrender_begin)(self.gbuf, width, height) {
                return None;
            }
            (api.gs_clear)(obs::GS_CLEAR_COLOR | obs::GS_CLEAR_DEPTH, &[0.0; 4], 1.0, 0);
            (api.gs_blend_state_push)();
            let cull = (api.gs_get_cull_mode)();
            (api.gs_set_cull_mode)(obs::GS_NEITHER);
            (api.gs_enable_depth_test)(true);
            (api.gs_depth_function)(obs::GS_LESS);
            (api.gs_enable_blending)(false);
            // Blended bits (lashes, brows) would outline every hair of them
            self.pass(shader, &values, c"GBuffer", |m| m.alpha != Alpha::Blend);
            (api.gs_load_vertexbuffer)(std::ptr::null_mut());
            (api.gs_load_indexbuffer)(std::ptr::null_mut());
            (api.gs_enable_depth_test)(false);
            (api.gs_set_cull_mode)(cull);
            (api.gs_blend_state_pop)();
            (api.gs_texrender_end)(self.gbuf);
            let gbuf = (api.gs_texrender_get_texture)(self.gbuf);
            (!gbuf.is_null()).then_some(gbuf)
        }
    }

    /// Ink lines over `picture`: where the avatar's normals and distances
    /// (`gbuf`) change sharply (ink.effect), into `inked`. Its texture, or
    /// None when it couldn't draw.
    unsafe fn ink(&self, picture: Ptr, gbuf: Ptr, width: u32, height: u32, frame: &Frame) -> Option<Ptr> {
        let api = obs::api();
        let ink = self.ink.as_ref()?;
        let values = FrameValues { mvp: [0.0; 16], light: [0.0; 4], light_color: [1.0; 4], eye: [0.0; 4], time: seconds(), frame, reflection: None };
        unsafe {
            (api.gs_texrender_reset)(self.inked);
            if !(api.gs_texrender_begin)(self.inked, width, height) {
                return None;
            }
            (api.gs_clear)(obs::GS_CLEAR_COLOR, &[0.0; 4], 1.0, 0);
            (api.gs_ortho)(0.0, width as f32, 0.0, height as f32, -100.0, 100.0);
            (api.gs_blend_state_push)();
            (api.gs_enable_blending)(false);
            let l = &frame.lines;
            let p = |n: &str| param(ink.effect, n);
            (api.gs_effect_set_texture)(p("image"), picture);
            (api.gs_effect_set_texture)(p("gbuf"), gbuf);
            (api.gs_effect_set_vec2)(p("texel"), &[1.0 / width as f32, 1.0 / height as f32]);
            (api.gs_effect_set_float)(p("width_px"), l.width);
            (api.gs_effect_set_vec4)(p("ink_color"), &l.color);
            (api.gs_effect_set_float)(p("depth_sens"), l.depth.max(0.0001));
            (api.gs_effect_set_float)(p("normal_sens"), l.normal.max(0.0001));
            (api.gs_effect_set_float)(p("jitter"), l.jitter);
            (api.gs_effect_set_float)(p("boil"), l.boil);
            (api.gs_effect_set_float)(p("time"), values.time);
            while (api.gs_effect_loop)(ink.effect, c"Draw".as_ptr()) {
                (api.gs_draw_sprite)(picture, 0, width, height);
            }
            (api.gs_blend_state_pop)();
            (api.gs_texrender_end)(self.inked);
            let inked = (api.gs_texrender_get_texture)(self.inked);
            (!inked.is_null()).then_some(inked)
        }
    }

    /// Draws the meshes whose material `wanted`, with `technique`
    unsafe fn pass(&self, shader: &Shader, values: &FrameValues, technique: &std::ffi::CStr, wanted: impl Fn(&Material) -> bool) {
        let api = obs::api();
        let mut first = true;
        let colors = self.colors.lock().unwrap().clone();
        let fade = self.fade.lock().unwrap().clone();
        let uv_now = self.uv_now.lock().unwrap().clone();
        let uv_fade = self.uv_fade.lock().unwrap().clone();
        // The controller isn't a body part to hit
        let skip_layer = if technique == c"Parts" { *self.prop_layer.lock().unwrap() } else { None };
        for (i, mesh) in self.meshes.iter().enumerate() {
            let m = &self.materials[mesh.material];
            let shows = fade.get(mesh.layer).copied().unwrap_or(1.0);
            if !wanted(m) || shows <= 0.0 || Some(mesh.layer) == skip_layer {
                continue;
            }
            // Its colour slot's colour, when the slot has one
            let recoloured = m.color_slot.and_then(|s| colors.get(&s)).map(|rgb| crate::model::recoloured(m, *rgb));
            unsafe {
                shader.set_frame(values);
                self.set_material(shader, recoloured.as_ref().unwrap_or(m), &self.textures);
                // Its GLB fading, its tile crossfading (set after set_frame zeroed them)
                let p = &shader.params;
                (api.gs_effect_set_float)(p.glb_fade, shows.min(1.0));
                let crossfade = uv_fade.get(i).copied().flatten().and_then(|f| Some((tile_map(f.from, *uv_now.get(i)?)?, f.time / f.duration.max(1e-3))));
                let (map, mix) = crossfade.unwrap_or(([1.0, 1.0, 0.0, 0.0], 1.0));
                (api.gs_effect_set_vec4)(p.tile_map, &map);
                (api.gs_effect_set_float)(p.tile_mix, mix);
                // Where its UVs are placed now (its tile): undone for the other maps
                (api.gs_effect_set_vec4)(p.uv_place, uv_now.get(i).unwrap_or(&m.uv));
                if first {
                    first = false;
                    shader.log_unset(technique);
                }
                (api.gs_load_vertexbuffer)(mesh.vb);
                (api.gs_load_indexbuffer)(mesh.ib);
                while (api.gs_effect_loop)(shader.effect, technique.as_ptr()) {
                    (api.gs_draw)(obs::GS_TRIS, 0, mesh.count);
                }
            }
        }
        // The prefabs, after (not body parts to hit: not in the parts map)
        if technique == c"Parts" {
            return;
        }
        for (_, key, meshes, shows) in &self.prefabs.shown {
            let Some((textures, materials)) = self.prefabs.looks.get(key) else { continue };
            if *shows <= 0.0 {
                continue;
            }
            for mesh in meshes {
                let Some(m) = materials.get(mesh.material) else { continue };
                if !wanted(m) {
                    continue;
                }
                let recoloured = m.color_slot.and_then(|s| colors.get(&s)).map(|rgb| crate::model::recoloured(m, *rgb));
                unsafe {
                    shader.set_frame(values);
                    self.set_material(shader, recoloured.as_ref().unwrap_or(m), textures);
                    let p = &shader.params;
                    (api.gs_effect_set_float)(p.glb_fade, shows.min(1.0));
                    (api.gs_effect_set_vec4)(p.tile_map, &[1.0, 1.0, 0.0, 0.0]);
                    (api.gs_effect_set_float)(p.tile_mix, 1.0);
                    (api.gs_effect_set_vec4)(p.uv_place, &m.uv);
                    (api.gs_load_vertexbuffer)(mesh.vb);
                    (api.gs_load_indexbuffer)(mesh.ib);
                    while (api.gs_effect_loop)(shader.effect, technique.as_ptr()) {
                        (api.gs_draw)(obs::GS_TRIS, 0, mesh.count);
                    }
                }
            }
        }
    }

    /// A material's settings and textures (`textures`: the list its indices are in)
    unsafe fn set_material(&self, shader: &Shader, m: &Material, textures: &[Ptr]) {
        let api = obs::api();
        let p = &shader.params;
        let texture = m.image.and_then(|i| textures.get(i).copied()).filter(|t| !t.is_null());
        let cutoff = match m.alpha {
            Alpha::Mask(c) => c,
            // Blended: skip only what's fully clear (keeps the depth clean)
            Alpha::Blend => 0.004,
            Alpha::Opaque => -1.0,
        };
        unsafe {
            (api.gs_effect_set_vec4)(p.base_color, &m.color);
            (api.gs_effect_set_vec4)(p.shade_color, &[m.shade[0], m.shade[1], m.shade[2], 1.0]);
            (api.gs_effect_set_float)(p.use_texture, if texture.is_some() { 1.0 } else { 0.0 });
            if let Some(t) = texture {
                (api.gs_effect_set_texture)(p.image, t);
            }
            (api.gs_effect_set_float)(p.alpha_cutoff, cutoff);
            (api.gs_effect_set_float)(p.blended, if m.alpha == Alpha::Blend { 1.0 } else { 0.0 });
            // What it gives off, its normal map, its gloss (each texture when it has one)
            let tex = |i: Option<usize>| i.and_then(|i| textures.get(i).copied()).filter(|t| !t.is_null());
            for (texture, param, flag) in [(tex(m.emissive_image), p.emissive_tex, p.use_emissive), (tex(m.normal_image), p.normal_tex, p.use_normal), (tex(m.mr_image), p.mr_tex, p.use_mr)] {
                (api.gs_effect_set_float)(flag, if texture.is_some() { 1.0 } else { 0.0 });
                if let Some(t) = texture {
                    (api.gs_effect_set_texture)(param, t);
                }
            }
            (api.gs_effect_set_vec4)(p.emissive, &[m.emissive[0], m.emissive[1], m.emissive[2], 1.0]);
            (api.gs_effect_set_float)(p.normal_scale, m.normal_scale);
            (api.gs_effect_set_float)(p.metallic, m.metallic);
            (api.gs_effect_set_float)(p.roughness, m.roughness);
            (api.gs_effect_set_float)(p.coat, m.coat);
            (api.gs_effect_set_float)(p.coat_roughness, m.coat_roughness);
        }
    }

    /// Frees it all. Call on the graphics thread.
    pub fn destroy(self) {
        let api = obs::api();
        unsafe {
            for m in self.meshes {
                (api.gs_vertexbuffer_destroy)(m.vb);
                (api.gs_indexbuffer_destroy)(m.ib);
            }
            for t in self.textures.into_iter().filter(|t| !t.is_null()) {
                (api.gs_texture_destroy)(t);
            }
            for (_, _, meshes, _) in self.prefabs.shown {
                destroy_meshes(meshes);
            }
            for (_, (textures, _)) in self.prefabs.looks {
                destroy_textures(textures);
            }
            (api.gs_texrender_destroy)(self.target);
            (api.gs_texrender_destroy)(self.gbuf);
            (api.gs_texrender_destroy)(self.inked);
            for t in self.chain {
                (api.gs_texrender_destroy)(t);
            }
            destroy_effect(self.unpremultiply);
            (api.gs_texrender_destroy)(self.parts_rt);
            (api.gs_texrender_destroy)(self.env_rt);
            if let Some((surface, _, _)) = self.parts_stage.into_inner().ok().and_then(|s| s.surface) {
                (api.gs_stagesurface_destroy)(surface);
            }
            if let Some(ink) = self.ink {
                (api.gs_effect_destroy)(ink.effect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lights_defaults_are_the_light_puppet_always_had() {
        // Its own light's starting settings (-22 degrees around, 29 up) point where the old one did
        let own = View::light_from(-22.0, 29.0);
        assert!(own.angle_between(View::LIGHT.normalize()) < 0.02, "{own:?}");
        // Straight above, and from the camera's side
        assert!((View::light_from(0.0, 90.0) - Vec3::Y).length() < 1e-5);
        assert!((View::light_from(0.0, 0.0) - Vec3::Z).length() < 1e-5);
    }

    fn view(orbit: f32, orthographic: bool) -> View {
        View { zoom: 1.0, height: 0.0, yaw: 0.0, orbit, orthographic, position: Vec3::ZERO, light: View::LIGHT, light_color: [1.0; 3] }
    }

    /// Where a point of the avatar lands in the picture (-1..1)
    fn on_screen(cam: &Camera, p: Vec3) -> glam::Vec3 {
        let c = cam.mvp * p.extend(1.0);
        c.truncate() / c.w
    }

    #[test]
    fn turn_takes_the_camera_around_the_avatar() {
        let (min, max) = (Vec3::new(-0.3, 0.0, -0.15), Vec3::new(0.3, 1.7, 0.15));
        // At 0 the camera is in front (+Z); at 180 behind, at 90 on a side, always as far
        let front = camera(min, max, 1920, 1080, view(0.0, false));
        let back = camera(min, max, 1920, 1080, view(180.0, false));
        let side = camera(min, max, 1920, 1080, view(90.0, false));
        assert!(front.eye.z > 1.0 && back.eye.z < -1.0, "{:?} {:?}", front.eye, back.eye);
        assert!(side.eye.z.abs() < 1e-3 && side.eye.x.abs() > 1.0, "{:?}", side.eye);
        assert!((front.eye.length() - back.eye.length()).abs() < 1e-4);
        // The light goes around with it: from the camera's side, front or back
        assert!(front.light.z > 0.0 && back.light.z < 0.0);
        // The avatar's middle stays in the middle of the picture
        let mid = on_screen(&back, Vec3::new(0.0, 0.85, 0.0));
        assert!(mid.x.abs() < 1e-4 && mid.y.abs() < 1e-4, "{mid:?}");
    }

    #[test]
    fn turn_is_a_circle_around_the_avatar_wherever_it_stands() {
        // A wide avatar (arms out), moved left and back: all the way round, the
        // camera is as far from it, and it stays where it is in the picture
        let (min, max) = (Vec3::new(-0.8, 0.0, -0.15), Vec3::new(0.8, 1.7, 0.15));
        let moved = |orbit: f32| View { position: Vec3::new(-0.9, 0.0, -0.8), ..view(orbit, false) };
        let at0 = camera(min, max, 1920, 1080, moved(0.0));
        let mid0 = on_screen(&at0, Vec3::new(0.0, 0.85, 0.0));
        for orbit in [30.0, 90.0, 135.0, 180.0, -90.0] {
            let cam = camera(min, max, 1920, 1080, moved(orbit));
            let d = glam::Vec2::new(cam.eye.x, cam.eye.z).length();
            assert!((d - glam::Vec2::new(at0.eye.x, at0.eye.z).length()).abs() < 1e-3, "{orbit}: {:?}", cam.eye);
            let mid = on_screen(&cam, Vec3::new(0.0, 0.85, 0.0));
            assert!((mid - mid0).length() < 1e-3, "{orbit}: {mid:?} vs {mid0:?}");
        }
        // Position at Turn 0 moves it as it always did: left in the picture
        assert!(mid0.x < -0.1);
    }

    #[test]
    fn zoom_keeps_zooming_close_up() {
        // A wide avatar (arms out): each zoom shows it bigger, up to 50, in both cameras
        let (min, max) = (Vec3::new(-0.8, 0.0, -0.15), Vec3::new(0.8, 1.7, 0.15));
        for orthographic in [false, true] {
            let mut last = 0.0;
            for zoom in [1.0, 4.0, 8.0, 16.0, 30.0, 50.0] {
                let cam = camera(min, max, 1920, 1080, View { zoom, height: 0.5, ..view(0.0, orthographic) });
                let (a, b) = (on_screen(&cam, Vec3::new(0.0, 1.30, 0.0)), on_screen(&cam, Vec3::new(0.0, 1.31, 0.0)));
                let size = b.y - a.y;
                assert!(size > last * 1.4, "zoom {zoom} ({orthographic}): {size} after {last}");
                last = size;
            }
        }
    }

    #[test]
    fn orthographic_has_no_perspective() {
        let (min, max) = (Vec3::new(-0.3, 0.0, -0.15), Vec3::new(0.3, 1.7, 0.15));
        // A point near the camera and one far from it, the same height: the same height in the picture
        let ortho = camera(min, max, 1920, 1080, view(0.0, true));
        let (near, far) = (on_screen(&ortho, Vec3::new(0.2, 1.5, 0.15)), on_screen(&ortho, Vec3::new(0.2, 1.5, -0.15)));
        assert!((near.y - far.y).abs() < 1e-5 && (near.x - far.x).abs() < 1e-5, "{near:?} {far:?}");
        // With perspective, the near one is farther from the middle
        let persp = camera(min, max, 1920, 1080, view(0.0, false));
        let (near, far) = (on_screen(&persp, Vec3::new(0.2, 1.5, 0.15)), on_screen(&persp, Vec3::new(0.2, 1.5, -0.15)));
        assert!(near.x > far.x);
        // Both in the picture, the same size at its middle (switching doesn't jump)
        let tall = |cam: &Camera| {
            let (top, bottom) = (on_screen(cam, Vec3::new(0.0, 1.7, 0.0)), on_screen(cam, Vec3::new(0.0, 0.0, 0.0)));
            assert!(top.y < 1.0 && bottom.y > -1.0, "{top:?} {bottom:?}");
            assert!((0.0..1.0).contains(&top.z) && (0.0..1.0).contains(&bottom.z));
            top.y - bottom.y
        };
        assert!((tall(&ortho) - tall(&persp)).abs() < 1e-3);
    }
}
