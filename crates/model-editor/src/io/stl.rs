//! Pure-Rust Binary and ASCII STL importer and Binary STL exporter.

use crate::model::{MeshPart, ModelDoc};
use bevy::prelude::Vec3;
use std::path::Path;

pub fn load(path: &Path) -> Result<ModelDoc, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Couldn't read STL '{}': {e}", path.display()))?;

    let (positions, normals, indices) = if is_binary_stl(&bytes) {
        parse_binary(&bytes)?
    } else {
        parse_ascii(&bytes)?
    };
    if indices.len() < 3 {
        return Err(format!("No triangles found in STL '{}'", path.display()));
    }

    let mut doc = ModelDoc::empty();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("STL Mesh")
        .to_string();
    let id = doc.alloc_id();
    doc.parts.push(MeshPart {
        id,
        name,
        visible: true,
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        positions,
        normals,
        uvs: Vec::new(),
        tangents: None,
        colors: None,
        joints: None,
        weights: None,
        indices,
        material: 0,
        orig_mesh_group: None,
        revision: 1,
    });
    Ok(doc)
}

fn is_binary_stl(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let expected = 84usize.saturating_add(count.saturating_mul(50));
    if bytes.len() == expected {
        return true;
    }
    // Fall back to checking whether it starts with valid ASCII "solid" + "facet"
    let head = std::str::from_utf8(&bytes[..bytes.len().min(512)]).unwrap_or("");
    !(head.trim_start().starts_with("solid") && head.contains("facet"))
}

fn read_f32_le(bytes: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
}

fn parse_binary(bytes: &[u8]) -> Result<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>), String> {
    if bytes.len() < 84 {
        return Err("STL file too small".into());
    }
    let tri_count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let max_tris = (bytes.len() - 84) / 50;
    let count = tri_count.min(max_tris);

    let mut positions = Vec::with_capacity(count * 3);
    let mut normals = Vec::with_capacity(count * 3);
    let mut indices = Vec::with_capacity(count * 3);

    for i in 0..count {
        let base = 84 + i * 50;
        let nx = read_f32_le(bytes, base);
        let ny = read_f32_le(bytes, base + 4);
        let nz = read_f32_le(bytes, base + 8);
        let v0 = [read_f32_le(bytes, base + 12), read_f32_le(bytes, base + 16), read_f32_le(bytes, base + 20)];
        let v1 = [read_f32_le(bytes, base + 24), read_f32_le(bytes, base + 28), read_f32_le(bytes, base + 32)];
        let v2 = [read_f32_le(bytes, base + 36), read_f32_le(bytes, base + 40), read_f32_le(bytes, base + 44)];

        let mut n = Vec3::new(nx, ny, nz).normalize_or_zero();
        if n == Vec3::ZERO {
            n = (Vec3::from_array(v1) - Vec3::from_array(v0))
                .cross(Vec3::from_array(v2) - Vec3::from_array(v0))
                .normalize_or_zero();
        }
        let n_arr = if n == Vec3::ZERO { [0.0, 1.0, 0.0] } else { n.to_array() };
        let b = positions.len() as u32;
        positions.extend_from_slice(&[v0, v1, v2]);
        normals.extend_from_slice(&[n_arr, n_arr, n_arr]);
        indices.extend_from_slice(&[b, b + 1, b + 2]);
    }
    Ok((positions, normals, indices))
}

fn parse_ascii(bytes: &[u8]) -> Result<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in ASCII STL".to_string())?;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut cur_verts: Vec<[f32; 3]> = Vec::new();
    let mut cur_norm = [0.0f32, 1.0, 0.0];

    for line in text.lines() {
        let mut toks = line.split_whitespace();
        let Some(first) = toks.next() else { continue };
        match first {
            "facet" => {
                cur_verts.clear();
                if toks.next() == Some("normal") {
                    let x = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    let y = toks.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
                    let z = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    cur_norm = [x, y, z];
                }
            }
            "vertex" => {
                let x = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let y = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let z = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                cur_verts.push([x, y, z]);
            }
            "endfacet" => {
                if cur_verts.len() >= 3 {
                    let mut n = Vec3::from_array(cur_norm).normalize_or_zero();
                    if n == Vec3::ZERO {
                        n = (Vec3::from_array(cur_verts[1]) - Vec3::from_array(cur_verts[0]))
                            .cross(Vec3::from_array(cur_verts[2]) - Vec3::from_array(cur_verts[0]))
                            .normalize_or_zero();
                    }
                    let n_arr = if n == Vec3::ZERO { [0.0, 1.0, 0.0] } else { n.to_array() };
                    for w in cur_verts[1..].windows(2) {
                        let b = positions.len() as u32;
                        positions.extend_from_slice(&[cur_verts[0], w[0], w[1]]);
                        normals.extend_from_slice(&[n_arr, n_arr, n_arr]);
                        indices.extend_from_slice(&[b, b + 1, b + 2]);
                    }
                }
                cur_verts.clear();
            }
            _ => {}
        }
    }
    Ok((positions, normals, indices))
}

/// Writes a Binary STL file containing all visible parts baked to world space.
pub fn save(doc: &ModelDoc, path: &Path) -> Result<(), String> {
    let mut tris: Vec<([f32; 3], [f32; 3], [f32; 3], [f32; 3])> = Vec::new();
    for part in &doc.parts {
        if !part.visible {
            continue;
        }
        let m = part.model_matrix();
        for tri in part.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= part.positions.len() || i1 >= part.positions.len() || i2 >= part.positions.len() {
                continue;
            }
            let v0 = m.transform_point3(Vec3::from_array(part.positions[i0]));
            let v1 = m.transform_point3(Vec3::from_array(part.positions[i1]));
            let v2 = m.transform_point3(Vec3::from_array(part.positions[i2]));
            let n = (v1 - v0).cross(v2 - v0).normalize_or_zero();
            let n = if n == Vec3::ZERO { Vec3::Y } else { n };
            tris.push((n.to_array(), v0.to_array(), v1.to_array(), v2.to_array()));
        }
    }

    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = [0u8; 80];
    let banner = b"Exported by Ink-cognito Model Editor";
    header[..banner.len()].copy_from_slice(banner);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());

    for (n, v0, v1, v2) in tris {
        for v in [n, v0, v1, v2] {
            for f in v {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }

    std::fs::write(path, out).map_err(|e| format!("Couldn't write STL '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stl_round_trips_cube() {
        let doc = ModelDoc::starter_cube();
        let tmp = std::env::temp_dir().join("ink_model_editor_test_stl.stl");
        save(&doc, &tmp).expect("save stl");
        let loaded = load(&tmp).expect("load stl");
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(loaded.triangle_count(), 12);
    }
}
