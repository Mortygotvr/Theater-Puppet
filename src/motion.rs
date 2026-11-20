//! The avatar alive: its pose every frame. Idle motion first (arms down out
//! of the T-pose, breathing, a little sway and head motion, blinking, the
//! Turn setting eased in so the hair swings after it), then whatever is
//! tracked (tracking.rs: the face's shapes, the head, the hands, the
//! fingers…) through the IK routine (ik.rs), then its spring bones, and the
//! mesh skinned to it. An animation of the avatar's own (animation.rs) takes
//! the idle motion's place while it plays; the Turn and what's tracked still
//! go over it. Runs in the source's video_tick; the GPU takes the
//! vertices in its render.

use std::sync::Arc;

use glam::{Quat, Vec3};

use crate::ik::Skeleton;
use crate::model::Model;
use crate::pose::{self, Pose};
use crate::springs::{Adjust, Springs};
use crate::tracking::{self, ARKIT, Hand, Part, Snapshot, Target};

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
    /// Whether what's tracked (the Configuration window's inputs) moves this avatar
    pub tracked: bool,
    /// The controller it holds moved from its place on the chest: up (0 =
    /// the lowest), forward (metres)
    pub controller_offset: [f32; 2],
    /// The hands on it moved from its grips: up, apart (each side; metres)
    pub hand_offset: [f32; 2],
    /// The hands on it turned about their forearm (degrees)
    pub hand_roll: f32,
    /// The hands on it moved forward (+) or back (-) along it (metres)
    pub hand_forward: f32,
    /// The thumbs on it raised (+) or lowered (-), radians at each joint
    pub thumb_lift: f32,
    /// At a desk (Arms on the Keyboard & mouse input): what the keys show, the hands
    pub desk_look: crate::desk::Look,
    /// The desk moved from its place: up (+) or down (-), forward (metres)
    pub desk_offset: [f32; 2],
}

impl Default for Motion {
    fn default() -> Self {
        Motion { arms_down: 65.0, idle: 0.5, yaw: 0.0, springs: true, adjust: Adjust::default(), tracked: true, controller_offset: [0.0, 0.0], hand_offset: [0.0, 0.03], hand_roll: 0.0, hand_forward: 0.0, thumb_lift: 0.0, desk_look: Default::default(), desk_offset: [0.0, 0.0] }
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

/// Each ARKit shape's key in `model`: its own, or the VRoid key nearest to it
fn arkit_keys(model: &Model) -> [Option<usize>; 52] {
    std::array::from_fn(|i| model.morph(ARKIT[i]).or_else(|| VROID.iter().find(|(a, _)| *a == ARKIT[i]).and_then(|(_, v)| model.morph(v))))
}

/// The face the avatar shows this frame, whatever keys it has: ARKit's 52
/// (tracked, blinking, the microphone's ARKit vowels) and the keys the
/// microphone names (its VRM vowels). An extra GLB's keys the avatar doesn't
/// have follow this.
#[derive(Clone, Debug)]
pub struct Face {
    pub arkit: [f32; 52],
    pub named: Vec<(String, f32)>,
}

impl Default for Face {
    fn default() -> Self {
        Face { arkit: [0.0; 52], named: Vec::new() }
    }
}

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

pub(crate) fn mix_hand(a: Hand, b: Hand, t: f32) -> Hand {
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
    /// The face this frame (for extra GLBs' keys the avatar doesn't have)
    pub face: Face,
    /// How much of the hands', hips' and feet's tracked movement shows (a VR input's feel)
    shares: [f32; 3],
    followed: Followed,
    /// The avatar's own animations (animation.rs)
    pub player: crate::animation::Player,
    /// The GLBs drawn with it, copying its pose
    pub followers: Vec<Follower>,
    /// What each of them copies (the source's settings, the followers' order)
    pub follow: Vec<FollowSettings>,
    /// Shape keys driven from outside (remote.rs), on any of its GLBs
    pub overrides: Vec<crate::remote::Override>,
    /// GLBs (layers) to hide, and the fade they take: their shape-key
    /// animation ended so
    pub hide: Vec<(usize, f32)>,
    /// The controller it holds while its Arms are on the Controller input
    pub prop: Option<crate::controller::Prop>,
    /// The controller's state, while it's held
    pad: Option<crate::tracking::gamepad::Pad>,
    /// The keyboard and mouse, while the avatar is at its desk
    desk: Option<crate::tracking::desk::Desk>,
    /// The chest as the last frame left it (where, how turned from rest):
    /// the controller is bound to it
    chest_last: Option<(Vec3, Quat)>,
    /// The source's thumb setting (Motion's), for the controller's fingers
    thumb_lift: f32,
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
        let arkit = arkit_keys(&model);
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
            player: Default::default(),
            followers: Vec::new(),
            follow: Vec::new(),
            face: Face::default(),
            shares: [1.0; 3],
            overrides: Vec::new(),
            hide: Vec::new(),
            prop: None,
            pad: None,
            desk: None,
            chest_last: None,
            thumb_lift: 0.0,
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

    /// Shape keys driven from outside: each (layer, key) replaces the one
    /// before it; None lets it go
    pub fn drive_shapes(&mut self, requests: Vec<(usize, String, Option<crate::remote::Drive>)>) {
        for (layer, key, drive) in requests {
            self.overrides.retain(|o| !(o.layer == layer && o.key.eq_ignore_ascii_case(&key)));
            if let Some(drive) = drive {
                self.overrides.push(crate::remote::Override { layer, key, drive, time: 0.0 });
            }
        }
    }

    /// The driven shape keys `dt` later: an animation that ended is let go
    /// (and its GLB hidden, when it asked)
    fn step_overrides(&mut self, dt: f32) {
        for o in &mut self.overrides {
            o.time += dt;
        }
        let hide = &mut self.hide;
        self.overrides.retain(|o| {
            let going = o.value().is_none();
            if let (true, crate::remote::Drive::Once { hide_at_end: true, fade, .. }) = (going, o.drive) {
                hide.push((o.layer, fade));
            }
            !going
        });
    }

    /// One frame with `tracked` (as the inputs gave it: the user's size, not mirrored)
    pub fn step_with(&mut self, dt: f32, motion: Motion, tracked: &Snapshot, config: &tracking::config::Config) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        self.step_overrides(dt);
        // The body eases into a new turn (a jump of the setting swings the hair)
        if self.yaw.is_nan() {
            self.yaw = motion.yaw;
        }
        self.yaw += (motion.yaw - self.yaw) * (dt * 6.0).min(1.0);

        let tracked = if config.mirror { mirrored(tracked) } else { tracked.clone() };
        self.follow(&tracked, dt, config);
        self.pad = tracked.controller;
        self.desk = tracked.desk;

        let model = self.model.clone();
        let bones = self.bones();
        let deg = |d: f32| d.to_radians();
        let turn = Quat::from_rotation_y(deg(self.yaw));
        let head_tracked = self.followed.head_turn.get().map(|(_, w)| w).unwrap_or(0.0);
        let pose = &mut self.pose;
        pose.reset(&model);
        // An animation playing takes the idle motion's place (fading between them)
        self.player.step(&model, dt);
        let animated = self.player.apply(&model, pose);
        let still = 1.0 - animated;
        let (t, idle) = (self.time, motion.idle.clamp(0.0, 1.0) * still);
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
        let arms = motion.arms_down * still + (t * 0.8).sin() * 1.5 * idle;
        if let Some(a) = bones.left_arm {
            pose.rotate_in_world(&model, a, turn * Quat::from_rotation_z(-deg(arms)) * turn.inverse());
        }
        if let Some(a) = bones.right_arm {
            pose.rotate_in_world(&model, a, turn * Quat::from_rotation_z(deg(arms)) * turn.inverse());
        }
        pose.update_world(&model);

        // The controller held: bound to the chest as the last frame left it
        // (the arms then reach it), coming into the hands or going
        let chest_bone = model.node("Spine2").or(bones.spine1);
        if let Some(prop) = self.prop.as_mut().filter(|p| p.is_desk()) {
            // A desk: in front of the chest at rest, turned with the body (it doesn't lean)
            prop.fade(self.desk.is_some(), dt);
            prop.step_desk(self.desk, motion.desk_look, dt);
            let rest_head = self.skeleton.rest_head();
            let anchor = chest_bone.map(|c| turn * self.rest.position(c)).unwrap_or(rest_head * 0.8);
            prop.place_desk(anchor, turn, rest_head.y / 1.5, motion.desk_offset);
        } else if let Some(prop) = self.prop.as_mut() {
            prop.fade(self.pad.is_some(), dt);
            let rest_head = self.skeleton.rest_head();
            let (chest, chest_turn) = self.chest_last.unwrap_or_else(|| (chest_bone.map(|c| turn * self.rest.position(c)).unwrap_or(rest_head * 0.8), turn));
            prop.hand_roll = motion.hand_roll;
            prop.hand_forward = motion.hand_forward;
            self.thumb_lift = motion.thumb_lift;
            prop.place(chest, chest_turn, rest_head.y / 1.5, motion.controller_offset, motion.hand_offset, &self.pad.unwrap_or_default());
        }

        // What's tracked, over the idle pose
        let targets = self.targets(turn, &bones);
        self.skeleton.apply(&model, &mut self.pose, &targets, config.head_strength, config.spine_lean, config.torso_follow);
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
        for (i, f) in self.followers.iter_mut().enumerate() {
            let how = self.follow.get(i).cloned().unwrap_or_default();
            f.follow(&model, &self.rest, &self.pose, &self.morph, &self.face, dt, &motion, &self.overrides, i + 1, &how);
        }
        if let Some(prop) = self.prop.as_mut().filter(|p| p.weight > 0.0) {
            prop.pose_parts(&self.pad.unwrap_or_default());
        }
        // The chest now, for the controller next frame
        self.chest_last = chest_bone.map(|c| (self.pose.position(c), self.pose.world_rotation(c) * self.rest.world_rotation(c).inverse()));
    }

    /// The controller it holds while its Arms are on the Controller input (None: none)
    pub fn set_prop(&mut self, model: Option<Arc<Model>>) {
        self.prop = model.map(crate::controller::Prop::new);
    }

    /// The GLBs drawn with the avatar (each copies its pose), in place of the ones before
    pub fn set_extras(&mut self, models: &[Arc<Model>]) {
        self.followers = models.iter().map(|m| Follower::new(m.clone(), &self.model)).collect();
    }

    /// Follows what's tracked: smoothed, each part fading in and out
    fn follow(&mut self, s: &Snapshot, dt: f32, config: &tracking::config::Config) {
        // A VR input driving a part: its share of the movement and its smoothing
        let feel = |part: Part| -> Option<&tracking::config::VrFeel> {
            match s.from.iter().find(|(p, _)| *p == part).map(|(_, src)| *src) {
                Some(tracking::Source::Telemetry) => Some(&config.telemetry_feel),
                Some(tracking::Source::SteamVr) => Some(&config.steamvr_feel),
                _ => None,
            }
        };
        let share = |part: Part, pick: fn(&tracking::config::VrFeel) -> f32| feel(part).map_or(1.0, |f| pick(f).clamp(0.0, 1.0));
        self.shares = [share(Part::Arms, |f| f.hands), share(Part::LowerBody, |f| f.hips), share(Part::LowerBody, |f| f.feet)];
        // The avatar's size to the user's (the head bone is about 87% of a body's height)
        let scale = (self.skeleton.rest_head().y / 0.87) / config.user_height.clamp(0.5, 2.5);
        let rest_head = self.skeleton.rest_head();
        // Where the user's head is when standing straight (eye height)
        let user_head = Vec3::new(0.0, config.user_height.clamp(0.5, 2.5) * 0.93, 0.0);
        let sized = |t: Target| Target { position: t.position * scale, rotation: t.rotation };
        // How much of the way this frame: none of the lag at smoothing 0; a VR
        // input's own smoothing on top, for the parts it drives
        let ease_of = |part: Part| {
            let extra = feel(part).map_or(0.0, |f| f.smoothing.clamp(0.0, 1.0) * 0.3);
            let tau = (config.smoothing.clamp(0.0, 1.0) * 0.12).max(extra);
            if tau <= 1e-4 { 1.0 } else { 1.0 - (-dt / tau).exp() }
        };
        let ease = ease_of(Part::Expressions);
        let (turn_share, move_share) = (share(Part::HeadTurn, |f| f.head_turn), share(Part::HeadPosition, |f| f.head_move));
        let f = &mut self.followed;
        f.shapes.step(s.shapes, ease, dt, |a, b, t| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
        f.head_turn.step(s.head_turn.map(|q| Quat::IDENTITY.slerp(q, turn_share)), ease_of(Part::HeadTurn), dt, |a, b, t| a.slerp(b, t));
        f.head_position.step(s.head_position.map(|p| rest_head + (p - user_head) * scale * move_share), ease_of(Part::HeadPosition), dt, |a, b, t| a.lerp(b, t));
        let arms = ease_of(Part::Arms);
        f.hands[0].step(s.left_hand.map(sized), arms, dt, mix_target);
        f.hands[1].step(s.right_hand.map(sized), arms, dt, mix_target);
        f.elbows = [s.left_elbow.map(|p| p * scale), s.right_elbow.map(|p| p * scale)];
        let fingers = ease_of(Part::Fingers);
        f.fingers[0].step(s.left_fingers, fingers, dt, mix_hand);
        f.fingers[1].step(s.right_fingers, fingers, dt, mix_hand);
        let lower = ease_of(Part::LowerBody);
        f.hips.step(s.hips.map(sized), lower, dt, mix_target);
        f.feet[0].step(s.left_foot.map(sized), lower, dt, mix_target);
        f.feet[1].step(s.right_foot.map(sized), lower, dt, mix_target);
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
        let [hands, hips, feet] = self.shares;
        out.left_hand = f.hands[0].get().map(|(t, w)| blend(bones.left_hand, t, w * hands));
        out.right_hand = f.hands[1].get().map(|(t, w)| blend(bones.right_hand, t, w * hands));
        out.left_elbow = f.elbows[0].filter(|_| out.left_hand.is_some()).map(|p| turn * p);
        out.right_elbow = f.elbows[1].filter(|_| out.right_hand.is_some()).map(|p| turn * p);
        let fade_hand = |(h, w): (Hand, f32)| mix_hand(Hand::default(), h, w);
        out.left_fingers = f.fingers[0].get().map(fade_hand);
        out.right_fingers = f.fingers[1].get().map(fade_hand);
        out.hips = f.hips.get().map(|(t, w)| blend(bones.hips, t, w * hips));
        out.left_foot = f.feet[0].get().map(|(t, w)| blend(self.model.node("LeftFoot"), t, w * feet));
        out.right_foot = f.feet[1].get().map(|(t, w)| blend(self.model.node("RightFoot"), t, w * feet));
        // Holding the controller: the hands on it, the fingers on its buttons
        if let Some(prop) = self.prop.as_ref().filter(|p| p.weight > 0.0) {
            let w = prop.weight;
            let held = |bone: Option<usize>, t: Target| match bone {
                Some(b) if w < 1.0 => mix_target(Target { position: pose.position(b), rotation: pose.world_rotation(b) * self.rest.world_rotation(b).inverse() }, t, w),
                _ => t,
            };
            let [l, r] = prop.hands();
            out.left_hand = Some(held(bones.left_hand, l));
            out.right_hand = Some(held(bones.right_hand, r));
            out.left_elbow = None;
            out.right_elbow = None;
            let [lf, rf] = prop.desk_fingers().unwrap_or_else(|| crate::controller::fingers(&self.pad.unwrap_or_default(), self.thumb_lift));
            out.left_fingers = Some(mix_hand(Hand::default(), lf, w));
            out.right_fingers = Some(mix_hand(Hand::default(), rf, w));
        }
        out
    }

    /// The face's shape keys: tracked, or a blink now and then
    fn shapes(&mut self, targets: &Snapshot, idle: f32) {
        self.morph.iter_mut().for_each(|w| *w = 0.0);
        self.face = Face::default();
        match targets.shapes {
            Some(shapes) => {
                self.face.arkit = shapes;
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
                        if let Some(i) = tracking::arkit_index(name) {
                            self.face.arkit[i] = closed.clamp(0.0, 1.0);
                        }
                        if let Some(m) = tracking::arkit_index(name).and_then(|i| self.arkit[i]) {
                            self.morph[m] = closed.clamp(0.0, 1.0);
                        }
                    }
                }
            }
            None => {}
        }
        // The microphone's mouth (the Lipsync filter), as its mode says
        if let Some(mouth) = crate::lipsync::mouth() {
            use crate::lipsync::Mode;
            let tracked = targets.shapes.is_some();
            if !(mouth.mode == Mode::Fallback && tracked) {
                if mouth.mode == Mode::Replace && tracked {
                    // The face app's mouth goes; its eyes, brows… stay
                    for (i, name) in ARKIT.iter().enumerate() {
                        if name.starts_with("mouth") || name.starts_with("jaw") {
                            self.face.arkit[i] = 0.0;
                            if let Some(m) = self.arkit[i] {
                                self.morph[m] = 0.0;
                            }
                        }
                    }
                }
                self.mic_mouth(&mouth);
            }
        }
        // The avatar's own animations' shape keys
        let model = self.model.clone();
        self.player.apply_shapes(&model, &mut self.morph);
        // Then the ones driven from outside, over everything
        apply_overrides(&model, &self.overrides, 0, &mut self.morph);
    }
}

impl Rig {
    /// The microphone's mouth on the shape keys, as the filter's "vowel
    /// shapes" says: VRM (each vowel its key: the filter's own name, else
    /// Fcl_MTH_<vowel>) or ARKit (each vowel a mix of ARKit's mouth shapes);
    /// by how open the mouth is and how much it's that vowel; the more open wins
    fn mic_mouth(&mut self, mouth: &crate::lipsync::Mouth) {
        // ARKit's shapes for each vowel: A, I, U, E, O
        const ARKIT_VOWELS: [&[(&str, f32)]; 5] = [
            &[("jawOpen", 0.9)],
            &[("jawOpen", 0.25), ("mouthStretchLeft", 0.6), ("mouthStretchRight", 0.6), ("mouthSmileLeft", 0.3), ("mouthSmileRight", 0.3)],
            &[("jawOpen", 0.15), ("mouthPucker", 0.9), ("mouthFunnel", 0.3)],
            &[("jawOpen", 0.4), ("mouthStretchLeft", 0.4), ("mouthStretchRight", 0.4), ("mouthSmileLeft", 0.2), ("mouthSmileRight", 0.2)],
            &[("jawOpen", 0.5), ("mouthFunnel", 0.8)],
        ];
        const NAMES: [&str; 5] = ["A", "I", "U", "E", "O"];
        let model = self.model.clone();
        for v in 0..5 {
            let w = (mouth.vowels[v] * mouth.open).clamp(0.0, 1.0);
            if w <= 0.0 {
                continue;
            }
            match mouth.shapes {
                crate::lipsync::Shapes::Vrm => {
                    let name = if mouth.keys[v].is_empty() { format!("Fcl_MTH_{}", NAMES[v]) } else { mouth.keys[v].clone() };
                    if let Some(m) = model.morph(&name) {
                        self.morph[m] = self.morph[m].max(w);
                    }
                    self.face.named.push((name, w));
                }
                crate::lipsync::Shapes::Arkit => {
                    for (name, k) in ARKIT_VOWELS[v] {
                        if let Some(i) = tracking::arkit_index(name) {
                            self.face.arkit[i] = self.face.arkit[i].max(w * k);
                        }
                        if let Some(m) = tracking::arkit_index(name).and_then(|i| self.arkit[i]) {
                            self.morph[m] = self.morph[m].max(w * k);
                        }
                    }
                }
            }
        }
    }
}

/// The shape keys driven on layer `layer` (its model `model`) over `morph`
fn apply_overrides(model: &Model, overrides: &[crate::remote::Override], layer: usize, morph: &mut [f32]) {
    for o in overrides.iter().filter(|o| o.layer == layer) {
        if let (Some(m), Some(v)) = (model.morph(&o.key), o.value()) {
            morph[m] = v;
        }
    }
}

/// A GLB drawn with the avatar, in the same picture (so under the same
/// filters), copying its pose: each bone turns as the avatar's bone of the
/// same name turns, each shape key follows the avatar's of the same name.
/// Its own bone lengths are kept; its spring bones swing by themselves.
/// What it copies is the source's choice (FollowSettings).
pub struct Follower {
    pub model: Arc<Model>,
    pose: Pose,
    rest: Pose,
    /// Each of its nodes' node in the avatar (the same name)
    from: Vec<Option<usize>>,
    /// Each of its shape keys' in the avatar
    morph_from: Vec<Option<usize>>,
    /// Its own keys for ARKit's shapes (for those the avatar doesn't have)
    arkit: [Option<usize>; 52],
    morph: Vec<f32>,
    springs: Springs,
    settled: bool,
    pub points: Vec<Vec<[f32; 4]>>,
    pub normals: Vec<Vec<[f32; 4]>>,
    pub dirty: bool,
}

/// What an Also draw GLB copies from the avatar (the source's settings for it):
/// its movement and its rotation, each from the whole armature (each bone
/// from the avatar's of the same name) or from one bone (the whole GLB moves
/// or turns with it), and its shape keys. All off: it doesn't move at all.
#[derive(Clone, Debug, PartialEq)]
pub struct FollowSettings {
    pub move_on: bool,
    /// "" the armature, else a bone's name
    pub move_from: String,
    pub rotate_on: bool,
    pub rotate_from: String,
    pub shapes: bool,
}

impl Default for FollowSettings {
    fn default() -> Self {
        FollowSettings { move_on: true, move_from: String::new(), rotate_on: true, rotate_from: String::new(), shapes: true }
    }
}

impl Follower {
    pub fn new(model: Arc<Model>, main: &Model) -> Follower {
        let pose = Pose::rest(&model);
        let from = model.nodes.iter().map(|n| main.node(n.name.strip_prefix("mixamorig:").unwrap_or(&n.name))).collect();
        let morph_from = model.morph_names.iter().map(|n| main.morph(n)).collect();
        let arkit = arkit_keys(&model);
        let springs = Springs::new(&model, &pose);
        Follower {
            arkit,
            points: model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect(),
            normals: model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect(),
            morph: vec![0.0; model.morph_names.len()],
            rest: pose.clone(),
            pose,
            from,
            morph_from,
            springs,
            settled: false,
            dirty: false,
            model,
        }
    }

    /// Takes the avatar's pose (`main_pose`, from `main_rest`) and shape
    /// keys, then the ones driven on it (layer `layer`)
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn follow(&mut self, main: &Model, main_rest: &Pose, main_pose: &Pose, main_morph: &[f32], face: &Face, dt: f32, motion: &Motion, overrides: &[crate::remote::Override], layer: usize, how: &FollowSettings) {
        let model = self.model.clone();
        let pose = &mut self.pose;
        pose.reset(&model);
        // From one bone (a name the avatar has), or from the armature
        let bone = |name: &str| if name.trim().is_empty() { None } else { main.node(name.trim()) };
        let (move_bone, rotate_bone) = (bone(&how.move_from), bone(&how.rotate_from));
        let (copy_move, copy_rotate) = (how.move_on && move_bone.is_none(), how.rotate_on && rotate_bone.is_none());
        for (i, from) in self.from.iter().enumerate() {
            let Some(j) = *from else { continue };
            if j >= main.nodes.len() {
                continue;
            }
            // How the avatar's bone moved from its rest, on this one's rest
            if copy_rotate {
                pose.rotation[i] = (main_pose.rotation[j] * main_rest.rotation[j].inverse() * self.rest.rotation[i]).normalize();
            }
            if copy_move {
                pose.translation[i] = self.rest.translation[i] + (main_pose.translation[j] - main_rest.translation[j]);
            }
            if copy_move || copy_rotate {
                pose.scale[i] = self.rest.scale[i] * main_pose.scale[j] / main_rest.scale[j].max(Vec3::splat(1e-6));
            }
        }
        // The whole GLB with one bone: turned about it as it turned, moved as it moved
        let turned = rotate_bone.filter(|_| how.rotate_on);
        let moved = move_bone.filter(|_| how.move_on);
        if turned.is_some() || moved.is_some() {
            let (turn, pivot) = turned.map_or((Quat::IDENTITY, Vec3::ZERO), |b| (main_pose.world_rotation(b) * main_rest.world_rotation(b).inverse(), main_rest.position(b)));
            let shift = moved.map_or(Vec3::ZERO, |b| main_pose.position(b) - main_rest.position(b));
            for n in 0..model.nodes.len() {
                if model.nodes[n].parent.is_none() {
                    pose.rotation[n] = (turn * pose.rotation[n]).normalize();
                    pose.translation[n] = pivot + turn * (pose.translation[n] - pivot) + shift;
                }
            }
        }
        pose.update_world(&model);
        if motion.springs {
            if !self.settled {
                self.springs.reset(pose);
                self.settled = true;
            }
            self.springs.step(&model, pose, dt, &motion.adjust);
        } else {
            self.settled = false;
        }
        // Its keys: the avatar's of the same name, or (one the avatar doesn't
        // have) the face itself: tracked ARKit shapes, blinks, the microphone
        self.morph.iter_mut().for_each(|w| *w = 0.0);
        if how.shapes {
            for (k, from) in self.morph_from.iter().enumerate() {
                if let Some(m) = from {
                    self.morph[k] = main_morph.get(*m).copied().unwrap_or(0.0);
                }
            }
            for (i, slot) in self.arkit.iter().enumerate() {
                if let Some(k) = *slot {
                    if self.morph_from[k].is_none() {
                        self.morph[k] = self.morph[k].max(face.arkit[i]);
                    }
                }
            }
            for (name, w) in &face.named {
                if let Some(k) = model.morph(name).filter(|k| self.morph_from[*k].is_none()) {
                    self.morph[k] = self.morph[k].max(*w);
                }
            }
        }
        apply_overrides(&model, overrides, layer, &mut self.morph);
        pose::skin(&model, pose, &self.morph, &mut self.points, &mut self.normals);
        self.dirty = true;
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

    /// THEATER_PUPPET_ANIMATED_GLB=<an export with a clip "Wave" on RightArm>
    #[test]
    fn a_clip_from_blender_plays_and_gives_back_the_idle_pose() {
        let Ok(path) = std::env::var("THEATER_PUPPET_ANIMATED_GLB") else { return };
        let mut rig = Rig::new(Arc::new(crate::model::load(std::path::Path::new(&path)).unwrap()));
        let names: Vec<String> = rig.model.clips.iter().map(|c| c.name.clone()).collect();
        println!("clips: {names:?}");
        assert!(names.iter().any(|n| n == "Wave"));
        let config = tracking::config::Config::default();
        let hand = rig.model.node("RightHand").unwrap();
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        let idle = rig.pose.position(hand);
        let mut control = crate::animation::Control { play: Some("Wave".into()), ..Default::default() };
        let model = rig.model.clone();
        rig.player.sync(&model, &mut control);
        // Half way: the arm at its highest
        let mut highest = idle;
        for _ in 0..60 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
            let p = rig.pose.position(hand);
            if (p - idle).length() > (highest - idle).length() {
                highest = p;
            }
        }
        println!("right hand: idle {idle:?}, furthest {highest:?}");
        assert!((highest - idle).length() > 0.2, "the hand hardly moved");
        // Played once: back to the idle pose
        for _ in 0..120 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        assert!((rig.pose.position(hand) - idle).length() < 1e-3);
        rig.player.sync(&model, &mut control);
        assert_eq!(control.playing, "");

        // A shape key clip ("Smile": Fcl_MTH_Fun up and back, if the file has it)
        if let Some(smile) = names.iter().position(|n| *n == "Smile").map(|_| rig.model.morph("Fcl_MTH_Fun").unwrap()) {
            control.play = Some("Smile".into());
            rig.player.sync(&model, &mut control);
            let mut most = 0.0f32;
            for _ in 0..90 {
                rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
                most = most.max(rig.morph[smile]);
            }
            println!("Fcl_MTH_Fun at most {most}");
            assert!(most > 0.5, "the smile hardly showed ({most})");
            assert_eq!(rig.morph[smile], 0.0);
        }
    }

    #[test]
    fn a_glb_drawn_with_it_copies_its_pose_and_face() {
        let Some(mut rig) = rig() else { return };
        // The same avatar as its own follower: it must land exactly on it
        let same = rig.model.clone();
        rig.set_extras(&[same]);
        let config = tracking::config::Config { smoothing: 0.0, user_height: rig.skeleton.rest_head().y / 0.87, ..Default::default() };
        let place = Vec3::new(0.2, rig.skeleton.rest_head().y - 0.3, 0.3);
        let mut shapes = [0.0; 52];
        shapes[tracking::arkit_index("jawOpen").unwrap()] = 1.0;
        let tracked = Snapshot { left_hand: Some(Target { position: place, rotation: Quat::IDENTITY }), shapes: Some(shapes), ..Default::default() };
        let motion = Motion { yaw: 30.0, springs: false, ..Motion::default() };
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, motion, &tracked, &config);
        }
        let hand = rig.model.node("LeftHand").unwrap();
        let f = &rig.followers[0];
        assert!((f.pose.position(hand) - rig.pose.position(hand)).length() < 1e-4);
        assert_eq!(f.morph, rig.morph);
        let most = rig.points.iter().zip(&f.points).flat_map(|(a, b)| a.iter().zip(b).map(|(p, q)| (Vec3::from_slice(&p[..3]) - Vec3::from_slice(&q[..3])).length())).fold(0.0, f32::max);
        assert!(most < 1e-4, "a vertex is {most} off");

        // THEATER_PUPPET_EXTRA_GLB: the skeleton with a box held in the right
        // hand (bone-parented): the box goes where the hand goes
        if let Ok(path) = std::env::var("THEATER_PUPPET_EXTRA_GLB") {
            let boxed = Arc::new(crate::model::load(std::path::Path::new(&path)).unwrap());
            rig.set_extras(&[boxed]);
            let still_box = Motion { springs: false, idle: 0.0, ..Motion::default() };
            rig.step_with(1.0 / 60.0, still_box, &Snapshot::default(), &config);
            let centre = |pts: &Vec<Vec<[f32; 4]>>| pts.iter().flatten().fold(Vec3::ZERO, |a, p| a + Vec3::from_slice(&p[..3])) / pts.iter().map(Vec::len).sum::<usize>() as f32;
            let right = rig.model.node("RightHand").unwrap();
            let before = centre(&rig.followers[0].points) - rig.pose.position(right);
            let reach = Snapshot { right_hand: Some(Target { position: Vec3::new(-0.3, rig.skeleton.rest_head().y, 0.35), rotation: Quat::IDENTITY }), ..Default::default() };
            for _ in 0..30 {
                rig.step_with(1.0 / 60.0, still_box, &reach, &config);
            }
            let after = centre(&rig.followers[0].points) - rig.pose.position(right);
            println!("box from the hand: before {before:?}, after the hand moved {after:?}");
            assert!(before.length() < 0.1 && after.length() < 0.1, "the box isn't in the hand");
        }
    }

    #[test]
    fn shape_keys_driven_from_outside() {
        use crate::remote::{Drive, Shape};
        let Some(mut rig) = rig() else { return };
        let extra = rig.model.clone();
        rig.set_extras(&[extra]);
        let config = tracking::config::Config::default();
        let jaw = rig.arkit[tracking::arkit_index("jawOpen").unwrap()].unwrap();
        let key = rig.model.morph_names[jaw].clone();
        // On, on the avatar only; ping-pong on the extra GLB
        rig.drive_shapes(vec![(0, key.clone(), Some(Drive::Hold(0.8))), (1, key.clone(), Some(Drive::PingPong { top: 1.0, period: 1.0 }))]);
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        assert!((rig.morph[jaw] - 0.8).abs() < 1e-6);
        let f = rig.followers[0].model.morph(&key).unwrap();
        assert!((rig.followers[0].morph[f] - 1.0).abs() < 0.05, "half a period in: {}", rig.followers[0].morph[f]);
        // Off: back to what tracking gives (nothing)
        rig.drive_shapes(vec![(0, key.clone(), None)]);
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        assert_eq!(rig.morph[jaw], 0.0);
        // Once, up, half a second, then its GLB hidden
        rig.drive_shapes(vec![(1, key.clone(), Some(Drive::Once { from: 0.0, to: 1.0, duration: 0.5, shape: Shape::Up, hide_at_end: true, fade: 0.3 }))]);
        for _ in 0..15 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        assert!((rig.followers[0].morph[f] - 0.5).abs() < 0.05);
        assert!(rig.hide.is_empty());
        for _ in 0..20 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        assert_eq!(rig.hide, vec![(1, 0.3)]);
        assert!(rig.overrides.is_empty());
    }

    #[test]
    fn it_holds_the_controller() {
        use crate::tracking::gamepad::{Pad, button};
        let Some(mut rig) = rig() else { return };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("controller").join("gamepad.glb");
        rig.set_prop(Some(Arc::new(crate::model::load(&path).unwrap())));
        let config = tracking::config::Config { smoothing: 0.0, ..Default::default() };
        let held = Snapshot { controller: Some(Pad::default()), ..Default::default() };
        for _ in 0..40 {
            rig.step_with(1.0 / 60.0, still(), &held, &config);
        }
        let prop = rig.prop.as_ref().unwrap();
        assert_eq!(prop.weight, 1.0);
        let [l, r] = prop.hands();
        let at = |n: &str| rig.pose.position(rig.model.node(n).unwrap());
        // The wrists reach the controller's hand markers
        let (lh, rh) = (at("LeftHand"), at("RightHand"));
        println!("left wrist {lh:?} (marker {:?}), right {rh:?} (marker {:?}), chest {:?}", l.position, r.position, at("Spine1"));
        assert!((lh - l.position).length() < 0.03 && (rh - r.position).length() < 0.03, "the hands don't reach it");
        // In front of the body, below the shoulders, the elbows below the shoulders
        assert!(lh.z > at("Spine1").z + 0.1 && lh.y < at("LeftArm").y);
        assert!(at("LeftForeArm").y < at("LeftArm").y, "the left elbow is up");
        // Pressing A: the right thumb bends more
        let thumb = rig.model.node("RightHandThumb2").or(rig.model.node("RightHandThumb1")).unwrap();
        let rest_thumb = rig.pose.rotation[thumb];
        let pressed = Snapshot { controller: Some(Pad { buttons: button::A, ..Default::default() }), ..Default::default() };
        rig.step_with(1.0 / 60.0, still(), &pressed, &config);
        assert!(rig.pose.rotation[thumb].angle_between(rest_thumb) > 0.02);
        // Let go: it fades out of the hands
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        assert_eq!(rig.prop.as_ref().unwrap().weight, 0.0);
    }

    #[test]
    fn an_extra_glb_copies_what_its_settings_say() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let main = Arc::new(crate::model::load(std::path::Path::new(&path)).unwrap());
        let rest = Pose::rest(&main);
        // The avatar: its head turned, its hips moved
        let mut moved = rest.clone();
        let (head, hips) = (main.node("Head").unwrap(), main.node("Hips").unwrap());
        moved.rotation[head] = Quat::from_rotation_y(0.6) * moved.rotation[head];
        moved.translation[hips] += Vec3::new(0.1, 0.0, 0.0);
        moved.update_world(&main);
        let mut morph = vec![0.0; main.morph_names.len()];
        morph[0] = 1.0;
        let motion = Motion { springs: false, ..Motion::default() };
        let run = |how: FollowSettings| {
            let mut f = Follower::new(main.clone(), &main);
            f.follow(&main, &rest, &moved, &morph, &Face::default(), 1.0 / 60.0, &motion, &[], 1, &how);
            f
        };
        let close = |a: Vec3, b: Vec3| (a - b).length() < 1e-4;
        // All on (as always): its head and hips where the avatar's are
        let all = run(FollowSettings::default());
        assert!(close(all.pose.position(head), moved.position(head)) && close(all.pose.position(hips), moved.position(hips)));
        assert!(all.pose.world_rotation(head).angle_between(moved.world_rotation(head)) < 1e-3);
        assert_eq!(all.morph[0], 1.0);
        // All off: still, at rest
        let still = run(FollowSettings { move_on: false, rotate_on: false, shapes: false, ..Default::default() });
        assert!(close(still.pose.position(hips), rest.position(hips)) && close(still.pose.position(head), rest.position(head)));
        assert_eq!(still.morph[0], 0.0);
        // Rotation from the head only: the whole GLB turns about the head as it
        // turned (its own head bone stays at rest in it), nothing moves with the hips
        let turned = run(FollowSettings { move_on: false, rotate_from: "Head".into(), ..Default::default() });
        let turn = moved.world_rotation(head) * rest.world_rotation(head).inverse();
        assert!(turned.pose.world_rotation(head).angle_between(turn * rest.world_rotation(head)) < 1e-3);
        assert!(close(turned.pose.position(head), rest.position(head)), "turned about the head: it stays where it was");
        let feet = main.node("LeftFoot").unwrap();
        assert!(!close(turned.pose.position(feet), rest.position(feet)), "the rest of it swings round with the head");
        // Movement from the hips only: shifted with them, not turned
        let shifted = run(FollowSettings { rotate_on: false, move_from: "Hips".into(), ..Default::default() });
        assert!(close(shifted.pose.position(head), rest.position(head) + Vec3::new(0.1, 0.0, 0.0)));
        assert!(shifted.pose.world_rotation(head).angle_between(rest.world_rotation(head)) < 1e-3);

        // Keys the avatar doesn't have: the face drives them anyway (here the
        // "avatar" has no shape keys at all: the built-in gamepad)
        let bare = Arc::new(crate::model::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("controller").join("gamepad.glb")).unwrap());
        let bare_rest = Pose::rest(&bare);
        let mut face = Face::default();
        let jaw = tracking::arkit_index("jawOpen").unwrap();
        face.arkit[jaw] = 1.0;
        face.named.push(("Fcl_MTH_I".into(), 0.7));
        let mut f = Follower::new(main.clone(), &bare);
        f.follow(&bare, &bare_rest, &bare_rest, &[], &face, 1.0 / 60.0, &motion, &[], 1, &FollowSettings::default());
        let jaw_key = f.arkit[jaw].expect("the test avatar has a key for jawOpen");
        assert_eq!(f.morph[jaw_key], 1.0, "tracked jawOpen on {}", main.morph_names[jaw_key]);
        if let Some(i) = main.morph("Fcl_MTH_I") {
            assert!((f.morph[i] - 0.7).abs() < 1e-6, "the microphone's I");
        }
        // Copy shape keys off: none of it
        f.follow(&bare, &bare_rest, &bare_rest, &[], &face, 1.0 / 60.0, &motion, &[], 1, &FollowSettings { shapes: false, ..Default::default() });
        assert_eq!(f.morph[jaw_key], 0.0);
    }

    #[test]
    fn a_vr_input_copies_its_share_of_each_part() {
        let Some(base) = rig() else { return };
        let model = base.model.clone();
        let hips = model.node("Hips").unwrap();
        let head = model.node("Head").unwrap();
        // VR telemetry tracks the hips well off to the side, the head turned
        let tracked = Snapshot {
            hips: Some(Target { position: Vec3::new(0.25, 0.95, 0.1), rotation: Quat::IDENTITY }),
            head_turn: Some(Quat::from_rotation_y(0.8)),
            from: vec![(Part::LowerBody, tracking::Source::Telemetry), (Part::HeadTurn, tracking::Source::Telemetry)],
            ..Default::default()
        };
        let run = |share: f32| {
            let mut rig = Rig::new(model.clone());
            let mut config = tracking::config::Config { smoothing: 0.0, ..Default::default() };
            config.telemetry_feel.hips = share;
            config.telemetry_feel.head_turn = share;
            for _ in 0..40 {
                rig.step_with(1.0 / 60.0, still(), &tracked, &config);
            }
            (rig.pose.position(hips), rig.pose.world_rotation(head))
        };
        let (none, none_head) = run(0.0);
        let (half, half_head) = run(0.5);
        let (all, all_head) = run(1.0);
        let (moved_half, moved_all) = ((half - none).length(), (all - none).length());
        println!("hips moved: half {moved_half:.3} m, all {moved_all:.3} m");
        assert!(moved_all > 0.05, "the hips don't follow at all: {moved_all}");
        assert!(moved_half > moved_all * 0.25 && moved_half < moved_all * 0.75, "half isn't about half: {moved_half} of {moved_all}");
        let (turn_half, turn_all) = (half_head.angle_between(none_head), all_head.angle_between(none_head));
        println!("head turned: half {turn_half:.3}, all {turn_all:.3} rad");
        assert!(turn_all > 0.3 && turn_half > turn_all * 0.3 && turn_half < turn_all * 0.7);
        // Another input (not VR): all of it, whatever VR's feel says
        let mut config = tracking::config::Config { smoothing: 0.0, ..Default::default() };
        config.telemetry_feel.hips = 0.0;
        let other = Snapshot { from: vec![(Part::LowerBody, tracking::Source::MediaPipe)], ..tracked.clone() };
        let mut rig = Rig::new(model.clone());
        for _ in 0..40 {
            rig.step_with(1.0 / 60.0, still(), &other, &config);
        }
        assert!((rig.pose.position(hips) - all).length() < 0.02, "another input's hips are cut too");
    }

    #[test]
    fn no_avatar_only_extra_glbs() {
        // A source with only Also draw GLBs: an empty avatar, the extras still posed
        let mut rig = Rig::new(Arc::new(Model::empty()));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("controller").join("gamepad.glb");
        rig.set_extras(&[Arc::new(crate::model::load(&path).unwrap())]);
        rig.follow = vec![FollowSettings { move_on: false, rotate_on: false, shapes: false, ..Default::default() }];
        let config = tracking::config::Config::default();
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, Motion::default(), &Snapshot::default(), &config);
        }
        assert!(rig.followers[0].points.iter().flatten().all(|p| p.iter().all(|v| v.is_finite())));
        assert!(rig.followers[0].points.iter().any(|p| !p.is_empty()));
    }

    #[test]
    fn the_avatar_sits_at_its_desk() {
        use crate::tracking::desk::Desk;
        let Some(mut rig) = rig() else { return };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("controller").join("desk.glb");
        rig.set_prop(Some(Arc::new(crate::model::load(&path).unwrap())));
        assert!(rig.prop.as_ref().unwrap().is_desk());
        let config = tracking::config::Config { smoothing: 0.0, ..Default::default() };
        let at_desk = |d: Desk| Snapshot { desk: Some(d), ..Default::default() };
        let still_desk = Desk { mouse_idle: 10.0, keys_idle: 10.0, ..Default::default() };
        for _ in 0..40 {
            rig.step_with(1.0 / 60.0, still(), &at_desk(still_desk), &config);
        }
        let prop = rig.prop.as_ref().unwrap();
        assert_eq!(prop.weight, 1.0);
        let [l, r] = prop.hands();
        fn at_of(rig: &Rig, n: &str) -> Vec3 {
            rig.pose.position(rig.model.node(n).unwrap())
        }
        let (lh, rh) = (at_of(&rig, "LeftHand"), at_of(&rig, "RightHand"));
        println!("left wrist {lh:?} (target {:?}), right {rh:?} (target {:?}), chest {:?}", l.position, r.position, at_of(&rig, "Spine1"));
        // The wrists reach the keys and the mouse: in front, below the chest,
        // the right hand (on the mouse) farther right than the left (on WASD)
        assert!((lh - l.position).length() < 0.04 && (rh - r.position).length() < 0.04, "the hands don't reach the desk");
        assert!(lh.z > at_of(&rig, "Spine1").z + 0.1 && rh.z > at_of(&rig, "Spine1").z + 0.1);
        assert!(lh.y < at_of(&rig, "Spine1").y && rh.y < at_of(&rig, "Spine1").y);
        assert!(lh.x > rh.x + 0.15, "left on the keys (+X), right on the mouse");
        // W down (gaming, Area: it shows): the left middle finger bends
        let middle = rig.model.node("LeftHandMiddle1").unwrap();
        let rest_middle = rig.pose.rotation[middle];
        let mut w = still_desk;
        w.press(0x57);
        for _ in 0..3 {
            rig.step_with(1.0 / 60.0, still(), &at_desk(w), &config);
        }
        assert!(rig.pose.rotation[middle].angle_between(rest_middle) > 0.05);
        // The mouse moved right: the right hand follows it
        let before = at_of(&rig, "RightHand");
        for _ in 0..10 {
            rig.step_with(1.0 / 60.0, still(), &at_desk(Desk { mouse: [0.04, 0.0], ..still_desk }), &config);
        }
        assert!(before.x - at_of(&rig, "RightHand").x > 0.02, "{before:?} → {:?}", at_of(&rig, "RightHand"));
        // Off the desk: it fades
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        }
        assert_eq!(rig.prop.as_ref().unwrap().weight, 0.0);
    }

    #[test]
    fn the_microphone_opens_the_mouth() {
        use crate::lipsync::{Mode, Mouth, hear};
        let Some(mut rig) = rig() else { return };
        let config = tracking::config::Config::default();
        let a = rig.model.morph("Fcl_MTH_A").expect("a VRoid avatar");
        let o = rig.model.morph("Fcl_MTH_O").unwrap();
        // "A", fully open: its key opens
        hear(Mouth { open: 1.0, vowels: [1.0, 0.0, 0.0, 0.0, 0.0], ..Default::default() });
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        assert!((rig.morph[a] - 1.0).abs() < 1e-6 && rig.morph[o] == 0.0);
        // The filter's own key for A: that one opens instead
        let mut keys: [String; 5] = Default::default();
        keys[0] = "Fcl_MTH_O".into();
        hear(Mouth { open: 0.6, vowels: [1.0, 0.0, 0.0, 0.0, 0.0], keys, ..Default::default() });
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        assert!((rig.morph[o] - 0.6).abs() < 1e-6 && rig.morph[a] == 0.0);
        // ARKit: "A" is jawOpen (here, on a VRoid face, its stand-in key Fcl_MTH_A), at 0.9
        hear(Mouth { open: 1.0, vowels: [1.0, 0.0, 0.0, 0.0, 0.0], shapes: crate::lipsync::Shapes::Arkit, ..Default::default() });
        rig.step_with(1.0 / 60.0, still(), &Snapshot::default(), &config);
        assert!((rig.morph[a] - 0.9).abs() < 1e-6, "{}", rig.morph[a]);
        // Fallback with a tracked face: the microphone stays out of it
        hear(Mouth { open: 1.0, vowels: [1.0, 0.0, 0.0, 0.0, 0.0], mode: Mode::Fallback, ..Default::default() });
        let face = Snapshot { shapes: Some([0.0; 52]), ..Default::default() };
        for _ in 0..30 {
            rig.step_with(1.0 / 60.0, still(), &face, &config);
        }
        assert_eq!(rig.morph[a], 0.0);
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

