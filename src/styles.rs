//! Styles: how the avatar is shaded, written as Rhai scripts (`*.rhai`), the
//! way Theater Scene's filters are. A style's shader sees each pixel of the
//! avatar in 3D (its texture colour, shade colour, normal, light and view),
//! so a toon style can band the light and draw outlines. They're the
//! avatar's permanent look; Theater Scene's filters, on the source, are the
//! passing effects.
//!
//! Styles stack: a source has several Style slots, each style getting the
//! colour the ones before it made (`c`; the first gets the texture colour).
//! Each slot's uniforms and functions are renamed apart (`prefix_code`), so
//! styles never clash, even the same one twice.
//!
//! ```text
//! const ID = "toon";                    // kept in scenes: never change it
//! const NAME = "Toon";                  // in the source's Style lists
//! const SHADER = `
//!     uniform float bands;              // set from tick's map, by name
//!     float4 style(Surface s, float4 c) { … }   // this pixel's colour, from c
//! `;
//! fn defaults() { #{ bands: 2.0 } }
//! fn properties() { [ slider("bands", "Bands", 1.0, 6.0, 1.0) ] }
//! fn tick(dt) { #{ bands: this.settings.bands, outline_width: 0.002 } }
//! ```
//!
//! `Surface`: `albedo` (float4: texture x colour, alpha), `shade` (float3:
//! texture x the material's shade_color, MToon's shadow colour), `normal`,
//! `light` (towards the light), `view` (towards the camera), `uv`.
//!
//! tick's map sets the shader's uniforms by name: a number (float), a bool
//! (0 or 1), or an array of 2–4 numbers (float2–float4; `color_vec(c)`
//! turns a colour property into float4). Two keys are Puppet's own:
//! `outline_width` (metres, 0 = none) and `outline_color`
//! (`color_vec(…)` or an array), `outline_jitter` (0–1: the width wanders
//! along the stroke) and `outline_boil` (times a second the wander is
//! redrawn, like hand-drawn animation); the last slot with an outline draws
//! it.
//!
//! Ink lines, found in the picture like Blender's Line Art (they catch
//! silhouettes, hair over the face, folds, and single-sheet hair cards the
//! outline can't): `lines` (pixels, 0 = none), `lines_color`,
//! `lines_depth` (a relative distance jump that makes a line, ~0.02),
//! `lines_normal` (a surface turn, 0–2, ~0.4), `lines_jitter`,
//! `lines_boil`; the last slot with lines draws them.
//!
//! `Surface` also has `screen` (the pixel, from the top left) and `time`
//! (seconds), for grain, hatching and anything that moves.
//!
//! `this`: `this.settings`, `this.time` (seconds), `this.width` /
//! `this.height` (the source's size), and anything the script keeps there.
//! Properties: `slider`, `int_slider`, `toggle`, `color` (0xAARRGGBB),
//! `choice(key, label, [[label, value], …])`, `info`.
//!
//! Styles are read from the plugin's `data/styles` and from
//! `plugin_config/theater-puppet/styles` (yours); a saved file reloads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use rhai::{AST, Array, CallFnOptions, Dynamic, Engine, ImmutableString, Map, Scope};

const MAX_OPERATIONS: u64 = 1_000_000;

const PRELUDE: &str = r#"
fn slider(key, label, min, max, step) { #{ kind: "slider", key: key, label: label, min: min, max: max, step: step } }
fn int_slider(key, label, min, max, step) { #{ kind: "int_slider", key: key, label: label, min: min, max: max, step: step } }
fn toggle(key, label) { #{ kind: "toggle", key: key, label: label } }
fn color(key, label) { #{ kind: "color", key: key, label: label } }
fn choice(key, label, options) { #{ kind: "choice", key: key, label: label, options: options } }
fn info(label) { #{ kind: "info", label: label } }
"#;

/// One property of a style, as its script declared it
#[derive(Clone, Debug)]
pub enum Prop {
    Slider { key: String, label: String, min: f64, max: f64, step: f64 },
    IntSlider { key: String, label: String, min: i64, max: i64, step: i64 },
    Toggle { key: String, label: String },
    /// 0xAARRGGBB
    Color { key: String, label: String },
    Choice { key: String, label: String, options: Vec<(String, String)> },
    Info { label: String },
}

impl Prop {
    pub fn key(&self) -> Option<&str> {
        match self {
            Prop::Slider { key, .. } | Prop::IntSlider { key, .. } | Prop::Toggle { key, .. } | Prop::Color { key, .. } | Prop::Choice { key, .. } => Some(key),
            Prop::Info { .. } => None,
        }
    }
}

pub struct Style {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub shader: String,
    pub defaults: Map,
    pub props: Vec<Prop>,
    ast: AST,
    /// Changes when the file is reloaded: the source rebuilds its shader
    pub version: u64,
}

/// A value for one of the shader's uniforms
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Float(f32),
    Vec(Vec<f32>),
}

/// What a style's tick asks for this frame
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub uniforms: Vec<(String, Value)>,
    pub outline_width: f32,
    pub outline_color: [f32; 4],
    /// 0 = an even line; 1 = its width wanders a lot along the stroke
    pub outline_jitter: f32,
    /// How many times a second the wander changes (0 = never)
    pub outline_boil: f32,
    /// Ink lines found in the picture (render.rs, ink.effect)
    pub lines: Lines,
}

/// Lines drawn where the picture's depth jumps or its surface folds
#[derive(Clone, Debug, PartialEq)]
pub struct Lines {
    /// Pixels; 0 = none
    pub width: f32,
    pub color: [f32; 4],
    /// A distance jump (relative) that makes a line: lower = more lines
    pub depth: f32,
    /// A turn of the surface (0–2) that makes a line: lower = more lines
    pub normal: f32,
    pub jitter: f32,
    pub boil: f32,
}

impl Default for Lines {
    fn default() -> Self {
        Lines { width: 0.0, color: [0.1, 0.1, 0.1, 1.0], depth: 0.02, normal: 0.4, jitter: 0.0, boil: 0.0 }
    }
}

pub fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_call_levels(32);
        engine.set_max_expr_depths(64, 32);
        engine.on_print(|s| crate::log(&format!("[Puppet Rhai] {s}")));
        engine.on_debug(|s, src, pos| crate::log(&format!("[Puppet Rhai] {} {pos:?} {s}", src.unwrap_or(""))));
        // A colour property (0xAARRGGBB) as a shader's float4, 0–1
        engine.register_fn("color_vec", |c: i64| -> Array {
            let ch = |shift: i64| Dynamic::from_float(((c >> shift) & 255) as f64 / 255.0);
            vec![ch(16), ch(8), ch(0), ch(24)]
        });
        engine
    })
}

/// Compiles the style script at `path`.
pub fn compile(path: &Path, version: u64) -> Result<Style, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let engine = engine();
    let script = engine.compile(&source).map_err(|e| e.to_string())?;
    let mut scope = Scope::new();
    engine.run_ast_with_scope(&mut scope, &script).map_err(|e| e.to_string())?;
    let text = |name: &str| scope.get_value::<ImmutableString>(name).map(|s| s.to_string());
    let id = text("ID").filter(|s| !s.trim().is_empty()).ok_or("no `const ID = \"…\";`")?;
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("ID \"{id}\": letters, digits and _ only"));
    }
    let name = text("NAME").unwrap_or_else(|| id.clone());
    let shader = text("SHADER").ok_or("no `const SHADER`")?;
    if !shader.contains("style(") {
        return Err("SHADER has no `float4 style(Surface s, float4 c)`".into());
    }
    let ast = engine.compile(PRELUDE).map_err(|e| e.to_string())?.merge(&script).clone_functions_only();
    let mut style = Style { id, name, path: path.to_path_buf(), shader, defaults: Map::new(), props: Vec::new(), ast, version };
    if style.has("defaults") {
        style.defaults = call(&style, None, "defaults", ())?.try_cast::<Map>().ok_or("defaults() must return a map")?;
    }
    if style.has("properties") {
        let list = call(&style, None, "properties", ())?.try_cast::<Array>().ok_or("properties() must return an array")?;
        style.props = list.into_iter().filter_map(prop).collect();
    }
    if !style.has("tick") {
        return Err("no `fn tick(dt)`".into());
    }
    Ok(style)
}

impl Style {
    fn has(&self, name: &str) -> bool {
        self.ast.iter_functions().any(|f| f.name == name)
    }

    /// Runs tick(dt) with `this`; the uniforms it asks for
    pub fn tick(&self, this: &mut Dynamic, dt: f32) -> Result<Frame, String> {
        let out = call(self, Some(this), "tick", (dt as f64,))?;
        let mut frame = Frame::default();
        let Some(map) = out.try_cast::<Map>() else { return Ok(frame) };
        for (key, value) in map {
            let Some(value) = to_value(&value) else { continue };
            match (key.as_str(), &value) {
                ("outline_width", Value::Float(w)) => frame.outline_width = *w,
                ("outline_jitter", Value::Float(j)) => frame.outline_jitter = *j,
                ("outline_boil", Value::Float(b)) => frame.outline_boil = *b,
                ("lines", Value::Float(w)) => frame.lines.width = *w,
                ("lines_depth", Value::Float(d)) => frame.lines.depth = *d,
                ("lines_normal", Value::Float(n)) => frame.lines.normal = *n,
                ("lines_jitter", Value::Float(j)) => frame.lines.jitter = *j,
                ("lines_boil", Value::Float(b)) => frame.lines.boil = *b,
                ("lines_color", Value::Vec(c)) => {
                    for (i, x) in c.iter().take(4).enumerate() {
                        frame.lines.color[i] = *x;
                    }
                    if c.len() == 3 {
                        frame.lines.color[3] = 1.0;
                    }
                }
                ("outline_color", Value::Vec(c)) => {
                    for (i, x) in c.iter().take(4).enumerate() {
                        frame.outline_color[i] = *x;
                    }
                    if c.len() == 3 {
                        frame.outline_color[3] = 1.0;
                    }
                }
                _ => frame.uniforms.push((key.to_string(), value)),
            }
        }
        Ok(frame)
    }
}

fn call(style: &Style, this: Option<&mut Dynamic>, name: &str, args: impl rhai::FuncArgs) -> Result<Dynamic, String> {
    let mut options = CallFnOptions::new().eval_ast(false);
    if let Some(this) = this {
        options = options.bind_this_ptr(this);
    }
    engine()
        .call_fn_with_options::<Dynamic>(options, &mut Scope::new(), &style.ast, name, args)
        .map_err(|e| format!("{}: {name}(): {e}", style.path.display()))
}

fn number(v: &Dynamic) -> Option<f64> {
    v.as_float().ok().or_else(|| v.as_int().ok().map(|i| i as f64)).or_else(|| v.as_bool().ok().map(|b| if b { 1.0 } else { 0.0 }))
}

fn to_value(v: &Dynamic) -> Option<Value> {
    if let Some(n) = number(v) {
        return Some(Value::Float(n as f32));
    }
    let arr = v.clone().try_cast::<Array>()?;
    let nums: Vec<f32> = arr.iter().filter_map(number).map(|n| n as f32).collect();
    (2..=4).contains(&nums.len()).then_some(Value::Vec(nums))
}

fn prop(v: Dynamic) -> Option<Prop> {
    let m = v.try_cast::<Map>()?;
    let s = |k: &str| m.get(k).and_then(|v| v.clone().into_string().ok()).unwrap_or_default();
    let f = |k: &str| m.get(k).and_then(number).unwrap_or(0.0);
    let (key, label) = (s("key"), s("label"));
    Some(match s("kind").as_str() {
        "slider" => Prop::Slider { key, label, min: f("min"), max: f("max"), step: f("step") },
        "int_slider" => Prop::IntSlider { key, label, min: f("min") as i64, max: f("max") as i64, step: (f("step") as i64).max(1) },
        "toggle" => Prop::Toggle { key, label },
        "color" => Prop::Color { key, label },
        "choice" => {
            let options = m.get("options")?.clone().try_cast::<Array>()?;
            let options = options
                .into_iter()
                .filter_map(|o| {
                    let pair = o.try_cast::<Array>()?;
                    Some((pair.first()?.to_string(), pair.get(1)?.to_string()))
                })
                .collect();
            Prop::Choice { key, label, options }
        }
        "info" => Prop::Info { label },
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Stacking: each slot's code renamed apart, then called in order

const TYPES: &[&str] = &[
    "void", "bool", "int", "uint", "float", "float2", "float3", "float4", "half", "half2", "half3", "half4", "float2x2", "float3x3", "float4x4",
];

/// The uniforms and functions `code` declares
fn declared(code: &str) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for line in code.lines() {
        let words: Vec<&str> = line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        let t = line.trim_start();
        // uniform float4 tint;  /  uniform float bands = 1.0;
        if t.starts_with("uniform ") && words.len() >= 3 {
            names.insert(words[2].to_string());
        }
        // float4 style(Surface s, float4 c)  (a declaration: no `;` on the line)
        else if words.len() >= 2 && TYPES.contains(&words[0]) && !line.contains(';') {
            let after = t[t.find(words[1]).unwrap_or(0) + words[1].len()..].trim_start();
            if after.starts_with('(') && t.starts_with(words[0]) {
                names.insert(words[1].to_string());
            }
        }
    }
    names
}

/// `code` with its uniforms and functions renamed `<prefix><name>`
/// (whole words only, not after a `.`: `s.light` stays)
pub fn prefix_code(code: &str, prefix: &str) -> String {
    let names = declared(code);
    let mut out = String::with_capacity(code.len() + 64);
    let mut word = String::new();
    let mut prev = ' ';
    let flush = |word: &mut String, out: &mut String, prev: char| {
        if !word.is_empty() {
            if prev != '.' && names.contains(word.as_str()) {
                out.push_str(prefix);
            }
            out.push_str(word);
            word.clear();
        }
    };
    let mut before_word = ' ';
    for ch in code.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if word.is_empty() {
                before_word = prev;
            }
            word.push(ch);
        } else {
            flush(&mut word, &mut out, before_word);
            out.push(ch);
        }
        if !ch.is_whitespace() {
            prev = ch;
        }
    }
    flush(&mut word, &mut out, before_word);
    out
}

/// The prefix of slot `i`'s names in the shader
pub fn slot_prefix(i: usize) -> String {
    format!("L{i}_")
}

/// The shader code for a stack of styles' SHADERs (slot order), as the
/// template's `style(Surface s)`
pub fn compose(shaders: &[(usize, &str)]) -> String {
    let mut code = String::new();
    for (slot, shader) in shaders {
        code.push_str(&prefix_code(shader, &slot_prefix(*slot)));
        code.push('\n');
    }
    code.push_str("float4 style(Surface s)\n{\n\tfloat4 c = s.albedo;\n");
    for (slot, _) in shaders {
        code.push_str(&format!("\tc = {}style(s, c);\n", slot_prefix(*slot)));
    }
    code.push_str("\treturn c;\n}\n");
    code
}

// ---------------------------------------------------------------------------
// The styles found on disk, reloaded when their files change

#[derive(Default)]
struct Registry {
    styles: Vec<Arc<Style>>,
    seen: HashMap<PathBuf, SystemTime>,
    last_scan: Option<Instant>,
    version: u64,
}

static REGISTRY: std::sync::LazyLock<Mutex<Registry>> = std::sync::LazyLock::new(|| Mutex::new(Registry::default()));

/// The folders styles are read from: the plugin's, then the user's
pub fn folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(dev) = std::env::var("THEATER_PUPPET_DIR") {
        out.push(PathBuf::from(dev).join("styles"));
    } else if let Some(data) = crate::data_dir() {
        out.push(data.join("styles"));
    }
    out.push(crate::config_dir().join("styles"));
    out
}

/// Every style, rescanned at most once a second (a saved file reloads)
pub fn all() -> Vec<Arc<Style>> {
    let mut reg = REGISTRY.lock().unwrap();
    if reg.last_scan.is_none_or(|t| t.elapsed().as_secs_f32() >= 1.0) {
        reg.last_scan = Some(Instant::now());
        scan(&mut reg);
    }
    reg.styles.clone()
}

pub fn find(id: &str) -> Option<Arc<Style>> {
    all().into_iter().find(|s| s.id == id)
}

/// Changes whenever a style is added, removed or reloaded
pub fn generation() -> u64 {
    let _ = all();
    let reg = REGISTRY.lock().unwrap();
    reg.version.wrapping_mul(1_000_003).wrapping_add(reg.styles.len() as u64)
}

fn scan(reg: &mut Registry) {
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    for dir in folders() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("rhai")) {
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((path, mtime));
            }
        }
    }
    let changed = files.len() != reg.seen.len() || files.iter().any(|(p, t)| reg.seen.get(p) != Some(t));
    if !changed {
        return;
    }
    let mut styles: Vec<Arc<Style>> = Vec::new();
    for (path, mtime) in &files {
        let unchanged = reg.seen.get(path) == Some(mtime);
        // An unchanged file keeps its compiled style (and version)
        if let Some(old) = reg.styles.iter().find(|s| &s.path == path).filter(|_| unchanged) {
            styles.push(old.clone());
            continue;
        }
        reg.version += 1;
        match compile(path, reg.version) {
            Ok(s) => {
                crate::log(&format!("[Puppet] 🎨 Style \"{}\" ({}) from {}", s.name, s.id, path.display()));
                styles.push(Arc::new(s));
            }
            // A broken edit keeps the last good version running
            Err(e) => {
                crate::log(&format!("[Puppet] ⚠️ Style {}: {e}", path.display()));
                if let Some(old) = reg.styles.iter().find(|s| &s.path == path) {
                    styles.push(old.clone());
                }
            }
        }
    }
    // The first of each id wins (the plugin's before the user's)
    let mut ids = std::collections::HashSet::new();
    styles.retain(|s| ids.insert(s.id.clone()));
    styles.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    reg.styles = styles;
    reg.seen = files.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacked_styles_are_renamed_apart() {
        let toon = "uniform float bands;\nuniform float4 rim_color;\nfloat curve(float x)\n{\n\treturn x * bands;\n}\nfloat4 style(Surface s, float4 c)\n{\n\tfloat l = curve(dot(s.normal, s.light));\n\treturn float4(c.rgb * l + rim_color.rgb, c.a);\n}\n";
        let code = compose(&[(0, toon), (1, toon)]);
        println!("{code}");
        // Each slot's own names
        assert!(code.contains("uniform float L0_bands;") && code.contains("uniform float L1_bands;"));
        assert!(code.contains("float L0_curve(float x)") && code.contains("L1_curve(dot(s.normal, s.light))"));
        assert!(code.contains("return x * L0_bands;"));
        // Members, types and HLSL's own functions untouched
        assert!(code.contains("dot(s.normal, s.light)") && !code.contains("L0_dot") && !code.contains("s.L0_"));
        assert!(code.contains("L0_rim_color.rgb"));
        // Called in order, each on the last one's colour
        let first = code.find("c = L0_style(s, c);").unwrap();
        let second = code.find("c = L1_style(s, c);").unwrap();
        assert!(first < second);
    }

    #[test]
    fn the_shipped_styles_compose() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("styles");
        let toon = compile(&dir.join("toon.rhai"), 1).unwrap();
        let grade = compile(&dir.join("grade.rhai"), 2).unwrap();
        let pencil = compile(&dir.join("pencil.rhai"), 3).unwrap();
        assert!(compose(&[(0, &pencil.shader)]).contains("float L0_hash(float2 p)"));
        let code = compose(&[(0, &toon.shader), (1, &grade.shader)]);
        assert!(code.contains("uniform float L0_threshold;") && code.contains("uniform float4 L1_tint;"));
        assert!(code.contains("float4 L0_style(Surface s, float4 c)") && code.contains("float4 L1_style(Surface s, float4 c)"));
        assert!(!code.contains("L0_saturate") && !code.contains("L1_lerp"));
    }

    #[test]
    fn the_toon_style_compiles_and_ticks() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("styles").join("toon.rhai");
        let style = compile(&path, 1).unwrap();
        assert_eq!(style.id, "toon");
        assert!(!style.props.is_empty());
        let mut this = Dynamic::from_map(Map::new());
        let mut settings = style.defaults.clone();
        settings.insert("outline_color".into(), Dynamic::from_int(0xFF102030));
        if let Some(mut m) = this.write_lock::<Map>() {
            m.insert("settings".into(), Dynamic::from_map(settings));
            m.insert("time".into(), Dynamic::from_float(0.0));
        }
        let frame = style.tick(&mut this, 1.0 / 60.0).unwrap();
        println!("{frame:?}");
        assert!(frame.outline_width > 0.0);
        assert!((frame.outline_color[0] - 0x10 as f32 / 255.0).abs() < 1e-4);
        assert!(frame.uniforms.iter().any(|(k, _)| k == "bands"));
    }
}
