//! A pose of the avatar's skeleton and the mesh skinned to it, on the CPU
//! (34k vertices: well under a millisecond). Everything is in the avatar's
//! own space (glTF: Y up, facing +Z, its left at +X).

use glam::{Mat4, Quat, Vec3};

use crate::model::{Binding, Model};

#[derive(Clone)]
pub struct Pose {
    pub translation: Vec<Vec3>,
    pub rotation: Vec<Quat>,
    pub scale: Vec<Vec3>,
    /// Each node's matrix in the avatar's space (update_world)
    pub world: Vec<Mat4>,
}

impl Pose {
    pub fn rest(model: &Model) -> Pose {
        let mut pose = Pose {
            translation: model.nodes.iter().map(|n| n.translation).collect(),
            rotation: model.nodes.iter().map(|n| n.rotation).collect(),
            scale: model.nodes.iter().map(|n| n.scale).collect(),
            world: vec![Mat4::IDENTITY; model.nodes.len()],
        };
        pose.update_world(model);
        pose
    }

    /// Back to the rest pose (world not updated)
    pub fn reset(&mut self, model: &Model) {
        for (i, n) in model.nodes.iter().enumerate() {
            self.translation[i] = n.translation;
            self.rotation[i] = n.rotation;
            self.scale[i] = n.scale;
        }
    }

    pub fn local(&self, i: usize) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale[i], self.rotation[i], self.translation[i])
    }

    /// Node `i`'s world matrix from its parent's (which must be current)
    pub fn update_node(&mut self, model: &Model, i: usize) {
        let parent = model.nodes[i].parent.map(|p| self.world[p]).unwrap_or(Mat4::IDENTITY);
        self.world[i] = parent * self.local(i);
    }

    pub fn update_world(&mut self, model: &Model) {
        for &i in &model.order {
            self.update_node(model, i);
        }
    }

    /// Where node `i` is (its world matrix must be current)
    pub fn position(&self, i: usize) -> Vec3 {
        self.world[i].w_axis.truncate()
    }

    /// Node `i`'s parent's rotation in the avatar's space
    pub fn parent_rotation(&self, model: &Model, i: usize) -> Quat {
        model.nodes[i].parent.map(|p| self.world_rotation(p)).unwrap_or(Quat::IDENTITY)
    }

    pub fn world_rotation(&self, i: usize) -> Quat {
        let (_, r, _) = self.world[i].to_scale_rotation_translation();
        r.normalize()
    }

    /// Turns node `i` by `by`, given in the avatar's space, about its own
    /// position (its parent's world matrix must be current)
    pub fn rotate_in_world(&mut self, model: &Model, i: usize, by: Quat) {
        let parent = self.parent_rotation(model, i);
        self.rotation[i] = (parent.inverse() * by * parent * self.rotation[i]).normalize();
    }
}

/// Every primitive's vertices skinned to `pose` (its world matrices must be
/// current), as the GPU takes them: [x, y, z, 0]
pub fn skin(model: &Model, pose: &Pose, morph: &[f32], points: &mut [Vec<[f32; 4]>], normals: &mut [Vec<[f32; 4]>]) {
    // Each skin's joint matrices
    let joint_matrices: Vec<Vec<Mat4>> = model
        .skins
        .iter()
        .map(|s| s.joints.iter().zip(&s.inverse_bind).map(|(&j, ib)| pose.world[j] * *ib).collect())
        .collect();
    // A liquid's level key (water.rs): how empty, not a shape
    let empty_key = model.morph(crate::water::EMPTY_KEY);
    for (k, prim) in model.primitives.iter().enumerate() {
        let (out_p, out_n) = (&mut points[k], &mut normals[k]);
        let water = model.materials.get(prim.material).and_then(|m| m.water);
        // Its shape keys at work: what they add to its unposed positions
        let shaped: Option<Vec<Vec3>> = prim.morphs.iter().any(|m| Some(m.name) != empty_key && morph.get(m.name).is_some_and(|w| w.abs() > 1e-4)).then(|| {
            let mut add = vec![Vec3::ZERO; prim.positions.len()];
            for m in &prim.morphs {
                if Some(m.name) == empty_key {
                    continue;
                }
                let w = morph.get(m.name).copied().unwrap_or(0.0);
                if w.abs() > 1e-4 {
                    for (i, d) in &m.deltas {
                        add[*i as usize] += Vec3::from(*d) * w;
                    }
                }
            }
            add
        });
        let shape = |i: usize| shaped.as_ref().map(|a| a[i]).unwrap_or(Vec3::ZERO);
        // A liquid: laid onto its level, its waves (its own space, before it's placed)
        let liquid: Option<(Vec<Vec3>, Vec<Vec3>)> = water.map(|w| {
            let (own_p, own_n) = match &prim.binding {
                Binding::Skin { positions, normals, .. } | Binding::Node { positions, normals, .. } => (positions, normals),
            };
            let p: Vec<Vec3> = own_p.iter().enumerate().map(|(i, v)| Vec3::from(*v) + shape(i)).collect();
            let n: Vec<Vec3> = own_n.iter().map(|v| Vec3::from(*v)).collect();
            let empty = empty_key.and_then(|e| morph.get(e)).copied().unwrap_or(0.0);
            crate::water::surface(&w, &p, &n, empty, crate::water::now())
        });
        let at = |i: usize, own: &[f32; 3]| liquid.as_ref().map_or(Vec3::from(*own) + shape(i), |(p, _)| p[i]);
        let facing = |i: usize, own: &[f32; 3]| liquid.as_ref().map_or(Vec3::from(*own), |(_, n)| n[i]);
        match &prim.binding {
            Binding::Skin { skin, positions, normals, joints, weights } => {
                let jm = &joint_matrices[*skin];
                for i in 0..positions.len() {
                    let mut m = Mat4::ZERO;
                    for c in 0..4 {
                        let w = weights[i][c];
                        if w > 0.0 {
                            if let Some(j) = jm.get(joints[i][c] as usize) {
                                m += *j * w;
                            }
                        }
                    }
                    let p = m.transform_point3(at(i, &positions[i]));
                    let n = m.transform_vector3(facing(i, &normals[i])).normalize_or(Vec3::Y);
                    out_p[i] = [p.x, p.y, p.z, 0.0];
                    out_n[i] = [n.x, n.y, n.z, 0.0];
                }
            }
            Binding::Node { node, positions, normals } => {
                let m = pose.world[*node];
                for i in 0..positions.len() {
                    let p = m.transform_point3(at(i, &positions[i]));
                    let n = m.transform_vector3(facing(i, &normals[i])).normalize_or(Vec3::Y);
                    out_p[i] = [p.x, p.y, p.z, 0.0];
                    out_n[i] = [n.x, n.y, n.z, 0.0];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THEATER_PUPPET_GLB=<a Rig export> cargo test
    #[test]
    fn the_rest_pose_skins_to_the_loaded_mesh() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let model = crate::model::load(std::path::Path::new(&path)).unwrap();
        let pose = Pose::rest(&model);
        let mut points: Vec<Vec<[f32; 4]>> = model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect();
        let mut normals = points.clone();
        let started = std::time::Instant::now();
        skin(&model, &pose, &[], &mut points, &mut normals);
        println!("skinned in {:?}", started.elapsed());
        let mut worst = 0f32;
        for (prim, pts) in model.primitives.iter().zip(&points) {
            for (a, b) in prim.positions.iter().zip(pts) {
                worst = worst.max((Vec3::from(*a) - Vec3::new(b[0], b[1], b[2])).length());
            }
        }
        assert!(worst < 1e-4, "rest pose drifts {worst}");

        // Arms down: the left hand goes down, the right one too
        let mut pose = Pose::rest(&model);
        let hand = model.node("LeftHand").unwrap();
        let before = pose.position(hand);
        let arm = model.node("LeftArm").unwrap();
        pose.rotate_in_world(&model, arm, Quat::from_rotation_z(-60f32.to_radians()));
        pose.update_world(&model);
        let after = pose.position(hand);
        println!("left hand {before:?} -> {after:?}");
        assert!(after.y < before.y - 0.2 && after.x < before.x);
    }
}
