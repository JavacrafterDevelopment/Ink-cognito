//! Self-contained 3D model document: parts (meshes), PBR materials, editable
//! RGBA textures, optional preserved glTF rig/animations, primitives, UV
//! unwrapping, normal calculation, and exact ray-triangle picking.

use bevy::prelude::{Mat4, Quat, Vec2, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

/// How a material handles alpha transparency (matches glTF 2.0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaKind {
    Opaque,
    Mask(f32),
    Blend,
}

impl AlphaKind {
    pub fn name(self) -> &'static str {
        match self {
            AlphaKind::Opaque => "Opaque",
            AlphaKind::Mask(_) => "Cutout (Mask)",
            AlphaKind::Blend => "Transparent",
        }
    }

    pub fn next(self) -> Self {
        match self {
            AlphaKind::Opaque => AlphaKind::Mask(0.5),
            AlphaKind::Mask(_) => AlphaKind::Blend,
            AlphaKind::Blend => AlphaKind::Opaque,
        }
    }
}

/// An editable RGBA8 texture stored in memory. Pixel data is wrapped in an
/// `Arc` so cloning `ModelDoc` for undo snapshots is nearly free for geometry
/// and material edits (copy-on-write when painted).
#[derive(Clone, Debug)]
pub struct TextureSlot {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
    /// Bumped whenever pixels change so the GPU texture updates immediately.
    pub revision: u64,
}

impl TextureSlot {
    pub fn new_solid(name: impl Into<String>, width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let count = (width as usize) * (height as usize);
        let mut pixels = Vec::with_capacity(count * 4);
        for _ in 0..count {
            pixels.extend_from_slice(&rgba);
        }
        Self {
            name: name.into(),
            width,
            height,
            pixels: Arc::new(pixels),
            revision: 1,
        }
    }

    pub fn from_rgba(name: impl Into<String>, width: u32, height: u32, pixels: Vec<u8>) -> Self {
        let expected = (width as usize) * (height as usize) * 4;
        let mut buf = pixels;
        buf.resize(expected, 255);
        Self {
            name: name.into(),
            width,
            height,
            pixels: Arc::new(buf),
            revision: 1,
        }
    }

    /// Mutable access to pixels (copy-on-write) and bumps revision.
    pub fn pixels_mut(&mut self) -> &mut Vec<u8> {
        self.revision = self.revision.wrapping_add(1);
        Arc::make_mut(&mut self.pixels)
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [255, 255, 255, 255];
        }
        let x = x.min(self.width - 1) as usize;
        let y = y.min(self.height - 1) as usize;
        let idx = (y * self.width as usize + x) * 4;
        if idx + 3 < self.pixels.len() {
            [
                self.pixels[idx],
                self.pixels[idx + 1],
                self.pixels[idx + 2],
                self.pixels[idx + 3],
            ]
        } else {
            [255, 255, 255, 255]
        }
    }

    /// Samples the texture at wrapped UV coordinates (`[0..1]`), returning sRGB RGBA8.
    pub fn sample_uv(&self, uv: [f32; 2]) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [255, 255, 255, 255];
        }
        let u = uv[0].rem_euclid(1.0);
        let v = uv[1].rem_euclid(1.0);
        let x = ((u * self.width as f32).floor() as i64).clamp(0, self.width as i64 - 1) as u32;
        let y = ((v * self.height as f32).floor() as i64).clamp(0, self.height as i64 - 1) as u32;
        self.get_pixel(x, y)
    }
}

/// Converts a single sRGB channel `[0..1]` to linear `[0..1]`.
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts a single linear channel `[0..1]` to sRGB `[0..1]`.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_rgba_to_linear(c: [f32; 4]) -> [f32; 4] {
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
        c[3].clamp(0.0, 1.0),
    ]
}

pub fn linear_rgba_to_srgb(c: [f32; 4]) -> [f32; 4] {
    [
        linear_to_srgb(c[0]),
        linear_to_srgb(c[1]),
        linear_to_srgb(c[2]),
        c[3].clamp(0.0, 1.0),
    ]
}

pub fn srgb_f32_to_u8(c: [f32; 4]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

pub fn u8_to_srgb_f32(c: [u8; 4]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

/// A PBR material slot (matches glTF 2.0 metallic-roughness).
#[derive(Clone, Debug)]
pub struct MaterialSlot {
    pub name: String,
    /// Linear RGBA base color factor (matches glTF `baseColorFactor`).
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Linear RGB emissive factor.
    pub emissive: [f32; 3],
    pub alpha_mode: AlphaKind,
    pub double_sided: bool,
    pub base_color_texture: Option<usize>,
    pub normal_texture: Option<usize>,
    pub metallic_roughness_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
}

impl Default for MaterialSlot {
    fn default() -> Self {
        Self {
            name: "Material".into(),
            base_color: [0.68, 0.70, 0.74, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            emissive: [0.0, 0.0, 0.0],
            alpha_mode: AlphaKind::Opaque,
            double_sided: false,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
        }
    }
}

impl MaterialSlot {
    pub fn base_color_srgb(&self) -> [f32; 4] {
        linear_rgba_to_srgb(self.base_color)
    }

    pub fn set_base_color_srgb(&mut self, srgb: [f32; 4]) {
        self.base_color = srgb_rgba_to_linear(srgb);
    }

    pub fn emissive_srgb(&self) -> [f32; 3] {
        [
            linear_to_srgb(self.emissive[0]),
            linear_to_srgb(self.emissive[1]),
            linear_to_srgb(self.emissive[2]),
        ]
    }
}

// --- Preserved glTF Rig & Animation Data ------------------------------------

/// A preserved glTF scene node so rigged / animated models keep their bone
/// hierarchy and animations intact when edited and re-exported as GLB.
#[derive(Clone, Debug)]
pub struct RigNode {
    pub name: Option<String>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub children: Vec<usize>,
    pub mesh_group: Option<usize>,
    pub skin: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct RigSkin {
    pub name: Option<String>,
    pub joints: Vec<usize>,
    pub skeleton: Option<usize>,
    pub inverse_bind_matrices: Option<Vec<[f32; 16]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimPath {
    Translation,
    Rotation,
    Scale,
    MorphTargetWeights,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimInterp {
    Linear,
    Step,
    CubicSpline,
}

#[derive(Clone, Debug)]
pub enum AnimOutput {
    Scalars(Vec<f32>),
    Vec3s(Vec<[f32; 3]>),
    Vec4s(Vec<[f32; 4]>),
}

#[derive(Clone, Debug)]
pub struct RigAnimSampler {
    pub input: Vec<f32>,
    pub interpolation: AnimInterp,
    pub output: AnimOutput,
}

#[derive(Clone, Debug)]
pub struct RigAnimChannel {
    pub sampler: usize,
    pub target_node: usize,
    pub path: AnimPath,
}

#[derive(Clone, Debug)]
pub struct RigAnimation {
    pub name: Option<String>,
    pub samplers: Vec<RigAnimSampler>,
    pub channels: Vec<RigAnimChannel>,
}

#[derive(Clone, Debug)]
pub struct PreservedRig {
    pub root_nodes: Vec<usize>,
    pub nodes: Vec<RigNode>,
    pub skins: Vec<RigSkin>,
    pub animations: Vec<RigAnimation>,
}

// --- Mesh Part --------------------------------------------------------------

/// Result of an exact ray-triangle hit against a `MeshPart`.
#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub distance: f32,
    pub world_pos: Vec3,
    pub local_pos: Vec3,
    pub world_normal: Vec3,
    pub local_normal: Vec3,
    pub tri_index: usize,
    pub uv: Option<[f32; 2]>,
}

/// One editable mesh part (corresponds to a glTF primitive or OBJ group).
#[derive(Clone, Debug)]
pub struct MeshPart {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Empty if the source model had no UVs; call `ensure_uvs()` to unwrap.
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub joints: Option<Vec<[u16; 4]>>,
    pub weights: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub material: usize,
    /// Original glTF mesh index if imported from a rigged/animated glTF.
    pub orig_mesh_group: Option<usize>,
    /// Bumped whenever vertex/index data changes so the GPU mesh refreshes.
    pub revision: u64,
}

impl MeshPart {
    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn quat(&self) -> Quat {
        Quat::from_xyzw(self.rotation[0], self.rotation[1], self.rotation[2], self.rotation[3])
            .normalize()
    }

    pub fn set_quat(&mut self, q: Quat) {
        let q = q.normalize();
        self.rotation = [q.x, q.y, q.z, q.w];
    }

    pub fn model_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::from_array(self.scale),
            self.quat(),
            Vec3::from_array(self.translation),
        )
    }

    pub fn inv_model_matrix(&self) -> Mat4 {
        self.model_matrix().inverse()
    }

    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn has_uvs(&self) -> bool {
        !self.uvs.is_empty() && self.uvs.len() == self.positions.len()
    }

    pub fn local_bounds(&self) -> Option<(Vec3, Vec3)> {
        if self.positions.is_empty() {
            return None;
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for &p in &self.positions {
            let v = Vec3::from_array(p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        Some((lo, hi))
    }

    pub fn world_bounds(&self) -> Option<(Vec3, Vec3)> {
        if self.positions.is_empty() {
            return None;
        }
        let m = self.model_matrix();
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for &p in &self.positions {
            let v = m.transform_point3(Vec3::from_array(p));
            lo = lo.min(v);
            hi = hi.max(v);
        }
        Some((lo, hi))
    }

    /// Exact Möller–Trumbore ray-triangle hit test in local space.
    pub fn raycast(&self, world_origin: Vec3, world_dir: Vec3) -> Option<RayHit> {
        if !self.visible || self.indices.len() < 3 {
            return None;
        }
        // Quick world AABB reject.
        if let Some((lo, hi)) = self.world_bounds()
            && !ray_hits_aabb(world_origin, world_dir, lo - Vec3::splat(1e-4), hi + Vec3::splat(1e-4))
        {
            return None;
        }

        let model = self.model_matrix();
        let inv = model.inverse();
        let local_origin = inv.transform_point3(world_origin);
        let local_dir_raw = inv.transform_vector3(world_dir);
        let dir_len = local_dir_raw.length();
        if dir_len < 1e-8 {
            return None;
        }
        let local_dir = local_dir_raw / dir_len;

        let mut best: Option<(f32, usize, [f32; 3], Vec3, Vec3)> = None;
        for (tri_idx, tri) in self.indices.chunks_exact(3).enumerate() {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= self.positions.len() || i1 >= self.positions.len() || i2 >= self.positions.len() {
                continue;
            }
            let v0 = Vec3::from_array(self.positions[i0]);
            let v1 = Vec3::from_array(self.positions[i1]);
            let v2 = Vec3::from_array(self.positions[i2]);
            let e1 = v1 - v0;
            let e2 = v2 - v0;
            let h = local_dir.cross(e2);
            let det = e1.dot(h);
            if det.abs() < 1e-7 {
                continue;
            }
            let inv_det = 1.0 / det;
            let s = local_origin - v0;
            let u = inv_det * s.dot(h);
            if !(-1e-5..=1.0 + 1e-5).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = inv_det * local_dir.dot(q);
            if v < -1e-5 || u + v > 1.0 + 1e-5 {
                continue;
            }
            let t_local = inv_det * e2.dot(q);
            if t_local <= 1e-5 {
                continue;
            }
            let local_hit = local_origin + local_dir * t_local;
            let world_hit = model.transform_point3(local_hit);
            let world_dist = world_hit.distance(world_origin);
            if best.is_none_or(|b| world_dist < b.0) {
                let w = (1.0 - u - v).clamp(0.0, 1.0);
                let face_n = e1.cross(e2).normalize_or_zero();
                let n = if face_n == Vec3::ZERO { Vec3::Y } else { face_n };
                // Ensure normal faces toward ray if hitting back-face of double-sided mesh
                let n = if n.dot(local_dir) > 0.0 { -n } else { n };
                best = Some((world_dist, tri_idx, [w, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)], local_hit, n));
            }
        }

        let (distance, tri_idx, bary, local_pos, local_normal) = best?;
        let world_pos = model.transform_point3(local_pos);
        // Normal matrix = inverse transpose of upper 3x3
        let normal_mat = inv.transpose();
        let world_normal = normal_mat.transform_vector3(local_normal).normalize_or_zero();
        let world_normal = if world_normal == Vec3::ZERO { Vec3::Y } else { world_normal };

        let uv = if self.has_uvs() {
            let tri = &self.indices[tri_idx * 3..tri_idx * 3 + 3];
            let uv0 = Vec2::from_array(self.uvs[tri[0] as usize]);
            let uv1 = Vec2::from_array(self.uvs[tri[1] as usize]);
            let uv2 = Vec2::from_array(self.uvs[tri[2] as usize]);
            let interp = uv0 * bary[0] + uv1 * bary[1] + uv2 * bary[2];
            Some([interp.x, interp.y])
        } else {
            None
        };

        Some(RayHit {
            distance,
            world_pos,
            local_pos,
            world_normal,
            local_normal,
            tri_index: tri_idx,
            uv,
        })
    }

    /// Recomputes smooth area-weighted vertex normals (welding coincident positions).
    pub fn recompute_normals_smooth(&mut self) {
        let mut accum: HashMap<[i32; 3], Vec3> = HashMap::new();
        for tri in self.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= self.positions.len() || i1 >= self.positions.len() || i2 >= self.positions.len() {
                continue;
            }
            let p0 = Vec3::from_array(self.positions[i0]);
            let p1 = Vec3::from_array(self.positions[i1]);
            let p2 = Vec3::from_array(self.positions[i2]);
            let n = (p1 - p0).cross(p2 - p0);
            for idx in [i0, i1, i2] {
                let key = quantize_pos(self.positions[idx]);
                *accum.entry(key).or_insert(Vec3::ZERO) += n;
            }
        }
        self.normals = self
            .positions
            .iter()
            .map(|p| {
                let n = accum.get(&quantize_pos(*p)).copied().unwrap_or(Vec3::Y).normalize_or_zero();
                if n == Vec3::ZERO { [0.0, 1.0, 0.0] } else { n.to_array() }
            })
            .collect();
        self.tangents = None;
        self.touch();
    }

    /// Splits shared vertices per triangle and computes crisp flat face normals.
    pub fn recompute_normals_flat(&mut self) {
        let mut positions = Vec::with_capacity(self.indices.len());
        let mut normals = Vec::with_capacity(self.indices.len());
        let has_uvs = self.has_uvs();
        let mut uvs = if has_uvs { Vec::with_capacity(self.indices.len()) } else { Vec::new() };
        let mut colors = self.colors.as_ref().map(|_| Vec::with_capacity(self.indices.len()));
        let mut joints = self.joints.as_ref().map(|_| Vec::with_capacity(self.indices.len()));
        let mut weights = self.weights.as_ref().map(|_| Vec::with_capacity(self.indices.len()));
        let mut indices = Vec::with_capacity(self.indices.len());

        for tri in self.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= self.positions.len() || i1 >= self.positions.len() || i2 >= self.positions.len() {
                continue;
            }
            let p0 = Vec3::from_array(self.positions[i0]);
            let p1 = Vec3::from_array(self.positions[i1]);
            let p2 = Vec3::from_array(self.positions[i2]);
            let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            let n = if n == Vec3::ZERO { [0.0, 1.0, 0.0] } else { n.to_array() };
            for idx in [i0, i1, i2] {
                indices.push(positions.len() as u32);
                positions.push(self.positions[idx]);
                normals.push(n);
                if has_uvs {
                    uvs.push(self.uvs[idx]);
                }
                if let (Some(dst), Some(src)) = (colors.as_mut(), self.colors.as_ref()) {
                    dst.push(src[idx]);
                }
                if let (Some(dst), Some(src)) = (joints.as_mut(), self.joints.as_ref()) {
                    dst.push(src[idx]);
                }
                if let (Some(dst), Some(src)) = (weights.as_mut(), self.weights.as_ref()) {
                    dst.push(src[idx]);
                }
            }
        }
        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
        self.colors = colors;
        self.joints = joints;
        self.weights = weights;
        self.indices = indices;
        self.tangents = None;
        self.touch();
    }

    /// Reverses triangle winding and flips all normals.
    pub fn flip_normals(&mut self) {
        for tri in self.indices.chunks_exact_mut(3) {
            tri.swap(1, 2);
        }
        for n in &mut self.normals {
            n[0] = -n[0];
            n[1] = -n[1];
            n[2] = -n[2];
        }
        self.tangents = None;
        self.touch();
    }

    /// Generates clean, non-overlapping 6-direction Box-Atlas UVs (3×2 tiles with
    /// padding) so every side (+X, -X, +Y, -Y, +Z, -Z) maps to its own area of
    /// the texture and can be painted independently without mirroring onto the
    /// opposite side!
    pub fn generate_box_uvs(&mut self) {
        let Some((lo, hi)) = self.local_bounds() else {
            return;
        };
        let size = (hi - lo).max(Vec3::splat(1e-4));

        // Split vertices per triangle so triangles on different cube sides don't
        // pull UVs across atlas tile seams.
        let mut positions = Vec::with_capacity(self.indices.len());
        let mut normals = Vec::with_capacity(self.indices.len());
        let mut uvs = Vec::with_capacity(self.indices.len());
        let mut colors = self.colors.as_ref().map(|_| Vec::with_capacity(self.indices.len()));
        let mut joints = self.joints.as_ref().map(|_| Vec::with_capacity(self.indices.len()));
        let mut weights = self.weights.as_ref().map(|_| Vec::with_capacity(self.indices.len()));
        let mut indices = Vec::with_capacity(self.indices.len());

        let pad = 0.025f32;
        let tile_w = 1.0 / 3.0;
        let tile_h = 1.0 / 2.0;

        for tri in self.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= self.positions.len() || i1 >= self.positions.len() || i2 >= self.positions.len() {
                continue;
            }
            let p0 = Vec3::from_array(self.positions[i0]);
            let p1 = Vec3::from_array(self.positions[i1]);
            let p2 = Vec3::from_array(self.positions[i2]);
            let fn_ = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            let ax = fn_.x.abs();
            let ay = fn_.y.abs();
            let az = fn_.z.abs();

            // 6 non-overlapping tiles in a 3x2 grid:
            // (0,0): +X, (1,0): -X, (2,0): +Y
            // (0,1): -Y, (1,1): +Z, (2,1): -Z
            let (col, row, mode) = if ax >= ay && ax >= az {
                if fn_.x >= 0.0 { (0, 0, 0) } else { (1, 0, 1) }
            } else if ay >= ax && ay >= az {
                if fn_.y >= 0.0 { (2, 0, 2) } else { (0, 1, 3) }
            } else if fn_.z >= 0.0 {
                (1, 1, 4)
            } else {
                (2, 1, 5)
            };

            for idx in [i0, i1, i2] {
                let p = Vec3::from_array(self.positions[idx]);
                let rel = (p - lo) / size;
                let (lu, lv) = match mode {
                    0 => (1.0 - rel.z, 1.0 - rel.y), // +X
                    1 => (rel.z, 1.0 - rel.y),       // -X
                    2 => (rel.x, rel.z),             // +Y
                    3 => (rel.x, 1.0 - rel.z),       // -Y
                    4 => (rel.x, 1.0 - rel.y),       // +Z
                    _ => (1.0 - rel.x, 1.0 - rel.y), // -Z
                };
                let u = (col as f32 + pad + lu.clamp(0.0, 1.0) * (1.0 - 2.0 * pad)) * tile_w;
                let v = (row as f32 + pad + lv.clamp(0.0, 1.0) * (1.0 - 2.0 * pad)) * tile_h;

                indices.push(positions.len() as u32);
                positions.push(self.positions[idx]);
                normals.push(self.normals.get(idx).copied().unwrap_or([0.0, 1.0, 0.0]));
                uvs.push([u, v]);
                if let (Some(dst), Some(src)) = (colors.as_mut(), self.colors.as_ref()) {
                    dst.push(src[idx]);
                }
                if let (Some(dst), Some(src)) = (joints.as_mut(), self.joints.as_ref()) {
                    dst.push(src[idx]);
                }
                if let (Some(dst), Some(src)) = (weights.as_mut(), self.weights.as_ref()) {
                    dst.push(src[idx]);
                }
            }
        }

        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
        self.colors = colors;
        self.joints = joints;
        self.weights = weights;
        self.indices = indices;
        self.tangents = None;
        self.touch();
    }

    /// Ensures the part has valid UVs (returns `true` if UVs were generated).
    pub fn ensure_uvs(&mut self) -> bool {
        if self.has_uvs() {
            return false;
        }
        self.generate_box_uvs();
        true
    }

    /// Bakes this part's TRS transform directly into its vertex positions/normals
    /// and resets translation/rotation/scale to identity.
    pub fn bake_transform(&mut self) {
        let m = self.model_matrix();
        let normal_mat = m.inverse().transpose();
        let det = m.determinant();
        for p in &mut self.positions {
            *p = m.transform_point3(Vec3::from_array(*p)).to_array();
        }
        for n in &mut self.normals {
            let v = normal_mat.transform_vector3(Vec3::from_array(*n)).normalize_or_zero();
            *n = if v == Vec3::ZERO { [0.0, 1.0, 0.0] } else { v.to_array() };
        }
        if det < 0.0 {
            for tri in self.indices.chunks_exact_mut(3) {
                tri.swap(1, 2);
            }
        }
        self.translation = [0.0, 0.0, 0.0];
        self.rotation = [0.0, 0.0, 0.0, 1.0];
        self.scale = [1.0, 1.0, 1.0];
        self.tangents = None;
        self.touch();
    }
}

pub fn quantize_pos(p: [f32; 3]) -> [i32; 3] {
    [
        (p[0] * 10_000.0).round() as i32,
        (p[1] * 10_000.0).round() as i32,
        (p[2] * 10_000.0).round() as i32,
    ]
}

fn ray_hits_aabb(origin: Vec3, dir: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = f32::MAX;
    for a in 0..3 {
        if dir[a].abs() < 1e-8 {
            if origin[a] < lo[a] || origin[a] > hi[a] {
                return false;
            }
            continue;
        }
        let inv = 1.0 / dir[a];
        let (mut n, mut f) = ((lo[a] - origin[a]) * inv, (hi[a] - origin[a]) * inv);
        if n > f {
            std::mem::swap(&mut n, &mut f);
        }
        t0 = t0.max(n);
        t1 = t1.min(f);
        if t0 > t1 {
            return false;
        }
    }
    true
}

// --- Built-in Primitives ----------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveKind {
    Cube,
    Sphere,
    Cylinder,
    Cone,
    Plane,
    Torus,
    Capsule,
}

impl PrimitiveKind {
    pub const ALL: [PrimitiveKind; 7] = [
        PrimitiveKind::Cube,
        PrimitiveKind::Sphere,
        PrimitiveKind::Cylinder,
        PrimitiveKind::Cone,
        PrimitiveKind::Plane,
        PrimitiveKind::Torus,
        PrimitiveKind::Capsule,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PrimitiveKind::Cube => "Cube",
            PrimitiveKind::Sphere => "Sphere",
            PrimitiveKind::Cylinder => "Cylinder",
            PrimitiveKind::Cone => "Cone",
            PrimitiveKind::Plane => "Plane",
            PrimitiveKind::Torus => "Torus",
            PrimitiveKind::Capsule => "Capsule",
        }
    }
}

pub fn build_primitive(kind: PrimitiveKind, id: u64, material: usize) -> MeshPart {
    let (name, positions, normals, uvs, indices) = match kind {
        PrimitiveKind::Cube => build_cube(),
        PrimitiveKind::Sphere => build_sphere(24, 16, 0.5),
        PrimitiveKind::Cylinder => build_cylinder(24, 0.5, 1.0),
        PrimitiveKind::Cone => build_cone(24, 0.5, 1.0),
        PrimitiveKind::Plane => build_plane(2.0),
        PrimitiveKind::Torus => build_torus(28, 16, 0.45, 0.16),
        PrimitiveKind::Capsule => build_capsule(20, 8, 0.35, 0.6),
    };
    MeshPart {
        id,
        name: name.to_string(),
        visible: true,
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        positions,
        normals,
        uvs,
        tangents: None,
        colors: None,
        joints: None,
        weights: None,
        indices,
        material,
        orig_mesh_group: None,
        revision: 1,
    }
}

/// Unit cube sitting on y = 0..1 with non-overlapping 3×2 atlas UVs per face.
fn build_cube() -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let h = 0.5f32;
    let y0 = 0.0f32;
    let y1 = 1.0f32;
    // (normal, 4 corners CCW when viewed from outside, atlas (col, row))
    let faces: [([f32; 3], [[f32; 3]; 4], usize, usize); 6] = [
        ([1.0, 0.0, 0.0], [[h, y0, h], [h, y0, -h], [h, y1, -h], [h, y1, h]], 0, 0),
        ([-1.0, 0.0, 0.0], [[-h, y0, -h], [-h, y0, h], [-h, y1, h], [-h, y1, -h]], 1, 0),
        ([0.0, 1.0, 0.0], [[-h, y1, h], [h, y1, h], [h, y1, -h], [-h, y1, -h]], 2, 0),
        ([0.0, -1.0, 0.0], [[-h, y0, -h], [h, y0, -h], [h, y0, h], [-h, y0, h]], 0, 1),
        ([0.0, 0.0, 1.0], [[-h, y0, h], [h, y0, h], [h, y1, h], [-h, y1, h]], 1, 1),
        ([0.0, 0.0, -1.0], [[h, y0, -h], [-h, y0, -h], [-h, y1, -h], [h, y1, -h]], 2, 1),
    ];
    let pad = 0.025f32;
    let tw = 1.0 / 3.0;
    let th = 1.0 / 2.0;
    let local_uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut uvs = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    for (n, corners, col, row) in faces {
        let base = positions.len() as u32;
        for (i, c) in corners.into_iter().enumerate() {
            positions.push(c);
            normals.push(n);
            let [lu, lv] = local_uv[i];
            let u = (col as f32 + pad + lu * (1.0 - 2.0 * pad)) * tw;
            let v = (row as f32 + pad + lv * (1.0 - 2.0 * pad)) * th;
            uvs.push([u, v]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    ("Cube", positions, normals, uvs, indices)
}

fn build_sphere(seg_u: u32, seg_v: u32, r: f32) -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    for y in 0..=seg_v {
        let v = y as f32 / seg_v as f32;
        let phi = v * std::f32::consts::PI;
        for x in 0..=seg_u {
            let u = x as f32 / seg_u as f32;
            let theta = u * std::f32::consts::TAU;
            let nx = phi.sin() * theta.cos();
            let ny = phi.cos();
            let nz = phi.sin() * theta.sin();
            positions.push([nx * r, ny * r + r, nz * r]);
            normals.push([nx, ny, nz]);
            uvs.push([1.0 - u, v]);
        }
    }
    let row = seg_u + 1;
    for y in 0..seg_v {
        for x in 0..seg_u {
            let a = y * row + x;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    ("Sphere", positions, normals, uvs, indices)
}

fn build_cylinder(segs: u32, r: f32, h: f32) -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    // Side wall occupies V in [0.02..0.64]
    for i in 0..=segs {
        let u = i as f32 / segs as f32;
        let a = u * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        positions.push([c * r, 0.0, s * r]);
        normals.push([c, 0.0, s]);
        uvs.push([u, 0.64]);
        positions.push([c * r, h, s * r]);
        normals.push([c, 0.0, s]);
        uvs.push([u, 0.02]);
    }
    for i in 0..segs {
        let b = i * 2;
        indices.extend_from_slice(&[b, b + 1, b + 2, b + 1, b + 3, b + 2]);
    }

    // Top cap centered at UV (0.25, 0.82), radius 0.16
    let top_center = positions.len() as u32;
    positions.push([0.0, h, 0.0]);
    normals.push([0.0, 1.0, 0.0]);
    uvs.push([0.25, 0.82]);
    for i in 0..=segs {
        let a = i as f32 / segs as f32 * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        positions.push([c * r, h, s * r]);
        normals.push([0.0, 1.0, 0.0]);
        uvs.push([0.25 + c * 0.16, 0.82 + s * 0.16]);
    }
    for i in 0..segs {
        indices.extend_from_slice(&[top_center, top_center + 1 + i + 1, top_center + 1 + i]);
    }

    // Bottom cap centered at UV (0.75, 0.82), radius 0.16
    let bot_center = positions.len() as u32;
    positions.push([0.0, 0.0, 0.0]);
    normals.push([0.0, -1.0, 0.0]);
    uvs.push([0.75, 0.82]);
    for i in 0..=segs {
        let a = i as f32 / segs as f32 * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        positions.push([c * r, 0.0, s * r]);
        normals.push([0.0, -1.0, 0.0]);
        uvs.push([0.75 + c * 0.16, 0.82 + s * 0.16]);
    }
    for i in 0..segs {
        indices.extend_from_slice(&[bot_center, bot_center + 1 + i, bot_center + 1 + i + 1]);
    }
    ("Cylinder", positions, normals, uvs, indices)
}

fn build_cone(segs: u32, r: f32, h: f32) -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    let slope = Vec2::new(h, r).normalize();
    for i in 0..=segs {
        let u = i as f32 / segs as f32;
        let a = u * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        let n = [c * slope.x, slope.y, s * slope.x];
        positions.push([c * r, 0.0, s * r]);
        normals.push(n);
        uvs.push([u, 0.64]);
        positions.push([0.0, h, 0.0]);
        normals.push(n);
        uvs.push([u, 0.02]);
    }
    for i in 0..segs {
        let b = i * 2;
        indices.extend_from_slice(&[b, b + 1, b + 2]);
    }

    let bot_center = positions.len() as u32;
    positions.push([0.0, 0.0, 0.0]);
    normals.push([0.0, -1.0, 0.0]);
    uvs.push([0.5, 0.82]);
    for i in 0..=segs {
        let a = i as f32 / segs as f32 * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        positions.push([c * r, 0.0, s * r]);
        normals.push([0.0, -1.0, 0.0]);
        uvs.push([0.5 + c * 0.16, 0.82 + s * 0.16]);
    }
    for i in 0..segs {
        indices.extend_from_slice(&[bot_center, bot_center + 1 + i, bot_center + 1 + i + 1]);
    }
    ("Cone", positions, normals, uvs, indices)
}

fn build_plane(size: f32) -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let h = size * 0.5;
    let positions = vec![[-h, 0.0, h], [h, 0.0, h], [h, 0.0, -h], [-h, 0.0, -h]];
    let normals = vec![[0.0, 1.0, 0.0]; 4];
    let uvs = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let indices = vec![0, 1, 2, 0, 2, 3];
    ("Plane", positions, normals, uvs, indices)
}

fn build_torus(major_segs: u32, minor_segs: u32, major_r: f32, minor_r: f32) -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let lift = minor_r;

    for i in 0..=major_segs {
        let u = i as f32 / major_segs as f32;
        let theta = u * std::f32::consts::TAU;
        let (ct, st) = (theta.cos(), theta.sin());
        for j in 0..=minor_segs {
            let v = j as f32 / minor_segs as f32;
            let phi = v * std::f32::consts::TAU;
            let (cp, sp) = (phi.cos(), phi.sin());
            let nx = cp * ct;
            let ny = sp;
            let nz = cp * st;
            let px = (major_r + minor_r * cp) * ct;
            let py = minor_r * sp + lift;
            let pz = (major_r + minor_r * cp) * st;
            positions.push([px, py, pz]);
            normals.push([nx, ny, nz]);
            uvs.push([u, v]);
        }
    }
    let row = minor_segs + 1;
    for i in 0..major_segs {
        for j in 0..minor_segs {
            let a = i * row + j;
            let b = a + row;
            let c = a + 1;
            let d = b + 1;
            indices.extend_from_slice(&[a, c, b, c, d, b]);
        }
    }
    ("Torus", positions, normals, uvs, indices)
}

fn build_capsule(seg_u: u32, hemi_rings: u32, r: f32, cyl_h: f32) -> (&'static str, Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    // Collect vertical profile rings from top pole to bottom pole
    let mut rings: Vec<(f32, f32, f32, f32)> = Vec::new(); // (y, radius, ny, nr)
    for k in 0..=hemi_rings {
        let t = k as f32 / hemi_rings as f32;
        let phi = t * std::f32::consts::FRAC_PI_2;
        rings.push((r + cyl_h + phi.cos() * r, phi.sin() * r, phi.cos(), phi.sin()));
    }
    for k in 0..=hemi_rings {
        let t = k as f32 / hemi_rings as f32;
        let phi = std::f32::consts::FRAC_PI_2 + t * std::f32::consts::FRAC_PI_2;
        rings.push((r + phi.cos() * r, phi.sin() * r, phi.cos(), phi.sin()));
    }

    let total_rings = rings.len() as u32 - 1;
    for (ri, &(y, rad, ny, nr)) in rings.iter().enumerate() {
        let v = ri as f32 / total_rings as f32;
        for x in 0..=seg_u {
            let u = x as f32 / seg_u as f32;
            let theta = u * std::f32::consts::TAU;
            let (c, s) = (theta.cos(), theta.sin());
            positions.push([c * rad, y, s * rad]);
            normals.push([c * nr, ny, s * nr]);
            uvs.push([1.0 - u, v]);
        }
    }
    let row = seg_u + 1;
    for y in 0..total_rings {
        for x in 0..seg_u {
            let a = y * row + x;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    ("Capsule", positions, normals, uvs, indices)
}

// --- Document ---------------------------------------------------------------

/// The complete 3D model currently being edited.
#[derive(Clone, Debug)]
pub struct ModelDoc {
    pub parts: Vec<MeshPart>,
    pub materials: Vec<MaterialSlot>,
    pub textures: Vec<TextureSlot>,
    pub preserved_rig: Option<PreservedRig>,
    pub next_id: u64,
}

impl Default for ModelDoc {
    fn default() -> Self {
        Self::starter_cube()
    }
}

impl ModelDoc {
    pub fn empty() -> Self {
        Self {
            parts: Vec::new(),
            materials: vec![MaterialSlot::default()],
            textures: Vec::new(),
            preserved_rig: None,
            next_id: 1,
        }
    }

    pub fn starter_cube() -> Self {
        let mut doc = Self::empty();
        doc.add_primitive(PrimitiveKind::Cube);
        doc
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn ensure_default_material(&mut self) -> usize {
        if self.materials.is_empty() {
            self.materials.push(MaterialSlot::default());
        }
        0
    }

    pub fn add_primitive(&mut self, kind: PrimitiveKind) -> usize {
        let mat = self.ensure_default_material();
        let id = self.alloc_id();
        let mut part = build_primitive(kind, id, mat);
        // Give a distinct name if one already exists
        let base = part.name.clone();
        let mut n = 1;
        while self.parts.iter().any(|p| p.name == part.name) {
            n += 1;
            part.name = format!("{base} {n}");
        }
        self.parts.push(part);
        self.parts.len() - 1
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut any = false;
        for p in &self.parts {
            if !p.visible {
                continue;
            }
            if let Some((a, b)) = p.world_bounds() {
                lo = lo.min(a);
                hi = hi.max(b);
                any = true;
            }
        }
        any.then_some((lo, hi))
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(MeshPart::tri_count).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.parts.iter().map(|p| p.positions.len()).sum()
    }

    /// Raycasts against every visible part, returning `(part_index, hit)`.
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<(usize, RayHit)> {
        let mut best: Option<(usize, RayHit)> = None;
        for (i, part) in self.parts.iter().enumerate() {
            if let Some(hit) = part.raycast(origin, dir)
                && best.is_none_or(|(_, b)| hit.distance < b.distance)
            {
                best = Some((i, hit));
            }
        }
        best
    }

    /// Duplicates a part with a slight offset so the copy is easy to see.
    pub fn duplicate_part(&mut self, idx: usize) -> Option<usize> {
        let mut copy = self.parts.get(idx)?.clone();
        copy.id = self.alloc_id();
        copy.orig_mesh_group = None;
        copy.name = format!("{} Copy", copy.name);
        copy.translation[0] += 0.35;
        copy.touch();
        self.parts.push(copy);
        Some(self.parts.len() - 1)
    }

    /// Creates a mirrored copy across `axis` (0 = X, 1 = Y, 2 = Z) with
    /// corrected triangle winding so lighting and backface culling stay right.
    pub fn mirror_part(&mut self, idx: usize, axis: usize) -> Option<usize> {
        let mut copy = self.parts.get(idx)?.clone();
        copy.id = self.alloc_id();
        copy.orig_mesh_group = None;
        copy.bake_transform();
        for p in &mut copy.positions {
            p[axis] = -p[axis];
        }
        for n in &mut copy.normals {
            n[axis] = -n[axis];
        }
        for tri in copy.indices.chunks_exact_mut(3) {
            tri.swap(1, 2);
        }
        let label = ["X", "Y", "Z"][axis.min(2)];
        copy.name = format!("{} Mirror {label}", copy.name);
        copy.tangents = None;
        copy.touch();
        self.parts.push(copy);
        Some(self.parts.len() - 1)
    }

    /// Centers the whole model horizontally around X=0, Z=0 and places its
    /// lowest point cleanly on Y=0.
    pub fn center_and_ground(&mut self) -> bool {
        let Some((lo, hi)) = self.bounds() else {
            return false;
        };
        let shift = Vec3::new(-(lo.x + hi.x) * 0.5, -lo.y, -(lo.z + hi.z) * 0.5);
        for p in &mut self.parts {
            p.translation[0] += shift.x;
            p.translation[1] += shift.y;
            p.translation[2] += shift.z;
        }
        true
    }

    /// Appends every part, material, and texture from another `ModelDoc` into
    /// this one (great for kitbashing / importing sub-models!).
    pub fn append_from(&mut self, other: ModelDoc) -> Option<usize> {
        let tex_offset = self.textures.len();
        self.textures.extend(other.textures);

        let mat_offset = self.materials.len();
        for mut m in other.materials {
            m.base_color_texture = m.base_color_texture.map(|t| t + tex_offset);
            m.normal_texture = m.normal_texture.map(|t| t + tex_offset);
            m.metallic_roughness_texture = m.metallic_roughness_texture.map(|t| t + tex_offset);
            m.emissive_texture = m.emissive_texture.map(|t| t + tex_offset);
            self.materials.push(m);
        }

        let first_new = if other.parts.is_empty() {
            None
        } else {
            Some(self.parts.len())
        };
        for mut p in other.parts {
            p.id = self.alloc_id();
            p.material += mat_offset;
            p.orig_mesh_group = None;
            p.touch();
            self.parts.push(p);
        }
        first_new
    }

    /// Ensures `part_idx` has UVs AND its material has a dedicated RGBA base
    /// color texture ready for 3D painting. Returns `(tex_idx, created_tex, created_uvs)`.
    pub fn ensure_paintable_texture(&mut self, part_idx: usize, default_size: u32) -> Option<(usize, bool, bool)> {
        if part_idx >= self.parts.len() {
            return None;
        }
        let created_uvs = self.parts[part_idx].ensure_uvs();
        let mat_idx = self.parts[part_idx].material;
        if mat_idx >= self.materials.len() {
            self.ensure_default_material();
            self.parts[part_idx].material = 0;
        }
        let mat_idx = self.parts[part_idx].material;
        if let Some(tex_idx) = self.materials[mat_idx].base_color_texture
            && tex_idx < self.textures.len()
        {
            return Some((tex_idx, false, created_uvs));
        }

        // Fill the newly created canvas with the material's current sRGB color,
        // and reset base_color factor to white so painted colors show accurately!
        let srgb = self.materials[mat_idx].base_color_srgb();
        let fill = srgb_f32_to_u8(srgb);
        let tex_name = format!("{}_paint", self.materials[mat_idx].name);
        let tex = TextureSlot::new_solid(tex_name, default_size, default_size, fill);
        let tex_idx = self.textures.len();
        self.textures.push(tex);
        self.materials[mat_idx].base_color_texture = Some(tex_idx);
        self.materials[mat_idx].base_color = [1.0, 1.0, 1.0, self.materials[mat_idx].base_color[3]];
        Some((tex_idx, true, created_uvs))
    }

    /// Adds a new material slot and returns its index.
    pub fn add_material(&mut self, name: impl Into<String>) -> usize {
        let mut mat = MaterialSlot::default();
        mat.name = name.into();
        self.materials.push(mat);
        self.materials.len() - 1
    }

    /// Merges two parts into one (baking transforms, keeping `a`'s material).
    pub fn merge_into(&mut self, dst_idx: usize, src_idx: usize) -> bool {
        if dst_idx == src_idx || dst_idx >= self.parts.len() || src_idx >= self.parts.len() {
            return false;
        }
        let mut src = self.parts[src_idx].clone();
        src.bake_transform();
        let dst = &mut self.parts[dst_idx];
        dst.bake_transform();
        // Ensure both have UVs if either does
        if dst.has_uvs() || src.has_uvs() {
            dst.ensure_uvs();
            src.ensure_uvs();
        }
        let base_v = dst.positions.len() as u32;
        dst.positions.extend_from_slice(&src.positions);
        dst.normals.extend_from_slice(&src.normals);
        if dst.has_uvs() {
            dst.uvs.extend_from_slice(&src.uvs);
        }
        dst.colors = None;
        dst.tangents = None;
        dst.joints = None;
        dst.weights = None;
        dst.orig_mesh_group = None;
        dst.indices.extend(src.indices.iter().map(|&i| i + base_v));
        dst.touch();
        self.parts.remove(src_idx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_build_valid_geometry_and_uvs() {
        for kind in PrimitiveKind::ALL {
            let part = build_primitive(kind, 1, 0);
            assert!(!part.positions.is_empty(), "{kind:?}");
            assert_eq!(part.positions.len(), part.normals.len(), "{kind:?}");
            assert_eq!(part.positions.len(), part.uvs.len(), "{kind:?}");
            assert_eq!(part.indices.len() % 3, 0, "{kind:?}");
            for &idx in &part.indices {
                assert!((idx as usize) < part.positions.len(), "{kind:?}");
            }
        }
    }

    #[test]
    fn box_uv_unwrap_populates_valid_range_without_overlaps() {
        let mut cube = build_primitive(PrimitiveKind::Cube, 1, 0);
        cube.uvs.clear();
        assert!(!cube.has_uvs());
        assert!(cube.ensure_uvs());
        assert!(cube.has_uvs());
        for &[u, v] in &cube.uvs {
            assert!((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v), "uv out of range: {u},{v}");
        }
    }

    #[test]
    fn raycast_hits_cube_front_and_misses_side() {
        let doc = ModelDoc::starter_cube();
        let hit = doc.raycast(Vec3::new(0.0, 0.5, 3.0), Vec3::NEG_Z).expect("should hit cube");
        assert_eq!(hit.0, 0);
        assert!((hit.1.world_pos.z - 0.5).abs() < 1e-3);
        assert!(doc.raycast(Vec3::new(5.0, 0.5, 3.0), Vec3::NEG_Z).is_none());
    }

    #[test]
    fn ensure_paintable_texture_creates_canvas_with_material_color() {
        let mut doc = ModelDoc::starter_cube();
        doc.materials[0].set_base_color_srgb([1.0, 0.0, 0.0, 1.0]);
        let (tex_idx, created_tex, _) = doc.ensure_paintable_texture(0, 64).unwrap();
        assert!(created_tex);
        let px = doc.textures[tex_idx].get_pixel(10, 10);
        assert_eq!(px, [255, 0, 0, 255]);
        // Second call reuses the texture
        let (tex_idx2, created_tex2, _) = doc.ensure_paintable_texture(0, 64).unwrap();
        assert_eq!(tex_idx, tex_idx2);
        assert!(!created_tex2);
    }
}
