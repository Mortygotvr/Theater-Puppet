//! The detail hitbox for Theater Scene: which body part each pixel of a
//! Puppet source is (head, torso, arms, hands, legs, feet; the avatar's own
//! left and right), from its Mixamo bones (model.rs). Redrawn ten times a
//! second, small (render.rs), and offered on OBS's procedure handler:
//!
//! - `theater_puppet_parts(in string source, out string json)`: every
//!   part's box, centre and size in the source's pixels:
//!   `{"source","width","height","parts":{"head":{"x","y","w","h","cx","cy","pixels"},…}}`
//!   (`source` "" = the first Puppet; "{}" when there's none);
//! - `theater_puppet_part_at(in string source, in float x, in float y, out string part)`:
//!   the part at that point of the source (its pixels), "" for none.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Arc, Mutex};

use crate::model::PARTS;
use crate::obs::{self, Ptr};
use crate::render::{PARTS_SCALE, PartPixels};

/// A source's latest parts map, in its own pixels
pub struct PartMap {
    pub pixels: PartPixels,
    /// The source's size
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub cx: f32,
    pub cy: f32,
    pub pixels: u32,
}

impl PartMap {
    /// The part at (x, y), the source's pixels
    pub fn part_at(&self, x: f32, y: f32) -> Option<&'static str> {
        let p = &self.pixels;
        let mx = (x / self.width as f32 * p.width as f32).floor();
        let my = (y / self.height as f32 * p.height as f32).floor();
        if mx < 0.0 || my < 0.0 || mx >= p.width as f32 || my >= p.height as f32 {
            return None;
        }
        let id = p.ids[my as usize * p.width as usize + mx as usize];
        (id > 0).then(|| PARTS.get(id as usize - 1).copied()).flatten()
    }

    /// Each part's box, centre and size, the source's pixels
    pub fn boxes(&self) -> Vec<(&'static str, PartBox)> {
        let p = &self.pixels;
        let (sx, sy) = (self.width as f32 / p.width as f32, self.height as f32 / p.height as f32);
        // min x, min y, max x, max y, sum x, sum y, count
        let mut acc = vec![(u32::MAX, u32::MAX, 0u32, 0u32, 0u64, 0u64, 0u32); PARTS.len()];
        for y in 0..p.height {
            for x in 0..p.width {
                let id = p.ids[(y * p.width + x) as usize];
                if id == 0 || id as usize > PARTS.len() {
                    continue;
                }
                let a = &mut acc[id as usize - 1];
                a.0 = a.0.min(x);
                a.1 = a.1.min(y);
                a.2 = a.2.max(x);
                a.3 = a.3.max(y);
                a.4 += x as u64;
                a.5 += y as u64;
                a.6 += 1;
            }
        }
        acc.iter()
            .enumerate()
            .filter(|(_, a)| a.6 > 0)
            .map(|(i, a)| {
                let n = a.6 as f32;
                (
                    PARTS[i],
                    PartBox {
                        x: a.0 as f32 * sx,
                        y: a.1 as f32 * sy,
                        w: (a.2 - a.0 + 1) as f32 * sx,
                        h: (a.3 - a.1 + 1) as f32 * sy,
                        cx: (a.4 as f32 / n + 0.5) * sx,
                        cy: (a.5 as f32 / n + 0.5) * sy,
                        // In the source's pixels
                        pixels: a.6 * PARTS_SCALE * PARTS_SCALE,
                    },
                )
            })
            .collect()
    }

    pub fn json(&self, source: &str) -> String {
        let parts: serde_json::Map<String, serde_json::Value> = self
            .boxes()
            .into_iter()
            .map(|(name, b)| {
                (name.to_string(), serde_json::json!({ "x": b.x, "y": b.y, "w": b.w, "h": b.h, "cx": b.cx, "cy": b.cy, "pixels": b.pixels }))
            })
            .collect();
        serde_json::json!({ "source": source, "width": self.width, "height": self.height, "parts": parts }).to_string()
    }
}

pub type Shared = Arc<Mutex<Option<PartMap>>>;

/// Every Puppet source (OBS's pointer) and its parts map
static SOURCES: Mutex<Vec<(usize, Shared)>> = Mutex::new(Vec::new());

pub fn add_source(source: Ptr, map: Shared) {
    SOURCES.lock().unwrap().push((source as usize, map));
}

pub fn remove_source(source: Ptr) {
    SOURCES.lock().unwrap().retain(|(s, _)| *s != source as usize);
}

/// The Puppet source called `name` ("" = the first), and its OBS name
fn find(name: &str) -> Option<(String, Shared)> {
    let api = obs::api();
    let sources = SOURCES.lock().unwrap().clone();
    let name_of = |s: usize| unsafe {
        let p = (api.obs_source_get_name)(s as Ptr);
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    if name.is_empty() {
        return sources.first().map(|(s, m)| (name_of(*s), m.clone()));
    }
    sources.into_iter().find(|(s, _)| name_of(*s) == name).map(|(s, m)| (name_of(s), m))
}

// ---------------------------------------------------------------------------
// OBS's procedure handler

/// libobs's calldata_t
#[repr(C)]
pub struct Calldata {
    stack: *mut u8,
    size: usize,
    capacity: usize,
    fixed: bool,
}

/// A procedure as proc_handler_add takes it
pub type ProcFn = unsafe extern "C" fn(Ptr, *mut Calldata);

const PARTS_DECL: &CStr = c"void theater_puppet_parts(in string source, out string json)";
const PART_AT_DECL: &CStr = c"void theater_puppet_part_at(in string source, in float x, in float y, out string part)";

/// Registers the procedures. Call from obs_module_load.
pub fn register_procs() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if handler.is_null() {
            crate::log("[Puppet] ⚠️ No procedure handler: Theater Scene can't ask for body parts");
            return;
        }
        (api.proc_handler_add)(handler, PARTS_DECL.as_ptr(), parts_proc, std::ptr::null_mut());
        (api.proc_handler_add)(handler, PART_AT_DECL.as_ptr(), part_at_proc, std::ptr::null_mut());
    }
    crate::log("[Puppet] 🔗 Registered theater_puppet_parts / theater_puppet_part_at");
}

pub(crate) fn get_string(cd: *mut Calldata, name: &CStr) -> String {
    let mut s: *const c_char = std::ptr::null();
    if unsafe { (obs::api().calldata_get_string)(cd as Ptr, name.as_ptr(), &mut s) } && !s.is_null() {
        unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

/// A `float` argument (libobs keeps them as double)
fn get_float(cd: *mut Calldata, name: &CStr) -> f64 {
    let mut v = 0f64;
    unsafe { (obs::api().calldata_get_data)(cd as Ptr, name.as_ptr(), &mut v as *mut f64 as *mut c_void, size_of::<f64>()) };
    v
}

pub(crate) fn set_string(cd: *mut Calldata, name: &CStr, value: &str) {
    let value = CString::new(value.replace('\0', "")).unwrap_or_default();
    let bytes = value.as_bytes_with_nul();
    unsafe { (obs::api().calldata_set_data)(cd as Ptr, name.as_ptr(), bytes.as_ptr() as *const c_void, bytes.len()) };
}

unsafe extern "C" fn parts_proc(_data: Ptr, cd: *mut Calldata) {
    let json = std::panic::catch_unwind(|| {
        let (name, map) = find(&get_string(cd, c"source"))?;
        let map = map.lock().unwrap();
        map.as_ref().map(|m| m.json(&name))
    })
    .ok()
    .flatten()
    .unwrap_or_else(|| "{}".into());
    set_string(cd, c"json", &json);
}

unsafe extern "C" fn part_at_proc(_data: Ptr, cd: *mut Calldata) {
    let part = std::panic::catch_unwind(|| {
        let (_, map) = find(&get_string(cd, c"source"))?;
        let (x, y) = (get_float(cd, c"x") as f32, get_float(cd, c"y") as f32);
        let map = map.lock().unwrap();
        map.as_ref()?.part_at(x, y)
    })
    .ok()
    .flatten()
    .unwrap_or("");
    set_string(cd, c"part", part);
}

/// The parts map coloured, for checking it (snapshot.rs): parts.png
pub fn save_png(map: &PartMap) {
    const COLOURS: [[u8; 3]; 10] = [
        [200, 200, 200], [240, 80, 80], [80, 160, 240], [80, 220, 120], [40, 90, 200], [30, 150, 70], [230, 180, 60], [200, 120, 220], [150, 100, 30], [120, 40, 140],
    ];
    let p = &map.pixels;
    let mut rgba = Vec::with_capacity((p.width * p.height * 4) as usize);
    for &id in &p.ids {
        match COLOURS.get((id as usize).wrapping_sub(1)) {
            Some(c) if id > 0 => rgba.extend_from_slice(&[c[0], c[1], c[2], 255]),
            _ => rgba.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    let path = crate::config_dir().join("parts.png");
    match image::save_buffer(&path, &rgba, p.width, p.height, image::ExtendedColorType::Rgba8) {
        Ok(()) => crate::log(&format!("[Puppet] 📸 Parts map saved: {}  {}", path.display(), map.json(""))),
        Err(e) => crate::log(&format!("[Puppet] ⚠️ Parts map: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_and_points_are_in_the_source_pixels() {
        // A 4x2 map of an 16x8 source: head top left, torso below it
        let ids = vec![2, 2, 0, 0, 1, 1, 1, 0];
        let map = PartMap { pixels: PartPixels { width: 4, height: 2, ids }, width: 16, height: 8 };
        assert_eq!(map.part_at(1.0, 1.0), Some("head"));
        assert_eq!(map.part_at(10.0, 5.0), Some("torso"));
        assert_eq!(map.part_at(14.0, 1.0), None);
        assert_eq!(map.part_at(-1.0, 1.0), None);
        let boxes = map.boxes();
        let head = boxes.iter().find(|(n, _)| *n == "head").unwrap().1;
        assert_eq!((head.x, head.y, head.w, head.h), (0.0, 0.0, 8.0, 4.0));
        assert_eq!((head.cx, head.cy), (4.0, 2.0));
        let torso = boxes.iter().find(|(n, _)| *n == "torso").unwrap().1;
        assert_eq!((torso.x, torso.w, torso.cx), (0.0, 12.0, 6.0));
        assert!(map.json("Puppet").contains("\"head\""));
    }
}
