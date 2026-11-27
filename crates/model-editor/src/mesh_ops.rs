//! 3D mesh geometry editing operations: vertex/edge/face selection, coplanar
//! face selection, transform, extrude, inset, subdivide, delete, split by
//! material, Laplacian smooth, and 3D sculpt brushes.

use crate::model::{MeshPart, ModelDoc, quantize_pos};
use bevy::prelude::{Vec2, Vec3};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubSelMode {
    Vertex,
    Edge,
    Face,
}

impl SubSelMode {
    pub fn name(self) -> &'static str {
        match self {
            SubSelMode::Vertex => "Vertex",
            SubSelMode::Edge => "Edge",
            SubSelMode::Face => "Face",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SculptTool {
    Push,
    Pull,
    Smooth,
    Flatten,
}

impl SculptTool {
    pub const ALL: [SculptTool; 4] = [
        SculptTool::Push,
        SculptTool::Pull,
        SculptTool::Smooth,
        SculptTool::Flatten,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SculptTool::Push => "Push",
            SculptTool::Pull => "Pull",
            SculptTool::Smooth => "Smooth",
            SculptTool::Flatten => "Flatten",
        }
    }
}

/// Active sub-element selection on the selected part in Mesh Edit mode.
#[derive(Clone, Debug, Default)]
pub struct MeshSelection {
    /// Selected vertex indices (includes coincident seam-split twins).
    pub verts: HashSet<usize>,
    /// Selected triangle indices (`0..part.tri_count()`).
    pub faces: HashSet<usize>,
}

impl MeshSelection {
    pub fn clear(&mut self) {
        self.verts.clear();
        self.faces.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty() && self.faces.is_empty()
    }

    /// Returns all effective vertex indices affected by the selection (including
    /// vertices of selected faces and all coincident seam twins).
    pub fn effective_verts(&self, part: &MeshPart) -> HashSet<usize> {
        let mut seeds = self.verts.clone();
        for &f in &self.faces {
            if f * 3 + 2 < part.indices.len() {
                seeds.insert(part.indices[f * 3] as usize);
                seeds.insert(part.indices[f * 3 + 1] as usize);
                seeds.insert(part.indices[f * 3 + 2] as usize);
            }
        }
        expand_coincident(part, &seeds)
    }

    /// World-space centroid of the selected geometry (used to position the edit gizmo).
    pub fn world_centroid(&self, part: &MeshPart) -> Option<Vec3> {
        let verts = self.effective_verts(part);
        if verts.is_empty() {
            return None;
        }
        // Average over unique quantized positions so seam-split corners aren't double-weighted.
        let mut unique: HashMap<[i32; 3], Vec3> = HashMap::new();
        for v in verts {
            if let Some(&p) = part.positions.get(v) {
                unique.entry(quantize_pos(p)).or_insert(Vec3::from_array(p));
            }
        }
        if unique.is_empty() {
            return None;
        }
        let sum: Vec3 = unique.values().copied().sum();
        let local = sum / unique.len() as f32;
        Some(part.model_matrix().transform_point3(local))
    }
}

/// Expands a set of vertex indices to include all vertices at identical quantized
/// 3D positions so UV/normal seams stay watertight when editing.
pub fn expand_coincident(part: &MeshPart, seeds: &HashSet<usize>) -> HashSet<usize> {
    if seeds.is_empty() {
        return HashSet::new();
    }
    let mut keys: HashSet<[i32; 3]> = HashSet::new();
    for &i in seeds {
        if let Some(&p) = part.positions.get(i) {
            keys.insert(quantize_pos(p));
        }
    }
    let mut out = HashSet::new();
    for (i, &p) in part.positions.iter().enumerate() {
        if keys.contains(&quantize_pos(p)) {
            out.insert(i);
        }
    }
    out
}

/// Picks the nearest vertex index (and coincident twins) on a hit triangle.
pub fn pick_vertex_on_tri(part: &MeshPart, tri_idx: usize, local_hit: Vec3) -> HashSet<usize> {
    if tri_idx * 3 + 2 >= part.indices.len() {
        return HashSet::new();
    }
    let tri = &part.indices[tri_idx * 3..tri_idx * 3 + 3];
    let best = tri
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| i < part.positions.len())
        .min_by(|&a, &b| {
            let da = Vec3::from_array(part.positions[a]).distance_squared(local_hit);
            let db = Vec3::from_array(part.positions[b]).distance_squared(local_hit);
            da.total_cmp(&db)
        });
    let mut seeds = HashSet::new();
    if let Some(v) = best {
        seeds.insert(v);
    }
    expand_coincident(part, &seeds)
}

/// Picks the two endpoints of the nearest edge on a hit triangle (and coincident twins).
pub fn pick_edge_on_tri(part: &MeshPart, tri_idx: usize, local_hit: Vec3) -> HashSet<usize> {
    if tri_idx * 3 + 2 >= part.indices.len() {
        return HashSet::new();
    }
    let tri = [
        part.indices[tri_idx * 3] as usize,
        part.indices[tri_idx * 3 + 1] as usize,
        part.indices[tri_idx * 3 + 2] as usize,
    ];
    let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])];
    let best = edges.into_iter().min_by(|&(a0, a1), &(b0, b1)| {
        let da = point_seg_dist_sq(
            local_hit,
            Vec3::from_array(part.positions[a0]),
            Vec3::from_array(part.positions[a1]),
        );
        let db = point_seg_dist_sq(
            local_hit,
            Vec3::from_array(part.positions[b0]),
            Vec3::from_array(part.positions[b1]),
        );
        da.total_cmp(&db)
    });
    let mut seeds = HashSet::new();
    if let Some((a, b)) = best {
        seeds.insert(a);
        seeds.insert(b);
    }
    expand_coincident(part, &seeds)
}

fn point_seg_dist_sq(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
    p.distance_squared(a + ab * t)
}

/// Flood-selects triangles connected by a shared edge that are coplanar (within
/// ~3 degrees) with `start_tri`. Makes clicking a quad cube face select both
/// triangles cleanly at once!
pub fn select_coplanar_faces(part: &MeshPart, start_tri: usize) -> HashSet<usize> {
    let tri_count = part.tri_count();
    if start_tri >= tri_count {
        return HashSet::new();
    }
    let tri_normal = |t: usize| -> Vec3 {
        let i0 = part.indices[t * 3] as usize;
        let i1 = part.indices[t * 3 + 1] as usize;
        let i2 = part.indices[t * 3 + 2] as usize;
        let p0 = Vec3::from_array(part.positions[i0]);
        let p1 = Vec3::from_array(part.positions[i1]);
        let p2 = Vec3::from_array(part.positions[i2]);
        (p1 - p0).cross(p2 - p0).normalize_or_zero()
    };

    let base_n = tri_normal(start_tri);
    // Build edge -> triangles adjacency using quantized positions.
    let mut edge_map: HashMap<([i32; 3], [i32; 3]), Vec<usize>> = HashMap::new();
    for t in 0..tri_count {
        let k = [
            quantize_pos(part.positions[part.indices[t * 3] as usize]),
            quantize_pos(part.positions[part.indices[t * 3 + 1] as usize]),
            quantize_pos(part.positions[part.indices[t * 3 + 2] as usize]),
        ];
        for (a, b) in [(k[0], k[1]), (k[1], k[2]), (k[2], k[0])] {
            let key = if a <= b { (a, b) } else { (b, a) };
            edge_map.entry(key).or_default().push(t);
        }
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(start_tri);
    queue.push_back(start_tri);

    while let Some(t) = queue.pop_front() {
        let k = [
            quantize_pos(part.positions[part.indices[t * 3] as usize]),
            quantize_pos(part.positions[part.indices[t * 3 + 1] as usize]),
            quantize_pos(part.positions[part.indices[t * 3 + 2] as usize]),
        ];
        for (a, b) in [(k[0], k[1]), (k[1], k[2]), (k[2], k[0])] {
            let key = if a <= b { (a, b) } else { (b, a) };
            if let Some(neighbors) = edge_map.get(&key) {
                for &nt in neighbors {
                    if !visited.contains(&nt) && tri_normal(nt).dot(base_n) > 0.998 {
                        visited.insert(nt);
                        queue.push_back(nt);
                    }
                }
            }
        }
    }
    visited
}

/// Scales selected vertices around their local centroid.
pub fn scale_vertices(part: &mut MeshPart, verts: &HashSet<usize>, factor: f32) -> bool {
    let expanded = expand_coincident(part, verts);
    if expanded.is_empty() {
        return false;
    }
    let mut sum = Vec3::ZERO;
    let mut cnt = 0usize;
    for &i in &expanded {
        if let Some(&p) = part.positions.get(i) {
            sum += Vec3::from_array(p);
            cnt += 1;
        }
    }
    if cnt == 0 {
        return false;
    }
    let center = sum / cnt as f32;
    for idx in expanded {
        if let Some(p) = part.positions.get_mut(idx) {
            let v = center + (Vec3::from_array(*p) - center) * factor;
            *p = v.to_array();
        }
    }
    part.recompute_normals_smooth();
    true
}

/// Flattens selected vertices onto their average plane (useful for straightening faces).
pub fn flatten_vertices(part: &mut MeshPart, verts: &HashSet<usize>) -> bool {
    let expanded = expand_coincident(part, verts);
    if expanded.len() < 3 {
        return false;
    }
    let mut center = Vec3::ZERO;
    let mut avg_n = Vec3::ZERO;
    for &i in &expanded {
        center += Vec3::from_array(part.positions[i]);
        avg_n += Vec3::from_array(part.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]));
    }
    center /= expanded.len() as f32;
    let n = avg_n.normalize_or_zero();
    if n == Vec3::ZERO {
        return false;
    }
    for idx in expanded {
        let p = Vec3::from_array(part.positions[idx]);
        let projected = p - n * (p - center).dot(n);
        part.positions[idx] = projected.to_array();
    }
    part.recompute_normals_smooth();
    true
}

/// Applies one pass of Laplacian smoothing to the selected vertices (or the
/// whole part if `verts` is empty).
pub fn smooth_vertices(part: &mut MeshPart, verts: &HashSet<usize>, factor: f32) -> bool {
    if part.positions.is_empty() {
        return false;
    }
    let target: HashSet<[i32; 3]> = if verts.is_empty() {
        part.positions.iter().map(|&p| quantize_pos(p)).collect()
    } else {
        verts
            .iter()
            .filter_map(|&i| part.positions.get(i).copied().map(quantize_pos))
            .collect()
    };

    // Map quantized position -> neighbors' positions
    let mut pos_by_key: HashMap<[i32; 3], Vec3> = HashMap::new();
    for &p in &part.positions {
        pos_by_key.entry(quantize_pos(p)).or_insert(Vec3::from_array(p));
    }
    let mut adj: HashMap<[i32; 3], HashSet<[i32; 3]>> = HashMap::new();
    for tri in part.indices.chunks_exact(3) {
        let k = [
            quantize_pos(part.positions[tri[0] as usize]),
            quantize_pos(part.positions[tri[1] as usize]),
            quantize_pos(part.positions[tri[2] as usize]),
        ];
        for (a, b) in [(k[0], k[1]), (k[1], k[2]), (k[2], k[0])] {
            adj.entry(a).or_default().insert(b);
            adj.entry(b).or_default().insert(a);
        }
    }

    let mut next_pos: HashMap<[i32; 3], Vec3> = HashMap::new();
    for &key in &target {
        let Some(&p) = pos_by_key.get(&key) else { continue };
        let Some(nbs) = adj.get(&key) else { continue };
        if nbs.is_empty() {
            continue;
        }
        let avg: Vec3 = nbs.iter().filter_map(|k| pos_by_key.get(k).copied()).sum::<Vec3>()
            / nbs.len() as f32;
        next_pos.insert(key, p.lerp(avg, factor.clamp(0.0, 1.0)));
    }

    for p in &mut part.positions {
        let key = quantize_pos(*p);
        if let Some(&np) = next_pos.get(&key) {
            *p = np.to_array();
        }
    }
    part.recompute_normals_smooth();
    true
}

/// Extrudes the selected triangles outward by `distance` along their average
/// normal, building watertight side walls around the selection's boundary.
/// Returns the triangle indices of the extruded cap.
pub fn extrude_faces(part: &mut MeshPart, faces: &HashSet<usize>, distance: f32) -> HashSet<usize> {
    let tri_count = part.tri_count();
    let selected: Vec<usize> = faces.iter().copied().filter(|&f| f < tri_count).collect();
    if selected.is_empty() {
        return HashSet::new();
    }

    // Average normal of the selected region.
    let mut avg_n = Vec3::ZERO;
    for &f in &selected {
        let p0 = Vec3::from_array(part.positions[part.indices[f * 3] as usize]);
        let p1 = Vec3::from_array(part.positions[part.indices[f * 3 + 1] as usize]);
        let p2 = Vec3::from_array(part.positions[part.indices[f * 3 + 2] as usize]);
        avg_n += (p1 - p0).cross(p2 - p0);
    }
    let dir = avg_n.normalize_or_zero();
    let offset = if dir == Vec3::ZERO { Vec3::Y * distance } else { dir * distance };

    // Count directed boundary edges (quantized) within the selected region.
    // An edge (A -> B) is on the outer boundary if (B -> A) is NOT also in the selection.
    let mut dir_edges: HashMap<([i32; 3], [i32; 3]), (usize, usize)> = HashMap::new();
    let mut edge_counts: HashMap<([i32; 3], [i32; 3]), usize> = HashMap::new();
    for &f in &selected {
        let i = [
            part.indices[f * 3] as usize,
            part.indices[f * 3 + 1] as usize,
            part.indices[f * 3 + 2] as usize,
        ];
        for (a, b) in [(i[0], i[1]), (i[1], i[2]), (i[2], i[0])] {
            let ka = quantize_pos(part.positions[a]);
            let kb = quantize_pos(part.positions[b]);
            *edge_counts.entry((ka, kb)).or_insert(0) += 1;
            dir_edges.entry((ka, kb)).or_insert((a, b));
        }
    }
    let mut boundary: Vec<(usize, usize)> = Vec::new();
    for (&(ka, kb), &(a, b)) in &dir_edges {
        if !edge_counts.contains_key(&(kb, ka)) {
            boundary.push((a, b));
        }
    }

    let has_uvs = part.has_uvs();
    let push_vert = |part: &mut MeshPart, src: usize, pos_shift: Vec3, normal: [f32; 3]| -> u32 {
        let idx = part.positions.len() as u32;
        let p = Vec3::from_array(part.positions[src]) + pos_shift;
        part.positions.push(p.to_array());
        part.normals.push(normal);
        if has_uvs {
            part.uvs.push(part.uvs.get(src).copied().unwrap_or([0.0, 0.0]));
        }
        if let Some(c) = &mut part.colors {
            let v = c.get(src).copied().unwrap_or([1.0; 4]);
            c.push(v);
        }
        if let Some(j) = &mut part.joints {
            let v = j.get(src).copied().unwrap_or([0; 4]);
            j.push(v);
        }
        if let Some(w) = &mut part.weights {
            let v = w.get(src).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]);
            w.push(v);
        }
        idx
    };

    // Duplicate vertices for the extruded top cap so original base vertices stay intact for walls.
    let mut cap_map: HashMap<usize, u32> = HashMap::new();
    for &f in &selected {
        for k in 0..3 {
            let orig = part.indices[f * 3 + k] as usize;
            let new_idx = *cap_map.entry(orig).or_insert_with(|| {
                let n = part.normals.get(orig).copied().unwrap_or([0.0, 1.0, 0.0]);
                push_vert(part, orig, offset, n)
            });
            part.indices[f * 3 + k] = new_idx;
        }
    }

    // Build side wall quads along every boundary edge.
    for (a, b) in boundary {
        let pa = Vec3::from_array(part.positions[a]);
        let pb = Vec3::from_array(part.positions[b]);
        let wall_n = (pb - pa).cross(offset).normalize_or_zero();
        let wall_n = if wall_n == Vec3::ZERO { [0.0, 1.0, 0.0] } else { wall_n.to_array() };
        let v0 = push_vert(part, a, Vec3::ZERO, wall_n);
        let v1 = push_vert(part, b, Vec3::ZERO, wall_n);
        let v2 = push_vert(part, b, offset, wall_n);
        let v3 = push_vert(part, a, offset, wall_n);
        part.indices.extend_from_slice(&[v0, v1, v2, v0, v2, v3]);
    }

    part.tangents = None;
    part.recompute_normals_smooth();
    selected.into_iter().collect()
}

/// Insets the selected triangles inward toward their centroid by `amount` (`0.0..0.8`),
/// creating a clean framed rim around the selection.
pub fn inset_faces(part: &mut MeshPart, faces: &HashSet<usize>, amount: f32) -> HashSet<usize> {
    let tri_count = part.tri_count();
    let selected: Vec<usize> = faces.iter().copied().filter(|&f| f < tri_count).collect();
    if selected.is_empty() {
        return HashSet::new();
    }
    let amount = amount.clamp(0.05, 0.8);

    // Compute centroid of selected region
    let mut sum = Vec3::ZERO;
    let mut count = 0usize;
    let mut seen_keys = HashSet::new();
    for &f in &selected {
        for k in 0..3 {
            let p = part.positions[part.indices[f * 3 + k] as usize];
            if seen_keys.insert(quantize_pos(p)) {
                sum += Vec3::from_array(p);
                count += 1;
            }
        }
    }
    if count == 0 {
        return HashSet::new();
    }
    let center = sum / count as f32;

    // Find boundary edges
    let mut dir_edges: HashMap<([i32; 3], [i32; 3]), (usize, usize)> = HashMap::new();
    let mut edge_counts: HashMap<([i32; 3], [i32; 3]), usize> = HashMap::new();
    for &f in &selected {
        let i = [
            part.indices[f * 3] as usize,
            part.indices[f * 3 + 1] as usize,
            part.indices[f * 3 + 2] as usize,
        ];
        for (a, b) in [(i[0], i[1]), (i[1], i[2]), (i[2], i[0])] {
            let ka = quantize_pos(part.positions[a]);
            let kb = quantize_pos(part.positions[b]);
            *edge_counts.entry((ka, kb)).or_insert(0) += 1;
            dir_edges.entry((ka, kb)).or_insert((a, b));
        }
    }
    let boundary: Vec<(usize, usize)> = dir_edges
        .iter()
        .filter(|&(&(ka, kb), _)| !edge_counts.contains_key(&(kb, ka)))
        .map(|(_, &pair)| pair)
        .collect();

    let has_uvs = part.has_uvs();
    let mut inset_map: HashMap<usize, u32> = HashMap::new();
    let mut make_inset = |part: &mut MeshPart, src: usize| -> u32 {
        if let Some(&v) = inset_map.get(&src) {
            return v;
        }
        let idx = part.positions.len() as u32;
        let p = Vec3::from_array(part.positions[src]).lerp(center, amount);
        part.positions.push(p.to_array());
        let n = part.normals.get(src).copied().unwrap_or([0.0, 1.0, 0.0]);
        part.normals.push(n);
        if has_uvs {
            part.uvs.push(part.uvs.get(src).copied().unwrap_or([0.0, 0.0]));
        }
        if let Some(c) = &mut part.colors {
            let v = c.get(src).copied().unwrap_or([1.0; 4]);
            c.push(v);
        }
        if let Some(j) = &mut part.joints {
            let v = j.get(src).copied().unwrap_or([0; 4]);
            j.push(v);
        }
        if let Some(w) = &mut part.weights {
            let v = w.get(src).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]);
            w.push(v);
        }
        inset_map.insert(src, idx);
        idx
    };

    for &f in &selected {
        for k in 0..3 {
            let orig = part.indices[f * 3 + k] as usize;
            let ni = make_inset(part, orig);
            part.indices[f * 3 + k] = ni;
        }
    }
    for (a, b) in boundary {
        let ia = make_inset(part, a);
        let ib = make_inset(part, b);
        part.indices.extend_from_slice(&[a as u32, b as u32, ib, a as u32, ib, ia]);
    }
    part.tangents = None;
    part.recompute_normals_smooth();
    selected.into_iter().collect()
}

/// 1-to-4 midpoint triangle subdivision for selected faces (or all faces if
/// `faces` is empty). Preserves UVs, normals, colors, and skin weights.
pub fn subdivide_faces(part: &mut MeshPart, faces: &HashSet<usize>) {
    let tri_count = part.tri_count();
    let all = faces.is_empty();
    let has_uvs = part.has_uvs();
    let old_indices = std::mem::take(&mut part.indices);
    let mut new_indices = Vec::with_capacity(old_indices.len() * 4);
    let mut mid_cache: HashMap<(u32, u32), u32> = HashMap::new();

    let mut midpoint = |part: &mut MeshPart, a: u32, b: u32| -> u32 {
        let key = if a <= b { (a, b) } else { (b, a) };
        if let Some(&m) = mid_cache.get(&key) {
            return m;
        }
        let (ia, ib) = (a as usize, b as usize);
        let pa = Vec3::from_array(part.positions[ia]);
        let pb = Vec3::from_array(part.positions[ib]);
        let na = Vec3::from_array(part.normals.get(ia).copied().unwrap_or([0.0, 1.0, 0.0]));
        let nb = Vec3::from_array(part.normals.get(ib).copied().unwrap_or([0.0, 1.0, 0.0]));
        let nm = (na + nb).normalize_or_zero();
        let m = part.positions.len() as u32;
        part.positions.push(((pa + pb) * 0.5).to_array());
        part.normals.push(if nm == Vec3::ZERO { [0.0, 1.0, 0.0] } else { nm.to_array() });
        if has_uvs {
            let ua = Vec2::from_array(part.uvs[ia]);
            let ub = Vec2::from_array(part.uvs[ib]);
            part.uvs.push(((ua + ub) * 0.5).to_array());
        }
        if let Some(c) = &mut part.colors {
            let ca = c[ia];
            let cb = c[ib];
            c.push([
                (ca[0] + cb[0]) * 0.5,
                (ca[1] + cb[1]) * 0.5,
                (ca[2] + cb[2]) * 0.5,
                (ca[3] + cb[3]) * 0.5,
            ]);
        }
        if let Some(j) = &mut part.joints {
            let v = j[ia];
            j.push(v);
        }
        if let Some(w) = &mut part.weights {
            let v = w[ia];
            w.push(v);
        }
        mid_cache.insert(key, m);
        m
    };

    for t in 0..tri_count {
        let (i0, i1, i2) = (old_indices[t * 3], old_indices[t * 3 + 1], old_indices[t * 3 + 2]);
        if all || faces.contains(&t) {
            let m01 = midpoint(part, i0, i1);
            let m12 = midpoint(part, i1, i2);
            let m20 = midpoint(part, i2, i0);
            new_indices.extend_from_slice(&[
                i0, m01, m20,
                m01, i1, m12,
                m20, m12, i2,
                m01, m12, m20,
            ]);
        } else {
            new_indices.extend_from_slice(&[i0, i1, i2]);
        }
    }

    part.indices = new_indices;
    part.tangents = None;
    part.touch();
}

/// Removes selected triangles from a part.
pub fn delete_faces(part: &mut MeshPart, faces: &HashSet<usize>) -> bool {
    if faces.is_empty() {
        return false;
    }
    let tri_count = part.tri_count();
    let mut kept = Vec::with_capacity(part.indices.len());
    let mut removed = false;
    for t in 0..tri_count {
        if faces.contains(&t) {
            removed = true;
        } else {
            kept.extend_from_slice(&part.indices[t * 3..t * 3 + 3]);
        }
    }
    if removed {
        part.indices = kept;
        part.touch();
    }
    removed
}

/// Splits selected faces off `part_idx` into a new `MeshPart` with `new_mat`
/// (allowing per-face material assignment on a multi-face mesh!).
pub fn split_faces_to_new_part(
    doc: &mut ModelDoc,
    part_idx: usize,
    faces: &HashSet<usize>,
    new_mat: usize,
) -> Option<usize> {
    if part_idx >= doc.parts.len() || faces.is_empty() {
        return None;
    }
    let tri_count = doc.parts[part_idx].tri_count();
    let valid_count = faces.iter().filter(|&&f| f < tri_count).count();
    if valid_count == 0 || valid_count == tri_count {
        // If all faces selected, just reassign the part's material directly!
        if valid_count == tri_count {
            doc.parts[part_idx].material = new_mat;
            return Some(part_idx);
        }
        return None;
    }

    let src = &doc.parts[part_idx];
    let has_uvs = src.has_uvs();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut colors = src.colors.as_ref().map(|_| Vec::new());
    let mut joints = src.joints.as_ref().map(|_| Vec::new());
    let mut weights = src.weights.as_ref().map(|_| Vec::new());
    let mut indices = Vec::new();
    let mut remap: HashMap<u32, u32> = HashMap::new();

    for t in 0..tri_count {
        if !faces.contains(&t) {
            continue;
        }
        for k in 0..3 {
            let old = src.indices[t * 3 + k];
            let new_i = *remap.entry(old).or_insert_with(|| {
                let ni = positions.len() as u32;
                let oi = old as usize;
                positions.push(src.positions[oi]);
                normals.push(src.normals.get(oi).copied().unwrap_or([0.0, 1.0, 0.0]));
                if has_uvs {
                    uvs.push(src.uvs[oi]);
                }
                if let (Some(dst), Some(s)) = (colors.as_mut(), src.colors.as_ref()) {
                    dst.push(s[oi]);
                }
                if let (Some(dst), Some(s)) = (joints.as_mut(), src.joints.as_ref()) {
                    dst.push(s[oi]);
                }
                if let (Some(dst), Some(s)) = (weights.as_mut(), src.weights.as_ref()) {
                    dst.push(s[oi]);
                }
                ni
            });
            indices.push(new_i);
        }
    }

    let id = doc.alloc_id();
    let new_part = MeshPart {
        id,
        name: format!("{} Split", doc.parts[part_idx].name),
        visible: true,
        translation: doc.parts[part_idx].translation,
        rotation: doc.parts[part_idx].rotation,
        scale: doc.parts[part_idx].scale,
        positions,
        normals,
        uvs,
        tangents: None,
        colors,
        joints,
        weights,
        indices,
        material: new_mat,
        orig_mesh_group: doc.parts[part_idx].orig_mesh_group,
        revision: 1,
    };

    delete_faces(&mut doc.parts[part_idx], faces);
    doc.parts.push(new_part);
    Some(doc.parts.len() - 1)
}

/// Applies an interactive 3D sculpt brush (Push, Pull, Smooth, Flatten) at a
/// local-space hit point with cubic falloff and optional Mirror-X symmetry.
pub fn apply_sculpt_brush(
    part: &mut MeshPart,
    local_center: Vec3,
    local_normal: Vec3,
    local_radius: f32,
    strength: f32,
    tool: SculptTool,
    mirror_x: bool,
) -> bool {
    let mut changed = sculpt_dab(part, local_center, local_normal, local_radius, strength, tool);
    if mirror_x {
        let m_center = Vec3::new(-local_center.x, local_center.y, local_center.z);
        let m_norm = Vec3::new(-local_normal.x, local_normal.y, local_normal.z);
        changed |= sculpt_dab(part, m_center, m_norm, local_radius, strength, tool);
    }
    if changed {
        part.recompute_normals_smooth();
    }
    changed
}

fn sculpt_dab(
    part: &mut MeshPart,
    center: Vec3,
    normal: Vec3,
    radius: f32,
    strength: f32,
    tool: SculptTool,
) -> bool {
    let r = radius.max(1e-4);
    let r2 = r * r;
    let mut any = false;

    match tool {
        SculptTool::Push | SculptTool::Pull => {
            let sign = if tool == SculptTool::Push { 1.0 } else { -1.0 };
            let disp_dir = if normal.length_squared() > 1e-6 { normal.normalize() } else { Vec3::Y };
            for p in &mut part.positions {
                let v = Vec3::from_array(*p);
                let d2 = v.distance_squared(center);
                if d2 < r2 {
                    let t = 1.0 - (d2.sqrt() / r);
                    let w = t * t * (3.0 - 2.0 * t); // smoothstep falloff
                    let moved = v + disp_dir * (sign * strength * w * r * 0.12);
                    *p = moved.to_array();
                    any = true;
                }
            }
        }
        SculptTool::Flatten => {
            let n = if normal.length_squared() > 1e-6 { normal.normalize() } else { Vec3::Y };
            for p in &mut part.positions {
                let v = Vec3::from_array(*p);
                let d2 = v.distance_squared(center);
                if d2 < r2 {
                    let t = 1.0 - (d2.sqrt() / r);
                    let w = t * t * (3.0 - 2.0 * t) * strength.clamp(0.0, 1.0) * 0.35;
                    let proj = v - n * (v - center).dot(n);
                    *p = v.lerp(proj, w).to_array();
                    any = true;
                }
            }
        }
        SculptTool::Smooth => {
            let mut affected = HashSet::new();
            for (i, &p) in part.positions.iter().enumerate() {
                if Vec3::from_array(p).distance_squared(center) < r2 {
                    affected.insert(i);
                }
            }
            if !affected.is_empty() {
                smooth_vertices(part, &affected, (strength * 0.35).clamp(0.05, 0.7));
                any = true;
            }
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PrimitiveKind, build_primitive};

    #[test]
    fn extrude_and_subdivide_keep_valid_geometry() {
        let mut part = build_primitive(PrimitiveKind::Cube, 1, 0);
        let coplanar = select_coplanar_faces(&part, 0);
        assert_eq!(coplanar.len(), 2, "cube face should select both coplanar triangles");
        let before_tris = part.tri_count();
        let cap = extrude_faces(&mut part, &coplanar, 0.5);
        assert_eq!(cap.len(), 2);
        assert!(part.tri_count() > before_tris);

        let before_sub = part.tri_count();
        subdivide_faces(&mut part, &HashSet::new());
        assert_eq!(part.tri_count(), before_sub * 4);
    }

    #[test]
    fn split_faces_separates_into_second_part() {
        let mut doc = ModelDoc::starter_cube();
        let m2 = doc.add_material("Red");
        let coplanar = select_coplanar_faces(&doc.parts[0], 0);
        let new_idx = split_faces_to_new_part(&mut doc, 0, &coplanar, m2).unwrap();
        assert_eq!(doc.parts.len(), 2);
        assert_eq!(doc.parts[new_idx].material, m2);
        assert_eq!(doc.parts[0].tri_count() + doc.parts[new_idx].tri_count(), 12);
    }
}
