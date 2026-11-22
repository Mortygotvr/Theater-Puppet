//! The Animation folder: clips made on another skeleton (Theater Rig's
//! animation GLBs, Mixamo's DAE: dae.rs), each in its T-pose, put on the
//! avatar when it loads, so any avatar plays them. The avatar's own clips (animation.rs)
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
        out.push(Clip { name: clip.name.clone(), start: 0.0, duration: clip.duration, channels, contacts: clip.contacts.clone(), file: None });
    }
    out
}

/// An animation file: Theater Rig's GLB, or Mixamo's Collada (dae.rs)
fn is_animation(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("glb") || x.eq_ignore_ascii_case("dae"))
}

/// Every animation file (GLB, Mixamo DAE) in `folder` on `dst`: (clips, a line per file for the log)
pub fn folder_onto(folder: &Path, dst: &Model) -> (Vec<Clip>, Vec<String>) {
    let mut files: Vec<_> = std::fs::read_dir(folder)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| is_animation(p)).collect())
        .unwrap_or_default();
    files.sort();
    let mut clips: Vec<Clip> = Vec::new();
    let mut log = Vec::new();
    for f in files {
        let loaded = if f.extension().is_some_and(|x| x.eq_ignore_ascii_case("dae")) { crate::dae::load(&f) } else { crate::model::load_animation(&f) };
        match loaded {
            Ok(src) => {
                let made = clips_onto(&src, dst);
                if made.is_empty() {
                    log.push(format!("{}: no clip on bones the avatar has (Hips, Spine…)", f.display()));
                }
                for mut c in made {
                    c.file = Some(f.to_string_lossy().into_owned());
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

/// Every animation file in `folder` with its time modified (changes: loaded again)
pub fn folder_stamp(folder: &str) -> Vec<(String, u64)> {
    if folder.trim().is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(String, u64)> = std::fs::read_dir(folder)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| is_animation(&e.path()))
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

/// The animations library: the files added in the Configuration window
/// (🎬 Animations), every Puppet's
pub fn library() -> std::path::PathBuf {
    crate::config_dir().join("animations")
}

/// An animation file as the folder reads it: (its clips: name, seconds), or
/// why it won't do
pub fn check_file(path: &Path) -> Result<Vec<(String, f32)>, String> {
    if !is_animation(path) {
        return Err("not a .dae or .glb".into());
    }
    // (Its problem said without its path: the window shows the file's name)
    let short = |e: String| e.strip_prefix(&format!("{}: ", path.display())).map(str::to_string).unwrap_or(e);
    let m = if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("dae")) { crate::dae::load(path).map_err(short)? } else { crate::model::load_animation(path).map_err(short)? };
    if m.node("Hips").is_none() {
        return Err("no Hips bone: not a Mixamo skeleton, nor one Theater Rig named".into());
    }
    if m.clips.is_empty() {
        return Err("no animation in it".into());
    }
    Ok(m.clips.iter().map(|c| (c.name.clone(), c.duration)).collect())
}

/// The library's files, each with its clips or its problem (read again
/// only when a file changed)
pub fn library_listing() -> serde_json::Value {
    static CACHE: std::sync::Mutex<Option<(Vec<(String, u64)>, serde_json::Value)>> = std::sync::Mutex::new(None);
    let dir = library();
    let stamp = folder_stamp(&dir.to_string_lossy());
    let mut cache = CACHE.lock().unwrap();
    if let Some((s, v)) = cache.as_ref() {
        if *s == stamp {
            return v.clone();
        }
    }
    let files: Vec<serde_json::Value> = stamp
        .iter()
        .map(|(f, _)| {
            let p = Path::new(f);
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match check_file(p) {
                Ok(clips) => serde_json::json!({ "file": name, "clips": clips.iter().map(|(n, d)| serde_json::json!({ "name": n, "seconds": d })).collect::<Vec<_>>(), "error": "" }),
                Err(e) => serde_json::json!({ "file": name, "clips": [], "error": e }),
            }
        })
        .collect();
    let v = serde_json::json!({ "folder": dir.to_string_lossy(), "files": files });
    *cache = Some((stamp, v.clone()));
    v
}

/// `path` into the library (checked first; another file of that name: "name (2)").
/// Returns the file's name in it and its clips
pub fn library_add(path: &Path) -> Result<(String, Vec<(String, f32)>), String> {
    let clips = check_file(path)?;
    let dir = library();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "animation".into());
    let ext = path.extension().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} ({n}).{ext}") };
        let dest = dir.join(&name);
        match std::fs::read(&dest) {
            // The same file already there: nothing to do
            Ok(have) if have == bytes => return Ok((name, clips)),
            Ok(_) => n += 1,
            Err(_) => {
                std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
                return Ok((name, clips));
            }
        }
    }
}

/// A file out of the library (its name only: nothing outside it)
pub fn library_remove(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return Err(format!("\"{name}\": a file's name in the library"));
    }
    std::fs::remove_file(library().join(name)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THEATER_PUPPET_ANIMATION_GLB=<an animation GLB> THEATER_PUPPET_DAE_NO_SKIN=<a DAE without skin>
    #[test]
    fn a_file_is_checked_before_it_goes_in_the_library() {
        let Ok(glb) = std::env::var("THEATER_PUPPET_ANIMATION_GLB") else { return };
        let clips = check_file(Path::new(&glb)).unwrap();
        println!("{glb}: {clips:?}");
        assert!(!clips.is_empty());
        if let Ok(bad) = std::env::var("THEATER_PUPPET_DAE_NO_SKIN") {
            let e = check_file(Path::new(&bad)).unwrap_err();
            println!("without skin: {e}");
            assert!(e.contains("With Skin") && !e.contains(&bad), "{e}");
        }
        assert!(check_file(Path::new("x.txt")).is_err());
        assert!(library_remove("../x.glb").is_err() && library_remove("a/b.glb").is_err());
    }

    /// THEATER_PUPPET_GLB=<an avatar from Theater Rig> THEATER_PUPPET_ANIMATION_FOLDER=<Rig's
    /// Export Animation of "Knock Test" (Mixamo's Knocked Out, a contact at 1.3 s) and "Wave"
    /// (made on another avatar's skeleton: the right arm swung 60 degrees forward at 0.3 s, a contact there)>
    #[test]
    fn an_animation_folder_plays_on_the_avatar() {
        let (Ok(avatar), Ok(folder)) = (std::env::var("THEATER_PUPPET_GLB"), std::env::var("THEATER_PUPPET_ANIMATION_FOLDER")) else { return };
        let dst = crate::model::load(Path::new(&avatar)).unwrap();
        let (clips, notes) = folder_onto(Path::new(&folder), &dst);
        println!("clips: {:?}; notes: {notes:?}", clips.iter().map(|c| (&c.name, c.duration, &c.contacts)).collect::<Vec<_>>());
        let get = |n: &str| clips.iter().find(|c| c.name == n).unwrap_or_else(|| panic!("no {n}"));
        let (knock, wave) = (get("Knock Test"), get("Wave"));
        assert!((knock.contacts[0] - 1.3).abs() < 1e-3 && (wave.contacts[0] - 0.3).abs() < 1e-3, "contacts lost");
        let mut pose = Pose::rest(&dst);
        let at = |clip: &Clip, t: f32, pose: &mut Pose, bone: &str| {
            pose.reset(&dst);
            clip.apply(pose, t, 1.0);
            pose.update_world(&dst);
            pose.position(dst.node(bone).unwrap())
        };
        // Knocked out: the head to the floor
        let standing = Pose::rest(&dst).position(dst.node("Head").unwrap()).y;
        let lowest = (0..=(knock.duration * 30.0) as usize).map(|f| at(knock, f as f32 / 30.0, &mut pose, "Head").y).fold(f32::MAX, f32::min);
        println!("head: standing {standing:.2} m, lowest {lowest:.2} m");
        assert!(lowest < standing * 0.2, "the head didn't reach the floor");
        // Wave: the right hand swung at its contact (as on the skeleton it was
        // made on: from the shoulder, the same way), back at the end
        let rest = at(wave, 0.0, &mut pose, "RightHand");
        let swung = at(wave, wave.contacts[0], &mut pose, "RightHand");
        let end = at(wave, wave.duration, &mut pose, "RightHand");
        let shoulder = Pose::rest(&dst).position(dst.node("RightArm").unwrap());
        println!("right hand: moved {:.2} m at the contact, {:.3} m off rest at the end", (swung - rest).length(), (end - rest).length());
        assert!((swung - rest).length() > 0.2 && (end - rest).length() < 0.01, "the wave isn't on this avatar's arm");
        // Forward (+Z), level: as the arm on the skeleton it was made on
        let dir = (swung - shoulder).normalize();
        println!("arm direction at the contact: {dir:?}");
        assert!(dir.z > 0.4 && dir.y.abs() < 0.1, "swung another way than on its own skeleton");
    }
}

