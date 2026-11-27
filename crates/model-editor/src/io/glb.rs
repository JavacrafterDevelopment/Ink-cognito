//! glTF 2.0 / GLB importer (via the `gltf` crate) and spec-compliant pure-Rust
//! GLB & glTF 2.0 exporter with embedded PNG textures and preserved rigs/animations.

use crate::{
    io::encode_texture_png,
    model::{
        AlphaKind, AnimInterp, AnimOutput, AnimPath, MaterialSlot, MeshPart, ModelDoc, PreservedRig,
        RigAnimChannel, RigAnimSampler, RigAnimation, RigNode, RigSkin, TextureSlot,
    },
};
use base64::Engine as _;
use bevy::prelude::{Mat4, Quat, Vec3};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

/// Imports a `.glb` or `.gltf` file into a `ModelDoc`.
pub fn load(path: &Path) -> Result<ModelDoc, String> {
    let (document, buffers, images) =
        gltf::import(path).map_err(|e| format!("Couldn't import glTF/GLB '{}': {e}", path.display()))?;

    let mut doc = ModelDoc::empty();
    doc.materials.clear();

    // 1. Decode every embedded/external image to RGBA8.
    let mut image_to_tex: Vec<usize> = Vec::with_capacity(images.len());
    for (i, img) in images.iter().enumerate() {
        let name = document
            .images()
            .nth(i)
            .and_then(|im| im.name())
            .map(str::to_string)
            .unwrap_or_else(|| format!("texture_{i}"));
        let rgba = gltf_image_to_rgba8(img);
        let idx = doc.textures.len();
        doc.textures.push(TextureSlot::from_rgba(name, img.width, img.height, rgba));
        image_to_tex.push(idx);
    }

    // 2. Materials
    for (i, mat) in document.materials().enumerate() {
        let pbr = mat.pbr_metallic_roughness();
        let alpha_mode = match mat.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaKind::Opaque,
            gltf::material::AlphaMode::Mask => AlphaKind::Mask(mat.alpha_cutoff().unwrap_or(0.5)),
            gltf::material::AlphaMode::Blend => AlphaKind::Blend,
        };
        let tex_idx = |info: Option<gltf::texture::Info<'_>>| {
            info.and_then(|t| image_to_tex.get(t.texture().source().index()).copied())
        };
        let norm_idx = mat
            .normal_texture()
            .and_then(|t| image_to_tex.get(t.texture().source().index()).copied());

        doc.materials.push(MaterialSlot {
            name: mat.name().map(str::to_string).unwrap_or_else(|| format!("Material {i}")),
            base_color: pbr.base_color_factor(),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            emissive: mat.emissive_factor(),
            alpha_mode,
            double_sided: mat.double_sided(),
            base_color_texture: tex_idx(pbr.base_color_texture()),
            normal_texture: norm_idx,
            metallic_roughness_texture: tex_idx(pbr.metallic_roughness_texture()),
            emissive_texture: tex_idx(mat.emissive_texture()),
        });
    }
    if doc.materials.is_empty() {
        doc.materials.push(MaterialSlot::default());
    }

    // 3. Check if the file has skins or animations so we preserve the rig cleanly.
    let has_rig = document.skins().len() > 0 || document.animations().len() > 0;
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());

    if has_rig {
        doc.preserved_rig = Some(extract_rig(&document, &buffers, scene.as_ref()));
    }

    // 4. Walk scene nodes (or all meshes if scene is empty) to build `MeshPart`s.
    if let Some(scene) = scene {
        for node in scene.nodes() {
            visit_node(&node, Mat4::IDENTITY, &buffers, has_rig, &mut doc);
        }
    } else {
        for mesh in document.meshes() {
            extract_mesh_primitives(&mesh, None, Mat4::IDENTITY, has_rig, &buffers, &mut doc);
        }
    }

    Ok(doc)
}

fn gltf_image_to_rgba8(img: &gltf::image::Data) -> Vec<u8> {
    let px_count = (img.width as usize) * (img.height as usize);
    let mut out = Vec::with_capacity(px_count * 4);
    let p = &img.pixels;
    match img.format {
        gltf::image::Format::R8G8B8A8 => return p.clone(),
        gltf::image::Format::R8G8B8 => {
            for c in p.chunks_exact(3) {
                out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        gltf::image::Format::R8G8 => {
            for c in p.chunks_exact(2) {
                out.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
            }
        }
        gltf::image::Format::R8 => {
            for &g in p {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        gltf::image::Format::R16G16B16A16 => {
            for c in p.chunks_exact(8) {
                out.extend_from_slice(&[c[1], c[3], c[5], c[7]]);
            }
        }
        gltf::image::Format::R16G16B16 => {
            for c in p.chunks_exact(6) {
                out.extend_from_slice(&[c[1], c[3], c[5], 255]);
            }
        }
        gltf::image::Format::R16G16 => {
            for c in p.chunks_exact(4) {
                out.extend_from_slice(&[c[1], c[1], c[1], c[3]]);
            }
        }
        gltf::image::Format::R16 => {
            for c in p.chunks_exact(2) {
                out.extend_from_slice(&[c[1], c[1], c[1], 255]);
            }
        }
        gltf::image::Format::R32G32B32FLOAT => {
            for c in p.chunks_exact(12) {
                let r = (f32::from_le_bytes(c[0..4].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                let g = (f32::from_le_bytes(c[4..8].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                let b = (f32::from_le_bytes(c[8..12].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
        gltf::image::Format::R32G32B32A32FLOAT => {
            for c in p.chunks_exact(16) {
                let r = (f32::from_le_bytes(c[0..4].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                let g = (f32::from_le_bytes(c[4..8].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                let b = (f32::from_le_bytes(c[8..12].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                let a = (f32::from_le_bytes(c[12..16].try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8;
                out.extend_from_slice(&[r, g, b, a]);
            }
        }
    }
    out.resize(px_count * 4, 255);
    out
}

fn node_local_mat(node: &gltf::Node<'_>) -> Mat4 {
    let (t, r, s) = node.transform().decomposed();
    Mat4::from_scale_rotation_translation(
        Vec3::from_array(s),
        Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(),
        Vec3::from_array(t),
    )
}

fn visit_node(
    node: &gltf::Node<'_>,
    parent_mat: Mat4,
    buffers: &[gltf::buffer::Data],
    has_rig: bool,
    doc: &mut ModelDoc,
) {
    let world = parent_mat * node_local_mat(node);
    if let Some(mesh) = node.mesh() {
        extract_mesh_primitives(&mesh, node.name(), world, has_rig, buffers, doc);
    }
    for child in node.children() {
        visit_node(&child, world, buffers, has_rig, doc);
    }
}

fn extract_mesh_primitives(
    mesh: &gltf::Mesh<'_>,
    node_name: Option<&str>,
    world: Mat4,
    has_rig: bool,
    buffers: &[gltf::buffer::Data],
    doc: &mut ModelDoc,
) {
    let base_name = node_name
        .or_else(|| mesh.name())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Mesh {}", mesh.index() + 1));

    let prim_count = mesh.primitives().len();
    for prim in mesh.primitives() {
        let reader = prim.reader(|b| Some(&buffers[b.index()].0));
        let Some(pos_iter) = reader.read_positions() else {
            continue;
        };
        let mut positions: Vec<[f32; 3]> = pos_iter.collect();
        if positions.is_empty() {
            continue;
        }
        let mut normals: Vec<[f32; 3]> = reader
            .read_normals()
            .map(|it| it.collect())
            .unwrap_or_default();
        let uvs: Vec<[f32; 2]> = reader
            .read_tex_coords(0)
            .map(|tc| tc.into_f32().collect())
            .unwrap_or_default();
        let mut tangents: Option<Vec<[f32; 4]>> = reader.read_tangents().map(|it| it.collect());
        let colors: Option<Vec<[f32; 4]>> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
        let joints: Option<Vec<[u16; 4]>> = reader.read_joints(0).map(|j| j.into_u16().collect());
        let weights: Option<Vec<[f32; 4]>> = reader.read_weights(0).map(|w| w.into_f32().collect());

        let raw_indices: Vec<u32> = match reader.read_indices() {
            Some(it) => it.into_u32().collect(),
            None => (0..positions.len() as u32).collect(),
        };

        let indices = match prim.mode() {
            gltf::mesh::Mode::Triangles => {
                let n = raw_indices.len() - (raw_indices.len() % 3);
                raw_indices[..n].to_vec()
            }
            gltf::mesh::Mode::TriangleStrip => {
                let mut tris = Vec::new();
                for i in 0..raw_indices.len().saturating_sub(2) {
                    if i % 2 == 0 {
                        tris.extend_from_slice(&[raw_indices[i], raw_indices[i + 1], raw_indices[i + 2]]);
                    } else {
                        tris.extend_from_slice(&[raw_indices[i], raw_indices[i + 2], raw_indices[i + 1]]);
                    }
                }
                tris
            }
            gltf::mesh::Mode::TriangleFan => {
                let mut tris = Vec::new();
                for i in 1..raw_indices.len().saturating_sub(1) {
                    tris.extend_from_slice(&[raw_indices[0], raw_indices[i], raw_indices[i + 1]]);
                }
                tris
            }
            _ => continue,
        };
        if indices.len() < 3 {
            continue;
        }

        let (translation, rotation, scale) = if has_rig {
            // Keep rig mesh vertices in bind space; node TRS stays in PreservedRig
            let (s, r, t) = safe_decompose(world);
            (t.to_array(), [r.x, r.y, r.z, r.w], s.to_array())
        } else {
            // Check if world matrix has clean TRS decomposition (no shear / negative reflection)
            let det = world.determinant();
            let (s, r, t) = safe_decompose(world);
            let recomposed = Mat4::from_scale_rotation_translation(s, r, t);
            let err = mat_max_diff(world, recomposed);
            if err < 1e-3 && det > 0.0 {
                (t.to_array(), [r.x, r.y, r.z, r.w], s.to_array())
            } else {
                // Bake sheared/reflected transform cleanly into vertices
                let normal_mat = world.inverse().transpose();
                for p in &mut positions {
                    *p = world.transform_point3(Vec3::from_array(*p)).to_array();
                }
                for n in &mut normals {
                    let v = normal_mat.transform_vector3(Vec3::from_array(*n)).normalize_or_zero();
                    *n = if v == Vec3::ZERO { [0.0, 1.0, 0.0] } else { v.to_array() };
                }
                tangents = None;
                ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0])
            }
        };

        let mat_index = prim.material().index().unwrap_or(0);
        let name = if prim_count > 1 {
            let mat_name = doc
                .materials
                .get(mat_index)
                .map(|m| m.name.as_str())
                .unwrap_or("Part");
            format!("{base_name} [{mat_name}]")
        } else {
            base_name.clone()
        };

        let id = doc.alloc_id();
        let mut part = MeshPart {
            id,
            name,
            visible: true,
            translation,
            rotation,
            scale,
            positions,
            normals,
            uvs,
            tangents,
            colors,
            joints,
            weights,
            indices,
            material: mat_index,
            orig_mesh_group: has_rig.then_some(mesh.index()),
            revision: 1,
        };
        if part.normals.len() != part.positions.len() {
            part.recompute_normals_smooth();
        }
        doc.parts.push(part);
    }
}

fn safe_decompose(m: Mat4) -> (Vec3, Quat, Vec3) {
    let sx = m.x_axis.truncate().length();
    let sy = m.y_axis.truncate().length();
    let sz = m.z_axis.truncate().length();
    if sx < 1e-6 || sy < 1e-6 || sz < 1e-6 || !sx.is_finite() || !sy.is_finite() || !sz.is_finite() {
        return (Vec3::ONE, Quat::IDENTITY, m.w_axis.truncate());
    }
    let (s, r, t) = m.to_scale_rotation_translation();
    (s, r.normalize(), t)
}

fn mat_max_diff(a: Mat4, b: Mat4) -> f32 {
    let aa = a.to_cols_array();
    let ba = b.to_cols_array();
    aa.iter()
        .zip(ba.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

fn extract_rig(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    scene: Option<&gltf::Scene<'_>>,
) -> PreservedRig {
    let root_nodes = scene
        .map(|s| s.nodes().map(|n| n.index()).collect())
        .unwrap_or_default();

    let nodes = doc
        .nodes()
        .map(|n| {
            let (t, r, s) = n.transform().decomposed();
            RigNode {
                name: n.name().map(str::to_string),
                translation: t,
                rotation: r,
                scale: s,
                children: n.children().map(|c| c.index()).collect(),
                mesh_group: n.mesh().map(|m| m.index()),
                skin: n.skin().map(|sk| sk.index()),
            }
        })
        .collect();

    let skins = doc
        .skins()
        .map(|sk| {
            let reader = sk.reader(|b| Some(&buffers[b.index()].0));
            let ibm = reader.read_inverse_bind_matrices().map(|it| {
                it.map(|m| {
                    [
                        m[0][0], m[0][1], m[0][2], m[0][3], m[1][0], m[1][1], m[1][2], m[1][3],
                        m[2][0], m[2][1], m[2][2], m[2][3], m[3][0], m[3][1], m[3][2], m[3][3],
                    ]
                })
                .collect()
            });
            RigSkin {
                name: sk.name().map(str::to_string),
                joints: sk.joints().map(|j| j.index()).collect(),
                skeleton: sk.skeleton().map(|s| s.index()),
                inverse_bind_matrices: ibm,
            }
        })
        .collect();

    let animations = doc
        .animations()
        .map(|anim| {
            let mut sampler_map: HashMap<usize, usize> = HashMap::new();
            let mut samplers = Vec::new();
            let mut channels = Vec::new();

            for ch in anim.channels() {
                let orig_s = ch.sampler().index();
                let mapped_s = if let Some(&existing) = sampler_map.get(&orig_s) {
                    existing
                } else {
                    let reader = ch.reader(|b| Some(&buffers[b.index()].0));
                    let input: Vec<f32> = reader
                        .read_inputs()
                        .map(|it| it.collect())
                        .unwrap_or_default();
                    let output = match reader.read_outputs() {
                        Some(gltf::animation::util::ReadOutputs::Translations(it)) => {
                            AnimOutput::Vec3s(it.collect())
                        }
                        Some(gltf::animation::util::ReadOutputs::Scales(it)) => {
                            AnimOutput::Vec3s(it.collect())
                        }
                        Some(gltf::animation::util::ReadOutputs::Rotations(it)) => {
                            AnimOutput::Vec4s(it.into_f32().collect())
                        }
                        Some(gltf::animation::util::ReadOutputs::MorphTargetWeights(it)) => {
                            AnimOutput::Scalars(it.into_f32().collect())
                        }
                        None => AnimOutput::Scalars(Vec::new()),
                    };
                    let interp = match ch.sampler().interpolation() {
                        gltf::animation::Interpolation::Linear => AnimInterp::Linear,
                        gltf::animation::Interpolation::Step => AnimInterp::Step,
                        gltf::animation::Interpolation::CubicSpline => AnimInterp::CubicSpline,
                    };
                    let idx = samplers.len();
                    samplers.push(RigAnimSampler {
                        input,
                        interpolation: interp,
                        output,
                    });
                    sampler_map.insert(orig_s, idx);
                    idx
                };
                let path = match ch.target().property() {
                    gltf::animation::Property::Translation => AnimPath::Translation,
                    gltf::animation::Property::Rotation => AnimPath::Rotation,
                    gltf::animation::Property::Scale => AnimPath::Scale,
                    gltf::animation::Property::MorphTargetWeights => AnimPath::MorphTargetWeights,
                };
                channels.push(RigAnimChannel {
                    sampler: mapped_s,
                    target_node: ch.target().node().index(),
                    path,
                });
            }

            RigAnimation {
                name: anim.name().map(str::to_string),
                samplers,
                channels,
            }
        })
        .collect();

    PreservedRig {
        root_nodes,
        nodes,
        skins,
        animations,
    }
}

// --- GLB / glTF 2.0 Exporter ------------------------------------------------

struct BinBuilder {
    bin: Vec<u8>,
    buffer_views: Vec<Value>,
    accessors: Vec<Value>,
}

impl BinBuilder {
    fn new() -> Self {
        Self {
            bin: Vec::new(),
            buffer_views: Vec::new(),
            accessors: Vec::new(),
        }
    }

    fn align4(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn push_view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align4();
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let idx = self.buffer_views.len();
        let mut obj = json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": bytes.len(),
        });
        if let Some(t) = target {
            obj["target"] = json!(t);
        }
        self.buffer_views.push(obj);
        idx
    }

    fn push_vec3(&mut self, data: &[[f32; 3]], target: Option<u32>, with_bounds: bool) -> usize {
        let bv = self.push_view(bytemuck::cast_slice(data), target);
        let mut acc = json!({
            "bufferView": bv,
            "componentType": 5126, // FLOAT
            "count": data.len(),
            "type": "VEC3",
        });
        if with_bounds && !data.is_empty() {
            let mut mn = [f32::MAX; 3];
            let mut mx = [f32::MIN; 3];
            for v in data {
                for k in 0..3 {
                    mn[k] = mn[k].min(v[k]);
                    mx[k] = mx[k].max(v[k]);
                }
            }
            acc["min"] = json!(mn);
            acc["max"] = json!(mx);
        }
        let idx = self.accessors.len();
        self.accessors.push(acc);
        idx
    }

    fn push_vec2(&mut self, data: &[[f32; 2]], target: Option<u32>) -> usize {
        let bv = self.push_view(bytemuck::cast_slice(data), target);
        let idx = self.accessors.len();
        self.accessors.push(json!({
            "bufferView": bv,
            "componentType": 5126,
            "count": data.len(),
            "type": "VEC2",
        }));
        idx
    }

    fn push_vec4(&mut self, data: &[[f32; 4]], target: Option<u32>) -> usize {
        let bv = self.push_view(bytemuck::cast_slice(data), target);
        let idx = self.accessors.len();
        self.accessors.push(json!({
            "bufferView": bv,
            "componentType": 5126,
            "count": data.len(),
            "type": "VEC4",
        }));
        idx
    }

    fn push_joints_u16(&mut self, data: &[[u16; 4]]) -> usize {
        let bv = self.push_view(bytemuck::cast_slice(data), Some(34962));
        let idx = self.accessors.len();
        self.accessors.push(json!({
            "bufferView": bv,
            "componentType": 5123, // UNSIGNED_SHORT
            "count": data.len(),
            "type": "VEC4",
        }));
        idx
    }

    fn push_indices(&mut self, indices: &[u32], vert_count: usize) -> usize {
        if vert_count <= u16::MAX as usize {
            let u16s: Vec<u16> = indices.iter().map(|&i| i as u16).collect();
            let bv = self.push_view(bytemuck::cast_slice(&u16s), Some(34963));
            let idx = self.accessors.len();
            self.accessors.push(json!({
                "bufferView": bv,
                "componentType": 5123, // UNSIGNED_SHORT
                "count": u16s.len(),
                "type": "SCALAR",
            }));
            idx
        } else {
            let bv = self.push_view(bytemuck::cast_slice(indices), Some(34963));
            let idx = self.accessors.len();
            self.accessors.push(json!({
                "bufferView": bv,
                "componentType": 5125, // UNSIGNED_INT
                "count": indices.len(),
                "type": "SCALAR",
            }));
            idx
        }
    }

    fn push_scalars(&mut self, data: &[f32], with_bounds: bool) -> usize {
        let bv = self.push_view(bytemuck::cast_slice(data), None);
        let mut acc = json!({
            "bufferView": bv,
            "componentType": 5126,
            "count": data.len(),
            "type": "SCALAR",
        });
        if with_bounds && !data.is_empty() {
            let mn = data.iter().copied().fold(f32::MAX, f32::min);
            let mx = data.iter().copied().fold(f32::MIN, f32::max);
            acc["min"] = json!([mn]);
            acc["max"] = json!([mx]);
        }
        let idx = self.accessors.len();
        self.accessors.push(acc);
        idx
    }

    fn push_mat4(&mut self, data: &[[f32; 16]]) -> usize {
        let bv = self.push_view(bytemuck::cast_slice(data), None);
        let idx = self.accessors.len();
        self.accessors.push(json!({
            "bufferView": bv,
            "componentType": 5126,
            "count": data.len(),
            "type": "MAT4",
        }));
        idx
    }
}

fn build_gltf_payload(doc: &ModelDoc) -> Result<(Value, Vec<u8>), String> {
    let mut bin = BinBuilder::new();

    // 1. Collect and encode referenced textures as PNG in the binary buffer.
    let mut tex_remap: HashMap<usize, usize> = HashMap::new();
    let mut images_json = Vec::new();
    let mut textures_json = Vec::new();
    for m in &doc.materials {
        for opt in [
            m.base_color_texture,
            m.normal_texture,
            m.metallic_roughness_texture,
            m.emissive_texture,
        ] {
            if let Some(ti) = opt
                && ti < doc.textures.len()
                && !tex_remap.contains_key(&ti)
            {
                let png_bytes = encode_texture_png(&doc.textures[ti])?;
                let bv = bin.push_view(&png_bytes, None);
                let img_idx = images_json.len();
                images_json.push(json!({
                    "name": doc.textures[ti].name,
                    "bufferView": bv,
                    "mimeType": "image/png",
                }));
                let out_ti = textures_json.len();
                textures_json.push(json!({
                    "sampler": 0,
                    "source": img_idx,
                }));
                tex_remap.insert(ti, out_ti);
            }
        }
    }

    // 2. Materials
    let mut materials_json = Vec::new();
    for m in &doc.materials {
        let mut pbr = json!({
            "baseColorFactor": m.base_color,
            "metallicFactor": m.metallic.clamp(0.0, 1.0),
            "roughnessFactor": m.roughness.clamp(0.0, 1.0),
        });
        if let Some(ti) = m.base_color_texture.and_then(|i| tex_remap.get(&i).copied()) {
            pbr["baseColorTexture"] = json!({ "index": ti });
        }
        if let Some(ti) = m.metallic_roughness_texture.and_then(|i| tex_remap.get(&i).copied()) {
            pbr["metallicRoughnessTexture"] = json!({ "index": ti });
        }
        let mut mat = json!({
            "name": m.name,
            "pbrMetallicRoughness": pbr,
            "doubleSided": m.double_sided,
        });
        if m.emissive != [0.0, 0.0, 0.0] {
            mat["emissiveFactor"] = json!(m.emissive);
        }
        if let Some(ti) = m.normal_texture.and_then(|i| tex_remap.get(&i).copied()) {
            mat["normalTexture"] = json!({ "index": ti });
        }
        if let Some(ti) = m.emissive_texture.and_then(|i| tex_remap.get(&i).copied()) {
            mat["emissiveTexture"] = json!({ "index": ti });
        }
        match m.alpha_mode {
            AlphaKind::Opaque => {}
            AlphaKind::Mask(cut) => {
                mat["alphaMode"] = json!("MASK");
                mat["alphaCutoff"] = json!(cut);
            }
            AlphaKind::Blend => {
                mat["alphaMode"] = json!("BLEND");
            }
        }
        materials_json.push(mat);
    }

    let make_primitive = |part: &MeshPart, bin: &mut BinBuilder| -> Option<Value> {
        if part.positions.is_empty() || part.indices.len() < 3 {
            return None;
        }
        let pos_acc = bin.push_vec3(&part.positions, Some(34962), true);
        let mut attrs = json!({ "POSITION": pos_acc });
        if part.normals.len() == part.positions.len() {
            attrs["NORMAL"] = json!(bin.push_vec3(&part.normals, Some(34962), false));
        }
        if part.has_uvs() {
            attrs["TEXCOORD_0"] = json!(bin.push_vec2(&part.uvs, Some(34962)));
        }
        if let Some(t) = &part.tangents
            && t.len() == part.positions.len()
        {
            attrs["TANGENT"] = json!(bin.push_vec4(t, Some(34962)));
        }
        if let Some(c) = &part.colors
            && c.len() == part.positions.len()
        {
            attrs["COLOR_0"] = json!(bin.push_vec4(c, Some(34962)));
        }
        if let Some(j) = &part.joints
            && j.len() == part.positions.len()
        {
            attrs["JOINTS_0"] = json!(bin.push_joints_u16(j));
        }
        if let Some(w) = &part.weights
            && w.len() == part.positions.len()
        {
            attrs["WEIGHTS_0"] = json!(bin.push_vec4(w, Some(34962)));
        }
        let idx_acc = bin.push_indices(&part.indices, part.positions.len());
        Some(json!({
            "attributes": attrs,
            "indices": idx_acc,
            "material": part.material.min(materials_json.len().saturating_sub(1)),
            "mode": 4, // TRIANGLES
        }))
    };

    // 3. Meshes, nodes, and optional preserved rig/animations.
    let mut meshes_json: Vec<Value> = Vec::new();
    let mut nodes_json: Vec<Value> = Vec::new();
    let mut root_nodes: Vec<usize> = Vec::new();
    let mut skins_json: Vec<Value> = Vec::new();
    let mut anims_json: Vec<Value> = Vec::new();

    if let Some(rig) = &doc.preserved_rig {
        // Group parts belonging to original glTF mesh indices.
        let mut by_group: HashMap<usize, Vec<&MeshPart>> = HashMap::new();
        let mut extra_parts: Vec<&MeshPart> = Vec::new();
        for p in &doc.parts {
            if !p.visible {
                continue;
            }
            if let Some(g) = p.orig_mesh_group {
                by_group.entry(g).or_default().push(p);
            } else {
                extra_parts.push(p);
            }
        }

        let mut group_to_mesh: HashMap<usize, usize> = HashMap::new();
        let mut group_ids: Vec<usize> = by_group.keys().copied().collect();
        group_ids.sort();
        for g in group_ids {
            let mut prims = Vec::new();
            for p in &by_group[&g] {
                if let Some(pr) = make_primitive(p, &mut bin) {
                    prims.push(pr);
                }
            }
            if !prims.is_empty() {
                let mi = meshes_json.len();
                meshes_json.push(json!({ "primitives": prims }));
                group_to_mesh.insert(g, mi);
            }
        }

        for rn in &rig.nodes {
            let mut node = json!({});
            if let Some(name) = &rn.name {
                node["name"] = json!(name);
            }
            if rn.translation != [0.0, 0.0, 0.0] {
                node["translation"] = json!(rn.translation);
            }
            if rn.rotation != [0.0, 0.0, 0.0, 1.0] {
                node["rotation"] = json!(rn.rotation);
            }
            if rn.scale != [1.0, 1.0, 1.0] {
                node["scale"] = json!(rn.scale);
            }
            if !rn.children.is_empty() {
                node["children"] = json!(rn.children);
            }
            if let Some(g) = rn.mesh_group
                && let Some(&mi) = group_to_mesh.get(&g)
            {
                node["mesh"] = json!(mi);
            }
            if let Some(sk) = rn.skin {
                node["skin"] = json!(sk);
            }
            nodes_json.push(node);
        }
        root_nodes.extend_from_slice(&rig.root_nodes);

        // Append any newly created parts as standalone root nodes.
        for p in extra_parts {
            if let Some(pr) = make_primitive(p, &mut bin) {
                let mi = meshes_json.len();
                meshes_json.push(json!({ "name": p.name, "primitives": [pr] }));
                let ni = nodes_json.len();
                nodes_json.push(build_part_node_json(p, mi));
                root_nodes.push(ni);
            }
        }

        // Skins
        for sk in &rig.skins {
            let mut sj = json!({ "joints": sk.joints });
            if let Some(name) = &sk.name {
                sj["name"] = json!(name);
            }
            if let Some(skel) = sk.skeleton {
                sj["skeleton"] = json!(skel);
            }
            if let Some(ibm) = &sk.inverse_bind_matrices {
                sj["inverseBindMatrices"] = json!(bin.push_mat4(ibm));
            }
            skins_json.push(sj);
        }

        // Animations
        for anim in &rig.animations {
            let mut samplers_j = Vec::new();
            for s in &anim.samplers {
                let in_acc = bin.push_scalars(&s.input, true);
                let out_acc = match &s.output {
                    AnimOutput::Scalars(v) => bin.push_scalars(v, false),
                    AnimOutput::Vec3s(v) => bin.push_vec3(v, None, false),
                    AnimOutput::Vec4s(v) => bin.push_vec4(v, None),
                };
                let interp = match s.interpolation {
                    AnimInterp::Linear => "LINEAR",
                    AnimInterp::Step => "STEP",
                    AnimInterp::CubicSpline => "CUBICSPLINE",
                };
                samplers_j.push(json!({
                    "input": in_acc,
                    "interpolation": interp,
                    "output": out_acc,
                }));
            }
            let channels_j: Vec<Value> = anim
                .channels
                .iter()
                .map(|c| {
                    let path = match c.path {
                        AnimPath::Translation => "translation",
                        AnimPath::Rotation => "rotation",
                        AnimPath::Scale => "scale",
                        AnimPath::MorphTargetWeights => "weights",
                    };
                    json!({
                        "sampler": c.sampler,
                        "target": {
                            "node": c.target_node,
                            "path": path,
                        }
                    })
                })
                .collect();
            let mut aj = json!({
                "samplers": samplers_j,
                "channels": channels_j,
            });
            if let Some(name) = &anim.name {
                aj["name"] = json!(name);
            }
            anims_json.push(aj);
        }
    } else {
        for p in &doc.parts {
            if !p.visible {
                continue;
            }
            let Some(pr) = make_primitive(p, &mut bin) else {
                continue;
            };
            let mi = meshes_json.len();
            meshes_json.push(json!({
                "name": p.name,
                "primitives": [pr],
            }));
            let ni = nodes_json.len();
            nodes_json.push(build_part_node_json(p, mi));
            root_nodes.push(ni);
        }
    }

    bin.align4();

    let mut root = json!({
        "asset": {
            "version": "2.0",
            "generator": "Ink-cognito Model Editor"
        },
        "scene": 0,
        "scenes": [{ "nodes": root_nodes }],
        "nodes": nodes_json,
        "meshes": meshes_json,
        "materials": materials_json,
    });
    if !bin.bin.is_empty() {
        root["accessors"] = json!(bin.accessors);
        root["bufferViews"] = json!(bin.buffer_views);
        root["buffers"] = json!([{ "byteLength": bin.bin.len() }]);
    }

    if !textures_json.is_empty() {
        root["samplers"] = json!([{
            "magFilter": 9729, // LINEAR
            "minFilter": 9987, // LINEAR_MIPMAP_LINEAR
            "wrapS": 10497,    // REPEAT
            "wrapT": 10497,
        }]);
        root["images"] = json!(images_json);
        root["textures"] = json!(textures_json);
    }
    if !skins_json.is_empty() {
        root["skins"] = json!(skins_json);
    }
    if !anims_json.is_empty() {
        root["animations"] = json!(anims_json);
    }

    Ok((root, bin.bin))
}

fn build_part_node_json(p: &MeshPart, mesh_idx: usize) -> Value {
    let mut node = json!({
        "name": p.name,
        "mesh": mesh_idx,
    });
    if p.translation != [0.0, 0.0, 0.0] {
        node["translation"] = json!(p.translation);
    }
    if p.rotation != [0.0, 0.0, 0.0, 1.0] {
        node["rotation"] = json!(p.rotation);
    }
    if p.scale != [1.0, 1.0, 1.0] {
        node["scale"] = json!(p.scale);
    }
    node
}

/// Writes a self-contained binary `.glb` file (glTF 2.0 spec).
pub fn save_glb(doc: &ModelDoc, path: &Path) -> Result<(), String> {
    let (json_val, mut bin_bytes) = build_gltf_payload(doc)?;
    let mut json_bytes =
        serde_json::to_vec(&json_val).map_err(|e| format!("glTF JSON serialization failed: {e}"))?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    while !bin_bytes.len().is_multiple_of(4) {
        bin_bytes.push(0);
    }

    let has_bin = !bin_bytes.is_empty();
    let total_len = 12 + 8 + json_bytes.len() + if has_bin { 8 + bin_bytes.len() } else { 0 };
    let mut out = Vec::with_capacity(total_len);
    // 12-byte GLB header
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total_len as u32).to_le_bytes());
    // JSON chunk
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    // BIN chunk
    if has_bin {
        out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E4942u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin_bytes);
    }

    std::fs::write(path, out).map_err(|e| format!("Couldn't write '{}': {e}", path.display()))
}

/// Writes a self-contained `.gltf` JSON file (embedding its binary buffer as a
/// base64 data URI so there are no loose external files to lose).
pub fn save_gltf(doc: &ModelDoc, path: &Path) -> Result<(), String> {
    let (mut json_val, bin_bytes) = build_gltf_payload(doc)?;
    if !bin_bytes.is_empty() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bin_bytes);
        json_val["buffers"] = json!([{
            "byteLength": bin_bytes.len(),
            "uri": format!("data:application/octet-stream;base64,{b64}"),
        }]);
    }
    let text = serde_json::to_string_pretty(&json_val)
        .map_err(|e| format!("glTF JSON formatting failed: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("Couldn't write '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn glb_round_trips_painted_cube() {
        let mut doc = ModelDoc::starter_cube();
        let (ti, _, _) = doc.ensure_paintable_texture(0, 32).unwrap();
        doc.textures[ti].pixels_mut()[0..4].copy_from_slice(&[255, 64, 32, 255]);
        doc.materials[0].metallic = 0.4;
        doc.materials[0].roughness = 0.35;

        let tmp = std::env::temp_dir().join("ink_model_editor_test_cube.glb");
        save_glb(&doc, &tmp).expect("save glb");
        let loaded = load(&tmp).expect("reload glb");
        let _ = std::fs::remove_file(&tmp);

        assert_eq!(loaded.parts.len(), 1);
        assert_eq!(loaded.parts[0].tri_count(), 12);
        assert_eq!(loaded.textures.len(), 1);
        assert_eq!(loaded.textures[0].get_pixel(0, 0), [255, 64, 32, 255]);
        assert!((loaded.materials[0].metallic - 0.4).abs() < 1e-4);
    }

    #[test]
    fn gltf_json_round_trips_starter_cube() {
        let doc = ModelDoc::starter_cube();
        let tmp = std::env::temp_dir().join("ink_model_editor_test_cube.gltf");
        save_gltf(&doc, &tmp).expect("save gltf");
        let loaded = load(&tmp).expect("reload gltf");
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(loaded.parts.len(), 1);
        assert_eq!(loaded.parts[0].tri_count(), 12);
    }

    #[test]
    fn real_repo_glbs_import_and_round_trip_if_present() {
        let repo_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
        let samples = [
            "bunkers/brick.glb",
            "players/blue_player_rigged.glb",
            "water-guns-v2/minnow_v2.glb",
        ];
        for rel in samples {
            let p = repo_assets.join(rel);
            if !p.exists() {
                continue;
            }
            let doc = load(&p).unwrap_or_else(|e| panic!("failed to load {rel}: {e}"));
            assert!(!doc.parts.is_empty(), "{rel} had no parts");
            let out = std::env::temp_dir().join(format!(
                "ink_model_editor_rt_{}.glb",
                rel.replace(['/', '.'], "_")
            ));
            save_glb(&doc, &out).unwrap_or_else(|e| panic!("failed to save {rel}: {e}"));
            let reloaded = load(&out).unwrap_or_else(|e| panic!("failed to reload {rel}: {e}"));
            let _ = std::fs::remove_file(&out);
            assert_eq!(reloaded.triangle_count(), doc.triangle_count(), "{rel} tri count mismatch");
            if let Some(orig_rig) = &doc.preserved_rig {
                let new_rig = reloaded.preserved_rig.as_ref().expect("rig should survive round-trip");
                assert_eq!(new_rig.skins.len(), orig_rig.skins.len(), "{rel} skins");
                assert_eq!(new_rig.animations.len(), orig_rig.animations.len(), "{rel} anims");
            }
        }
    }
}
