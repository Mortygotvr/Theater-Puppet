//! The avatar as Theater Rig exports it (GLB), read into plain arrays the GPU
//! side uploads. Skinned meshes are placed by their skin at rest, so the
//! avatar stands as it does in Blender. No OBS here: it loads off the
//! graphics thread and is tested alone.

use std::path::Path;

use glam::{Mat4, Quat, Vec3};

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alpha {
    Opaque,
    /// Cut out below this alpha
    Mask(f32),
    Blend,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    /// Multiplies the texture (the glTF base colour factor, or a colour
    /// slot's color_default)
    pub color: [f32; 4],
    /// The shadow colour (MToon's shade colour), times the texture
    pub shade: [f32; 3],
    pub image: Option<usize>,
    pub alpha: Alpha,
    /// Color<N> in its name: follows colour slot N
    pub color_slot: Option<u32>,
}

pub struct Primitive {
    /// At rest (skinned), in the avatar's space
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Each vertex's body part (an index into PARTS)
    pub parts: Vec<f32>,
    pub indices: Vec<u32>,
    pub material: usize,
    /// How it follows the skeleton (pose.rs)
    pub binding: Binding,
    /// Each vertex's node that moves it most (colliders are fitted to them)
    pub dominant: Vec<usize>,
    /// Its shape keys (glTF morph targets), each with the vertices it moves
    pub morphs: Vec<Morph>,
}

/// A shape key of one primitive: what it adds to the (unposed) positions at
/// full weight, for the vertices it moves only
pub struct Morph {
    /// Its name's place in Model::morph_names
    pub name: usize,
    pub deltas: Vec<(u32, [f32; 3])>,
}

/// How a primitive follows a pose
pub enum Binding {
    /// Skinned: its own (bind) positions and normals, blended by its joints
    Skin { skin: usize, positions: Vec<[f32; 3]>, normals: Vec<[f32; 3]>, joints: Vec<[u16; 4]>, weights: Vec<[f32; 4]> },
    /// Moves with one node (its own positions, before the node's matrix)
    Node { node: usize, positions: Vec<[f32; 3]>, normals: Vec<[f32; 3]> },
}

/// A node of the glTF scene: a bone, a mesh's node, the armature
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    /// glTF extras (Theater Rig keeps spring settings here)
    pub extras: serde_json::Value,
}

pub struct Skin {
    /// Node indices, in the skin's joint order
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

/// The body parts Theater Scene can aim at and hear about
pub const PARTS: [&str; 10] = [
    "torso", "head", "left_arm", "right_arm", "left_hand", "right_hand", "left_leg", "right_leg", "left_foot", "right_foot",
];

/// The part a Mixamo bone is (Theater Rig's names; "mixamorig:" allowed),
/// None for the rest (springs, accessories): they take their parent's
pub fn bone_part(name: &str) -> Option<usize> {
    let n = name.strip_prefix("mixamorig:").unwrap_or(name);
    Some(match n {
        "Hips" | "Spine" | "Spine1" | "Spine2" | "LeftShoulder" | "RightShoulder" => 0,
        "Neck" | "Head" | "HeadTop_End" | "LeftEye" | "RightEye" | "Jaw" => 1,
        "LeftArm" | "LeftForeArm" => 2,
        "RightArm" | "RightForeArm" => 3,
        "LeftUpLeg" | "LeftLeg" => 6,
        "RightUpLeg" | "RightLeg" => 7,
        _ if n.starts_with("LeftHand") => 4,
        _ if n.starts_with("RightHand") => 5,
        _ if n.starts_with("LeftFoot") || n.starts_with("LeftToe") => 8,
        _ if n.starts_with("RightFoot") || n.starts_with("RightToe") => 9,
        _ => return None,
    })
}

/// Every node's part: its own, or the nearest ancestor's that has one
/// (hair springs hang under the head, the skirt under the hips); torso when none
fn node_parts(doc: &gltf::Document) -> Vec<usize> {
    let count = doc.nodes().len();
    let mut parent = vec![None; count];
    for node in doc.nodes() {
        for child in node.children() {
            parent[child.index()] = Some(node.index());
        }
    }
    let own: Vec<Option<usize>> = doc.nodes().map(|n| n.name().and_then(bone_part)).collect();
    (0..count)
        .map(|mut i| loop {
            if let Some(p) = own[i] {
                break p;
            }
            match parent[i] {
                Some(up) => i = up,
                None => break 0,
            }
        })
        .collect()
}

pub struct Model {
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub min: Vec3,
    pub max: Vec3,
    pub nodes: Vec<Node>,
    /// Node indices, every parent before its children
    pub order: Vec<usize>,
    pub skins: Vec<Skin>,
    /// Every shape key's name, once (a face's ARKit keys, mostly)
    pub morph_names: Vec<String>,
    /// Its animations, made in Blender (animation.rs)
    pub clips: Vec<crate::animation::Clip>,
}

impl Model {
    /// The shape key called `name` (any case), as its place in morph_names
    pub fn morph(&self, name: &str) -> Option<usize> {
        self.morph_names.iter().position(|n| n.eq_ignore_ascii_case(name))
    }

    /// The node called `name`
    pub fn node(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name || n.name.strip_prefix("mixamorig:") == Some(name))
    }
}

/// Color<N> anywhere in a name, any case (the Rig's convention)
pub fn color_slot(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("color") {
        let digits: String = rest[i + 5..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
        rest = &rest[i + 5..];
    }
    None
}

pub fn load(path: &Path) -> Result<Model, String> {
    let (doc, buffers, images) = gltf::import(path).map_err(|e| format!("{}: {e}", path.display()))?;

    // Every node's place in the scene
    let mut world = vec![Mat4::IDENTITY; doc.nodes().len()];
    fn walk(node: &gltf::Node, parent: Mat4, world: &mut [Mat4]) {
        let m = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        world[node.index()] = m;
        for child in node.children() {
            walk(&child, m, world);
        }
    }
    for scene in doc.scenes() {
        for node in scene.nodes() {
            walk(&node, Mat4::IDENTITY, &mut world);
        }
    }

    // The skeleton: every node's rest transform and parent, parents first
    let mut parent = vec![None; doc.nodes().len()];
    for node in doc.nodes() {
        for child in node.children() {
            parent[child.index()] = Some(node.index());
        }
    }
    let nodes: Vec<Node> = doc
        .nodes()
        .map(|n| {
            let (t, r, s) = n.transform().decomposed();
            Node {
                name: n.name().unwrap_or("").to_string(),
                parent: parent[n.index()],
                translation: Vec3::from(t),
                rotation: Quat::from_array(r).normalize(),
                scale: Vec3::from(s),
                extras: n.extras().as_ref().and_then(|e| serde_json::from_str(e.get()).ok()).unwrap_or_default(),
            }
        })
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    fn visit(i: usize, children: &[Vec<usize>], order: &mut Vec<usize>) {
        order.push(i);
        for &c in &children[i] {
            visit(c, children, order);
        }
    }
    let mut children = vec![Vec::new(); nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            children[p].push(i);
        }
    }
    for i in (0..nodes.len()).filter(|&i| nodes[i].parent.is_none()) {
        visit(i, &children, &mut order);
    }
    let skins: Vec<Skin> = doc
        .skins()
        .map(|skin| Skin {
            joints: skin.joints().map(|j| j.index()).collect(),
            inverse_bind: skin
                .reader(|b| Some(&buffers[b.index()]))
                .read_inverse_bind_matrices()
                .map(|it| it.map(|m| Mat4::from_cols_array_2d(&m)).collect())
                .unwrap_or_else(|| vec![Mat4::IDENTITY; skin.joints().count()]),
        })
        .collect();

    let mut materials: Vec<Material> = doc.materials().map(material).collect();
    let default_material = materials.len();
    materials.push(Material { name: "default".into(), color: [1.0; 4], shade: [0.7; 3], image: None, alpha: Alpha::Opaque, color_slot: None });

    let parts_of = node_parts(&doc);
    let mut morph_names: Vec<String> = Vec::new();
    let mut primitives = Vec::new();
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        // Each joint's part, in the skin's joint order
        let joint_parts: Vec<usize> = node.skin().map(|s| s.joints().map(|j| parts_of[j.index()]).collect()).unwrap_or_default();
        // A skinned mesh is placed by its joints, not its node
        let joints: Option<Vec<Mat4>> = node.skin().map(|skin| {
            let inverse: Vec<Mat4> = skin
                .reader(|b| Some(&buffers[b.index()]))
                .read_inverse_bind_matrices()
                .map(|it| it.map(|m| Mat4::from_cols_array_2d(&m)).collect())
                .unwrap_or_default();
            skin.joints()
                .enumerate()
                .map(|(i, j)| world[j.index()] * inverse.get(i).copied().unwrap_or(Mat4::IDENTITY))
                .collect()
        });
        let target_names: Vec<String> = mesh
            .extras()
            .as_ref()
            .and_then(|e| serde_json::from_str::<serde_json::Value>(e.get()).ok())
            .and_then(|v| Some(v.get("targetNames")?.as_array()?.iter().map(|n| n.as_str().unwrap_or("").to_string()).collect()))
            .unwrap_or_default();
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| Some(&buffers[b.index()]));
            let Some(positions) = reader.read_positions() else { continue };
            let positions: Vec<[f32; 3]> = positions.collect();
            let n = positions.len();
            let normals: Vec<[f32; 3]> = reader.read_normals().map(|it| it.collect()).unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; n]);
            let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).map(|it| it.into_f32().collect()).unwrap_or_else(|| vec![[0.0; 2]; n]);
            let indices: Vec<u32> = reader.read_indices().map(|it| it.into_u32().collect()).unwrap_or_else(|| (0..n as u32).collect());

            // Each vertex's own matrix: its joints blended by weight, or the node's
            let skin: Option<(Vec<[u16; 4]>, Vec<[f32; 4]>)> = joints.as_ref().and_then(|_| {
                Some((reader.read_joints(0)?.into_u16().collect(), reader.read_weights(0)?.into_f32().collect()))
            });
            let node_m = world[node.index()];
            let mut out_p = Vec::with_capacity(n);
            let mut out_n = Vec::with_capacity(n);
            // The node that moves each vertex most, and so its body part
            let skin_joints: Vec<usize> = node.skin().map(|s| s.joints().map(|j| j.index()).collect()).unwrap_or_default();
            let dominant: Vec<usize> = (0..n)
                .map(|i| match &skin {
                    Some((js, ws)) => {
                        let k = (0..4).max_by(|&a, &b| ws[i][a].total_cmp(&ws[i][b])).unwrap_or(0);
                        skin_joints.get(js[i][k] as usize).copied().unwrap_or(node.index())
                    }
                    None => node.index(),
                })
                .collect();
            let out_parts: Vec<f32> = (0..n)
                .map(|i| match &skin {
                    Some((js, ws)) => {
                        let k = (0..4).max_by(|&a, &b| ws[i][a].total_cmp(&ws[i][b])).unwrap_or(0);
                        joint_parts.get(js[i][k] as usize).copied().unwrap_or(0) as f32
                    }
                    None => parts_of[node.index()] as f32,
                })
                .collect();
            for i in 0..n {
                let m = match (&joints, &skin) {
                    (Some(jm), Some((js, ws))) => {
                        let mut m = Mat4::ZERO;
                        for k in 0..4 {
                            if ws[i][k] > 0.0 {
                                m += jm.get(js[i][k] as usize).copied().unwrap_or(Mat4::IDENTITY) * ws[i][k];
                            }
                        }
                        if m == Mat4::ZERO { node_m } else { m }
                    }
                    _ => node_m,
                };
                let p = m.transform_point3(Vec3::from(positions[i]));
                let nn = m.transform_vector3(Vec3::from(normals[i])).normalize_or(Vec3::Y);
                min = min.min(p);
                max = max.max(p);
                out_p.push(p.to_array());
                out_n.push(nn.to_array());
            }
            // Its shape keys, named by the mesh's extras.targetNames (Blender's exporter)
            let morphs: Vec<Morph> = reader
                .read_morph_targets()
                .enumerate()
                .filter_map(|(t, (deltas, _, _))| {
                    let name = target_names.get(t).cloned().unwrap_or_else(|| format!("target_{t}"));
                    let deltas: Vec<(u32, [f32; 3])> =
                        deltas?.enumerate().filter(|(_, d)| d.iter().any(|x| x.abs() > 1e-7)).map(|(i, d)| (i as u32, d)).collect();
                    let name = morph_names.iter().position(|n| *n == name).unwrap_or_else(|| {
                        morph_names.push(name);
                        morph_names.len() - 1
                    });
                    Some(Morph { name, deltas })
                })
                .collect();
            let binding = match (node.skin(), skin) {
                (Some(s), Some((js, ws))) => Binding::Skin { skin: s.index(), positions, normals, joints: js, weights: ws },
                _ => Binding::Node { node: node.index(), positions, normals },
            };
            primitives.push(Primitive {
                positions: out_p,
                normals: out_n,
                uvs,
                parts: out_parts,
                indices,
                material: prim.material().index().unwrap_or(default_material),
                binding,
                dominant,
                morphs,
            });
        }
    }
    if primitives.is_empty() {
        return Err(format!("{}: no triangle meshes", path.display()));
    }
    // A material whose image is missing draws untextured
    for m in &mut materials {
        if m.image.is_some_and(|i| i >= images.len()) {
            m.image = None;
        }
    }
    let clips = crate::animation::load(&doc, &buffers, &morph_names);
    Ok(Model { primitives, materials, images: images.into_iter().map(rgba).collect(), min, max, nodes, order, skins, morph_names, clips })
}

fn material(m: gltf::Material) -> Material {
    let pbr = m.pbr_metallic_roughness();
    let name = m.name().unwrap_or("").to_string();
    let mut color = pbr.base_color_factor();
    let color_slot = color_slot(&name);
    let extras: serde_json::Value = m.extras().as_ref().and_then(|e| serde_json::from_str(e.get()).ok()).unwrap_or_default();
    let rgb = |key: &str| -> Option<[f32; 3]> {
        let a = extras.get(key)?.as_array()?;
        Some([0, 1, 2].map(|i| a.get(i).and_then(|x| x.as_f64()).unwrap_or(1.0) as f32))
    };
    // A colour slot's material is plain grey; until the slot gets a colour
    // it shows the one it had (Theater Rig keeps it as color_default)
    if color_slot.is_some() {
        if let Some(c) = rgb("color_default") {
            for k in 0..3 {
                color[k] *= c[k];
            }
        }
    }
    // MToon's shadow colour (Theater Rig's shade_color), times the texture
    // like the base colour; a plain GLB gets a darker base colour
    let mut shade = rgb("shade_color").unwrap_or([color[0] * 0.7, color[1] * 0.7, color[2] * 0.7]);
    // glTF factors are linear; the textures, and the shading here, are sRGB
    for c in color[..3].iter_mut().chain(shade.iter_mut()) {
        *c = linear_to_srgb(*c);
    }
    let alpha = match m.alpha_mode() {
        gltf::material::AlphaMode::Opaque => Alpha::Opaque,
        gltf::material::AlphaMode::Mask => Alpha::Mask(m.alpha_cutoff().unwrap_or(0.5)),
        gltf::material::AlphaMode::Blend => Alpha::Blend,
    };
    Material {
        name,
        color,
        shade,
        image: pbr.base_color_texture().map(|t| t.texture().source().index()),
        alpha,
        color_slot,
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Any decoded glTF image as 8-bit RGBA
fn rgba(img: gltf::image::Data) -> Image {
    use gltf::image::Format;
    let (w, h, p) = (img.width, img.height, img.pixels);
    let rgba = match img.format {
        Format::R8G8B8A8 => p,
        Format::R8G8B8 => p.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        Format::R8G8 => p.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        Format::R8 => p.iter().flat_map(|&c| [c, c, c, 255]).collect(),
        // 16-bit and float images: white, until an avatar needs them
        _ => vec![255; (w * h * 4) as usize],
    };
    Image { width: w, height: h, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bones_name_their_parts() {
        let part = |n: &str| bone_part(n).map(|i| PARTS[i]);
        assert_eq!(part("Head"), Some("head"));
        assert_eq!(part("mixamorig:LeftForeArm"), Some("left_arm"));
        assert_eq!(part("LeftHandIndex2"), Some("left_hand"));
        assert_eq!(part("RightToeBase"), Some("right_foot"));
        assert_eq!(part("Spine2"), Some("torso"));
        assert_eq!(part("spring_hair_1_3"), None);
    }

    #[test]
    fn color_slots() {
        assert_eq!(color_slot("N00_Hair (Instance)_Color2"), Some(2));
        assert_eq!(color_slot("_17_color12.png"), Some(12));
        assert_eq!(color_slot("ColorScheme"), None);
        assert_eq!(color_slot("Body"), None);
    }

    /// THEATER_PUPPET_GLB=<a Rig export> cargo test -- --nocapture
    #[test]
    fn loads_a_rig_export() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let m = load(Path::new(&path)).unwrap();
        let verts: usize = m.primitives.iter().map(|p| p.positions.len()).sum();
        let tris: usize = m.primitives.iter().map(|p| p.indices.len() / 3).sum();
        println!("{} primitives, {verts} vertices, {tris} triangles, {} materials, {} images", m.primitives.len(), m.materials.len(), m.images.len());
        println!("bounds {:?} .. {:?} (size {:?})", m.min, m.max, m.max - m.min);
        for mat in &m.materials {
            println!("  {:50} {:?} image {:?} colour {:?} shade {:?} slot {:?}", mat.name, mat.alpha, mat.image, mat.color, mat.shade, mat.color_slot);
        }
        // Every part is somewhere, and where a person has it
        let mut sum = vec![(0usize, Vec3::ZERO); PARTS.len()];
        for p in &m.primitives {
            for (pos, part) in p.positions.iter().zip(&p.parts) {
                let e = &mut sum[*part as usize];
                e.0 += 1;
                e.1 += Vec3::from(*pos);
            }
        }
        let centre = |name: &str| {
            let (n, s) = sum[PARTS.iter().position(|p| *p == name).unwrap()];
            assert!(n > 0, "no vertices for {name}");
            s / n as f32
        };
        for name in PARTS {
            println!("  {name:11} {:6} vertices, centre {:?}", sum[PARTS.iter().position(|p| *p == name).unwrap()].0, centre(name));
        }
        assert!(centre("head").y > centre("torso").y && centre("torso").y > centre("left_leg").y);
        assert!(centre("left_foot").y < centre("left_leg").y);
        // The avatar's left is +X in glTF (it faces +Z)
        assert!(centre("left_hand").x > centre("left_arm").x && centre("left_arm").x > 0.0);
        assert!(centre("right_hand").x < centre("right_arm").x && centre("right_arm").x < 0.0);

        let size = m.max - m.min;
        // A person: about 1-2 m tall, standing up (Y), arms out (T-pose)
        assert!(size.y > 0.5 && size.y < 3.0, "height {}", size.y);
        assert!(m.images.iter().all(|i| i.rgba.len() == (i.width * i.height * 4) as usize));
    }
}
