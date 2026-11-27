//! Pure-Rust Wavefront OBJ + MTL importer and exporter.

use crate::{
    io::{load_texture, save_texture_png},
    model::{AlphaKind, MaterialSlot, MeshPart, ModelDoc, srgb_to_linear},
};
use bevy::prelude::Vec3;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

pub fn load(path: &Path) -> Result<ModelDoc, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Couldn't read OBJ '{}': {e}", path.display()))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let mut doc = ModelDoc::empty();
    doc.materials.clear();

    let mut mat_map: HashMap<String, usize> = HashMap::new();
    let mut raw_pos: Vec<[f32; 3]> = Vec::new();
    let mut raw_uv: Vec<[f32; 2]> = Vec::new();
    let mut raw_norm: Vec<[f32; 3]> = Vec::new();

    // Builders keyed by (group_name, mat_index)
    struct GroupBuf {
        name: String,
        mat: usize,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        vert_cache: HashMap<(usize, Option<usize>, Option<usize>), u32>,
        any_uv: bool,
        any_norm: bool,
    }

    let mut groups: Vec<GroupBuf> = Vec::new();
    let mut group_lookup: HashMap<(String, usize), usize> = HashMap::new();
    let mut cur_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Object")
        .to_string();
    let mut cur_mat: usize = 0;

    let ensure_default = |doc: &mut ModelDoc, mat_map: &mut HashMap<String, usize>| {
        if doc.materials.is_empty() {
            doc.materials.push(MaterialSlot::default());
            mat_map.insert("Material".to_string(), 0);
        }
    };

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { continue };
        match cmd {
            "mtllib" => {
                let rest = line[cmd.len()..].trim();
                let mtl_path = base_dir.join(rest);
                if mtl_path.is_file() {
                    let _ = parse_mtl(&mtl_path, &mut doc, &mut mat_map);
                }
            }
            "v" => {
                let x = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                let y = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                let z = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                raw_pos.push([x, y, z]);
            }
            "vt" => {
                let u = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                let v = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                // OBJ V=0 is bottom; glTF/Bevy V=0 is top.
                raw_uv.push([u, 1.0 - v]);
            }
            "vn" => {
                let x = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                let y = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
                let z = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                raw_norm.push([x, y, z]);
            }
            "o" | "g" => {
                let name = line[cmd.len()..].trim();
                if !name.is_empty() {
                    cur_name = name.to_string();
                }
            }
            "usemtl" => {
                let name = line[cmd.len()..].trim();
                if !name.is_empty() {
                    cur_mat = if let Some(&i) = mat_map.get(name) {
                        i
                    } else {
                        let i = doc.materials.len();
                        let mut m = MaterialSlot::default();
                        m.name = name.to_string();
                        doc.materials.push(m);
                        mat_map.insert(name.to_string(), i);
                        i
                    };
                }
            }
            "f" => {
                ensure_default(&mut doc, &mut mat_map);
                let verts: Vec<(usize, Option<usize>, Option<usize>)> = parts
                    .filter_map(|tok| parse_face_vert(tok, raw_pos.len(), raw_uv.len(), raw_norm.len()))
                    .collect();
                if verts.len() < 3 {
                    continue;
                }
                let key = (cur_name.clone(), cur_mat);
                let gi = if let Some(&i) = group_lookup.get(&key) {
                    i
                } else {
                    let i = groups.len();
                    groups.push(GroupBuf {
                        name: cur_name.clone(),
                        mat: cur_mat,
                        positions: Vec::new(),
                        normals: Vec::new(),
                        uvs: Vec::new(),
                        indices: Vec::new(),
                        vert_cache: HashMap::new(),
                        any_uv: false,
                        any_norm: false,
                    });
                    group_lookup.insert(key, i);
                    i
                };
                let g = &mut groups[gi];
                let mut resolve = |v: (usize, Option<usize>, Option<usize>)| -> u32 {
                    if let Some(&idx) = g.vert_cache.get(&v) {
                        return idx;
                    }
                    let idx = g.positions.len() as u32;
                    g.positions.push(raw_pos[v.0]);
                    if let Some(ti) = v.1 {
                        g.uvs.push(raw_uv[ti]);
                        g.any_uv = true;
                    } else {
                        g.uvs.push([0.0, 0.0]);
                    }
                    if let Some(ni) = v.2 {
                        g.normals.push(raw_norm[ni]);
                        g.any_norm = true;
                    } else {
                        g.normals.push([0.0, 1.0, 0.0]);
                    }
                    g.vert_cache.insert(v, idx);
                    idx
                };
                let i0 = resolve(verts[0]);
                for w in verts[1..].windows(2) {
                    let i1 = resolve(w[0]);
                    let i2 = resolve(w[1]);
                    g.indices.extend_from_slice(&[i0, i1, i2]);
                }
            }
            _ => {}
        }
    }

    ensure_default(&mut doc, &mut mat_map);
    for g in groups {
        if g.indices.len() < 3 {
            continue;
        }
        let id = doc.alloc_id();
        let mut part = MeshPart {
            id,
            name: g.name,
            visible: true,
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            positions: g.positions,
            normals: g.normals,
            uvs: if g.any_uv { g.uvs } else { Vec::new() },
            tangents: None,
            colors: None,
            joints: None,
            weights: None,
            indices: g.indices,
            material: g.mat,
            orig_mesh_group: None,
            revision: 1,
        };
        if !g.any_norm {
            part.recompute_normals_smooth();
        }
        doc.parts.push(part);
    }

    if doc.parts.is_empty() {
        return Err(format!("No mesh faces found in OBJ '{}'", path.display()));
    }
    Ok(doc)
}

fn parse_obj_index(s: &str, len: usize) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let idx: isize = s.parse().ok()?;
    let resolved = if idx > 0 {
        (idx - 1) as isize
    } else if idx < 0 {
        len as isize + idx
    } else {
        return None;
    };
    (resolved >= 0 && (resolved as usize) < len).then_some(resolved as usize)
}

fn parse_face_vert(
    tok: &str,
    pos_len: usize,
    uv_len: usize,
    norm_len: usize,
) -> Option<(usize, Option<usize>, Option<usize>)> {
    let mut split = tok.split('/');
    let p = parse_obj_index(split.next()?, pos_len)?;
    let t = split.next().and_then(|s| parse_obj_index(s, uv_len));
    let n = split.next().and_then(|s| parse_obj_index(s, norm_len));
    Some((p, t, n))
}

fn parse_mtl(
    path: &Path,
    doc: &mut ModelDoc,
    mat_map: &mut HashMap<String, usize>,
) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut current: Option<usize> = None;

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { continue };
        match cmd {
            "newmtl" => {
                let name = line[cmd.len()..].trim().to_string();
                let idx = if let Some(&i) = mat_map.get(&name) {
                    i
                } else {
                    let i = doc.materials.len();
                    let mut m = MaterialSlot::default();
                    m.name = name.clone();
                    doc.materials.push(m);
                    mat_map.insert(name, i);
                    i
                };
                current = Some(idx);
            }
            "Kd" => {
                if let Some(ci) = current {
                    let r = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.8);
                    let g = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.8);
                    let b = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.8);
                    doc.materials[ci].base_color[0] = srgb_to_linear(r);
                    doc.materials[ci].base_color[1] = srgb_to_linear(g);
                    doc.materials[ci].base_color[2] = srgb_to_linear(b);
                }
            }
            "Ke" => {
                if let Some(ci) = current {
                    let r = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                    let g = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                    let b = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                    doc.materials[ci].emissive = [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)];
                }
            }
            "d" => {
                if let Some(ci) = current
                    && let Some(a) = parts.next().and_then(|s| s.parse::<f32>().ok())
                {
                    doc.materials[ci].base_color[3] = a.clamp(0.0, 1.0);
                    if a < 0.99 {
                        doc.materials[ci].alpha_mode = AlphaKind::Blend;
                    }
                }
            }
            "Tr" => {
                if let Some(ci) = current
                    && let Some(tr) = parts.next().and_then(|s| s.parse::<f32>().ok())
                {
                    let a = (1.0 - tr).clamp(0.0, 1.0);
                    doc.materials[ci].base_color[3] = a;
                    if a < 0.99 {
                        doc.materials[ci].alpha_mode = AlphaKind::Blend;
                    }
                }
            }
            "Pr" => {
                if let Some(ci) = current
                    && let Some(r) = parts.next().and_then(|s| s.parse::<f32>().ok())
                {
                    doc.materials[ci].roughness = r.clamp(0.0, 1.0);
                }
            }
            "Pm" => {
                if let Some(ci) = current
                    && let Some(m) = parts.next().and_then(|s| s.parse::<f32>().ok())
                {
                    doc.materials[ci].metallic = m.clamp(0.0, 1.0);
                }
            }
            "map_Kd" => {
                if let Some(ci) = current {
                    // Last token is typically the file path (earlier tokens can be options like -s)
                    let rel = line[cmd.len()..].trim();
                    let tex_path = base_dir.join(rel);
                    if tex_path.is_file()
                        && let Ok(tex) = load_texture(&tex_path)
                    {
                        let ti = doc.textures.len();
                        doc.textures.push(tex);
                        doc.materials[ci].base_color_texture = Some(ti);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Exports a `ModelDoc` to Wavefront `.obj` alongside a `.mtl` and PNG textures.
pub fn save(doc: &ModelDoc, path: &Path) -> Result<(), String> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mtl_name = format!("{stem}.mtl");
    let mtl_path = dir.join(&mtl_name);

    let mut mtl = String::from("# Exported by Ink-cognito Model Editor\n");
    for (i, m) in doc.materials.iter().enumerate() {
        let safe_name = sanitize_name(&m.name, i);
        let srgb = m.base_color_srgb();
        let em = m.emissive_srgb();
        let _ = writeln!(mtl, "\nnewmtl {safe_name}");
        let _ = writeln!(mtl, "Kd {:.5} {:.5} {:.5}", srgb[0], srgb[1], srgb[2]);
        let _ = writeln!(mtl, "Ke {:.5} {:.5} {:.5}", em[0], em[1], em[2]);
        let _ = writeln!(mtl, "d {:.5}", srgb[3]);
        let _ = writeln!(mtl, "Pr {:.5}", m.roughness);
        let _ = writeln!(mtl, "Pm {:.5}", m.metallic);
        if let Some(ti) = m.base_color_texture
            && let Some(tex) = doc.textures.get(ti)
        {
            let png_name = format!("{stem}_tex_{ti}.png");
            save_texture_png(tex, &dir.join(&png_name))?;
            let _ = writeln!(mtl, "map_Kd {png_name}");
        }
    }
    std::fs::write(&mtl_path, mtl)
        .map_err(|e| format!("Couldn't write MTL '{}': {e}", mtl_path.display()))?;

    let mut obj = String::from("# Exported by Ink-cognito Model Editor\n");
    let _ = writeln!(obj, "mtllib {mtl_name}");

    let mut v_off: u32 = 1;
    let mut vt_off: u32 = 1;
    let mut vn_off: u32 = 1;

    for (pi, part) in doc.parts.iter().enumerate() {
        if !part.visible || part.indices.len() < 3 {
            continue;
        }
        let mat_name = doc
            .materials
            .get(part.material)
            .map(|m| sanitize_name(&m.name, part.material))
            .unwrap_or_else(|| "Material_0".into());
        let obj_name = sanitize_name(&part.name, pi);
        let _ = writeln!(obj, "\no {obj_name}");
        let _ = writeln!(obj, "usemtl {mat_name}");

        let m = part.model_matrix();
        let normal_mat = m.inverse().transpose();
        for &p in &part.positions {
            let wp = m.transform_point3(Vec3::from_array(p));
            let _ = writeln!(obj, "v {:.6} {:.6} {:.6}", wp.x, wp.y, wp.z);
        }
        let has_uv = part.has_uvs();
        if has_uv {
            for &[u, v] in &part.uvs {
                let _ = writeln!(obj, "vt {:.6} {:.6}", u, 1.0 - v);
            }
        }
        let has_n = part.normals.len() == part.positions.len();
        if has_n {
            for &n in &part.normals {
                let wn = normal_mat.transform_vector3(Vec3::from_array(n)).normalize_or_zero();
                let wn = if wn == Vec3::ZERO { Vec3::Y } else { wn };
                let _ = writeln!(obj, "vn {:.6} {:.6} {:.6}", wn.x, wn.y, wn.z);
            }
        }
        for tri in part.indices.chunks_exact(3) {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            match (has_uv, has_n) {
                (true, true) => {
                    let _ = writeln!(
                        obj,
                        "f {}/{}/{} {}/{}/{} {}/{}/{}",
                        v_off + a, vt_off + a, vn_off + a,
                        v_off + b, vt_off + b, vn_off + b,
                        v_off + c, vt_off + c, vn_off + c,
                    );
                }
                (true, false) => {
                    let _ = writeln!(
                        obj,
                        "f {}/{} {}/{} {}/{}",
                        v_off + a, vt_off + a,
                        v_off + b, vt_off + b,
                        v_off + c, vt_off + c,
                    );
                }
                (false, true) => {
                    let _ = writeln!(
                        obj,
                        "f {}//{} {}//{} {}//{}",
                        v_off + a, vn_off + a,
                        v_off + b, vn_off + b,
                        v_off + c, vn_off + c,
                    );
                }
                (false, false) => {
                    let _ = writeln!(obj, "f {} {} {}", v_off + a, v_off + b, v_off + c);
                }
            }
        }
        v_off += part.positions.len() as u32;
        if has_uv {
            vt_off += part.uvs.len() as u32;
        }
        if has_n {
            vn_off += part.normals.len() as u32;
        }
    }

    std::fs::write(path, obj).map_err(|e| format!("Couldn't write OBJ '{}': {e}", path.display()))
}

fn sanitize_name(name: &str, idx: usize) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if s.is_empty() { format!("part_{idx}") } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_round_trips_cube_with_material() {
        let doc = ModelDoc::starter_cube();
        let tmp = std::env::temp_dir().join("ink_model_editor_test_obj.obj");
        save(&doc, &tmp).expect("save obj");
        let loaded = load(&tmp).expect("load obj");
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(tmp.with_extension("mtl"));
        assert_eq!(loaded.triangle_count(), 12);
        assert!(loaded.parts[0].has_uvs());
    }
}
