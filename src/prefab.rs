//! GLB prefabs: a GLB a redeem spawns into a Puppet source, like a prefab in
//! Unity. It plays one of its animations once (baked in Blender: no
//! physics), where the redeem puts it, then fades out and goes. Several can
//! be on screen at once, each its own copy; the GLB is read once and kept.
//!
//! Its contact frames (Blender's "contact" markers, the animation's
//! `theater_contacts`) are handed back when it spawns, as seconds from then:
//! Theater Scene runs the redeem's contact effects at those times.
//!
//! Spawned through the `theater_puppet_command` procedure (remote.rs,
//! action "prefab"); drawn after the avatar (render.rs's prefab meshes).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use glam::{Mat4, Quat, Vec3};
use serde_json::{Value, json};

use crate::model::Model;
use crate::pose::{self, Pose};

/// Seconds it takes to fade out at its end
pub const FADE: f32 = 0.3;
/// Seconds one with no animation stays (unless the redeem says)
pub const HOLD: f32 = 3.0;
/// At most this many on screen in one source (the oldest go first)
pub const MAX: usize = 24;

/// What a redeem asks for
#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub glb: PathBuf,
    /// Its animation: "" its first, "random" any, else its name
    pub clip: String,
    /// Where, from the avatar's feet (metres: its right as seen, up, toward the camera)
    pub position: Vec3,
    pub scale: f32,
    /// Turned about up, degrees
    pub turn: f32,
    /// Seconds it stays when it has no animation
    pub hold: f32,
}

impl Spawn {
    /// From the procedure's arguments
    pub fn from_args(args: &Value) -> Result<Spawn, String> {
        let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let num = |k: &str, d: f32| args.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).map(|v| v as f32).filter(|v| v.is_finite()).unwrap_or(d);
        let glb = text("glb");
        if glb.is_empty() {
            return Err("no GLB given".into());
        }
        let position = match args.get("position").and_then(|v| v.as_array()) {
            Some(a) => Vec3::from_array(std::array::from_fn(|i| a.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32)),
            None => Vec3::new(num("x", 0.0), num("y", 0.0), num("z", 0.0)),
        };
        Ok(Spawn { glb: PathBuf::from(glb), clip: text("clip"), position, scale: num("scale", 1.0).clamp(0.01, 100.0), turn: num("turn", 0.0), hold: num("hold", HOLD).clamp(0.1, 600.0) })
    }
}

/// The GLBs read so far (path, when it was written, the model): read again
/// when the file changes
static MODELS: Mutex<Vec<(PathBuf, Option<SystemTime>, Arc<Model>)>> = Mutex::new(Vec::new());
const CACHED: usize = 16;

/// The GLB at `path`, read once (again when the file changes)
pub fn model(path: &Path) -> Result<Arc<Model>, String> {
    let written = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut cache = MODELS.lock().unwrap();
    if let Some(i) = cache.iter().position(|(p, w, _)| p == path && *w == written) {
        let hit = cache.remove(i);
        let m = hit.2.clone();
        cache.push(hit);
        return Ok(m);
    }
    let m = Arc::new(crate::model::load(path).map_err(|e| format!("{}: {e}", path.display()))?);
    cache.retain(|(p, _, _)| p != path);
    cache.push((path.to_path_buf(), written, m.clone()));
    while cache.len() > CACHED {
        cache.remove(0);
    }
    Ok(m)
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A spawned GLB on screen
pub struct Prefab {
    pub id: u64,
    /// Its GLB's path: its GPU copy is shared by every one of it
    pub key: String,
    pub model: Arc<Model>,
    clip: Option<usize>,
    time: f32,
    /// When its animation (or its hold) ends: then it fades out
    end: f32,
    place: Mat4,
    turn: Quat,
    pose: Pose,
    morph: Vec<f32>,
    pub points: Vec<Vec<[f32; 4]>>,
    pub normals: Vec<Vec<[f32; 4]>>,
    /// How much shows (1 until it ends, then down to 0)
    pub fade: f32,
    pub dirty: bool,
}

/// A pseudo-random index below `n` (a random animation)
fn pick(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(NEXT.load(Ordering::Relaxed));
    (h.finish() % n.max(1) as u64) as usize
}

impl Prefab {
    /// `spawn` ready to show, and what Scene needs: its animation, how long
    /// it lasts, its contacts (seconds from now)
    pub fn new(spawn: &Spawn) -> Result<(Prefab, Value), String> {
        let model = model(&spawn.glb)?;
        let names: Vec<&str> = model.clips.iter().map(|c| c.name.as_str()).collect();
        let clip = match spawn.clip.as_str() {
            "" => (!names.is_empty()).then_some(0),
            "random" => (!names.is_empty()).then(|| pick(names.len())),
            name => Some(names.iter().position(|n| *n == name).ok_or_else(|| format!("{} has no animation \"{name}\" (it has: {})", spawn.glb.display(), if names.is_empty() { "none".to_string() } else { names.join(", ") }))?),
        };
        let (end, contacts) = match clip {
            Some(c) => (model.clips[c].duration, model.clips[c].contacts.iter().copied().filter(|t| *t <= model.clips[c].duration + 1e-4).collect::<Vec<_>>()),
            None => (spawn.hold, Vec::new()),
        };
        let turn = Quat::from_rotation_y(spawn.turn.to_radians());
        let place = Mat4::from_scale_rotation_translation(Vec3::splat(spawn.scale), turn, spawn.position);
        let pose = Pose::rest(&model);
        let mut p = Prefab {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            key: spawn.glb.to_string_lossy().into_owned(),
            clip,
            time: 0.0,
            end,
            place,
            turn,
            morph: vec![0.0; model.morph_names.len()],
            points: model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect(),
            normals: model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect(),
            pose,
            fade: 1.0,
            dirty: false,
            model,
        };
        p.pose_now();
        let plan = json!({
            "id": p.id,
            "clip": clip.map(|c| p.model.clips[c].name.clone()),
            "duration": end,
            "gone_after": end + FADE,
            "contacts": contacts,
        });
        Ok((p, plan))
    }

    /// `dt` seconds on; false once it's gone
    pub fn step(&mut self, dt: f32) -> bool {
        self.time += dt;
        if self.time >= self.end {
            self.fade = (self.fade - dt / FADE).max(0.0);
        }
        // (Held on its last frame while it fades)
        self.pose_now();
        self.fade > 0.0
    }

    /// Its pose and shape keys now, skinned, where the redeem puts it
    fn pose_now(&mut self) {
        let model = self.model.clone();
        self.pose.reset(&model);
        self.morph.iter_mut().for_each(|w| *w = 0.0);
        if let Some(c) = self.clip {
            let t = self.time.min(model.clips[c].duration);
            model.clips[c].apply(&mut self.pose, t, 1.0);
            self.pose.update_world(&model);
            model.clips[c].apply_shapes(&mut self.morph, t, 1.0);
        } else {
            self.pose.update_world(&model);
        }
        pose::skin(&model, &self.pose, &self.morph, &mut self.points, &mut self.normals);
        for (pts, nrm) in self.points.iter_mut().zip(self.normals.iter_mut()) {
            for p in pts.iter_mut() {
                let v = self.place.transform_point3(Vec3::new(p[0], p[1], p[2]));
                *p = [v.x, v.y, v.z, p[3]];
            }
            for n in nrm.iter_mut() {
                let v = self.turn * Vec3::new(n[0], n[1], n[2]);
                *n = [v.x, v.y, v.z, n[3]];
            }
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_reads_its_settings() {
        let s = Spawn::from_args(&json!({ "glb": " C:/x/pie.glb ", "clip": "Throw", "position": [0.5, 1.2, 0.3], "scale": "2", "turn": 90 })).unwrap();
        assert_eq!(s.glb, PathBuf::from("C:/x/pie.glb"));
        assert_eq!((s.clip.as_str(), s.position, s.scale, s.turn, s.hold), ("Throw", Vec3::new(0.5, 1.2, 0.3), 2.0, 90.0, HOLD));
        let s = Spawn::from_args(&json!({ "glb": "a.glb", "x": 1, "y": 2 })).unwrap();
        assert_eq!((s.position, s.scale, s.clip.as_str()), (Vec3::new(1.0, 2.0, 0.0), 1.0, ""));
        assert!(Spawn::from_args(&json!({ "clip": "x" })).is_err(), "no GLB");
    }

    /// THEATER_PUPPET_CONTACTS_GLB: theater-rig's tests/run_contacts.py's GLB
    /// ("Pie Throw", frames 1–30 at 24 fps, contacts at frames 10 and 25)
    #[test]
    fn a_prefab_hands_back_its_contact_times() {
        let Ok(path) = std::env::var("THEATER_PUPPET_CONTACTS_GLB") else { return };
        let spawn = Spawn { glb: PathBuf::from(&path), clip: "Pie Throw".into(), position: Vec3::ZERO, scale: 1.0, turn: 0.0, hold: HOLD };
        let (_, plan) = Prefab::new(&spawn).unwrap();
        println!("plan: {plan}");
        let contacts: Vec<f64> = plan["contacts"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(contacts.len(), 2);
        assert!((contacts[0] - 9.0 / 24.0).abs() < 1e-4 && (contacts[1] - 1.0).abs() < 1e-4, "{contacts:?}");
        assert!((plan["duration"].as_f64().unwrap() - 29.0 / 24.0).abs() < 1e-3);
    }

    /// THEATER_PUPPET_FLIPBOOK_GLB: a GLB with an animation "Mouths" (3 frames)
    #[test]
    fn a_prefab_plays_once_then_fades_out() {
        let Ok(path) = std::env::var("THEATER_PUPPET_FLIPBOOK_GLB") else { return };
        let spawn = Spawn { glb: PathBuf::from(&path), clip: "Mouths".into(), position: Vec3::new(1.0, 0.0, 0.0), scale: 2.0, turn: 0.0, hold: HOLD };
        let (mut p, plan) = Prefab::new(&spawn).unwrap();
        assert_eq!(plan["clip"], "Mouths");
        let duration = plan["duration"].as_f64().unwrap() as f32;
        assert!((duration - 3.0 / 24.0).abs() < 1e-3, "{plan}");
        // Placed: moved 1 m and twice the size (the cube is 2 m: now 4, centred at x 1)
        let xs: Vec<f32> = p.points.iter().flatten().map(|v| v[0]).collect();
        let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!((lo + 1.0).abs() < 0.05 && (hi - 3.0).abs() < 0.6, "x from {lo} to {hi}");
        // Plays, then fades over FADE, then it's gone
        let mut t = 0.0;
        while p.step(1.0 / 60.0) {
            t += 1.0 / 60.0;
            assert!(t < duration + FADE + 0.1, "still there after {t} s");
        }
        assert!(t >= duration + FADE - 0.05, "gone after {t} s");
        // Its animation by name only; a wrong one says what it has
        let err = Prefab::new(&Spawn { clip: "Nope".into(), ..spawn.clone() }).err().unwrap();
        assert!(err.contains("Mouths"), "{err}");
        // The GLB read once
        let again = model(Path::new(&path)).unwrap();
        assert!(Arc::ptr_eq(&again, &p.model));
    }
}
