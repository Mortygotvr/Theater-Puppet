//! 2D in 3D: where the avatar is on screen, for Theater Scene's 2D effects.
//!
//! - Its points: every bone (Mixamo's names, "mixamorig:" left off) and the
//!   palms (`left_palm`, `right_palm`: between the hand and its middle
//!   finger's base), projected with the camera of the picture drawn, each
//!   frame: where on the source (its pixels, y down) and how far from the
//!   camera (metres). A lightning bolt from the palm, a pie at the face.
//! - Its depth: the avatar's distance from the camera at every pixel of the
//!   source (metres, as the points; far away where it isn't), a GPU texture
//!   (one float a pixel) drawn each frame while asked for (in the last
//!   second): a 2D effect is hidden where the avatar is nearer than it (a
//!   finger in front of the bolt, the body behind it).
//!
//! Asked for through OBS's procedures, beside the parts map's (parts.rs):
//!
//! - `theater_puppet_points(in string source, out string json)`:
//!   `{"source","width","height","points":{"left_palm":[x,y,depth],…}}`
//!   ("{}" when there's none yet);
//! - `theater_puppet_depth(in string source, out string json)`:
//!   `{"source","width","height","texture":<gs_texture_t*, as a number>}`
//!   (texture 0 until it's drawn: the first ask starts it). The texture is
//!   Puppet's: used on the graphics thread, the frame it's asked for.
//!
//! `source` "" = the first Puppet.

use std::ffi::CStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::Vec3;

use crate::obs::{self, Ptr};
use crate::parts::{Calldata, get_string, set_string};

/// How long the depth keeps being drawn after it was last asked for
const WANTED_FOR: Duration = Duration::from_secs(1);

/// One source's points and depth
#[derive(Default)]
pub struct Space {
    pub width: u32,
    pub height: u32,
    /// Each point: its name, where on the source (pixels) and how far (metres)
    pub points: Vec<(String, [f32; 3])>,
    /// The depth texture (gs_texture_t*), 0 none
    pub depth: usize,
    /// When the depth was last asked for
    pub wanted: Option<Instant>,
}

impl Space {
    pub fn depth_wanted(&self) -> bool {
        self.wanted.is_some_and(|t| t.elapsed() < WANTED_FOR)
    }

    fn points_json(&self, source: &str) -> String {
        let points: serde_json::Map<String, serde_json::Value> = self.points.iter().map(|(n, p)| (n.clone(), serde_json::json!([p[0], p[1], p[2]]))).collect();
        serde_json::json!({ "source": source, "width": self.width, "height": self.height, "points": points }).to_string()
    }
}

pub type Shared = Arc<Mutex<Space>>;

static SOURCES: Mutex<Vec<(usize, Shared)>> = Mutex::new(Vec::new());

pub fn add_source(source: Ptr, space: Shared) {
    SOURCES.lock().unwrap().push((source as usize, space));
}

pub fn remove_source(source: Ptr) {
    SOURCES.lock().unwrap().retain(|(s, _)| *s != source as usize);
}

fn name_of(s: usize) -> String {
    unsafe {
        let p = (obs::api().obs_source_get_name)(s as Ptr);
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

fn find(name: &str) -> Option<(String, Shared)> {
    let sources = SOURCES.lock().unwrap().clone();
    if name.is_empty() {
        return sources.first().map(|(s, m)| (name_of(*s), m.clone()));
    }
    sources.into_iter().find(|(s, _)| name_of(*s) == name).map(|(s, m)| (name_of(s), m))
}

/// The points to project: every bone of the avatar where it is now, and the palms
pub fn bone_points(model: &crate::model::Model, pose: &crate::pose::Pose) -> Vec<(String, Vec3)> {
    let joints: std::collections::HashSet<usize> = model.skins.iter().flat_map(|s| s.joints.iter().copied()).collect();
    let mut out: Vec<(String, Vec3)> = model
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| joints.contains(i) && *i < pose.world.len())
        .map(|(i, n)| (n.name.strip_prefix("mixamorig:").unwrap_or(&n.name).to_string(), pose.position(i)))
        .collect();
    for (side, name) in [("Left", "left_palm"), ("Right", "right_palm")] {
        let at = |bone: &str| model.node(bone).or_else(|| model.node(&format!("mixamorig:{bone}"))).filter(|i| *i < pose.world.len()).map(|i| pose.position(i));
        if let Some(hand) = at(&format!("{side}Hand")) {
            let palm = at(&format!("{side}HandMiddle1")).map_or(hand, |m| (hand + m) * 0.5);
            out.push((name.to_string(), palm));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// OBS's procedure handler

const POINTS_DECL: &CStr = c"void theater_puppet_points(in string source, out string json)";
const DEPTH_DECL: &CStr = c"void theater_puppet_depth(in string source, out string json)";

/// Registers the procedures. Call from obs_module_load.
pub fn register_procs() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if handler.is_null() {
            return;
        }
        (api.proc_handler_add)(handler, POINTS_DECL.as_ptr(), points_proc, std::ptr::null_mut());
        (api.proc_handler_add)(handler, DEPTH_DECL.as_ptr(), depth_proc, std::ptr::null_mut());
    }
    crate::log("[Puppet] 🔗 Registered theater_puppet_points / theater_puppet_depth (2D in 3D)");
}

unsafe extern "C" fn points_proc(_data: Ptr, cd: *mut Calldata) {
    let json = std::panic::catch_unwind(|| {
        let (name, space) = find(&get_string(cd, c"source"))?;
        let space = space.lock().unwrap();
        (!space.points.is_empty()).then(|| space.points_json(&name))
    })
    .ok()
    .flatten()
    .unwrap_or_else(|| "{}".into());
    set_string(cd, c"json", &json);
}

unsafe extern "C" fn depth_proc(_data: Ptr, cd: *mut Calldata) {
    let json = std::panic::catch_unwind(|| {
        let (name, space) = find(&get_string(cd, c"source"))?;
        let mut space = space.lock().unwrap();
        // Asked for: drawn from now on, for a second after the last ask
        space.wanted = Some(Instant::now());
        Some(serde_json::json!({ "source": name, "width": space.width, "height": space.height, "texture": space.depth }).to_string())
    })
    .ok()
    .flatten()
    .unwrap_or_else(|| "{}".into());
    set_string(cd, c"json", &json);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THEATER_PUPPET_GLB=<an avatar>: its bones and both palms, each palm
    /// between its hand and middle finger
    #[test]
    fn an_avatars_points_have_its_bones_and_palms() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let model = crate::model::load(std::path::Path::new(&path)).unwrap();
        let pose = crate::pose::Pose::rest(&model);
        let pts = bone_points(&model, &pose);
        let get = |n: &str| pts.iter().find(|(name, _)| name == n).map(|(_, p)| *p);
        let (Some(hand), Some(palm)) = (get("LeftHand"), get("left_palm")) else { panic!("no LeftHand / left_palm in {:?}", pts.iter().map(|p| &p.0).collect::<Vec<_>>()) };
        assert!(get("Head").is_some() && get("right_palm").is_some());
        let d = (palm - hand).length();
        println!("{} points; left palm {:.3} m from the wrist", pts.len(), d);
        assert!(d > 0.01 && d < 0.12, "the palm {d} m from the wrist");
    }
}
