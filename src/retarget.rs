//! Animation GLBs: clips made on another skeleton (Theater Rig's "Convert
//! Mixamo to Animation GLB": Mixamo's, in its T-pose) put on the avatar when
//! it loads, so any avatar plays them. The avatar's own clips (animation.rs)
//! are played as they are; these are retargeted once, into clips like them.
//!
//! The bones match by name (Theater Rig names the avatar's as Mixamo does:
//! Hips, Spine, LeftArm…). Each bone of the avatar turns, in the world, as the
//! same bone turns from its rest pose in the animation (whatever the two
//! skeletons' own bone axes); the hips also move, as much as the animation's,
//! scaled to the avatar's hip height. Both rest poses are T-poses (Theater
//! Rig's), so a turn of the arm is the same turn on both.

use std::path::Path;

use glam::{Mat4, Quat, Vec3};

use crate::animation::{Channel, Clip, Values};
use crate::model::Model;
use crate::pose::Pose;

/// Frames a second the clips are sampled at
const RATE: f32 = 30.0;

/// The bones an animation moves on any avatar (Mixamo's names)
const BONES: &[&str] = &[
    "Hips", "Spine", "Spine1", "Spine2", "Neck", "Head",
    "LeftShoulder", "LeftArm", "LeftForeArm", "LeftHand",
    "RightShoulder", "RightArm", "RightForeArm", "RightHand",
    "LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase",
    "RightUpLeg", "RightLeg", "RightFoot", "RightToeBase",
    "LeftHandThumb1", "LeftHandThumb2", "LeftHandThumb3",
    "LeftHandIndex1", "LeftHandIndex2", "LeftHandIndex3",
    "LeftHandMiddle1", "LeftHandMiddle2", "LeftHandMiddle3",
    "LeftHandRing1", "LeftHandRing2", "LeftHandRing3",
    "LeftHandPinky1", "LeftHandPinky2", "LeftHandPinky3",
    "RightHandThumb1", "RightHandThumb2", "RightHandThumb3",
    "RightHandIndex1", "RightHandIndex2", "RightHandIndex3",
    "RightHandMiddle1", "RightHandMiddle2", "RightHandMiddle3",
    "RightHandRing1", "RightHandRing2", "RightHandRing3",
    "RightHandPinky1", "RightHandPinky2", "RightHandPinky3",
];

fn rotation(m: &Mat4) -> Quat {
    m.to_scale_rotation_translation().1.normalize()
}

/// `src`'s clips on `dst`: the same names, contacts and shape keys (by name)
pub fn clips_onto(src: &Model, dst: &Model) -> Vec<Clip> {
    // (the animation's bone, the avatar's)
    let pairs: Vec<(usize, usize)> = BONES.iter().filter_map(|b| Some((src.node(b)?, dst.node(b)?))).collect();
    let Some(&(src_hips, dst_hips)) = pairs.first().filter(|_| src.node("Hips").is_some() && dst.node("Hips").is_some()) else {
        return Vec::new();
    };
    let src_rest = Pose::rest(src);
    let dst_rest = Pose::rest(dst);
    let src_rest_rot: Vec<Quat> = src_rest.world.iter().map(rotation).collect();
    let dst_rest_rot: Vec<Quat> = dst_rest.world.iter().map(rotation).collect();
    let src_hips_rest = src_rest.position(src_hips);
    let dst_hips_rest = dst_rest.position(dst_hips);
    // The avatar's hips height over the animation's (the floor at 0)
    let ratio = if src_hips_rest.y > 1e-4 { dst_hips_rest.y / src_hips_rest.y } else { 1.0 };
    let mapped: Vec<Option<usize>> = (0..dst.nodes.len()).map(|n| pairs.iter().find(|p| p.1 == n).map(|p| p.0)).collect();

    let mut out = Vec::new();
    for clip in &src.clips {
        let frames = ((clip.duration * RATE).round() as usize).max(1) + 1;
        let times: Vec<f32> = (0..frames).map(|f| (f as f32 / RATE).min(clip.duration)).collect();
        let mut rotations: Vec<Vec<Quat>> = vec![Vec::with_capacity(frames); pairs.len()];
        let mut hips_at: Vec<Vec3> = Vec::with_capacity(frames);
        let mut sp = Pose::rest(src);
        let mut dp = Pose::rest(dst);
        for &t in &times {
            sp.reset(src);
            clip.apply(&mut sp, t, 1.0);
            sp.update_world(src);
            dp.reset(dst);
            for &n in &dst.order {
                if let Some(s) = mapped[n] {
                    let parent = dst.nodes[n].parent.map(|p| dp.world[p]).unwrap_or(Mat4::IDENTITY);
                    // Its turn from rest, in the world, as the animation's bone's
                    let turn = rotation(&sp.world[s]) * src_rest_rot[s].inverse();
                    let want = (turn * dst_rest_rot[n]).normalize();
                    dp.rotation[n] = (rotation(&parent).inverse() * want).normalize();
                    if n == dst_hips {
                        let moved = (sp.position(src_hips) - src_hips_rest) * ratio;
                        dp.translation[n] = parent.inverse().transform_point3(dst_hips_rest + moved);
                    }
                }
                dp.update_node(dst, n);
            }
            for (i, &(_, d)) in pairs.iter().enumerate() {
                let mut q = dp.rotation[d];
                // The short way from the frame before (no spin between keys)
                if let Some(prev) = rotations[i].last() {
                    if prev.dot(q) < 0.0 {
                        q = -q;
                    }
                }
                rotations[i].push(q);
            }
            hips_at.push(dp.translation[dst_hips]);
        }
        let mut channels: Vec<Channel> = pairs
            .iter()
            .zip(rotations)
            .map(|(&(_, d), r)| Channel { node: d, times: times.clone(), values: Values::Rotation(r), step: false })
            .collect();
        channels.push(Channel { node: dst_hips, times: times.clone(), values: Values::Translation(hips_at), step: false });
        // Shape keys: the avatar's of the same names
        for c in &clip.channels {
            if let Values::Weights { targets, values } = &c.values {
                let targets = targets.iter().map(|t| t.and_then(|i| src.morph_names.get(i)).and_then(|name| dst.morph(name))).collect();
                let times = c.times.iter().map(|t| t - clip.start).collect();
                channels.push(Channel { node: 0, times, values: Values::Weights { targets, values: values.clone() }, step: c.step });
            }
        }
        out.push(Clip { name: clip.name.clone(), start: 0.0, duration: clip.duration, channels, contacts: clip.contacts.clone() });
    }
    out
}

/// Every animation GLB in `folder` on `dst`: (clips, a line per file for the log)
pub fn folder_onto(folder: &Path, dst: &Model) -> (Vec<Clip>, Vec<String>) {
    let mut files: Vec<_> = std::fs::read_dir(folder)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("glb"))).collect())
        .unwrap_or_default();
    files.sort();
    let mut clips: Vec<Clip> = Vec::new();
    let mut log = Vec::new();
    for f in files {
        match crate::model::load_animation(&f) {
            Ok(src) => {
                let made = clips_onto(&src, dst);
                if made.is_empty() {
                    log.push(format!("{}: no clip on bones the avatar has (Hips, Spine…)", f.display()));
                }
                for c in made {
                    // The avatar's own clip (or an earlier file's) of that name stays
                    if dst.clips.iter().chain(clips.iter()).any(|o| o.name == c.name) {
                        log.push(format!("{}: \"{}\" already there, skipped", f.display(), c.name));
                    } else {
                        clips.push(c);
                    }
                }
            }
            Err(e) => log.push(format!("{}: {e}", f.display())),
        }
    }
    (clips, log)
}

/// Every animation GLB in `folder` with its time modified (changes: loaded again)
pub fn folder_stamp(folder: &str) -> Vec<(String, u64)> {
    if folder.trim().is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(String, u64)> = std::fs::read_dir(folder)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("glb")))
                .map(|e| {
                    let t = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                    (e.path().to_string_lossy().into_owned(), t)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THEATER_PUPPET_GLB=<an avatar from Theater Rig> THEATER_PUPPET_ANIMATION_GLB=<Rig's
    /// conversion of Mixamo's "Knocked Out"> THEATER_PUPPET_MIXAMO_GLB=<the same, imported
    /// onto that avatar in Blender by Rig>: the avatar plays it as Blender does
    #[test]
    fn an_animation_glb_plays_on_the_avatar_as_blender_retargets_it() {
        let (Ok(avatar), Ok(anim)) = (std::env::var("THEATER_PUPPET_GLB"), std::env::var("THEATER_PUPPET_ANIMATION_GLB")) else { return };
        let dst = crate::model::load(Path::new(&avatar)).unwrap();
        let src = crate::model::load_animation(Path::new(&anim)).unwrap();
        let clips = clips_onto(&src, &dst);
        println!("clips: {:?}", clips.iter().map(|c| (&c.name, c.duration, c.channels.len())).collect::<Vec<_>>());
        assert_eq!(clips.len(), 1);
        let clip = &clips[0];
        let head = dst.node("Head").unwrap();
        let mut pose = Pose::rest(&dst);
        let standing = pose.position(head).y;
        let mut lowest = standing;
        let mut t = 0.0;
        while t <= clip.duration {
            pose.reset(&dst);
            clip.apply(&mut pose, t, 1.0);
            pose.update_world(&dst);
            lowest = lowest.min(pose.position(head).y);
            t += 1.0 / 30.0;
        }
        println!("head: standing {standing:.2} m, lowest {lowest:.2} m");
        assert!(lowest < standing * 0.2, "the head didn't reach the floor");

        // The same as Rig's import in Blender (the avatar's own clip, exported)
        let Ok(reference) = std::env::var("THEATER_PUPPET_MIXAMO_GLB") else { return };
        let rm = crate::model::load(Path::new(&reference)).unwrap();
        let rclip = rm.clips.iter().find(|c| c.name == "Knocked Out").unwrap();
        let mut rp = Pose::rest(&rm);
        let mut worst = (0.0f32, String::new(), 0.0f32);
        let mut t = 0.0;
        while t <= clip.duration.min(rclip.duration) {
            pose.reset(&dst);
            clip.apply(&mut pose, t, 1.0);
            pose.update_world(&dst);
            rp.reset(&rm);
            rclip.apply(&mut rp, t, 1.0);
            rp.update_world(&rm);
            for b in ["Hips", "Head", "LeftHand", "RightHand", "LeftFoot", "RightFoot", "LeftForeArm", "RightLeg"] {
                let (a, r) = (dst.node(b).unwrap(), rm.node(b).unwrap());
                let d = (pose.position(a) - rp.position(r)).length();
                if d > worst.0 {
                    worst = (d, b.to_string(), t);
                }
            }
            t += 1.0 / 30.0;
        }
        println!("furthest from Blender's: {:.3} m ({} at {:.2} s)", worst.0, worst.1, worst.2);
        assert!(worst.0 < 0.03, "not where Blender puts it");
    }
}
