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
    /// How much darker its shade is than its colour (brightness, 0–1): a
    /// slot given a new colour gets it this much darker as its shade
    pub shade_ratio: f32,
    /// Its texture's placement (KHR_texture_transform): scale x, y, offset
    /// x, y; uv' = uv * scale + offset. A tile (tiles.rs) moves it
    pub uv: [f32; 4],
    /// What it gives off by itself (sRGB), times its emissive texture: a
    /// baked or glowing material shows without light
    pub emissive: [f32; 3],
    pub emissive_image: Option<usize>,
    /// Its normal map (tangent space, glTF's +Y), and how strong
    pub normal_image: Option<usize>,
    pub normal_scale: f32,
    /// Its gloss: metalness and roughness (0–1), times its
    /// metallic-roughness texture's blue and green
    pub metallic: f32,
    pub roughness: f32,
    pub mr_image: Option<usize>,
    /// Its coat (Blender's Principled Coat, glTF's KHR_materials_clearcoat):
    /// a clear varnish over everything, how much (0 none) and how rough
    pub coat: f32,
    pub coat_roughness: f32,
    /// A liquid filling a container (water.rs): its waves
    pub water: Option<crate::water::Water>,
}

impl Material {
    /// A plain material (no texture, no gloss, nothing given off)
    pub fn plain(name: &str, color: [f32; 4], shade: [f32; 3]) -> Material {
        Material {
            name: name.into(),
            color,
            shade,
            image: None,
            alpha: Alpha::Opaque,
            color_slot: None,
            shade_ratio: 0.7,
            uv: UV_IDENTITY,
            emissive: [0.0; 3],
            emissive_image: None,
            normal_image: None,
            normal_scale: 1.0,
            metallic: 0.0,
            roughness: 1.0,
            mr_image: None,
            coat: 0.0,
            coat_roughness: 0.03,
            water: None,
        }
    }
}

/// No texture placement: the texture as it is
pub const UV_IDENTITY: [f32; 4] = [1.0, 1.0, 0.0, 0.0];

/// A tile texture: a named picture of a texture sheet (Theater Rig's Make
/// Tiles, or a VRM expression it converted), each material's placement when
/// it shows. Tiles of a set go together ("next", "random"), in their order
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub name: String,
    pub set: String,
    pub order: i64,
    pub binds: Vec<(usize, [f32; 4])>,
}

/// The tiles in Theater Rig's `theater_tiles` (JSON, in a node's extras),
/// their materials found by name: {name: {set, order, binds}}, or the old
/// form {name: binds} (the set "expressions")
pub fn read_tiles(extras: &serde_json::Value, materials: &[Material]) -> Vec<Tile> {
    let data = match extras.get("theater_tiles") {
        Some(serde_json::Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
        Some(v) => v.clone(),
        None => return Vec::new(),
    };
    let Some(map) = data.as_object() else { return Vec::new() };
    let pair = |v: Option<&serde_json::Value>, d: f32| -> [f32; 2] {
        let a = v.and_then(|v| v.as_array());
        [0, 1].map(|i| a.and_then(|a| a.get(i)).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d))
    };
    let mut out: Vec<Tile> = map
        .iter()
        .map(|(name, v)| {
            let (set, order, binds) = match v.get("binds") {
                Some(b) => (v.get("set").and_then(|s| s.as_str()).unwrap_or("").to_string(), v.get("order").and_then(|o| o.as_i64()).unwrap_or(0), b),
                None => ("expressions".to_string(), 0, v),
            };
            (name, set, order, binds)
        })
        .map(|(name, set, order, binds)| Tile {
            name: name.clone(),
            set,
            order,
            binds: binds
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    let mat = b.get("material")?.as_str()?;
                    let i = materials.iter().position(|m| m.name == mat)?;
                    let (s, o) = (pair(b.get("scale"), 1.0), pair(b.get("offset"), 0.0));
                    Some((i, [s[0], s[1], o[0], o[1]]))
                })
                .collect(),
        })
        .filter(|t| !t.binds.is_empty())
        .collect();
    out.sort_by(|a, b| (&a.set, a.order, &a.name).cmp(&(&b.set, b.order, &b.name)));
    out
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
    /// Its file's name, without .glb
    pub name: String,
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
    /// Its default animation (Theater Rig's "Default animation", tagged
    /// `theater_default`): looped while the source's Animation setting is
    /// left on its default
    pub default_clip: Option<String>,
    /// Its tile textures (VRM expressions moving textures)
    pub tiles: Vec<Tile>,
}

impl Model {
    /// No avatar: nothing drawn, no bones. A source with only Also draw
    /// GLBs (props, parts) has this as its avatar
    pub fn empty() -> Model {
        Model {
            name: String::new(),
            primitives: Vec::new(),
            materials: vec![Material::plain("default", [1.0; 4], [0.7; 3])],
            images: Vec::new(),
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            nodes: Vec::new(),
            order: Vec::new(),
            skins: Vec::new(),
            morph_names: Vec::new(),
            clips: Vec::new(),
            tiles: Vec::new(),
            default_clip: None,
        }
    }

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

/// A GLB whose all-zero accessors (no bufferView and no sparse: zeros, by
/// glTF's rules; Blender writes a shape key that moves nothing that way) are
/// given a block of zeros, as the gltf crate wants data behind each one. None
/// when it has none (or isn't a GLB with its own binary chunk): read as it is.
fn zeros_filled_in(bytes: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if bytes.get(0..4) != Some(b"glTF") {
        return None;
    }
    let json_len = u32_at(12)?;
    if u32_at(16)? != 0x4E4F_534A {
        return None;
    }
    let mut json: serde_json::Value = serde_json::from_slice(bytes.get(20..20 + json_len)?).ok()?;
    // The binary chunk (none: an empty one)
    let bin_at = 20 + json_len;
    let mut bin: Vec<u8> = match (u32_at(bin_at), u32_at(bin_at + 4)) {
        (Some(len), Some(0x004E_4942)) => bytes.get(bin_at + 8..bin_at + 8 + len)?.to_vec(),
        _ => Vec::new(),
    };
    let size = |a: &serde_json::Value| -> Option<usize> {
        let component = match a.get("componentType")?.as_u64()? {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            _ => 4,
        };
        let n = match a.get("type")?.as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" | "MAT2" => 4,
            "MAT3" => 9,
            _ => 16,
        };
        Some(a.get("count")?.as_u64()? as usize * component * n)
    };
    let empty: Vec<usize> = json.get("accessors")?.as_array()?.iter().enumerate().filter(|(_, a)| a.get("bufferView").is_none() && a.get("sparse").is_none()).map(|(i, _)| i).collect();
    if empty.is_empty() {
        return None;
    }
    // A buffer with a file of its own isn't the binary chunk: left alone
    if json.get("buffers").and_then(|b| b.get(0)).and_then(|b| b.get("uri")).is_some() {
        return None;
    }
    for i in empty {
        let length = size(&json["accessors"][i])?;
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let offset = bin.len();
        bin.resize(offset + length, 0);
        let views = json.as_object_mut()?.entry("bufferViews").or_insert_with(|| serde_json::json!([]));
        let view = views.as_array()?.len();
        views.as_array_mut()?.push(serde_json::json!({ "buffer": 0, "byteOffset": offset, "byteLength": length.max(1) }));
        json["accessors"][i]["bufferView"] = serde_json::json!(view);
        json["accessors"][i]["byteOffset"] = serde_json::json!(0);
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let buffers = json.as_object_mut()?.entry("buffers").or_insert_with(|| serde_json::json!([{}]));
    if buffers.as_array()?.is_empty() {
        buffers.as_array_mut()?.push(serde_json::json!({}));
    }
    buffers[0]["byteLength"] = serde_json::json!(bin.len());
    let mut text = serde_json::to_vec(&json).ok()?;
    while text.len() % 4 != 0 {
        text.push(b' ');
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    Some(out)
}

pub fn load(path: &Path) -> Result<Model, String> {
    load_with(path, true)
}

/// An animation GLB (Theater Rig's conversion: a skeleton and its clips, no
/// mesh): its nodes and clips (retarget.rs)
pub fn load_animation(path: &Path) -> Result<Model, String> {
    load_with(path, false)
}

fn load_with(path: &Path, meshes_needed: bool) -> Result<Model, String> {
    // (A GLB with all-zero accessors: read with zeros filled in)
    let fixed = std::fs::read(path).ok().and_then(|b| zeros_filled_in(&b));
    let imported = match &fixed {
        Some(bytes) => gltf::import_slice(bytes),
        None => gltf::import(path),
    };
    let (doc, buffers, images) = imported.map_err(|e| format!("{}: {e}", path.display()))?;

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
    materials.push(Material::plain("default", [1.0; 4], [0.7; 3]));

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
    if meshes_needed && primitives.is_empty() {
        return Err(format!("{}: no triangle meshes", path.display()));
    }
    // A material whose image is missing draws untextured
    for m in &mut materials {
        for image in [&mut m.image, &mut m.emissive_image, &mut m.normal_image, &mut m.mr_image] {
            if image.is_some_and(|i| i >= images.len()) {
                *image = None;
            }
        }
    }
    let clips = crate::animation::load(&doc, &buffers, &morph_names);
    let default_clip = crate::animation::default_of(&doc).filter(|d| clips.iter().any(|c| c.name == *d));
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // Theater Rig keeps them on the armature's node
    let tiles = nodes.iter().map(|n| read_tiles(&n.extras, &materials)).find(|t| !t.is_empty()).unwrap_or_default();
    Ok(Model { name, tiles, primitives, materials, images: images.into_iter().map(rgba).collect(), min, max, nodes, order, skins, morph_names, clips, default_clip })
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
    // it shows the one it had (Theater Rig keeps it as color_default). That's
    // the colour as the texture showed it (sRGB, measured on its pixels), not
    // a linear factor: made linear here, as the factor it multiplies, so it
    // comes back as it was (read as linear, an orange came out pale yellow)
    if color_slot.is_some() {
        if let Some(c) = rgb("color_default") {
            for k in 0..3 {
                color[k] *= srgb_to_linear(c[k]);
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
    let brightness = |c: &[f32]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let shade_ratio = (brightness(&shade) / brightness(&color[..3]).max(1e-4)).clamp(0.0, 1.0);
    let uv = pbr
        .base_color_texture()
        .and_then(|t| t.texture_transform())
        .map(|t| [t.scale()[0], t.scale()[1], t.offset()[0], t.offset()[1]])
        .unwrap_or(UV_IDENTITY);
    let emissive = m.emissive_factor().map(linear_to_srgb);
    let (coat, coat_roughness) = read_coat(&m);
    Material {
        name,
        color,
        shade,
        image: pbr.base_color_texture().map(|t| t.texture().source().index()),
        alpha,
        color_slot,
        shade_ratio,
        uv,
        emissive,
        emissive_image: m.emissive_texture().map(|t| t.texture().source().index()),
        normal_image: m.normal_texture().map(|t| t.texture().source().index()),
        normal_scale: m.normal_texture().map_or(1.0, |t| t.scale()),
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        mr_image: pbr.metallic_roughness_texture().map(|t| t.texture().source().index()),
        coat,
        coat_roughness,
        water: crate::water::Water::from_extras(&extras),
    }
}

/// A material's coat (KHR_materials_clearcoat): how much and how rough,
/// glTF's defaults (0, 0) when a factor is left out. (Its textures aren't read:
/// the factors are the whole coat)
fn read_coat(m: &gltf::Material) -> (f32, f32) {
    let ext = m.extensions().and_then(|e| e.get("KHR_materials_clearcoat"));
    let factor = |k: &str| ext.and_then(|e| e.get(k)).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    (factor("clearcoatFactor").clamp(0.0, 1.0), factor("clearcoatRoughnessFactor").clamp(0.0, 1.0))
}

/// A material with its slot's new colour (sRGB, 0–1): its shade the same
/// colour, as much darker as its own shade was
pub fn recoloured(m: &Material, rgb: [f32; 3]) -> Material {
    Material { color: [rgb[0], rgb[1], rgb[2], m.color[3]], shade: rgb.map(|c| c * m.shade_ratio), ..m.clone() }
}

/// A material as a slot's look shows it: `c` (its `a` or `b`) plus the
/// look's share of the material's own colour and shade (own 1, nothing else:
/// the material exactly as it is)
pub fn looked(m: &Material, look: &crate::colorfx::Look, c: [f32; 3]) -> Material {
    let mut out = recoloured(m, c);
    if look.own > 0.0 {
        for k in 0..3 {
            out.color[k] += look.own * m.color[k];
            out.shade[k] += look.own * m.shade[k];
        }
    }
    out
}

fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
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
        // 16-bit (a sculpted normal map, often) and float images, to 8 bits
        Format::R16 | Format::R16G16 | Format::R16G16B16 | Format::R16G16B16A16 => {
            let channels = match img.format {
                Format::R16 => 1,
                Format::R16G16 => 2,
                Format::R16G16B16 => 3,
                _ => 4,
            };
            let values: Vec<u8> = p.chunks_exact(2).map(|b| ((u16::from_ne_bytes([b[0], b[1]]) as u32 + 128) / 257) as u8).collect();
            widen(&values, channels)
        }
        Format::R32G32B32FLOAT | Format::R32G32B32A32FLOAT => {
            let channels = if img.format == Format::R32G32B32FLOAT { 3 } else { 4 };
            let values: Vec<u8> = p.chunks_exact(4).map(|b| (f32::from_ne_bytes([b[0], b[1], b[2], b[3]]).clamp(0.0, 1.0) * 255.0).round() as u8).collect();
            widen(&values, channels)
        }
    };
    Image { width: w, height: h, rgba }
}

/// 8-bit values of `channels` per pixel as RGBA (grey and grey-alpha
/// spread, no alpha: opaque)
fn widen(values: &[u8], channels: usize) -> Vec<u8> {
    values
        .chunks_exact(channels)
        .flat_map(|c| match channels {
            1 => [c[0], c[0], c[0], 255],
            2 => [c[0], c[0], c[0], c[1]],
            3 => [c[0], c[1], c[2], 255],
            _ => [c[0], c[1], c[2], c[3]],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glb_with_all_zero_accessors_loads() {
        // A triangle with a shape key whose normals move nothing: Blender
        // writes those with no bufferView (zeros, by glTF's rules)
        let positions: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "scene": 0, "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "targets": [{ "POSITION": 1, "NORMAL": 2 }] }], "extras": { "targetNames": ["Moves"] } }],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] },
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] },
                { "componentType": 5126, "count": 3, "type": "VEC3" }
            ],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }],
            "buffers": [{ "byteLength": 36 }]
        });
        let mut text = serde_json::to_vec(&json).unwrap();
        while text.len() % 4 != 0 {
            text.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((12 + 8 + text.len() + 8 + 36) as u32).to_le_bytes());
        glb.extend_from_slice(&(text.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        glb.extend_from_slice(&text);
        glb.extend_from_slice(&36u32.to_le_bytes());
        glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        glb.extend_from_slice(&positions);
        let path = std::env::temp_dir().join("theater_puppet_zero_accessor.glb");
        std::fs::write(&path, &glb).unwrap();
        // As the gltf crate reads it alone: refused
        assert!(gltf::import(&path).is_err());
        // As Puppet reads it: loaded, the shape key there
        let m = load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(m.primitives.len(), 1);
        assert_eq!(m.morph_names, vec!["Moves".to_string()]);
        // A GLB with nothing to fill in is read as it is
        assert!(zeros_filled_in(b"not a glb").is_none());
    }

    #[test]
    fn coat_is_read() {
        // Blender's Coat 0.8, roughness 0.1; and one with the factor left out (glTF: none)
        let json = r#"{"asset":{"version":"2.0"},"extensionsUsed":["KHR_materials_clearcoat"],"materials":[
            {"name":"Varnish","extensions":{"KHR_materials_clearcoat":{"clearcoatFactor":0.8,"clearcoatRoughnessFactor":0.1}}},
            {"name":"Bare","extensions":{"KHR_materials_clearcoat":{"clearcoatRoughnessFactor":0}}},
            {"name":"Plain"}]}"#;
        let g = gltf::Gltf::from_slice(json.as_bytes()).unwrap();
        let coats: Vec<(f32, f32)> = g.materials().map(|m| (material(m).coat, material(g.materials().nth(0).unwrap()).coat_roughness)).collect();
        assert_eq!(coats.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0.8, 0.0, 0.0]);
        assert!((coats[0].1 - 0.1).abs() < 1e-6);
    }

    #[test]
    fn sixteen_bit_and_float_images_are_read() {
        // A 16-bit RGBA pixel (a normal map's flat blue) and a float RGB one
        let px16: Vec<u8> = [32768u16, 32768, 65535, 65535].iter().flat_map(|v| v.to_ne_bytes()).collect();
        let img = rgba(gltf::image::Data { pixels: px16, format: gltf::image::Format::R16G16B16A16, width: 1, height: 1 });
        assert_eq!(img.rgba, vec![128, 128, 255, 255]);
        let pxf: Vec<u8> = [0.5f32, 0.0, 1.0].iter().flat_map(|v| v.to_ne_bytes()).collect();
        let img = rgba(gltf::image::Data { pixels: pxf, format: gltf::image::Format::R32G32B32FLOAT, width: 1, height: 1 });
        assert_eq!(img.rgba, vec![128, 0, 255, 255]);
    }

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
    fn what_an_avatar_takes_in_memory() {
        let Ok(path) = std::env::var("THEATER_PUPPET_GLB") else { return };
        let m = load(Path::new(&path)).unwrap();
        let images: usize = m.images.iter().map(|i| i.rgba.len()).sum();
        let vertices: usize = m.primitives.iter().map(|p| p.positions.len()).sum();
        let morphs: usize = m.primitives.iter().flat_map(|p| p.morphs.iter()).map(|x| x.deltas.len()).sum();
        println!(
            "memory: textures {:.0} MB ({} images), {} vertices, {} shape key deltas (~{:.0} MB of meshes)",
            images as f64 / 1e6,
            m.images.len(),
            vertices,
            morphs,
            (vertices * 100 + morphs * 16) as f64 / 1e6
        );
    }

    #[test]
    fn tiles_kept_by_theater_rig_are_read() {
        // THEATER_PUPPET_TILED_GLB: HavocFFT.vrm through Theater Rig's Export
        let Ok(path) = std::env::var("THEATER_PUPPET_TILED_GLB") else { return };
        let m = load(Path::new(&path)).unwrap();
        let names: Vec<&str> = m.tiles.iter().map(|t| t.name.as_str()).collect();
        println!("tiles: {names:?}");
        assert_eq!(m.tiles.len(), 8);
        let giga = m.tiles.iter().find(|t| t.name == "Gigashad").unwrap();
        let skin = m.materials.iter().position(|x| x.name == "Wolf3D_Skin").unwrap();
        assert!(giga.binds.contains(&(skin, [0.25, 0.5, 0.75, 0.0])), "{:?}", giga.binds);
        assert_eq!(m.materials[skin].uv, UV_IDENTITY);
    }

    #[test]
    fn a_new_colour_keeps_its_shadow_the_same_colour_darker() {
        let m = Material { color_slot: Some(2), shade_ratio: 0.5, ..Material::plain("Hair_Color2", [0.8, 0.6, 0.4, 1.0], [0.4, 0.3, 0.2]) };
        let r = recoloured(&m, [0.2, 0.4, 1.0]);
        assert_eq!(r.color, [0.2, 0.4, 1.0, 1.0]);
        assert_eq!(r.shade, [0.1, 0.2, 0.5]);
        assert_eq!(r.color_slot, Some(2));
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

#[cfg(test)]
mod colour_slot_tests {
    #[test]
    fn srgb_and_linear_go_both_ways() {
        for c in [0.0f32, 0.02, 0.2055, 0.5, 0.6915, 1.0] {
            assert!((super::linear_to_srgb(super::srgb_to_linear(c)) - c).abs() < 1e-4, "{c}");
        }
    }

    /// THEATER_PUPPET_COLOUR_GLB=<a Rig export with a colour slot> (Amouri's: its
    /// iris, Color3, color_default [1.0, 0.6915, 0.2055]): it shows that colour
    #[test]
    fn a_colour_slot_shows_the_colour_it_had() {
        let Ok(p) = std::env::var("THEATER_PUPPET_COLOUR_GLB") else { return };
        let m = super::load(std::path::Path::new(&p)).unwrap();
        let mat = m.materials.iter().find(|m| m.color_slot == Some(3)).expect("no Color3");
        println!("{}: shows {:?}", mat.name, &mat.color[..3]);
        let want = [1.0, 0.6915, 0.2055];
        assert!(mat.color[..3].iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01), "not the colour it had");
    }
}

#[cfg(test)]
mod default_animation_tests {
    /// THEATER_PUPPET_DEFAULT_GLB=<an avatar exported by Theater Rig with "Breathe" as its Default animation>
    #[test]
    fn rigs_default_animation_is_read() {
        let Ok(p) = std::env::var("THEATER_PUPPET_DEFAULT_GLB") else { return };
        let m = super::load(std::path::Path::new(&p)).unwrap();
        println!("clips {:?}, default {:?}", m.clips.iter().map(|c| &c.name).collect::<Vec<_>>(), m.default_clip);
        assert_eq!(m.default_clip.as_deref(), Some("Breathe"));
    }
}
