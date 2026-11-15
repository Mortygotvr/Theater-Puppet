//! Filters: Theater Scene's Rhai filters (`*.rhai`, the same files) run on
//! the whole avatar as one picture, after it's drawn (and inked), in the
//! source's Filter slots. A light, uniform look whatever the materials;
//! the styles (styles.rs) shade it in 3D before.
//!
//! The format is Scene's (see its script_filters.rs): `ID`, `NAME`, an
//! optional `BLEND` ("normal" or "premultiplied"), `SHADER` (a whole OBS
//! effect: `ViewProj`, `image`, technique `Draw`) or `SHADER_FILE`,
//! `defaults()`, `properties()`, `tick(dt)` returning the shader's values by
//! name. `this`: `settings`, `time`, `width` / `height` (the picture).
//!
//! Puppet gives a filter more when its shader asks for it: `gbuf` (texture:
//! each pixel's normal in rgb, its distance in a, 0 = nothing there) and
//! `has_gbuf` (1 here; Scene leaves it 0), for lines that follow the
//! avatar's shape. A Scene-only call (share, publish…) stops that filter's
//! tick, noted in the log; sources and docks (KIND) are left out.
//!
//! Read from the plugin's `data/filters`, yours in
//! `plugin_config/theater-puppet/filters`, and Theater Scene's two filter
//! folders; a saved file reloads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use rhai::{AST, Array, CallFnOptions, Dynamic, ImmutableString, Map, Scope};

use crate::styles::{self, Prop, Value};

pub struct Filter {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub shader: String,
    /// Its output is premultiplied (BLEND = "premultiplied")
    pub premultiplied: bool,
    pub defaults: Map,
    pub props: Vec<Prop>,
    ast: AST,
    pub version: u64,
}

pub fn compile(path: &Path, version: u64) -> Result<Filter, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let engine = styles::engine();
    let script = engine.compile(&source).map_err(|e| e.to_string())?;
    let mut scope = Scope::new();
    engine.run_ast_with_scope(&mut scope, &script).map_err(|e| e.to_string())?;
    let text = |name: &str| scope.get_value::<ImmutableString>(name).map(|s| s.to_string());
    match text("KIND").as_deref() {
        None | Some("filter") => {}
        Some(other) => return Err(format!("a {other}, not a filter")),
    }
    let id = text("ID").filter(|s| !s.trim().is_empty()).ok_or("no `const ID = \"…\";`")?;
    let name = text("NAME").unwrap_or_else(|| id.clone());
    let premultiplied = match text("BLEND").as_deref() {
        None | Some("normal") => false,
        Some("premultiplied") => true,
        Some(other) => return Err(format!("BLEND \"{other}\": \"normal\" or \"premultiplied\"")),
    };
    let shader = match (text("SHADER"), text("SHADER_FILE")) {
        (Some(code), _) => code,
        (None, Some(file)) => {
            let file = path.parent().unwrap_or(Path::new("")).join(file);
            std::fs::read_to_string(&file).map_err(|e| format!("SHADER_FILE {}: {e}", file.display()))?
        }
        (None, None) => return Err("no `const SHADER` (or SHADER_FILE)".into()),
    };
    let ast = engine.compile(styles::PRELUDE).map_err(|e| e.to_string())?.merge(&script).clone_functions_only();
    let mut filter = Filter { id, name, path: path.to_path_buf(), shader, premultiplied, defaults: Map::new(), props: Vec::new(), ast, version };
    if filter.has("defaults") {
        filter.defaults = filter.call(None, "defaults", ())?.try_cast::<Map>().ok_or("defaults() must return a map")?;
    }
    if filter.has("properties") {
        let list = filter.call(None, "properties", ())?.try_cast::<Array>().ok_or("properties() must return an array")?;
        filter.props = list.into_iter().filter_map(styles::prop).collect();
    }
    if !filter.has("tick") {
        return Err("no `fn tick(dt)`".into());
    }
    Ok(filter)
}

impl Filter {
    fn has(&self, name: &str) -> bool {
        self.ast.iter_functions().any(|f| f.name == name)
    }

    fn call(&self, this: Option<&mut Dynamic>, name: &str, args: impl rhai::FuncArgs) -> Result<Dynamic, String> {
        let mut options = CallFnOptions::new().eval_ast(false);
        if let Some(this) = this {
            options = options.bind_this_ptr(this);
        }
        styles::engine()
            .call_fn_with_options::<Dynamic>(options, &mut Scope::new(), &self.ast, name, args)
            .map_err(|e| format!("{}: {name}(): {e}", self.path.display()))
    }

    /// Runs tick(dt) with `this`: the shader's values this frame
    pub fn tick(&self, this: &mut Dynamic, dt: f32) -> Result<Vec<(String, Value)>, String> {
        let out = self.call(Some(this), "tick", (dt as f64,))?;
        let Some(map) = out.try_cast::<Map>() else { return Ok(Vec::new()) };
        Ok(map.into_iter().filter_map(|(k, v)| Some((k.to_string(), styles::to_value(&v)?))).collect())
    }
}

// ---------------------------------------------------------------------------
// The filters found on disk, reloaded when their files change

#[derive(Default)]
struct Registry {
    filters: Vec<Arc<Filter>>,
    seen: HashMap<PathBuf, SystemTime>,
    last_scan: Option<Instant>,
    version: u64,
    /// Files that aren't Puppet filters (sources, docks, Scene-only): said once
    skipped: HashMap<PathBuf, SystemTime>,
}

static REGISTRY: std::sync::LazyLock<Mutex<Registry>> = std::sync::LazyLock::new(|| Mutex::new(Registry::default()));

/// Puppet's filter folders, then Theater Scene's (the same files work in both)
pub fn folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(dev) = std::env::var("THEATER_PUPPET_DIR") {
        out.push(PathBuf::from(dev).join("filters"));
    } else if let Some(data) = crate::data_dir() {
        out.push(data.join("filters"));
    }
    out.push(crate::config_dir().join("filters"));
    if let Some(plugins) = crate::data_dir().and_then(|d| d.parent()?.parent().map(Path::to_path_buf)) {
        out.push(plugins.join("theater-scene").join("data").join("filters"));
    }
    if let Some(config) = crate::config_dir().parent() {
        out.push(config.join("theater-scene").join("filters"));
    }
    out
}

pub fn all() -> Vec<Arc<Filter>> {
    let mut reg = REGISTRY.lock().unwrap();
    if reg.last_scan.is_none_or(|t| t.elapsed().as_secs_f32() >= 1.0) {
        reg.last_scan = Some(Instant::now());
        scan(&mut reg);
    }
    reg.filters.clone()
}

/// Changes whenever a filter is added, removed or reloaded
pub fn generation() -> u64 {
    let _ = all();
    let reg = REGISTRY.lock().unwrap();
    reg.version.wrapping_mul(1_000_003).wrapping_add(reg.filters.len() as u64)
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
    let mut filters: Vec<Arc<Filter>> = Vec::new();
    for (path, mtime) in &files {
        let unchanged = reg.seen.get(path) == Some(mtime);
        if let Some(old) = reg.filters.iter().find(|f| &f.path == path).filter(|_| unchanged) {
            filters.push(old.clone());
            continue;
        }
        if unchanged && reg.skipped.contains_key(path) {
            continue;
        }
        reg.version += 1;
        match compile(path, reg.version) {
            Ok(f) => {
                crate::log(&format!("[Puppet] 🧪 Filter \"{}\" ({}) from {}", f.name, f.id, path.display()));
                reg.skipped.remove(path);
                filters.push(Arc::new(f));
            }
            Err(e) => {
                // Sources and docks are Scene's; a broken edit keeps the last good version
                if reg.skipped.insert(path.clone(), *mtime).is_none() && !e.starts_with("a ") {
                    crate::log(&format!("[Puppet] ⚠️ Filter {}: {e}", path.display()));
                }
                if let Some(old) = reg.filters.iter().find(|f| &f.path == path) {
                    filters.push(old.clone());
                }
            }
        }
    }
    // The first of each id wins (Puppet's before Scene's)
    let mut ids = std::collections::HashSet::new();
    filters.retain(|f| ids.insert(f.id.clone()));
    filters.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    reg.filters = filters;
    reg.seen = files.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toon_filter_compiles_and_ticks() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("filters").join("toon_filter.rhai");
        let f = compile(&path, 1).unwrap();
        assert_eq!(f.id, "theater_toon_filter");
        assert!(f.shader.contains("technique Draw") && f.shader.contains("gbuf"));
        let mut this = Dynamic::from_map(Map::new());
        if let Some(mut m) = this.write_lock::<Map>() {
            m.insert("settings".into(), Dynamic::from_map(f.defaults.clone()));
            m.insert("width".into(), Dynamic::from_int(1080));
            m.insert("height".into(), Dynamic::from_int(1080));
            m.insert("time".into(), Dynamic::from_float(0.0));
        }
        let values = f.tick(&mut this, 1.0 / 60.0).unwrap();
        println!("{values:?}");
        assert!(values.iter().any(|(k, _)| k == "texel"));
    }

    #[test]
    fn the_watercolour_filter_compiles_and_ticks() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("filters").join("watercolour_filter.rhai");
        let f = compile(&path, 1).unwrap();
        assert_eq!(f.id, "theater_watercolour_filter");
        assert!(f.premultiplied && f.shader.contains("PSWatercolour"));
        let mut this = Dynamic::from_map(Map::new());
        if let Some(mut m) = this.write_lock::<Map>() {
            m.insert("settings".into(), Dynamic::from_map(f.defaults.clone()));
            m.insert("width".into(), Dynamic::from_int(1080));
            m.insert("height".into(), Dynamic::from_int(1080));
            m.insert("time".into(), Dynamic::from_float(1.5));
        }
        let values = f.tick(&mut this, 1.0 / 60.0).unwrap();
        for key in ["texel", "wash", "pooling", "pencil_color", "time"] {
            assert!(values.iter().any(|(k, _)| k == key), "no {key}");
        }
    }

    /// Theater Scene's own filter (its repo next to this one) loads here too
    #[test]
    fn a_scene_filter_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("theater scene(rust)").join("filters").join("transparency_fix.rhai");
        if !path.exists() {
            return;
        }
        let f = compile(&path, 1).unwrap();
        assert_eq!(f.id, "theater_transparency_fix_filter");
        assert!(f.premultiplied && !f.props.is_empty());
    }
}
