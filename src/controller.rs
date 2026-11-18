//! The controller the avatar holds while its Arms are on the Controller input
//! (tracking/gamepad.rs): a GLB (the built-in controller/gamepad.glb, or the
//! source's own) placed in front of the avatar, its hands on it, its fingers
//! on the buttons, its sticks, triggers and buttons moving as the real ones do.
//!
//! The GLB's parts are found by name (controller/make_gamepad.py says which).
//! Its axes are the avatar's: +X the holder's left, +Y up, +Z away from the
//! holder; metres, a real controller's size (scaled with the avatar).

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

use crate::model::Model;
use crate::pose::{self, Pose};
use crate::tracking::gamepad::{Pad, button};
use crate::tracking::{Hand, Target};

/// Seconds it takes to come into the hands, and to go
const FADE: f32 = 0.3;

/// Its parts, by node
struct Parts {
    /// Where the wrists go (left, right)
    hands: [Option<usize>; 2],
    sticks: [Option<usize>; 2],
    triggers: [Option<usize>; 2],
    dpad: Option<usize>,
    /// Buttons that sink while pressed, and which
    buttons: Vec<(usize, u32)>,
}

pub struct Prop {
    pub model: Arc<Model>,
    pose: Pose,
    rest: Pose,
    parts: Parts,
    morph: Vec<f32>,
    pub points: Vec<Vec<[f32; 4]>>,
    pub normals: Vec<Vec<[f32; 4]>>,
    pub dirty: bool,
    /// How much it's held (0–1, easing in and out)
    pub weight: f32,
    /// Where it is this frame, in the avatar's space
    transform: Mat4,
    turn: Quat,
    size: f32,
}

impl Prop {
    pub fn new(model: Arc<Model>) -> Prop {
        let pose = Pose::rest(&model);
        let node = |n: &str| model.nodes.iter().position(|x| x.name == n);
        let parts = Parts {
            hands: [node("hand_left"), node("hand_right")],
            sticks: [node("stick_left"), node("stick_right")],
            triggers: [node("trigger_left"), node("trigger_right")],
            dpad: node("dpad"),
            buttons: [
                ("button_a", button::A),
                ("button_b", button::B),
                ("button_x", button::X),
                ("button_y", button::Y),
                ("bumper_left", button::LB),
                ("bumper_right", button::RB),
                ("button_back", button::BACK),
                ("button_start", button::START),
            ]
            .into_iter()
            .filter_map(|(n, b)| Some((node(n)?, b)))
            .collect(),
        };
        Prop {
            points: model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect(),
            normals: model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect(),
            morph: vec![0.0; model.morph_names.len()],
            rest: pose.clone(),
            pose,
            parts,
            dirty: false,
            weight: 0.0,
            transform: Mat4::IDENTITY,
            turn: Quat::IDENTITY,
            size: 1.0,
            model,
        }
    }

    /// `dt` seconds later, held or not
    pub fn fade(&mut self, held: bool, dt: f32) {
        let d = dt / FADE;
        self.weight = if held { (self.weight + d).min(1.0) } else { (self.weight - d).max(0.0) };
    }

    /// Where it is: bound to the chest (`chest`: where it is, `chest_turn`:
    /// how it's turned from rest, the avatar's space), in front of it and a
    /// little lower, `offset` (up, forward; metres) on top; tilted toward the
    /// face, swaying a little with the left stick. `size`: the avatar's scale,
    /// 1 for a grown-up
    pub fn place(&mut self, chest: Vec3, chest_turn: Quat, size: f32, offset: [f32; 2], pad: &Pad) {
        let tilt = Quat::from_rotation_x(-0.45) * Quat::from_rotation_z(pad.left_stick[0] * 0.05) * Quat::from_rotation_x(pad.left_stick[1] * 0.04);
        let at = chest + chest_turn * (Vec3::new(0.0, -0.12 + offset[0], 0.32 + offset[1]) * size);
        self.turn = chest_turn * tilt;
        self.size = size;
        self.transform = Mat4::from_scale_rotation_translation(Vec3::splat(size), self.turn, at);
    }

    /// The wrists' targets (left, right): on the hand markers, the fingers
    /// forward and in, the palms around the grips
    pub fn hands(&self) -> [Target; 2] {
        [0, 1].map(|side| {
            let left = side == 0;
            let s = if left { 1.0 } else { -1.0 };
            let marker = self.parts.hands[side].map(|i| self.rest.position(i)).unwrap_or(Vec3::new(0.072 * s, -0.02, -0.075));
            let fingers = self.turn * Vec3::new(-0.45 * s, -0.1, 1.0);
            let palm = self.turn * Vec3::new(-s, -0.35, 0.0);
            Target { position: self.transform.transform_point3(marker), rotation: crate::ik::hand_turn(left, fingers, palm) }
        })
    }

    /// Its parts as the controller is now, posed in the avatar's space, skinned
    pub fn pose_parts(&mut self, pad: &Pad) {
        let model = self.model.clone();
        let pose = &mut self.pose;
        pose.reset(&model);
        // Sticks: pushed away tilts the top away (+Z), pushed right toward -X
        for (side, stick) in [pad.left_stick, pad.right_stick].into_iter().enumerate() {
            if let Some(n) = self.parts.sticks[side] {
                pose.rotation[n] = self.rest.rotation[n] * Quat::from_rotation_x(stick[1] * 0.35) * Quat::from_rotation_z(stick[0] * 0.35);
            }
        }
        // Triggers: pulled toward the holder about their hinge
        for (side, t) in [pad.left_trigger, pad.right_trigger].into_iter().enumerate() {
            if let Some(n) = self.parts.triggers[side] {
                pose.rotation[n] = self.rest.rotation[n] * Quat::from_rotation_x(t * 0.4);
            }
        }
        if let Some(n) = self.parts.dpad {
            let (x, y) = (pad.pressed(button::LEFT) as i32 - pad.pressed(button::RIGHT) as i32, pad.pressed(button::UP) as i32 - pad.pressed(button::DOWN) as i32);
            pose.rotation[n] = self.rest.rotation[n] * Quat::from_rotation_x(y as f32 * -0.15) * Quat::from_rotation_z(x as f32 * -0.15);
        }
        for &(n, b) in &self.parts.buttons {
            if pad.pressed(b) {
                pose.translation[n] = self.rest.translation[n] - Vec3::Y * 0.002;
            }
        }
        pose.update_world(&model);
        // Then all of it where it's held
        for w in pose.world.iter_mut() {
            *w = self.transform * *w;
        }
        pose::skin(&model, pose, &self.morph, &mut self.points, &mut self.normals);
        self.dirty = true;
    }
}

/// The fingers holding the controller, pressing what's pressed (left, right)
pub fn fingers(pad: &Pad) -> [Hand; 2] {
    // Around the grip: middle, ring and little fingers closed
    let grip = |h: &mut Hand| {
        h.bend[2] = [1.0, 1.2, 0.8];
        h.bend[3] = [1.1, 1.25, 0.85];
        h.bend[4] = [1.2, 1.3, 0.9];
    };
    // The index on its trigger (pulled with it), or up on its bumper
    let index = |h: &mut Hand, trigger: f32, bumper: bool| {
        h.bend[1] = if bumper { [0.15, 0.2, 0.1] } else { [0.55 + trigger * 0.35, 0.6 + trigger * 0.45, 0.45 + trigger * 0.3] };
    };
    let moved = |s: [f32; 2]| s[0].abs() + s[1].abs() > 0.25;

    let mut left = Hand::default();
    grip(&mut left);
    index(&mut left, pad.left_trigger, pad.pressed(button::LB));
    // The left thumb: on the stick, down on the d-pad when it's pressed
    let dpad = pad.buttons & (button::UP | button::DOWN | button::LEFT | button::RIGHT) != 0;
    left.bend[0] = if dpad { [0.55, 0.5, 0.45] } else { [0.35, 0.35 - pad.left_stick[1] * 0.15, 0.25] };
    left.spread[0] = if dpad { -0.12 } else { pad.left_stick[0] * 0.1 };

    let mut right = Hand::default();
    grip(&mut right);
    index(&mut right, pad.right_trigger, pad.pressed(button::RB));
    // The right thumb: over ABXY, reaching for the one pressed; on the right stick when it's moved
    let (bend, spread) = if pad.pressed(button::A) {
        ([0.55, 0.5, 0.45], 0.0)
    } else if pad.pressed(button::Y) {
        ([0.2, 0.25, 0.3], 0.0)
    } else if pad.pressed(button::X) {
        ([0.4, 0.4, 0.4], 0.12)
    } else if pad.pressed(button::B) {
        ([0.4, 0.4, 0.4], -0.12)
    } else if moved(pad.right_stick) || pad.pressed(button::RS) {
        ([0.6, 0.55 - pad.right_stick[1] * 0.15, 0.4], 0.15 - pad.right_stick[0] * 0.1)
    } else {
        ([0.3, 0.35, 0.25], 0.0)
    };
    right.bend[0] = bend;
    right.spread[0] = spread;
    [left, right]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gamepad() -> Option<Arc<Model>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("controller").join("gamepad.glb");
        path.is_file().then(|| Arc::new(crate::model::load(&path).unwrap()))
    }

    #[test]
    fn the_built_in_controller_has_its_parts() {
        let Some(model) = gamepad() else { return };
        let p = Prop::new(model);
        assert!(p.parts.hands.iter().all(Option::is_some));
        assert!(p.parts.sticks.iter().all(Option::is_some));
        assert!(p.parts.triggers.iter().all(Option::is_some));
        assert!(p.parts.dpad.is_some());
        assert_eq!(p.parts.buttons.len(), 8);
    }

    #[test]
    fn it_sits_in_front_and_the_hands_go_on_its_markers() {
        let Some(model) = gamepad() else { return };
        let mut p = Prop::new(model);
        let pad = Pad::default();
        p.place(Vec3::new(0.0, 1.25, 0.0), Quat::IDENTITY, 1.0, [0.0, 0.0], &pad);
        let [l, r] = p.hands();
        // In front of the chest, the left hand on the left (+X), both lower than the chest
        assert!(l.position.z > 0.1 && r.position.z > 0.1, "{l:?} {r:?}");
        assert!(l.position.x > 0.04 && r.position.x < -0.04);
        assert!(l.position.y < 1.25 && r.position.y < 1.25);
        // Turned half round: in front of the turned body (-Z)
        p.place(Vec3::new(0.0, 1.25, 0.0), Quat::from_rotation_y(std::f32::consts::PI), 1.0, [0.0, 0.0], &pad);
        assert!(p.hands()[0].position.z < -0.1);
        // Pressing A sinks it, pulling the right trigger turns it
        let before = { p.pose_parts(&pad); p.points.clone() };
        let pressed = Pad { buttons: button::A, right_trigger: 1.0, ..Default::default() };
        p.pose_parts(&pressed);
        let moved = before.iter().zip(&p.points).filter(|(a, b)| a != b).count();
        assert!(moved >= 2, "{moved} parts moved");
    }

    #[test]
    fn the_fingers_press() {
        let rest = fingers(&Pad::default());
        let a = fingers(&Pad { buttons: button::A, ..Default::default() });
        assert!(a[1].bend[0][0] > rest[1].bend[0][0]);
        let lt = fingers(&Pad { left_trigger: 1.0, ..Default::default() });
        assert!(lt[0].bend[1][1] > rest[0].bend[1][1]);
        let lb = fingers(&Pad { buttons: button::LB, ..Default::default() });
        assert!(lb[0].bend[1][1] < rest[0].bend[1][1]);
    }
}
