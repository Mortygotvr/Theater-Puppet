//! The IK skeleton routine: from what's tracked (tracking::Snapshot: where
//! the head and hands are, how the head is turned, how the fingers bend) to
//! the Mixamo bones' rotations.
//!
//! - The body follows the head: the hips move under it (leaning, crouching).
//! - The feet stay planted where they stood (or go where they're tracked),
//!   the legs bending to reach them.
//! - Each hand goes where it's tracked, the arm bending at the elbow (toward
//!   the tracked elbow when there is one, down and back otherwise).
//! - The head turns as tracked, the neck taking part of it.
//! - The fingers bend joint by joint.
//!
//! Positions are the avatar's own (metres, Y up, facing +Z, its left at +X);
//! a hand's or the head's turn is from how it is at rest (the T-pose).

use glam::{Quat, Vec3};

use crate::model::Model;
use crate::pose::Pose;
use crate::tracking::{Hand, Snapshot, Target};

/// One arm's or leg's bones
#[derive(Clone, Copy, Debug)]
struct Limb {
    root: usize,
    mid: usize,
    end: usize,
}

/// One hand's finger bones: thumb, index, middle, ring, little, three each
type Fingers = [[Option<usize>; 3]; 5];

/// The bones the routine turns, and how the avatar stands at rest
pub struct Skeleton {
    hips: Option<usize>,
    neck: Option<usize>,
    head: Option<usize>,
    arms: [Option<Limb>; 2],
    legs: [Option<Limb>; 2],
    fingers: [Fingers; 2],
    /// At rest, in the avatar's space
    rest_head: Vec3,
    rest_hips: Vec3,
    rest_feet: [Vec3; 2],
    rest_rotation: Vec<Quat>,
}

const FINGERS: [&str; 5] = ["Thumb", "Index", "Middle", "Ring", "Pinky"];

impl Skeleton {
    /// `rest` is the avatar's rest pose, its world matrices current
    pub fn new(model: &Model, rest: &Pose) -> Skeleton {
        let limb = |a: &str, b: &str, c: &str| Some(Limb { root: model.node(a)?, mid: model.node(b)?, end: model.node(c)? });
        let fingers = |side: &str| -> Fingers {
            std::array::from_fn(|f| std::array::from_fn(|j| model.node(&format!("{side}Hand{}{}", FINGERS[f], j + 1))))
        };
        let at = |n: Option<usize>| n.map(|i| rest.position(i)).unwrap_or(Vec3::ZERO);
        let legs = [limb("LeftUpLeg", "LeftLeg", "LeftFoot"), limb("RightUpLeg", "RightLeg", "RightFoot")];
        Skeleton {
            hips: model.node("Hips"),
            neck: model.node("Neck"),
            head: model.node("Head"),
            arms: [limb("LeftArm", "LeftForeArm", "LeftHand"), limb("RightArm", "RightForeArm", "RightHand")],
            legs,
            fingers: [fingers("Left"), fingers("Right")],
            rest_head: at(model.node("Head")),
            rest_hips: at(model.node("Hips")),
            rest_feet: [at(legs[0].map(|l| l.end)), at(legs[1].map(|l| l.end))],
            rest_rotation: (0..model.nodes.len()).map(|i| rest.world_rotation(i)).collect(),
        }
    }

    /// Where the head is at rest: what tracked heights are measured against
    pub fn rest_head(&self) -> Vec3 {
        self.rest_head
    }

    /// Where a shoulder is at rest (0 left, 1 right)
    pub fn rest_shoulder(&self, rest: &Pose, side: usize) -> Option<Vec3> {
        self.arms[side].map(|a| rest.position(a.root))
    }

    /// Poses the body to `tracked`. `pose` already has the idle motion, its
    /// world current; it's left current. `head_strength`: how much of the
    /// head's turn is made (1 = all).
    pub fn apply(&self, model: &Model, pose: &mut Pose, tracked: &Snapshot, head_strength: f32) {
        // The body under the head: the hips move as the head does
        if let (Some(hips), Some(head)) = (self.hips, tracked.head_position) {
            let mut shift = head - self.rest_head;
            // Not through the floor, nor off its legs
            let legs = self.rest_hips.y - self.rest_feet[0].y.min(self.rest_feet[1].y);
            shift.y = shift.y.clamp(-legs * 0.6, legs * 0.08);
            move_in_world(model, pose, hips, shift);
        }
        if let (Some(hips), Some(t)) = (self.hips, tracked.hips) {
            let here = pose.position(hips);
            move_in_world(model, pose, hips, t.position - here);
            self.turn_to(model, pose, hips, t.rotation);
        }
        // The feet: tracked, or planted where they stood at rest
        let moved = tracked.head_position.is_some() || tracked.hips.is_some();
        for side in 0..2 {
            let Some(leg) = self.legs[side] else { continue };
            let foot = if side == 0 { tracked.left_foot } else { tracked.right_foot };
            match foot {
                Some(t) => {
                    two_bone(model, pose, leg, t.position, pose.position(leg.mid) + Vec3::Z);
                    self.turn_to(model, pose, leg.end, t.rotation);
                }
                // Only when the body moved: the idle pose's legs are fine otherwise
                None if moved => {
                    two_bone(model, pose, leg, self.rest_feet[side], pose.position(leg.mid) + Vec3::Z);
                    self.turn_to(model, pose, leg.end, Quat::IDENTITY);
                }
                None => {}
            }
        }
        // The head's turn, the neck taking a third of it
        if let Some(turn) = tracked.head_turn {
            let turn = Quat::IDENTITY.slerp(turn, head_strength.clamp(0.0, 1.5));
            // (in the body's frame: a turned body's head turns from there)
            let body = self.hips.map(|h| pose.world_rotation(h) * self.rest_rotation[h].inverse()).unwrap_or(Quat::IDENTITY);
            if let Some(neck) = self.neck {
                let part = Quat::IDENTITY.slerp(turn, 0.35);
                self.turn_to(model, pose, neck, body * part);
            }
            if let Some(head) = self.head {
                self.turn_to(model, pose, head, body * turn);
            }
        }
        // The hands
        for side in 0..2 {
            let Some(arm) = self.arms[side] else { continue };
            let (hand, elbow) = if side == 0 { (tracked.left_hand, tracked.left_elbow) } else { (tracked.right_hand, tracked.right_elbow) };
            let Some(Target { position, rotation }) = hand else { continue };
            // The elbow: where it's tracked, or down, a little back and out
            let out = if side == 0 { 1.0 } else { -1.0 };
            let pole = elbow.unwrap_or(pose.position(arm.root) + Vec3::new(0.25 * out, -1.0, -0.5));
            two_bone(model, pose, arm, position, pole);
            self.turn_to(model, pose, arm.end, rotation);
        }
        // The fingers
        for (side, hand) in [tracked.left_fingers, tracked.right_fingers].into_iter().enumerate() {
            let (Some(hand), Some(arm)) = (hand, self.arms[side]) else { continue };
            self.bend_fingers(model, pose, side, arm.end, &hand);
        }
        pose.update_world(model);
    }

    /// Turns `node` to `turn` from how it is at rest (in the avatar's space)
    fn turn_to(&self, model: &Model, pose: &mut Pose, node: usize, turn: Quat) {
        let world = turn * self.rest_rotation[node];
        let parent = pose.parent_rotation(model, node);
        pose.rotation[node] = (parent.inverse() * world).normalize();
        pose.update_world(model);
    }

    /// Bends a hand's fingers: each joint about the hand's own knuckle axis
    fn bend_fingers(&self, model: &Model, pose: &mut Pose, side: usize, hand_bone: usize, hand: &Hand) {
        // How the hand is turned from rest: its axes turn with it
        let hand_turn = pose.world_rotation(hand_bone) * self.rest_rotation[hand_bone].inverse();
        // At rest (T-pose) the fingers point along X and the palm faces
        // down: they curl about Z (the tips going down), and spread about Y.
        // The thumb lies across the palm: it curls about Y.
        let s = if side == 0 { 1.0 } else { -1.0 };
        for f in 0..5 {
            let curl_axis = if f == 0 { Vec3::Y * s } else { Vec3::NEG_Z * s };
            let spread_axis = if f == 0 { Vec3::NEG_Z * s } else { Vec3::Y * s };
            let spread = Quat::from_axis_angle(spread_axis, hand.spread[f]);
            // Every joint curls about the same axis: a joint's turn from
            // rest is the bends up to it, added
            let mut bent = 0.0;
            for j in 0..3 {
                let Some(bone) = self.fingers[side][f][j] else { continue };
                bent += hand.bend[f][j];
                let world = hand_turn * spread * Quat::from_axis_angle(curl_axis, bent) * self.rest_rotation[bone];
                pose.rotation[bone] = (pose.parent_rotation(model, bone).inverse() * world).normalize();
                pose.update_node(model, bone);
            }
        }
    }
}

/// Moves `node` by `by` (in the avatar's space), its parent's world current
fn move_in_world(model: &Model, pose: &mut Pose, node: usize, by: Vec3) {
    let parent = model.nodes[node].parent.map(|p| pose.world[p]).unwrap_or(glam::Mat4::IDENTITY);
    pose.translation[node] += parent.inverse().transform_vector3(by);
    pose.update_world(model);
}

/// Two-bone IK: turns `limb`'s root and middle bones so its end reaches
/// `target` (as near as its length allows), the middle joint bending toward
/// `pole`. The world is left current.
fn two_bone(model: &Model, pose: &mut Pose, limb: Limb, target: Vec3, pole: Vec3) {
    let (a, b, c) = (pose.position(limb.root), pose.position(limb.mid), pose.position(limb.end));
    let (upper, lower) = ((b - a).length(), (c - b).length());
    if upper < 1e-5 || lower < 1e-5 {
        return;
    }
    let reach = target - a;
    let distance = reach.length().clamp((upper - lower).abs() + 1e-4, upper + lower - 1e-4);
    let along = reach.normalize_or(Vec3::NEG_Y);
    // The middle joint: on the circle where both bones meet, on the pole's side
    let x = (upper * upper - lower * lower + distance * distance) / (2.0 * distance);
    let h = (upper * upper - x * x).max(0.0).sqrt();
    let toward = pole - a;
    let mut side = toward - along * toward.dot(along);
    if side.length_squared() < 1e-8 {
        let bent = b - a;
        side = bent - along * bent.dot(along);
    }
    let side = side.normalize_or(along.any_orthonormal_vector());
    let new_mid = a + along * x + side * h;
    let new_end = a + along * distance;

    pose.rotate_in_world(model, limb.root, Quat::from_rotation_arc((b - a).normalize(), (new_mid - a).normalize()));
    pose.update_world(model);
    let (mid, end) = (pose.position(limb.mid), pose.position(limb.end));
    pose.rotate_in_world(model, limb.mid, Quat::from_rotation_arc((end - mid).normalize_or(Vec3::Y), (new_end - mid).normalize_or(Vec3::Y)));
    pose.update_world(model);
}

/// A hand's turn from rest, from where its fingers point and its palm
/// faces (the avatar's space): at rest the left hand's fingers point +X,
/// the right's -X, both palms face down.
pub fn hand_turn(left: bool, fingers: Vec3, palm: Vec3) -> Quat {
    let rest_x = if left { Vec3::X } else { Vec3::NEG_X };
    let x = fingers.normalize_or(rest_x);
    // The palm's direction squared up to the fingers'
    let y = (palm - x * palm.dot(x)).normalize_or(Vec3::NEG_Y);
    let from = glam::Mat3::from_cols(rest_x, Vec3::NEG_Y, rest_x.cross(Vec3::NEG_Y));
    let to = glam::Mat3::from_cols(x, y, x.cross(y));
    Quat::from_mat3(&(to * from.transpose())).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> Option<(Model, Pose, Skeleton)> {
        let path = std::env::var("THEATER_PUPPET_GLB").ok()?;
        let model = crate::model::load(std::path::Path::new(&path)).unwrap();
        let pose = Pose::rest(&model);
        let skeleton = Skeleton::new(&model, &pose);
        Some((model, pose, skeleton))
    }

    #[test]
    fn hand_turns_from_rest() {
        // At rest: no turn
        assert!(hand_turn(true, Vec3::X, Vec3::NEG_Y).angle_between(Quat::IDENTITY) < 1e-3);
        assert!(hand_turn(false, Vec3::NEG_X, Vec3::NEG_Y).angle_between(Quat::IDENTITY) < 1e-3);
        // The left hand pointing forward, palm down: a quarter turn about Y
        let q = hand_turn(true, Vec3::Z, Vec3::NEG_Y);
        assert!((q * Vec3::X - Vec3::Z).length() < 1e-3 && (q * Vec3::NEG_Y - Vec3::NEG_Y).length() < 1e-3);
        // Palm up: the palm's direction follows
        let q = hand_turn(true, Vec3::X, Vec3::Y);
        assert!((q * Vec3::NEG_Y - Vec3::Y).length() < 1e-3);
    }

    /// THEATER_PUPPET_GLB=<a Rig export> cargo test -- --nocapture
    #[test]
    fn hands_reach_their_targets() {
        let Some((model, mut pose, skeleton)) = rig() else { return };
        let arm = skeleton.arms[0].unwrap();
        let shoulder = pose.position(arm.root);
        // In front of the chest, palm facing in
        let target = Target { position: shoulder + Vec3::new(-0.05, -0.15, 0.3), rotation: hand_turn(true, Vec3::Z, Vec3::NEG_X) };
        let tracked = Snapshot { left_hand: Some(target), ..Default::default() };
        skeleton.apply(&model, &mut pose, &tracked, 1.0);
        let hand = pose.position(arm.end);
        println!("left hand {hand:?}, target {:?}, elbow {:?}", target.position, pose.position(arm.mid));
        assert!((hand - target.position).length() < 0.005, "missed by {}", (hand - target.position).length());
        // The elbow bends down and back, not up
        assert!(pose.position(arm.mid).y < shoulder.y);
        // The hand is turned as asked: its fingers point forward
        let middle = model.node("LeftHandMiddle1").unwrap();
        let fingers = (pose.position(middle) - hand).normalize();
        assert!(fingers.z > 0.8, "fingers point {fingers:?}");
        // Bone lengths are kept
        let rest = Pose::rest(&model);
        let length = |p: &Pose, a: usize, b: usize| (p.position(a) - p.position(b)).length();
        assert!((length(&pose, arm.root, arm.mid) - length(&rest, arm.root, arm.mid)).abs() < 1e-4);

        // Out of reach: the arm straightens toward it
        let far = Target { position: shoulder + Vec3::new(3.0, 0.0, 0.0), rotation: Quat::IDENTITY };
        let mut pose = Pose::rest(&model);
        skeleton.apply(&model, &mut pose, &Snapshot { left_hand: Some(far), ..Default::default() }, 1.0);
        let reach = (pose.position(arm.end) - shoulder).length();
        let full = length(&rest, arm.root, arm.mid) + length(&rest, arm.mid, arm.end);
        assert!((reach - full).abs() < 0.002 && pose.position(arm.end).is_finite());
    }

    #[test]
    fn crouching_keeps_the_feet_planted() {
        let Some((model, mut pose, skeleton)) = rig() else { return };
        let (left, right) = (skeleton.legs[0].unwrap(), skeleton.legs[1].unwrap());
        let feet = [pose.position(left.end), pose.position(right.end)];
        let knee = pose.position(left.mid);
        // The head 25 cm lower and a little to the side
        let tracked = Snapshot { head_position: Some(skeleton.rest_head() + Vec3::new(0.05, -0.25, 0.0)), ..Default::default() };
        skeleton.apply(&model, &mut pose, &tracked, 1.0);
        let head = model.node("Head").unwrap();
        println!("head {:?} (rest {:?}), knee {:?} (rest {knee:?})", pose.position(head), skeleton.rest_head(), pose.position(left.mid));
        assert!((pose.position(head) - (skeleton.rest_head() + Vec3::new(0.05, -0.25, 0.0))).length() < 0.01);
        assert!((pose.position(left.end) - feet[0]).length() < 0.01 && (pose.position(right.end) - feet[1]).length() < 0.01);
        // The knees bend forward
        assert!(pose.position(left.mid).z > knee.z + 0.05);
    }

    #[test]
    fn the_head_turns_and_fingers_curl() {
        let Some((model, mut pose, skeleton)) = rig() else { return };
        let head = model.node("Head").unwrap();
        let turn = Quat::from_rotation_y(40f32.to_radians());
        skeleton.apply(&model, &mut pose, &Snapshot { head_turn: Some(turn), ..Default::default() }, 1.0);
        let now = pose.world_rotation(head) * skeleton.rest_rotation[head].inverse();
        assert!(now.angle_between(turn) < 0.02, "head off by {}", now.angle_between(turn));

        // A fist: the fingertips come toward the palm (down, at rest)
        let mut pose = Pose::rest(&model);
        let tip = model.node("LeftHandIndex3").unwrap();
        let before = pose.position(tip);
        let fist = Hand { bend: [[1.2; 3]; 5], spread: [0.0; 5] };
        skeleton.apply(&model, &mut pose, &Snapshot { left_fingers: Some(fist), ..Default::default() }, 1.0);
        let after = pose.position(tip);
        println!("index tip {before:?} -> {after:?}");
        assert!(after.y < before.y - 0.02 && after.x < before.x, "the index didn't curl toward the palm");
    }
}
