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
    eye: Ptr,
    outline_width: Ptr,
    outline_color: Ptr,
    outline_jitter: Ptr,
    outline_boil: Ptr,
    time: Ptr,
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
    eye: [f32; 4],
    /// Seconds since Puppet loaded (wraps every hour, for float precision)
    time: f32,
    frame: &'a Frame,
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
            eye: p("eye"),
            outline_width: p("outline_width"),
            outline_color: p("outline_color"),
            outline_jitter: p("outline_jitter"),
            outline_boil: p("outline_boil"),
            time: p("time"),
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
            (api.gs_effect_set_vec4)(p.eye, &v.eye);
            (api.gs_effect_set_float)(p.outline_width, v.frame.outline_width);
            (api.gs_effect_set_vec4)(p.outline_color, &v.frame.outline_color);
            (api.gs_effect_set_float)(p.outline_jitter, v.frame.outline_jitter);
            (api.gs_effect_set_float)(p.outline_boil, v.frame.outline_boil);
            (api.gs_effect_set_float)(p.time, v.time);
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
}

pub struct Gpu {
    meshes: Vec<Mesh>,
    textures: Vec<Ptr>,
    materials: Vec<Material>,
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
    pub min: Vec3,
    pub max: Vec3,
    /// The first frame's line in the log is written
    logged: AtomicBool,
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
    /// Turn, degrees
    pub yaw: f32,
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
    /// The camera, in the avatar's own space
    eye: Vec3,
}

impl Gpu {
    /// Uploads the model. Call on the graphics thread.
    pub fn upload(model: &Model) -> Result<Gpu, String> {
        let api = obs::api();
        let textures = model
            .images
            .iter()
            .map(|img| {
                let data = img.rgba.as_ptr();
                unsafe { (api.gs_texture_create)(img.width, img.height, obs::GS_RGBA, 1, &data, 0) }
            })
            .collect();

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
                (*tv).array = obs_copy(&p.uvs) as *mut _;
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
            meshes.push(Mesh { vb, ib, count: p.indices.len() as u32, material: p.material, prim, vertices: n });
        }
        let target = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_Z24_S8) };
        if target.is_null() {
            return Err("no off-screen texture".into());
        }
        let gbuf = unsafe { (api.gs_texrender_create)(obs::GS_RGBA16F, obs::GS_Z24_S8) };
        let inked = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_ZS_NONE) };
        let parts_rt = unsafe { (api.gs_texrender_create)(obs::GS_RGBA, obs::GS_Z24_S8) };
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
            textures,
            materials: model.materials.clone(),
            target,
            gbuf,
            inked,
            ink,
            chain,
            unpremultiply,
            parts_rt,
            parts_stage: Default::default(),
            min: model.min,
            max: model.max,
            logged: AtomicBool::new(false),
        })
    }

    /// The camera: the avatar centred, `zoom` x its height in view
    fn camera(&self, width: u32, height: u32, view: View) -> Camera {
        let size = self.max - self.min;
        let centre = (self.min + self.max) * 0.5;
        let fov = 30f32.to_radians();
        let aspect = width as f32 / height.max(1) as f32;
        // Standing on the origin, looked at from the front (+Z). (Turn turns
        // the body itself, motion.rs: its hair swings after it.)
        let model = Mat4::from_translation(Vec3::new(-centre.x, 0.0, -centre.z));
        let visible = size.y * 1.08 / view.zoom.max(0.05);
        let target = Vec3::new(0.0, centre.y + view.height * size.y * 0.5, 0.0);
        let distance = visible * 0.5 / (fov * 0.5).tan() + size.z;
        let eye = target + Vec3::Z * distance;
        let proj = Mat4::perspective_rh(fov, aspect, 0.01, distance + size.length() * 2.0);
        let inverse = model.inverse();
        Camera {
            mvp: proj * Mat4::look_at_rh(eye, target, Vec3::Y) * model,
            // From the front, upper left
            light: inverse.transform_vector3(Vec3::new(-0.4, 0.6, 1.0).normalize()),
            eye: inverse.transform_point3(eye),
        }
    }

    /// Draws the avatar into the current target, `width` x `height`, with
    /// `shader` and the style's `frame`.
    pub fn draw(&self, width: u32, height: u32, view: View, shader: &Shader, frame: &Frame, filters: &[FilterPass]) {
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
                    light: [cam.light.x, cam.light.y, cam.light.z, 0.0],
                    eye: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
                    time: seconds(),
                    frame,
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

    /// The posed vertices (pose::skin's, per primitive) into the GPU's
    /// buffers. Call on the graphics thread.
    pub fn update_vertices(&self, points: &[Vec<[f32; 4]>], normals: &[Vec<[f32; 4]>]) {
        let api = obs::api();
        for mesh in &self.meshes {
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
                eye: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
                time: seconds(),
                frame: &Frame::default(),
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
            light: [cam.light.x, cam.light.y, cam.light.z, 0.0],
            eye: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
            time: seconds(),
            frame,
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
        let values = FrameValues { mvp: [0.0; 16], light: [0.0; 4], eye: [0.0; 4], time: seconds(), frame };
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
        for mesh in &self.meshes {
            let m = &self.materials[mesh.material];
            if !wanted(m) {
                continue;
            }
            unsafe {
                shader.set_frame(values);
                self.set_material(shader, m);
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
    }

    unsafe fn set_material(&self, shader: &Shader, m: &Material) {
        let api = obs::api();
        let p = &shader.params;
        let texture = m.image.and_then(|i| self.textures.get(i).copied()).filter(|t| !t.is_null());
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
            (api.gs_texrender_destroy)(self.target);
            (api.gs_texrender_destroy)(self.gbuf);
            (api.gs_texrender_destroy)(self.inked);
            for t in self.chain {
                (api.gs_texrender_destroy)(t);
            }
            destroy_effect(self.unpremultiply);
            (api.gs_texrender_destroy)(self.parts_rt);
            if let Some((surface, _, _)) = self.parts_stage.into_inner().ok().and_then(|s| s.surface) {
                (api.gs_stagesurface_destroy)(surface);
            }
            if let Some(ink) = self.ink {
                (api.gs_effect_destroy)(ink.effect);
            }
        }
    }
}
