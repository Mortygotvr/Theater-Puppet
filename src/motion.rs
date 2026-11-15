//! The avatar alive: its pose every frame (arms down out of the T-pose,
//! breathing, a little sway and head motion, the Turn setting eased in so
//! the hair swings after it), its spring bones, and the mesh skinned to it.
//! Runs in the source's video_tick; the GPU takes the vertices in its render.

use std::sync::Arc;

use glam::Quat;

use crate::model::Model;
use crate::pose::{self, Pose};
use crate::springs::Springs;

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
}

impl Default for Motion {
    fn default() -> Self {
        Motion { arms_down: 65.0, idle: 0.5, yaw: 0.0, springs: true }
    }
}

pub struct Rig {
    pub model: Arc<Model>,
    pose: Pose,
    springs: Springs,
    time: f32,
    /// The turn the body has now, easing toward the setting
    yaw: f32,
    settled: bool,
    pub points: Vec<Vec<[f32; 4]>>,
    pub normals: Vec<Vec<[f32; 4]>>,
    /// New vertices since the GPU last took them
    pub dirty: bool,
}

/// The Mixamo bones the motion turns (None: the avatar hasn't that one)
struct Bones {
    hips: Option<usize>,
    spine1: Option<usize>,
    neck: Option<usize>,
    head: Option<usize>,
    left_arm: Option<usize>,
    right_arm: Option<usize>,
}

impl Rig {
    pub fn new(model: Arc<Model>) -> Rig {
        let pose = Pose::rest(&model);
        let springs = Springs::new(&model, &pose);
        crate::log(&format!("[Puppet] 🪢 Springs: {} chain(s), {} bone(s), {} collider(s)", springs.chains, springs.joints(), springs.colliders.len()));
        let points = model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect();
        let normals = model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect();
        Rig { model, pose, springs, time: 0.0, yaw: f32::NAN, settled: false, points, normals, dirty: false }
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
        }
    }

    /// One frame: `dt` seconds later
    pub fn step(&mut self, dt: f32, motion: Motion) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        // The body eases into a new turn (a jump of the setting swings the hair)
        if self.yaw.is_nan() {
            self.yaw = motion.yaw;
        }
        self.yaw += (motion.yaw - self.yaw) * (dt * 6.0).min(1.0);

        let model = self.model.clone();
        let bones = self.bones();
        let (t, idle) = (self.time, motion.idle.clamp(0.0, 1.0));
        let deg = |d: f32| d.to_radians();
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
        // The head looks around a little
        if let Some(h) = bones.head.or(bones.neck) {
            let look = Quat::from_rotation_y(deg((t * 0.7 + 1.0).sin() * 6.0 * idle))
                * Quat::from_rotation_x(deg((t * 0.5).sin() * 3.0 * idle))
                * Quat::from_rotation_z(deg((t * 0.9 + 2.0).sin() * 2.0 * idle));
            pose.rotate_in_world(&model, h, look);
            pose.update_world(&model);
        }
        // Arms down from the T-pose (the avatar faces +Z, its left is +X)
        let arms = motion.arms_down + (t * 0.8).sin() * 1.5 * idle;
        let turn = Quat::from_rotation_y(deg(self.yaw));
        if let Some(a) = bones.left_arm {
            pose.rotate_in_world(&model, a, turn * Quat::from_rotation_z(-deg(arms)) * turn.inverse());
        }
        if let Some(a) = bones.right_arm {
            pose.rotate_in_world(&model, a, turn * Quat::from_rotation_z(deg(arms)) * turn.inverse());
        }
        pose.update_world(&model);

        if motion.springs {
            // A new avatar (or springs turned back on): start at rest, still
            if !self.settled {
                self.springs.reset(pose);
                self.settled = true;
            }
            self.springs.step(&model, pose, dt, 1.0);
        } else {
            self.settled = false;
        }
        pose::skin(&model, pose, &mut self.points, &mut self.normals);
        self.dirty = true;
    }
}
