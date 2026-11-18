//! The avatar's own animations: clips made in Blender on the avatar's own
//! skeleton and exported in its GLB. Puppet plays them as they are, with no
//! retargeting: an animation made for another skeleton (Mixamo's…) is put on
//! the avatar in Blender first.
//!
//! Two layers: the source's looping clip (its "Animation" setting) in place
//! of the idle motion, and a clip played once over it (the dock, a redeem),
//! after which it goes back. Each fades in and out. What's tracked still
//! goes over both, part by part.
//!
//! Offered on OBS's procedure handler:
//! `theater_puppet_animation(in string args, out string json)`, `args` being JSON:
//! - `{"action": "get"}` (or none);
//! - `{"action": "play", "source", "clip"}`: once ("" source = every Puppet);
//! - `{"action": "stop", "source"}`: the clip played once stops;
//! - `{"action": "loop", "source", "clip"}`: the looping clip ("" = none, the
//!   idle motion), saved in the source's settings.
//!
//! It answers `{"puppets": [{"source", "clips": [names], "looping", "playing"}], "done"}`.

use std::ffi::CStr;
use std::sync::{Arc, Mutex};

use glam::{Quat, Vec3};
use serde_json::{Value, json};

use crate::model::Model;
use crate::obs::{self, Ptr};
use crate::parts::{Calldata, get_string, set_string};
use crate::pose::Pose;

/// Seconds a clip takes to fade in, and out
const FADE: f32 = 0.3;

pub enum Values {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
    /// A mesh's shape keys: each of its targets' key (Model::morph_names'
    /// place), and every key's weights one after another
    Weights { targets: Vec<Option<usize>>, values: Vec<f32> },
}

/// One node's translation, rotation, scale or shape keys over time
pub struct Channel {
    pub node: usize,
    pub times: Vec<f32>,
    pub values: Values,
    /// Held until the next key (else linear)
    pub step: bool,
}

pub struct Clip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}

/// The file's clips: bones, and shape keys (by name, as `morph_names` has them)
pub fn load(doc: &gltf::Document, buffers: &[gltf::buffer::Data], morph_names: &[String]) -> Vec<Clip> {
    use gltf::animation::{Interpolation, util::ReadOutputs};
    let mut clips = Vec::new();
    for (i, anim) in doc.animations().enumerate() {
        let mut channels = Vec::new();
        for channel in anim.channels() {
            let reader = channel.reader(|b| Some(&buffers[b.index()]));
            let Some(times) = reader.read_inputs().map(|t| t.collect::<Vec<f32>>()) else { continue };
            let interpolation = channel.sampler().interpolation();
            // A cubic spline has three outputs a key (in-tangent, value,
            // out-tangent): the values, linearly between them, are close enough
            let cubic = interpolation == Interpolation::CubicSpline;
            fn keep<T>(v: Vec<T>, cubic: bool) -> Vec<T> {
                if cubic { v.into_iter().skip(1).step_by(3).collect() } else { v }
            }
            let values = match reader.read_outputs() {
                Some(ReadOutputs::Translations(it)) => Values::Translation(keep(it.map(Vec3::from).collect(), cubic)),
                Some(ReadOutputs::Rotations(it)) => Values::Rotation(keep(it.into_f32().map(|q| Quat::from_array(q).normalize()).collect(), cubic)),
                Some(ReadOutputs::Scales(it)) => Values::Scale(keep(it.map(Vec3::from).collect(), cubic)),
                Some(ReadOutputs::MorphTargetWeights(it)) => {
                    // The mesh's targets, named by its extras.targetNames (Blender's exporter)
                    let Some(mesh) = channel.target().node().mesh() else { continue };
                    let count = mesh.primitives().next().map(|p| p.morph_targets().count()).unwrap_or(0);
                    let names: Vec<String> = mesh
                        .extras()
                        .as_ref()
                        .and_then(|e| serde_json::from_str::<Value>(e.get()).ok())
                        .and_then(|v| Some(v.get("targetNames")?.as_array()?.iter().map(|n| n.as_str().unwrap_or("").to_string()).collect()))
                        .unwrap_or_default();
                    let targets: Vec<Option<usize>> = (0..count)
                        .map(|t| {
                            let name = names.get(t).cloned().unwrap_or_else(|| format!("target_{t}"));
                            morph_names.iter().position(|n| *n == name)
                        })
                        .collect();
                    let all: Vec<f32> = it.into_f32().collect();
                    if count == 0 {
                        continue;
                    }
                    // A cubic spline's keys: in-tangents, values, out-tangents (the middle ones)
                    let values = if cubic { all.chunks(count).skip(1).step_by(3).flatten().copied().collect() } else { all };
                    Values::Weights { targets, values }
                }
                _ => continue,
            };
            let len = match &values {
                Values::Translation(v) | Values::Scale(v) => v.len(),
                Values::Rotation(v) => v.len(),
                Values::Weights { targets, values } => values.len() / targets.len().max(1),
            };
            if times.is_empty() || len != times.len() {
                continue;
            }
            channels.push(Channel { node: channel.target().node().index(), times, values, step: interpolation == Interpolation::Step });
        }
        if channels.is_empty() {
            continue;
        }
        let duration = channels.iter().filter_map(|c| c.times.last().copied()).fold(0.0, f32::max);
        let name = anim.name().map(str::to_string).unwrap_or_else(|| format!("Animation {}", i + 1));
        clips.push(Clip { name, duration, channels });
    }
    clips
}

/// The keys around `time`, and how far between them (0..1)
fn keys(times: &[f32], time: f32, step: bool) -> (usize, usize, f32) {
    let after = times.partition_point(|&t| t <= time);
    if after == 0 {
        return (0, 0, 0.0);
    }
    if after >= times.len() {
        let last = times.len() - 1;
        return (last, last, 0.0);
    }
    let (a, b) = (after - 1, after);
    let span = times[b] - times[a];
    let f = if step || span <= 0.0 { 0.0 } else { (time - times[a]) / span };
    (a, b, f)
}

impl Clip {
    /// The clip at `time` over `pose`'s local transforms, `weight` of the way
    pub fn apply(&self, pose: &mut Pose, time: f32, weight: f32) {
        if weight <= 0.0 {
            return;
        }
        for c in &self.channels {
            let n = c.node;
            if n >= pose.rotation.len() {
                continue;
            }
            let (a, b, f) = keys(&c.times, time, c.step);
            match &c.values {
                Values::Translation(v) => pose.translation[n] = pose.translation[n].lerp(v[a].lerp(v[b], f), weight),
                Values::Rotation(v) => pose.rotation[n] = pose.rotation[n].slerp(v[a].slerp(v[b], f), weight).normalize(),
                Values::Scale(v) => pose.scale[n] = pose.scale[n].lerp(v[a].lerp(v[b], f), weight),
                Values::Weights { .. } => {}
            }
        }
    }

    /// The clip's shape keys at `time`, `weight` of them, over `morph` (the
    /// stronger wins: a tracked face or a blink isn't undone by a key at 0)
    pub fn apply_shapes(&self, morph: &mut [f32], time: f32, weight: f32) {
        if weight <= 0.0 {
            return;
        }
        for c in &self.channels {
            let Values::Weights { targets, values } = &c.values else { continue };
            let (a, b, f) = keys(&c.times, time, c.step);
            let count = targets.len();
            for (t, m) in targets.iter().enumerate() {
                let Some(m) = *m else { continue };
                let (va, vb) = (values[a * count + t], values[b * count + t]);
                if let Some(w) = morph.get_mut(m) {
                    *w = w.max((va + (vb - va) * f) * weight);
                }
            }
        }
    }
}

/// A clip playing: which, how far in, how much of it shows
#[derive(Clone, Copy, Debug)]
struct Layer {
    clip: usize,
    time: f32,
    weight: f32,
    /// Fading out (the once clip ended or was stopped; the loop changed)
    leaving: bool,
}

impl Layer {
    fn new(clip: usize) -> Layer {
        Layer { clip, time: 0.0, weight: 0.0, leaving: false }
    }

    fn fade(&mut self, dt: f32) {
        let d = dt / FADE;
        self.weight = if self.leaving { (self.weight - d).max(0.0) } else { (self.weight + d).min(1.0) };
    }
}

/// What the source, the procedure and the dock ask of an avatar's player,
/// and what it says back
#[derive(Default)]
pub struct Control {
    /// The looping clip's name ("" = the idle motion): the source's setting
    pub loop_clip: String,
    /// A clip to play once, taken by the player
    pub play: Option<String>,
    pub stop: bool,
    /// The avatar's clips
    pub clips: Vec<String>,
    /// The clip playing once now, "" for none
    pub playing: String,
}

pub type Shared = Arc<Mutex<Control>>;

#[derive(Default)]
pub struct Player {
    /// The looping clip, and the one it replaced fading out
    base: Option<Layer>,
    old_base: Option<Layer>,
    once: Option<Layer>,
}

impl Player {
    /// Takes `control`'s requests, tells it what's playing
    pub fn sync(&mut self, model: &Model, control: &mut Control) {
        let find = |name: &str| model.clips.iter().position(|c| c.name == name);
        if control.clips.iter().map(String::as_str).ne(model.clips.iter().map(|c| c.name.as_str())) {
            control.clips = model.clips.iter().map(|c| c.name.clone()).collect();
        }
        let wanted = find(&control.loop_clip);
        if self.base.map(|b| b.clip) != wanted {
            if let Some(mut old) = self.base.take() {
                old.leaving = true;
                self.old_base = Some(old);
            }
            self.base = wanted.map(Layer::new);
        }
        if let Some(clip) = control.play.take().and_then(|n| find(&n)) {
            // Again from its start (a clip already showing doesn't fade in again)
            let weight = self.once.filter(|o| o.clip == clip).map(|o| o.weight).unwrap_or(0.0);
            self.once = Some(Layer { weight, ..Layer::new(clip) });
        }
        if std::mem::take(&mut control.stop) {
            if let Some(o) = self.once.as_mut() {
                o.leaving = true;
            }
        }
        control.playing = self.once.filter(|o| !o.leaving).map(|o| model.clips[o.clip].name.clone()).unwrap_or_default();
    }

    /// `dt` seconds later
    pub fn step(&mut self, model: &Model, dt: f32) {
        for layer in [&mut self.base, &mut self.old_base] {
            if let Some(l) = layer.as_mut() {
                let d = model.clips[l.clip].duration;
                l.time = if d > 0.0 { (l.time + dt) % d } else { 0.0 };
                l.fade(dt);
            }
        }
        if self.old_base.is_some_and(|l| l.weight <= 0.0) {
            self.old_base = None;
        }
        if let Some(o) = self.once.as_mut() {
            let d = model.clips[o.clip].duration;
            o.time = (o.time + dt).min(d);
            // Fading out as it ends
            if o.time >= d - FADE {
                o.leaving = true;
            }
            o.fade(dt);
            if o.leaving && o.weight <= 0.0 {
                self.once = None;
            }
        }
    }

    /// The clips over `pose` (just reset to rest); how much they replace the
    /// idle motion (0..1)
    pub fn apply(&self, model: &Model, pose: &mut Pose) -> f32 {
        let mut shown = 0.0f32;
        for l in [self.old_base, self.base, self.once].into_iter().flatten() {
            let clip = &model.clips[l.clip];
            clip.apply(pose, l.time, l.weight);
            // A face-only clip (shape keys) leaves the idle body motion alone
            if clip.channels.iter().any(|c| !matches!(c.values, Values::Weights { .. })) {
                shown = shown.max(l.weight);
            }
        }
        shown
    }

    /// The clips' shape keys over `morph`
    pub fn apply_shapes(&self, model: &Model, morph: &mut [f32]) {
        for l in [self.old_base, self.base, self.once].into_iter().flatten() {
            model.clips[l.clip].apply_shapes(morph, l.time, l.weight);
        }
    }
}

// ---------------------------------------------------------------------------
// The procedure

/// Every Puppet source (OBS's pointer) and its player's control
static SOURCES: Mutex<Vec<(usize, Shared)>> = Mutex::new(Vec::new());

pub fn add_source(source: Ptr, control: Shared) {
    SOURCES.lock().unwrap().push((source as usize, control));
}

pub fn remove_source(source: Ptr) {
    SOURCES.lock().unwrap().retain(|(s, _)| *s != source as usize);
}

const DECL: &CStr = c"void theater_puppet_animation(in string args, out string json)";

/// Registers the procedure. Call from obs_module_load.
pub fn register() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if !handler.is_null() {
            (api.proc_handler_add)(handler, DECL.as_ptr(), animation_proc, std::ptr::null_mut());
        }
    }
}

unsafe extern "C" fn animation_proc(_data: Ptr, cd: *mut Calldata) {
    let args = get_string(cd, c"args");
    let json = std::panic::catch_unwind(|| answer(&args)).unwrap_or_else(|_| "{}".into());
    set_string(cd, c"json", &json);
}

fn answer(args: &str) -> String {
    let args: Value = serde_json::from_str(args).unwrap_or(json!({}));
    let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (which, clip) = (text("source"), text("clip"));
    let names: Vec<(Ptr, String)> = crate::parts::sources();
    let controls = SOURCES.lock().unwrap().clone();
    let targets: Vec<(Ptr, String, Shared)> = controls
        .into_iter()
        .filter_map(|(s, c)| names.iter().find(|(p, _)| *p as usize == s).map(|(p, n)| (*p, n.clone(), c)))
        .filter(|(_, n, _)| which.is_empty() || *n == which)
        .collect();
    let action = args.get("action").and_then(|a| a.as_str()).unwrap_or("get");
    let mut done = action == "get" || !targets.is_empty();
    for (source, _, control) in &targets {
        match action {
            "play" => {
                let mut c = control.lock().unwrap();
                if c.clips.contains(&clip) {
                    c.play = Some(clip.clone());
                } else {
                    done = false;
                }
            }
            "stop" => control.lock().unwrap().stop = true,
            // The source's setting: saved with it, and its properties agree
            "loop" => unsafe {
                let api = obs::api();
                let data = (api.obs_data_create)();
                if !data.is_null() {
                    let value = std::ffi::CString::new(clip.replace('\0', "")).unwrap_or_default();
                    (api.obs_data_set_string)(data, crate::source::S_ANIMATION.as_ptr(), value.as_ptr());
                    (api.obs_source_update)(*source, data);
                    (api.obs_data_release)(data);
                }
            },
            _ => {}
        }
    }
    let list: Vec<Value> = SOURCES
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(s, control)| {
            let name = names.iter().find(|(p, _)| *p as usize == *s)?.1.clone();
            let c = control.lock().unwrap();
            // ("looping": `loop` is a Rhai keyword)
            Some(json!({ "source": name, "clips": c.clips, "looping": c.loop_clip, "playing": c.playing }))
        })
        .collect();
    json!({ "puppets": list, "done": done }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_found_and_held_at_the_ends() {
        let t = [0.0, 1.0, 3.0];
        assert_eq!(keys(&t, -1.0, false), (0, 0, 0.0));
        assert_eq!(keys(&t, 0.5, false), (0, 1, 0.5));
        assert_eq!(keys(&t, 2.0, false), (1, 2, 0.5));
        assert_eq!(keys(&t, 2.0, true), (1, 2, 0.0));
        assert_eq!(keys(&t, 9.0, false), (2, 2, 0.0));
    }

    fn model_with(clips: Vec<Clip>) -> Model {
        Model {
            primitives: Vec::new(),
            materials: Vec::new(),
            images: Vec::new(),
            min: Vec3::ZERO,
            max: Vec3::ONE,
            nodes: vec![crate::model::Node {
                name: "Hips".into(),
                parent: None,
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
                extras: Value::Null,
            }],
            order: vec![0],
            skins: Vec::new(),
            morph_names: Vec::new(),
            clips,
        }
    }

    fn lift(name: &str, duration: f32) -> Clip {
        Clip {
            name: name.into(),
            duration,
            channels: vec![Channel { node: 0, times: vec![0.0, duration], values: Values::Translation(vec![Vec3::Y, Vec3::Y]), step: false }],
        }
    }

    #[test]
    fn a_once_clip_fades_in_plays_and_goes_back() {
        let model = model_with(vec![lift("Jump", 1.0)]);
        let mut player = Player::default();
        let mut control = Control { play: Some("Jump".into()), ..Default::default() };
        player.sync(&model, &mut control);
        assert_eq!(control.clips, vec!["Jump".to_string()]);
        assert_eq!(control.playing, "Jump");
        let mut pose = Pose::rest(&model);
        let mut shown = Vec::new();
        for _ in 0..90 {
            player.step(&model, 1.0 / 60.0);
            pose.reset(&model);
            shown.push(player.apply(&model, &mut pose));
        }
        // Up, all the way, then back down after its second
        assert!(shown[3] > 0.0 && shown[3] < 1.0);
        assert_eq!(shown[30], 1.0);
        assert_eq!(*shown.last().unwrap(), 0.0);
        assert_eq!(pose.translation[0], Vec3::ZERO);
        player.sync(&model, &mut control);
        assert_eq!(control.playing, "");
    }

    #[test]
    fn the_loop_keeps_going_and_unknown_clips_are_ignored() {
        let model = model_with(vec![lift("Idle", 0.5)]);
        let mut player = Player::default();
        let mut control = Control { loop_clip: "Idle".into(), play: Some("Nope".into()), ..Default::default() };
        player.sync(&model, &mut control);
        assert_eq!(control.playing, "");
        let mut pose = Pose::rest(&model);
        let mut shown = 0.0;
        for _ in 0..120 {
            player.step(&model, 1.0 / 60.0);
            pose.reset(&model);
            shown = player.apply(&model, &mut pose);
        }
        assert_eq!(shown, 1.0);
        assert!((pose.translation[0] - Vec3::Y).length() < 1e-6);
        // Back to the idle motion: faded out
        control.loop_clip.clear();
        player.sync(&model, &mut control);
        for _ in 0..30 {
            player.step(&model, 1.0 / 60.0);
        }
        pose.reset(&model);
        assert_eq!(player.apply(&model, &mut pose), 0.0);
    }
}
