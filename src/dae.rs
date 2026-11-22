//! Mixamo's Collada (.dae) animations, read straight into Puppet's Animation
//! folder (retarget.rs puts them on the avatar, as Theater Rig's animation
//! GLBs). Mixamo's DAE: Y up, centimetres, each bone's local matrix baked per
//! frame. Its T-pose is only in the skin's bind matrices, so it must be
//! downloaded "With Skin" (without, a bone's rest is the animation's first
//! frame: nothing to measure the turns from).

use std::path::Path;

use glam::{Mat4, Quat, Vec3};

use crate::animation::{Channel, Clip, Values};
use crate::model::{Model, Node};

/// "mixamorig_Hips", "mixamorig:Hips", "mixamorig1_Hips" -> "Hips"
fn plain(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("mixamorig") {
        let rest = &name[9..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        let rest = &rest[digits..];
        if let Some(r) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('_')) {
            return r;
        }
    }
    name
}

fn floats(text: Option<&str>) -> Vec<f32> {
    text.unwrap_or("").split_ascii_whitespace().filter_map(|t| t.parse().ok()).collect()
}

/// A Collada matrix (16 floats, row by row)
fn matrix(v: &[f32]) -> Mat4 {
    let mut a = [0.0f32; 16];
    a.copy_from_slice(&v[..16]);
    Mat4::from_cols_array(&a).transpose()
}

fn scaled(m: Mat4, unit: f32) -> Mat4 {
    let mut m = m;
    m.w_axis = (m.w_axis.truncate() * unit).extend(1.0);
    m
}

fn trs(m: &Mat4) -> (Vec3, Quat, Vec3) {
    let (s, r, t) = m.to_scale_rotation_translation();
    (t, r.normalize(), s)
}

/// A Mixamo DAE as a skeleton (its T-pose) and one clip, named after the file
pub fn load(path: &Path) -> Result<Model, String> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| format!("{shown}: {e}"))?;
    let named = |n: &roxmltree::Node, tag: &str| n.is_element() && n.tag_name().name() == tag;
    let root = doc.root_element();
    let unit = root
        .descendants()
        .find(|n| named(n, "unit"))
        .and_then(|n| n.attribute("meter"))
        .and_then(|m| m.parse::<f32>().ok())
        .unwrap_or(1.0);
    let up = root.descendants().find(|n| named(n, "up_axis")).and_then(|n| n.text()).unwrap_or("Y_UP").trim().to_string();
    if up != "Y_UP" {
        return Err(format!("{shown}: {up} (Mixamo's DAE is Y_UP)"));
    }

    // The T-pose: each joint's bind matrix (the inverse of its INV_BIND_MATRIX)
    let mut binds: Vec<(String, Mat4)> = Vec::new();
    for skin in root.descendants().filter(|n| named(n, "skin")) {
        let source = |id: &str| skin.children().find(|s| named(s, "source") && s.attribute("id") == Some(id.trim_start_matches('#')));
        let Some(joints) = skin.children().find(|n| named(n, "joints")) else { continue };
        let input = |sem: &str| joints.children().find(|i| named(i, "input") && i.attribute("semantic") == Some(sem)).and_then(|i| i.attribute("source"));
        let (Some(js), Some(ms)) = (input("JOINT").and_then(source), input("INV_BIND_MATRIX").and_then(source)) else { continue };
        let names: Vec<&str> = js.descendants().find(|n| named(n, "Name_array")).and_then(|n| n.text()).unwrap_or("").split_ascii_whitespace().collect();
        let mats = floats(ms.descendants().find(|n| named(n, "float_array")).and_then(|n| n.text()));
        for (i, name) in names.iter().enumerate() {
            if mats.len() >= (i + 1) * 16 && !binds.iter().any(|(n, _)| n == name) {
                binds.push((name.to_string(), scaled(matrix(&mats[i * 16..]), unit).inverse()));
            }
        }
    }
    if binds.is_empty() {
        return Err(format!("{shown}: no T-pose in it: download it from Mixamo \"With Skin\""));
    }

    // The skeleton: the scene's nodes (parents first), at the T-pose
    let scene = root.descendants().find(|n| named(n, "visual_scene")).ok_or(format!("{shown}: no scene"))?;
    let mut nodes: Vec<Node> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut worlds: Vec<Mat4> = Vec::new();
    fn walk(n: roxmltree::Node, parent: Option<usize>, unit: f32, binds: &[(String, Mat4)], nodes: &mut Vec<Node>, ids: &mut Vec<String>, worlds: &mut Vec<Mat4>) {
        for c in n.children().filter(|c| c.is_element() && c.tag_name().name() == "node") {
            let name = c.attribute("name").or(c.attribute("id")).unwrap_or("").to_string();
            let own = c.children().find(|m| m.is_element() && m.tag_name().name() == "matrix").map(|m| scaled(matrix(&floats(m.text())), unit)).unwrap_or(Mat4::IDENTITY);
            let parent_world = parent.map(|p| worlds[p]).unwrap_or(Mat4::IDENTITY);
            let sid = c.attribute("sid").unwrap_or("");
            let world = binds.iter().find(|(b, _)| *b == name || *b == sid || Some(b.as_str()) == c.attribute("id")).map(|(_, m)| *m).unwrap_or(parent_world * own);
            let (translation, rotation, scale) = trs(&(parent_world.inverse() * world));
            let i = nodes.len();
            nodes.push(Node { name: plain(&name).to_string(), parent, translation, rotation, scale, extras: serde_json::Value::Null });
            ids.push(c.attribute("id").unwrap_or("").to_string());
            worlds.push(world);
            walk(c, Some(i), unit, binds, nodes, ids, worlds);
        }
    }
    walk(scene, None, unit, &binds, &mut nodes, &mut ids, &mut worlds);
    if nodes.is_empty() {
        return Err(format!("{shown}: no skeleton"));
    }

    // The animation: each bone's local matrix per frame
    let mut channels = Vec::new();
    let (mut first, mut last) = (f32::MAX, f32::MIN);
    for anim in root.descendants().filter(|n| named(n, "animation")) {
        for ch in anim.children().filter(|n| named(n, "channel")) {
            let Some(target) = ch.attribute("target") else { continue };
            let Some((id, what)) = target.split_once('/') else { continue };
            if !what.starts_with("matrix") && what != "transform" {
                continue;
            }
            let Some(node) = ids.iter().position(|i| i == id) else { continue };
            let Some(sampler) = anim.children().find(|s| named(s, "sampler") && Some(s.attribute("id").unwrap_or("")) == ch.attribute("source").map(|x| x.trim_start_matches('#'))) else { continue };
            let input = |sem: &str| {
                let src = sampler.children().find(|i| named(i, "input") && i.attribute("semantic") == Some(sem))?.attribute("source")?.trim_start_matches('#');
                let s = anim.children().find(|s| named(s, "source") && s.attribute("id") == Some(src))?;
                Some(floats(s.descendants().find(|n| named(n, "float_array")).and_then(|n| n.text())))
            };
            let (Some(times), Some(values)) = (input("INPUT"), input("OUTPUT")) else { continue };
            let count = times.len().min(values.len() / 16);
            if count == 0 {
                continue;
            }
            let mut rotations = Vec::with_capacity(count);
            let mut translations = Vec::with_capacity(count);
            for k in 0..count {
                let (t, r, _) = trs(&scaled(matrix(&values[k * 16..]), unit));
                let r = match rotations.last() {
                    Some(prev) if Quat::dot(*prev, r) < 0.0 => -r,
                    _ => r,
                };
                rotations.push(r);
                translations.push(t);
            }
            let times: Vec<f32> = times[..count].to_vec();
            first = first.min(times[0]);
            last = last.max(times[count - 1]);
            channels.push(Channel { node, times: times.clone(), values: Values::Rotation(rotations), step: false });
            channels.push(Channel { node, times, values: Values::Translation(translations), step: false });
        }
    }
    if channels.is_empty() {
        return Err(format!("{shown}: no animation in it"));
    }
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&i| {
        let mut d = 0;
        let mut p = nodes[i].parent;
        while let Some(q) = p {
            d += 1;
            p = nodes[q].parent;
        }
        d
    });
    let clip = Clip { name: name.clone(), start: first, duration: (last - first).max(0.0), channels, contacts: Vec::new() };
    let mut model = Model::empty();
    model.name = name;
    model.nodes = nodes;
    model.order = order;
    model.clips = vec![clip];
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::Pose;

    #[test]
    fn mixamo_names_lose_their_prefix() {
        assert_eq!(plain("mixamorig_Hips"), "Hips");
        assert_eq!(plain("mixamorig:LeftArm"), "LeftArm");
        assert_eq!(plain("mixamorig1_Spine"), "Spine");
        assert_eq!(plain("Head"), "Head");
    }

    /// THEATER_PUPPET_DAE=<Mixamo's Knocked Out, Collada, With Skin> THEATER_PUPPET_DAE_NO_SKIN=<the same, Without Skin>
    /// THEATER_PUPPET_GLB=<an avatar from Theater Rig> THEATER_PUPPET_ANIMATION_GLB=<Rig's conversion of the FBX>
    #[test]
    fn a_mixamo_dae_reads_as_its_t_pose_and_plays_on_the_avatar() {
        if let Ok(p) = std::env::var("THEATER_PUPPET_DAE_NO_SKIN") {
            let e = load(Path::new(&p)).err().expect("a DAE without skin was read");
            println!("without skin: {e}");
            assert!(e.contains("With Skin"));
        }
        let Ok(p) = std::env::var("THEATER_PUPPET_DAE") else { return };
        let m = load(Path::new(&p)).unwrap();
        let c = &m.clips[0];
        println!("{}: {} nodes, {:.2} s from {:.2}, {} channels", c.name, m.nodes.len(), c.duration, c.start, c.channels.len());
        // The T-pose: arms level along X, hips at Mixamo's height (1.04 m)
        let rest = Pose::rest(&m);
        let at = |b: &str| rest.position(m.node(b).unwrap());
        let arm = (at("LeftForeArm") - at("LeftArm")).normalize();
        println!("left arm at rest {arm:?}, hips {:.3} m", at("Hips").y);
        assert!(arm.x > 0.99 && (at("Hips").y - 1.043).abs() < 0.01);
        let (Ok(avatar), Ok(glb)) = (std::env::var("THEATER_PUPPET_GLB"), std::env::var("THEATER_PUPPET_ANIMATION_GLB")) else { return };
        let dst = crate::model::load(Path::new(&avatar)).unwrap();
        let from_dae = crate::retarget::clips_onto(&m, &dst);
        let from_glb = crate::retarget::clips_onto(&crate::model::load_animation(Path::new(&glb)).unwrap(), &dst);
        let (a, b) = (&from_dae[0], &from_glb[0]);
        // The same motion as the FBX's conversion (the DAE may be trimmed
        // differently: compared from its end, frame by frame)
        let head = dst.node("Head").unwrap();
        let (mut pa, mut pb) = (Pose::rest(&dst), Pose::rest(&dst));
        let mut lowest = f32::MAX;
        let mut worst = 0.0f32;
        for f in 0..=(a.duration * 30.0) as usize {
            let t = f as f32 / 30.0;
            pa.reset(&dst);
            a.apply(&mut pa, t, 1.0);
            pa.update_world(&dst);
            lowest = lowest.min(pa.position(head).y);
            let tb = b.duration - (a.duration - t);
            if tb >= 0.0 {
                pb.reset(&dst);
                b.apply(&mut pb, tb, 1.0);
                pb.update_world(&dst);
                worst = worst.max((pa.position(head) - pb.position(head)).length());
            }
        }
        println!("DAE on the avatar: head lowest {lowest:.2} m; furthest from the FBX's: {worst:.3} m");
        assert!(lowest < 0.3, "the head didn't reach the floor");
        assert!(worst < 0.05, "not the FBX's motion");
    }
}
