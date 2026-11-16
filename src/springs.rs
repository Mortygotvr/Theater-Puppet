//! Spring bones: hair, skirt, bust… swinging after the body, VRM's way (each
//! joint's tail keeps its inertia, springs back toward its rest direction,
//! falls with gravity, keeps its length, and is pushed out of colliders).
//!
//! The chains are Theater Rig's: bones named spring_<kind>_<n>_<i>, the
//! settings on each chain's root (_0) as extras: spring_stiffness,
//! spring_gravity, spring_drag, spring_radius. Colliders aren't in the file:
//! capsules along the Mixamo bones (head, neck, spine, shoulders, arms, hands,
//! legs), each as
//! thick as the mesh those bones move.

use glam::{Quat, Vec3};

use crate::model::Model;
use crate::pose::Pose;

/// A leaf joint's tail, past its end (VRM's)
const LEAF_LENGTH: f32 = 0.07;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub stiffness: f32,
    pub gravity: f32,
    pub drag: f32,
    pub radius: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { stiffness: 1.0, gravity: 0.0, drag: 0.4, radius: 0.02 }
    }
}

struct Joint {
    node: usize,
    /// Its chain's kind (hair, skirt…: an index into KINDS)
    kind: usize,
    /// Toward the tail, in the node's own space at rest
    axis: Vec3,
    length: f32,
    rest_rotation: Quat,
    settings: Settings,
    tail: Vec3,
    prev_tail: Vec3,
}

/// Theater Rig's spring kinds (spring_<kind>_…); anything else is "misc"
pub const KINDS: [&str; 8] = ["hair", "bust", "skirt", "tail", "ear", "ribbon", "cloth", "misc"];

/// The source's adjustments: multipliers on what the avatar's file says
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    pub stiffness: f32,
    pub gravity: f32,
    pub drag: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { stiffness: 1.0, gravity: 1.0, drag: 1.0 }
    }
}

/// Each kind's tuning (KINDS' order), and how thick the colliders are
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adjust {
    pub kinds: [Tuning; KINDS.len()],
    pub colliders: f32,
}

impl Default for Adjust {
    fn default() -> Self {
        Adjust { kinds: [Tuning::default(); KINDS.len()], colliders: 1.0 }
    }
}

/// A capsule from a bone to a point that moves with it (its child bone, or
/// the middle of the mesh it moves: the head has no end bone)
#[derive(Clone, Copy, Debug)]
pub struct Collider {
    pub from: usize,
    /// The capsule's near end, in the bone's own space (0: at the bone)
    pub base: Vec3,
    /// The capsule's far end, in the bone's own space
    pub tip: Vec3,
    pub radius: f32,
}

impl Collider {
    fn ends(&self, pose: &Pose) -> (Vec3, Vec3) {
        let (at, turn) = (pose.position(self.from), pose.world_rotation(self.from));
        (at + turn * self.base, at + turn * self.tip)
    }
}

pub struct Springs {
    joints: Vec<Joint>,
    pub colliders: Vec<Collider>,
    pub chains: usize,
}

/// `spring_<kind>_<n>_<i>` -> (chain "spring_<kind>_<n>", i)
fn spring_name(name: &str) -> Option<(&str, u32)> {
    let rest = name.strip_prefix("spring_")?;
    let (chain, i) = name.rsplit_once('_')?;
    let i: u32 = i.parse().ok()?;
    rest.contains('_').then_some((chain, i))
}

fn settings_of(extras: &serde_json::Value) -> Settings {
    let d = Settings::default();
    let f = |k: &str, or: f32| extras.get(k).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(or);
    Settings {
        stiffness: f("spring_stiffness", d.stiffness),
        gravity: f("spring_gravity", d.gravity),
        drag: f("spring_drag", d.drag).clamp(0.0, 1.0),
        radius: f("spring_radius", d.radius),
    }
}

/// Bones that get a collider, and the bone each one runs to
const COLLIDER_BONES: &[(&str, &[&str])] = &[
    ("Head", &["HeadTop_End"]),
    ("Neck", &["Head"]),
    ("Spine", &["Spine1"]),
    ("Spine1", &["Spine2"]),
    ("Spine2", &["Neck"]),
    ("Hips", &["Spine"]),
    ("LeftShoulder", &["LeftArm"]),
    ("RightShoulder", &["RightArm"]),
    ("LeftHand", &["LeftHandMiddle1"]),
    ("RightHand", &["RightHandMiddle1"]),
    ("LeftArm", &["LeftForeArm"]),
    ("LeftForeArm", &["LeftHand"]),
    ("RightArm", &["RightForeArm"]),
    ("RightForeArm", &["RightHand"]),
    ("LeftUpLeg", &["LeftLeg"]),
    ("LeftLeg", &["LeftFoot"]),
    ("RightUpLeg", &["RightLeg"]),
    ("RightLeg", &["RightFoot"]),
];

fn closest_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-10 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

impl Springs {
    pub fn new(model: &Model, pose: &Pose) -> Springs {
        // Chains: their roots' settings, and every spring bone's place in them
        let mut joints = Vec::new();
        let mut chains = 0;
        for (i, node) in model.nodes.iter().enumerate() {
            let Some((chain, _)) = spring_name(&node.name) else { continue };
            let root = model.node(&format!("{chain}_0")).unwrap_or(i);
            if root == i {
                chains += 1;
            }
            let settings = settings_of(&model.nodes[root].extras);
            let kind_name = chain.strip_prefix("spring_").and_then(|c| c.split('_').next()).unwrap_or("misc");
            let kind = KINDS.iter().position(|k| *k == kind_name).unwrap_or(KINDS.len() - 1);
            let head = pose.position(i);
            let tail_node = (0..model.nodes.len()).find(|&c| model.nodes[c].parent == Some(i));
            let tail = match tail_node {
                Some(c) => pose.position(c),
                None => {
                    let parent = model.nodes[i].parent.map(|p| pose.position(p)).unwrap_or(head - Vec3::Y);
                    head + (head - parent).normalize_or(Vec3::NEG_Y) * LEAF_LENGTH
                }
            };
            let length = (tail - head).length();
            if length < 1e-5 {
                continue;
            }
            let axis = (pose.world_rotation(i).inverse() * (tail - head)).normalize();
            joints.push(Joint { node: i, kind, axis, length, rest_rotation: pose.rotation[i], settings, tail, prev_tail: tail });
        }
        // Parents before children: a joint's head moves with the joints above it
        let rank: Vec<usize> = {
            let mut r = vec![0; model.nodes.len()];
            for (k, &n) in model.order.iter().enumerate() {
                r[n] = k;
            }
            r
        };
        joints.sort_by_key(|j| rank[j.node]);

        // Colliders: as thick as the mesh their bone moves (most of it outside)
        let mut colliders = Vec::new();
        for (bone, ends) in COLLIDER_BONES {
            let Some(from) = model.node(bone) else { continue };
            let moved: Vec<Vec3> = model
                .primitives
                .iter()
                .flat_map(|p| p.positions.iter().zip(&p.dominant))
                .filter(|(_, d)| **d == from)
                .map(|(v, _)| Vec3::from(*v))
                .collect();
            if moved.len() < 8 {
                continue;
            }
            let a = pose.position(from);
            // To its end bone, or the middle of what it moves (the head)
            let b = match ends.iter().find_map(|e| model.node(e)) {
                Some(to) => pose.position(to),
                None => moved.iter().sum::<Vec3>() / moved.len() as f32,
            };
            let mut distances: Vec<f32> = moved.iter().map(|&v| (v - closest_on_segment(v, a, b)).length()).collect();
            distances.sort_by(f32::total_cmp);
            // Most of what it moves is inside: the skin, not the eyes and
            // teeth, nor the half of a limb's ring that's nearer than the rest
            let radius = distances[distances.len() * 3 / 4];
            let tip = pose.world_rotation(from).inverse() * (b - a);
            colliders.push(Collider { from, base: Vec3::ZERO, tip, radius });
        }
        // A body is wider than deep: the spine's capsules leave the chest's
        // sides open, so one more goes across it, from arm to arm
        if let (Some(chest), Some(left), Some(right)) = (model.node("Spine2").or_else(|| model.node("Spine1")), model.node("LeftArm"), model.node("RightArm")) {
            let thick = colliders.iter().find(|c| c.from == chest).map(|c| c.radius);
            if let Some(radius) = thick {
                let (at, turn) = (pose.position(chest), pose.world_rotation(chest).inverse());
                // (a little in from the arms' joints, and under them: the chest, not the shoulders' tops)
                let (l, r) = (pose.position(left), pose.position(right));
                let down = Vec3::NEG_Y * radius * 0.5;
                let (l, r) = (l.lerp(r, 0.12) + down, r.lerp(l, 0.12) + down);
                colliders.push(Collider { from: chest, base: turn * (l - at), tip: turn * (r - at), radius: radius * 0.85 });
            }
        }
        Springs { joints, colliders, chains }
    }

    /// The kinds this avatar has (indices into KINDS)
    pub fn kinds(&self) -> Vec<usize> {
        let mut k: Vec<usize> = self.joints.iter().map(|j| j.kind).collect();
        k.sort();
        k.dedup();
        k
    }

    pub fn joints(&self) -> usize {
        self.joints.len()
    }

    /// Back at rest, still (after a jump: a new file, the source shown again)
    pub fn reset(&mut self, pose: &Pose) {
        for j in &mut self.joints {
            let head = pose.position(j.node);
            let rot = pose.world_rotation(j.node);
            j.tail = head + rot * j.axis * j.length;
            j.prev_tail = j.tail;
        }
    }

    /// Moves the springs `dt` seconds on `pose` (the body already posed and
    /// its world current), setting their rotations; the world is updated.
    pub fn step(&mut self, model: &Model, pose: &mut Pose, dt: f32, adjust: &Adjust) {
        // Small steps: stable whatever OBS's frame rate
        let steps = ((dt / (1.0 / 60.0)).ceil() as usize).clamp(1, 4);
        let dt = (dt / steps as f32).min(1.0 / 30.0);
        let capsules: Vec<(Vec3, Vec3, f32)> = self
            .colliders
            .iter()
            .map(|c| {
                let (a, b) = c.ends(pose);
                (a, b, c.radius * adjust.colliders.max(0.0))
            })
            .collect();
        for _ in 0..steps {
            for j in &mut self.joints {
                // Its head where its parent is now, turned as at rest from there
                pose.rotation[j.node] = j.rest_rotation;
                pose.update_node(model, j.node);
                let head = pose.position(j.node);
                let rest_dir = pose.world_rotation(j.node) * j.axis;
                let s = &j.settings;
                let t = adjust.kinds[j.kind];
                let drag = (s.drag * t.drag).clamp(0.0, 1.0);
                let inertia = (j.tail - j.prev_tail) * (1.0 - drag);
                let mut next = j.tail + inertia + rest_dir * s.stiffness * t.stiffness * dt + Vec3::NEG_Y * s.gravity * t.gravity * dt;
                next = head + (next - head).normalize_or(rest_dir) * j.length;
                for &(a, b, r) in &capsules {
                    let c = closest_on_segment(next, a, b);
                    let away = next - c;
                    let min = r + s.radius;
                    if away.length_squared() < min * min {
                        next = c + away.normalize_or(Vec3::Z) * min;
                        next = head + (next - head).normalize_or(rest_dir) * j.length;
                    }
                }
                j.prev_tail = j.tail;
                j.tail = next;
                // Turned from its rest direction to the tail's
                let to = (next - head).normalize_or(rest_dir);
                let turn = Quat::from_rotation_arc(rest_dir, to);
                pose.rotate_in_world(model, j.node, turn);
                pose.update_node(model, j.node);
            }
        }
        pose.update_world(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THEATER_PUPPET_GLB=<a Rig export> cargo test -- --nocapture
    #[test]
    fn adjustments_change_the_hang() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let model = crate::model::load(std::path::Path::new(&path)).unwrap();
        let tip = model.node("spring_hair_1_3").or_else(|| model.node("spring_hair_1_1")).unwrap();
        let hair = KINDS.iter().position(|k| *k == "hair").unwrap();
        let settle = |adjust: Adjust| {
            let mut pose = Pose::rest(&model);
            let mut springs = Springs::new(&model, &pose);
            assert!(springs.kinds().contains(&hair));
            for _ in 0..180 {
                pose.reset(&model);
                pose.update_world(&model);
                springs.step(&model, &mut pose, 1.0 / 60.0, &adjust);
            }
            pose.position(tip)
        };
        let normal = settle(Adjust::default());
        let mut heavy = Adjust::default();
        heavy.kinds[hair] = Tuning { stiffness: 0.2, gravity: 4.0, drag: 1.0 };
        let heavy = settle(heavy);
        let thick = settle(Adjust { colliders: 2.0, ..Adjust::default() });
        println!("hair tip: as made {normal:?}, heavy {heavy:?}, thick colliders {thick:?}");
        assert!(heavy.y < normal.y - 0.005, "heavier hair doesn't hang lower");
        assert!((thick - normal).length() > 0.001, "thicker colliders change nothing");
    }

    #[test]
    fn spring_names_read() {
        assert_eq!(spring_name("spring_hair_1_0"), Some(("spring_hair_1", 0)));
        assert_eq!(spring_name("spring_bust_2_14"), Some(("spring_bust_2", 14)));
        assert_eq!(spring_name("Head"), None);
        assert_eq!(spring_name("spring_x"), None);
    }

    /// THEATER_PUPPET_GLB=<a Rig export> cargo test -- --nocapture
    #[test]
    fn hair_settles_and_swings() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let model = crate::model::load(std::path::Path::new(&path)).unwrap();
        let mut pose = Pose::rest(&model);
        let mut springs = Springs::new(&model, &pose);
        println!("{} chains, {} joints, {} colliders", springs.chains, springs.joints(), springs.colliders.len());
        for c in &springs.colliders {
            println!("  {:14} length {:.3} r {:.3}", model.nodes[c.from].name, c.tip.length(), c.radius);
        }
        assert!(springs.colliders.iter().any(|c| model.nodes[c.from].name == "Head"), "no head collider");
        assert!(springs.chains > 0 && !springs.colliders.is_empty());
        let tip = model.node("spring_hair_1_3").or_else(|| model.node("spring_hair_1_1")).unwrap();
        let rest_tip = pose.position(tip);

        // Settles: two seconds still, then nothing moves much
        for _ in 0..120 {
            pose.reset(&model);
            pose.update_world(&model);
            springs.step(&model, &mut pose, 1.0 / 60.0, &Adjust::default());
        }
        let settled = pose.position(tip);
        pose.reset(&model);
        pose.update_world(&model);
        springs.step(&model, &mut pose, 1.0 / 60.0, &Adjust::default());
        let still = (pose.position(tip) - settled).length();
        println!("tip at rest {rest_tip:?}, settled {settled:?} (moved {:.3} m), then {still:.5} m in a frame", (settled - rest_tip).length());
        assert!(still < 0.002, "still moving {still}");
        assert!(settled.is_finite());

        // The body turns: the hair lags behind, then follows
        let hips = model.node("Hips").unwrap();
        let mut lag = 0f32;
        for f in 0..30 {
            pose.reset(&model);
            pose.update_world(&model);
            let angle = (f as f32 / 10.0).min(1.0) * 60f32.to_radians();
            pose.rotate_in_world(&model, hips, Quat::from_rotation_y(angle));
            pose.update_world(&model);
            let rigid = pose.position(tip);
            springs.step(&model, &mut pose, 1.0 / 60.0, &Adjust::default());
            lag = lag.max((pose.position(tip) - rigid).length());
        }
        println!("hair lag while turning: {lag:.3} m");
        assert!(lag > 0.005, "the hair doesn't swing");
    }
}
