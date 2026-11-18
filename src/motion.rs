//! The avatar alive: its pose every frame. Idle motion first (arms down out
//! of the T-pose, breathing, a little sway and head motion, blinking, the
//! Turn setting eased in so the hair swings after it), then whatever is
//! tracked (tracking.rs: the face's shapes, the head, the hands, the
//! fingers…) through the IK routine (ik.rs), then its spring bones, and the
//! mesh skinned to it. Runs in the source's video_tick; the GPU takes the
//! vertices in its render.

use std::sync::Arc;

use glam::{Quat, Vec3};

use crate::ik::Skeleton;
use crate::model::Model;
use crate::pose::{self, Pose};
use crate::springs::{Adjust, Springs};
use crate::tracking::{self, ARKIT, Hand, Snapshot, Target};

/// How the avatar moves, from the source's settings
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    /// Degrees down from the T-pose
    pub arms_down: f32,
    /// 0 = still, 1 = lively
    pub idle: f32,
    /// Turn, degrees
    pub yaw: f32,
    pub springs: bool,
    /// The springs' tuning per kind, and the colliders' thickness
    pub adjust: Adjust,
    /// Whether what's tracked (the Tracking window's inputs) moves this avatar
    pub tracked: bool,
}

impl Default for Motion {
    fn default() -> Self {
        Motion { arms_down: 65.0, idle: 0.5, yaw: 0.0, springs: true, adjust: Adjust::default(), tracked: true }
    }
}

/// For a face without ARKit's keys (a VRoid one, not converted): the VRoid
/// key nearest to each ARKit shape (Theater Rig's "approximate" list). Two
/// shapes on one key (both smiles on Fcl_MTH_Fun) give it the stronger.
const VROID: [(&str, &str); 21] = [
    ("eyeBlinkLeft", "Fcl_EYE_Close_L"),
    ("eyeBlinkRight", "Fcl_EYE_Close_R"),
    ("eyeWideLeft", "Fcl_EYE_Surprised"),
    ("eyeWideRight", "Fcl_EYE_Surprised"),
    ("eyeSquintLeft", "Fcl_EYE_Joy_L"),
    ("eyeSquintRight", "Fcl_EYE_Joy_R"),
    ("jawOpen", "Fcl_MTH_A"),
    ("mouthClose", "Fcl_MTH_Close"),
    ("mouthFunnel", "Fcl_MTH_O"),
    ("mouthPucker", "Fcl_MTH_U"),
    ("mouthSmileLeft", "Fcl_MTH_Fun"),
    ("mouthSmileRight", "Fcl_MTH_Fun"),
    ("mouthFrownLeft", "Fcl_MTH_Sorrow"),
    ("mouthFrownRight", "Fcl_MTH_Sorrow"),
    ("mouthStretchLeft", "Fcl_MTH_I"),
    ("mouthStretchRight", "Fcl_MTH_I"),
    ("browDownLeft", "Fcl_BRW_Angry"),
    ("browDownRight", "Fcl_BRW_Angry"),
    ("browInnerUp", "Fcl_BRW_Sorrow"),
    ("browOuterUpLeft", "Fcl_BRW_Surprised"),
    ("browOuterUpRight", "Fcl_BRW_Surprised"),
];

/// Seconds a part takes to fade in when its input starts, out when it stops
const FADE: f32 = 0.25;

/// A tracked value followed smoothly, fading in and out with its input
#[derive(Clone, Copy)]
struct Follow<T> {
    value: Option<T>,
    /// 0 (not tracked: the idle pose) .. 1 (tracked)
    weight: f32,
}

impl<T> Default for Follow<T> {
    fn default() -> Self {
        Follow { value: None, weight: 0.0 }
    }
}

impl<T: Copy> Follow<T> {
    /// `ease`: how much of the way to the new value this frame (1 = all)
    fn step(&mut self, new: Option<T>, ease: f32, dt: f32, mix: impl Fn(T, T, f32) -> T) {
        match (new, self.value) {
            (Some(n), Some(old)) if self.weight > 0.0 => {
                self.value = Some(mix(old, n, ease));
                self.weight = (self.weight + dt / FADE).min(1.0);
            }
            (Some(n), _) => {
                self.value = Some(n);
                self.weight = (self.weight + dt / FADE).min(1.0);
            }
            (None, _) => {
                self.weight = (self.weight - dt / FADE).max(0.0);
                if self.weight == 0.0 {
                    self.value = None;
                }
            }
        }
    }

    fn get(&self) -> Option<(T, f32)> {
        self.value.filter(|_| self.weight > 0.0).map(|v| (v, self.weight))
    }
}

fn mix_target(a: Target, b: Target, t: f32) -> Target {
    Target { position: a.position.lerp(b.position, t), rotation: a.rotation.slerp(b.rotation, t) }
}

fn mix_hand(a: Hand, b: Hand, t: f32) -> Hand {
    let mut out = a;
    for f in 0..5 {
        out.spread[f] = a.spread[f] + (b.spread[f] - a.spread[f]) * t;
        for j in 0..3 {
            out.bend[f][j] = a.bend[f][j] + (b.bend[f][j] - a.bend[f][j]) * t;
        }
    }
    out
}

/// What's tracked, followed
#[derive(Default)]
struct Followed {
    shapes: Follow<[f32; 52]>,
    head_turn: Follow<Quat>,
    head_position: Follow<Vec3>,
    hands: [Follow<Target>; 2],
    elbows: [Option<Vec3>; 2],
    fingers: [Follow<Hand>; 2],
    hips: Follow<Target>,
    feet: [Follow<Target>; 2],
}

pub struct Rig {
    pub model: Arc<Model>,
    pose: Pose,
    rest: Pose,
    springs: Springs,
    skeleton: Skeleton,
    time: f32,
    /// The turn the body has now, easing toward the setting
    yaw: f32,
    settled: bool,
    /// Each shape key's weight (the model's morph_names' order)
    morph: Vec<f32>,
    /// Where each ARKit shape is among the model's shape keys
    arkit: [Option<usize>; 52],
    followed: Followed,
    /// The next blink, when no face is tracked (seconds on `time`)
    next_blink: f32,
    pub points: Vec<Vec<[f32; 4]>>,
    pub normals: Vec<Vec<[f32; 4]>>,
    /// New vertices since the GPU last took them
    pub dirty: bool,
}

/// The Mixamo bones the idle motion turns (None: the avatar hasn't that one)
struct Bones {
    hips: Option<usize>,
    spine1: Option<usize>,
    neck: Option<usize>,
    head: Option<usize>,
    left_arm: Option<usize>,
    right_arm: Option<usize>,
    left_hand: Option<usize>,
    right_hand: Option<usize>,
}

/// As in a mirror: left and right swapped, across the avatar's middle
fn mirrored(s: &Snapshot) -> Snapshot {
    let flip = |q: Quat| Quat::from_xyzw(q.x, -q.y, -q.z, q.w);
    let across = |p: Vec3| Vec3::new(-p.x, p.y, p.z);
    let target = |t: Target| Target { position: across(t.position), rotation: flip(t.rotation) };
    let mut out = s.clone();
    out.shapes = s.shapes.map(|shapes| {
        let mut m = shapes;
        for (i, name) in ARKIT.iter().enumerate() {
            let other = if let Some(base) = name.strip_suffix("Left") {
                format!("{base}Right")
            } else if let Some(base) = name.strip_suffix("Right") {
                format!("{base}Left")
            } else {
                continue;
            };
            if let Some(j) = tracking::arkit_index(&other) {
                m[i] = shapes[j];
            }
        }
        m
    });
    out.eyes = s.eyes.map(|[l, r]| [flip(r), flip(l)]);
    out.head_turn = s.head_turn.map(flip);
    out.head_position = s.head_position.map(across);
    out.left_hand = s.right_hand.map(target);
    out.right_hand = s.left_hand.map(target);
    out.left_elbow = s.right_elbow.map(across);
    out.right_elbow = s.left_elbow.map(across);
    out.left_fingers = s.right_fingers;
    out.right_fingers = s.left_fingers;
    out.hips = s.hips.map(target);
    out.left_foot = s.right_foot.map(target);
    out.right_foot = s.left_foot.map(target);
    out
}

impl Rig {
    pub fn new(model: Arc<Model>) -> Rig {
        let pose = Pose::rest(&model);
        let springs = Springs::new(&model, &pose);
        let skeleton = Skeleton::new(&model, &pose);
        // Each ARKit shape's key: its own, or the VRoid key nearest to it
        let arkit: [Option<usize>; 52] = std::array::from_fn(|i| {
            model.morph(ARKIT[i]).or_else(|| VROID.iter().find(|(a, _)| *a == ARKIT[i]).and_then(|(_, v)| model.morph(v)))
        });
        crate::log(&format!(
            "[Puppet] 🪢 Springs: {} chain(s), {} bone(s), {} collider(s); {} shape key(s), {} of ARKit's 52",
            springs.chains,
            springs.joints(),
            springs.colliders.len(),
            model.morph_names.len(),
            arkit.iter().flatten().count()
        ));
        let points = model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect();
        let normals = model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect();
        let morph = vec![0.0; model.morph_names.len()];
        Rig {
            rest: pose.clone(),
            model,
            pose,
            springs,
            skeleton,
            time: 0.0,
            yaw: f32::NAN,
            settled: false,
            morph,
            arkit,
            followed: Followed::default(),
            next_blink: 2.0,
            points,
            normals,
            dirty: false,
        }
    }

    /// The spring kinds this avatar has (indices into springs::KINDS)
    pub fn spring_kinds(&self) -> Vec<usize> {
        self.springs.kinds()
    }

    fn bones(&self) -> Bones {
        let m = &self.model;
        Bones {
            hips: m.node("Hips"),
            spine1: m.node("Spine1").or_else(|| m.node("Spine")),
            neck: m.node("Neck"),
            head: m.node("Head"),
            left_arm: m.node("LeftArm"),
            right_arm: m.node("RightArm"),
            left_hand: m.node("LeftHand"),
            right_hand: m.node("RightHand"),
        }
    }

    /// One frame: `dt` seconds later, with what's tracked now
    pub fn step(&mut self, dt: f32, motion: Motion) {
        let config = tracking::config::get();
        let tracked = if motion.tracked { tracking::snapshot(&config.assign) } else { Snapshot::default() };
        self.step_with(dt, motion, &tracked, &config);
    }

    /// One frame with `tracked` (as the inputs gave it: the user's size, not mirrored)
    pub fn step_with(&mut self, dt: f32, motion: Motion, tracked: &Snapshot, config: &tracking::config::Config) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        // The body eases into a new turn (a jump of the setting swings the hair)
        if self.yaw.is_nan() {
            self.yaw = motion.yaw;
        }
        self.yaw += (motion.yaw - self.yaw) * (dt * 6.0).min(1.0);

        let tracked = if config.mirror { mirrored(tracked) } else { tracked.clone() };
        self.follow(&tracked, dt, config);

        let model = self.model.clone();
        let bones = self.bones();
        let (t, idle) = (self.time, motion.idle.clamp(0.0, 1.0));
        let deg = |d: f32| d.to_radians();
        let turn = Quat::from_rotation_y(deg(self.yaw));
        let head_tracked = self.followed.head_turn.get().map(|(_, w)| w).unwrap_or(0.0);
        let pose = &mut self.pose;
        pose.reset(&model);
        pose.update_world(&model);
        // Turned at the hips, swaying a little
        if let Some(h) = bones.hips {
            let sway = Quat::from_rotation_y(deg(self.yaw + (t * 0.4).sin() * 4.0 * idle)) * Quat::from_rotation_z(deg((t * 0.6).sin() * 1.2 * idle));
            pose.rotate_in_world(&model, h, sway);
            pose.update_world(&model);
        }
        // Breathing
        if let Some(s) = bones.spine1 {
            pose.rotate_in_world(&model, s, Quat::from_rotation_x(deg((t * std::f32::consts::TAU / 4.0).sin() * 1.5 * idle)));
            pose.update_world(&model);
        }
        // The head looks around a little (less as the tracked head takes over)
        if let Some(h) = bones.head.or(bones.neck) {
            let k = idle * (1.0 - head_tracked);
            let look = Quat::from_rotation_y(deg((t * 0.7 + 1.0).sin() * 6.0 * k))
                * Quat::from_rotation_x(deg((t * 0.5).sin() * 3.0 * k))
                * Quat::from_rotation_z(deg((t * 0.9 + 2.0).sin() * 2.0 * k));
            pose.rotate_in_world(&model, h, look);
            pose.update_world(&model);
        }
        // Arms down from the T-pose (the avatar faces +Z, its left is +X)
        let arms = motion.arms_down + (t * 0.8).sin() * 1.5 * idle;
        if let Some(a) = bones.left_arm {
            pose.rotate_in_world(&model, a, turn * Quat::from_rotation_z(-deg(arms)) * turn.inverse());
        }
        if let Some(a) = bones.right_arm {
            pose.rotate_in_world(&model, a, turn * Quat::from_rotation_z(deg(arms)) * turn.inverse());
        }
        pose.update_world(&model);

        // What's tracked, over the idle pose
        let targets = self.targets(turn, &bones);
        self.skeleton.apply(&model, &mut self.pose, &targets, config.head_strength, config.spine_lean);
        self.shapes(&targets, idle);

        let pose = &mut self.pose;
        if motion.springs {
            // A new avatar (or springs turned back on): start at rest, still
            if !self.settled {
                self.springs.reset(pose);
                self.settled = true;
            }
            self.springs.step(&model, pose, dt, &motion.adjust);
        } else {
            self.settled = false;
        }
        pose::skin(&model, pose, &self.morph, &mut self.points, &mut self.normals);
        self.dirty = true;
    }

    /// Follows what's tracked: smoothed, each part fading in and out
    fn follow(&mut self, s: &Snapshot, dt: f32, config: &tracking::config::Config) {
        // The avatar's size to the user's (the head bone is about 87% of a body's height)
        let scale = (self.skeleton.rest_head().y / 0.87) / config.user_height.clamp(0.5, 2.5);
        let rest_head = self.skeleton.rest_head();
        // Where the user's head is when standing straight (eye height)
        let user_head = Vec3::new(0.0, config.user_height.clamp(0.5, 2.5) * 0.93, 0.0);
        let sized = |t: Target| Target { position: t.position * scale, rotation: t.rotation };
        // How much of the way this frame: none of the lag at smoothing 0
        let tau = config.smoothing.clamp(0.0, 1.0) * 0.12;
        let ease = if tau <= 1e-4 { 1.0 } else { 1.0 - (-dt / tau).exp() };
        let f = &mut self.followed;
        f.shapes.step(s.shapes, ease, dt, |a, b, t| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
        f.head_turn.step(s.head_turn, ease, dt, |a, b, t| a.slerp(b, t));
        f.head_position.step(s.head_position.map(|p| rest_head + (p - user_head) * scale), ease, dt, |a, b, t| a.lerp(b, t));
        f.hands[0].step(s.left_hand.map(sized), ease, dt, mix_target);
        f.hands[1].step(s.right_hand.map(sized), ease, dt, mix_target);
        f.elbows = [s.left_elbow.map(|p| p * scale), s.right_elbow.map(|p| p * scale)];
        f.fingers[0].step(s.left_fingers, ease, dt, mix_hand);
        f.fingers[1].step(s.right_fingers, ease, dt, mix_hand);
        f.hips.step(s.hips.map(sized), ease, dt, mix_target);
        f.feet[0].step(s.left_foot.map(sized), ease, dt, mix_target);
        f.feet[1].step(s.right_foot.map(sized), ease, dt, mix_target);
    }

    /// The IK's targets this frame: what's followed, turned with the body
    /// (`turn`) and blended from the idle pose (`self.pose`) by each part's fade
    fn targets(&self, turn: Quat, bones: &Bones) -> Snapshot {
        let f = &self.followed;
        let pose = &self.pose;
        let rest_head = self.skeleton.rest_head();
        // A tracked place or turn, in the turned body's space
        let placed = |t: Target| Target { position: turn * t.position, rotation: turn * t.rotation };
        // From where the idle pose has a bone to where it's tracked
        let blend = |bone: Option<usize>, t: Target, w: f32| -> Target {
            let t = placed(t);
            match bone {
                Some(b) if w < 1.0 => {
                    let idle = Target { position: pose.position(b), rotation: pose.world_rotation(b) * self.rest.world_rotation(b).inverse() };
                    mix_target(idle, t, w)
                }
                _ => t,
            }
        };
        let mut out = Snapshot::default();
        out.shapes = f.shapes.get().map(|(s, w)| s.map(|v| v * w));
        out.head_turn = f.head_turn.get().map(|(q, w)| Quat::IDENTITY.slerp(q, w));
        out.head_position = f.head_position.get().map(|(p, w)| rest_head + turn * ((p - rest_head) * w));
        out.left_hand = f.hands[0].get().map(|(t, w)| blend(bones.left_hand, t, w));
        out.right_hand = f.hands[1].get().map(|(t, w)| blend(bones.right_hand, t, w));
        out.left_elbow = f.elbows[0].filter(|_| out.left_hand.is_some()).map(|p| turn * p);
        out.right_elbow = f.elbows[1].filter(|_| out.right_hand.is_some()).map(|p| turn * p);
        let fade_hand = |(h, w): (Hand, f32)| mix_hand(Hand::default(), h, w);
        out.left_fingers = f.fingers[0].get().map(fade_hand);
        out.right_fingers = f.fingers[1].get().map(fade_hand);
        out.hips = f.hips.get().map(|(t, w)| blend(bones.hips, t, w));
        out.left_foot = f.feet[0].get().map(|(t, w)| blend(self.model.node("LeftFoot"), t, w));
        out.right_foot = f.feet[1].get().map(|(t, w)| blend(self.model.node("RightFoot"), t, w));
        out
    }

    /// The face's shape keys: tracked, or a blink now and then
    fn shapes(&mut self, targets: &Snapshot, idle: f32) {
        self.morph.iter_mut().for_each(|w| *w = 0.0);
        match targets.shapes {
            Some(shapes) => {
                for (i, slot) in self.arkit.iter().enumerate() {
                    if let Some(m) = slot {
                        self.morph[*m] = self.morph[*m].max(shapes[i]);
                    }
                }
                self.next_blink = self.time + 2.0;
            }
            None if idle > 0.0 => {
                // Closes in 60 ms, opens in 120, every few seconds
                let since = self.time - self.next_blink;
                if since > 0.18 {
                    // (the wait varies with the time it's at: no clockwork)
                    self.next_blink = self.time + 2.5 + ((self.time * 12.9898).sin() * 43758.547).fract().abs() * 3.5;
                } else if since >= 0.0 {
                    let closed = if since < 0.06 { since / 0.06 } else { 1.0 - (since - 0.06) / 0.12 };
                    for name in ["eyeBlinkLeft", "eyeBlinkRight"] {
                        if let Some(m) = tracking::arkit_index(name).and_then(|i| self.arkit[i]) {
                            self.morph[m] = closed.clamp(0.0, 1.0);
                        }
                    }
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> Option<Rig> {
        let path = std::env::var("THEATER_PUPPET_GLB").ok()?;
        Some(Rig::new(Arc::new(crate::model::load(std::path::Path::new(&path)).unwrap())))
    }

    fn still() -> Motion {
        Motion { idle: 0.0, springs: false, ..Motion::default() }
    }

    /// THEATER_PUPPET_GLB=<a Rig export> cargo test -- --nocapture
    #[test]
    fn a_tracked_face_moves_the_mesh() {
        let Some(mut rig) = rig() else { return };
        let config = tracking::config::Config { smoothing: 0.0, ..Default::default() };
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        let before = rig.points.clone();
        // The jaw open, fully, for long enough to fade in
        let mut shapes = [0.0; 52];
        shapes[tracking::arkit_index("jawOpen").unwrap()] = 1.0;
        let tracked = Snapshot { shapes: Some(shapes), ..Default::default() };
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &tracked, &config);
        }
        let moved: usize = before.iter().zip(&rig.points).map(|(a, b)| a.iter().zip(b).filter(|(p, q)| (p[1] - q[1]).abs() > 1e-4).count()).sum();
        println!("jawOpen moves {moved} vertices");
        assert!(moved > 20, "jawOpen moved {moved} vertices");
        // The face is let go: back to rest once it has faded
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        let back: f32 = before.iter().zip(&rig.points).flat_map(|(a, b)| a.iter().zip(b).map(|(p, q)| (p[1] - q[1]).abs())).fold(0.0, f32::max);
        assert!(back < 1e-4, "still open by {back}");
    }

    #[test]
    fn a_tracked_hand_fades_in_and_is_mirrored() {
        let Some(mut rig) = rig() else { return };
        let config = tracking::config::Config { smoothing: 0.0, user_height: rig.skeleton.rest_head().y / 0.87, ..Default::default() };
        let hand = rig.model.node("LeftHand").unwrap();
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        let idle = rig.pose.position(hand);
        // In front of the chest, at the avatar's own size
        let place = Vec3::new(0.2, rig.skeleton.rest_head().y - 0.3, 0.3);
        let tracked = Snapshot { left_hand: Some(Target { position: place, rotation: Quat::IDENTITY }), ..Default::default() };
        rig.step_with(1.0 / 60.0, still(), &tracked, &config);
        let first = rig.pose.position(hand);
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &tracked, &config);
        }
        let there = rig.pose.position(hand);
        println!("left hand: idle {idle:?}, first frame {first:?}, then {there:?}");
        // Not there at once, there once faded in
        assert!((first - place).length() > 0.05 && (there - place).length() < 0.01);

        // Mirrored: the same hand moves the avatar's right one, across its middle
        let mut rig2 = Rig::new(rig.model.clone());
        let mirror = tracking::config::Config { mirror: true, ..config.clone() };
        for _ in 0..40 {
            rig2.step_with(1.0 / 60.0, still(), &tracked, &mirror);
        }
        let right = rig2.pose.position(rig2.model.node("RightHand").unwrap());
        assert!((right - Vec3::new(-place.x, place.y, place.z)).length() < 0.01, "mirrored hand at {right:?}");
    }

    #[test]
    fn it_blinks_alone() {
        let Some(mut rig) = rig() else { return };
        let config = tracking::config::Config::default();
        let blink = tracking::arkit_index("eyeBlinkLeft").and_then(|i| rig.arkit[i]).unwrap();
        let mut closed = 0.0f32;
        for _ in 0..600 {
            rig.step_with(1.0 / 60.0, Motion { springs: false, ..Motion::default() }, &Snapshot::default(), &config);
            closed = closed.max(rig.morph[blink]);
        }
        assert!(closed > 0.8, "no blink in ten seconds ({closed})");
    }
}
