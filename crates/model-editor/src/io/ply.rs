//! Pure-Rust PLY importer (ASCII and binary little-endian) and binary LE exporter.

use crate::model::{MeshPart, ModelDoc};
use bevy::prelude::Vec3;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlyFormat {
    Ascii,
    BinaryLe,
}

#[derive(Clone, Copy)]
enum ScalarTy {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl ScalarTy {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "char" | "int8" => Some(Self::I8),
            "uchar" | "uint8" => Some(Self::U8),
            "short" | "int16" => Some(Self::I16),
            "ushort" | "uint16" => Some(Self::U16),
            "int" | "int32" => Some(Self::I32),
            "uint" | "uint32" => Some(Self::U32),
            "float" | "float32" => Some(Self::F32),
            "double" | "float64" => Some(Self::F64),
            _ => None,
        }
    }

    fn read_le(self, bytes: &[u8], off: &mut usize) -> Result<f64, String> {
        let need = match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        };
        if *off + need > bytes.len() {
            return Err("Unexpected EOF reading PLY binary data".into());
        }
        let s = &bytes[*off..*off + need];
        *off += need;
        Ok(match self {
            Self::I8 => s[0] as i8 as f64,
            Self::U8 => s[0] as f64,
            Self::I16 => i16::from_le_bytes(s.try_into().unwrap()) as f64,
            Self::U16 => u16::from_le_bytes(s.try_into().unwrap()) as f64,
            Self::I32 => i32::from_le_bytes(s.try_into().unwrap()) as f64,
            Self::U32 => u32::from_le_bytes(s.try_into().unwrap()) as f64,
            Self::F32 => f32::from_le_bytes(s.try_into().unwrap()) as f64,
            Self::F64 => f64::from_le_bytes(s.try_into().unwrap()),
        })
    }

    fn is_uchar(self) -> bool {
        matches!(self, Self::U8)
    }
}

enum Prop {
    Scalar(ScalarTy, String),
    List(ScalarTy, ScalarTy),
}

struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

pub fn load(path: &Path) -> Result<ModelDoc, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Couldn't read PLY '{}': {e}", path.display()))?;

    // Find end_header line.
    let marker = b"end_header";
    let end_pos = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .ok_or_else(|| "Missing end_header in PLY file".to_string())?;
    let mut data_off = end_pos + marker.len();
    if data_off < bytes.len() && bytes[data_off] == b'\r' {
        data_off += 1;
    }
    if data_off < bytes.len() && bytes[data_off] == b'\n' {
        data_off += 1;
    }

    let header = std::str::from_utf8(&bytes[..end_pos]).map_err(|_| "Invalid PLY header".to_string())?;
    let mut format = PlyFormat::Ascii;
    let mut elements: Vec<Element> = Vec::new();

    for line in header.lines() {
        let mut toks = line.split_whitespace();
        let Some(cmd) = toks.next() else { continue };
        match cmd {
            "format" => match toks.next() {
                Some("ascii") => format = PlyFormat::Ascii,
                Some("binary_little_endian") => format = PlyFormat::BinaryLe,
                Some(other) => return Err(format!("Unsupported PLY format '{other}'")),
                None => {}
            },
            "element" => {
                let name = toks.next().unwrap_or("").to_string();
                let count = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                elements.push(Element {
                    name,
                    count,
                    props: Vec::new(),
                });
            }
            "property" => {
                if let Some(el) = elements.last_mut() {
                    let first = toks.next().unwrap_or("");
                    if first == "list" {
                        let ct = toks.next().and_then(ScalarTy::parse);
                        let it = toks.next().and_then(ScalarTy::parse);
                        if let (Some(ct), Some(it)) = (ct, it) {
                            el.props.push(Prop::List(ct, it));
                        }
                    } else if let Some(ty) = ScalarTy::parse(first) {
                        let name = toks.next().unwrap_or("").to_string();
                        el.props.push(Prop::Scalar(ty, name));
                    }
                }
            }
            _ => {}
        }
    }

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut any_norm = false;
    let mut any_uv = false;
    let mut any_color = false;

    match format {
        PlyFormat::Ascii => {
            let body = std::str::from_utf8(&bytes[data_off..])
                .map_err(|_| "Invalid UTF-8 in ASCII PLY body".to_string())?;
            let mut lines = body.lines().filter(|l| !l.trim().is_empty());
            for el in &elements {
                for _ in 0..el.count {
                    let Some(line) = lines.next() else { break };
                    let mut toks = line.split_whitespace();
                    if el.name == "vertex" {
                        let mut p = [0.0f32; 3];
                        let mut n = [0.0f32, 1.0, 0.0];
                        let mut uv = [0.0f32; 2];
                        let mut col = [1.0f32; 4];
                        for prop in &el.props {
                            if let Prop::Scalar(ty, name) = prop {
                                let v = toks.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                                apply_vertex_prop(name, *ty, v, &mut p, &mut n, &mut uv, &mut col, &mut any_norm, &mut any_uv, &mut any_color);
                            }
                        }
                        positions.push(p);
                        normals.push(n);
                        uvs.push(uv);
                        colors.push(col);
                    } else if el.name == "face" {
                        for prop in &el.props {
                            match prop {
                                Prop::List(_, _) => {
                                    let cnt = toks.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                                    let mut face = Vec::with_capacity(cnt);
                                    for _ in 0..cnt {
                                        if let Some(i) = toks.next().and_then(|s| s.parse::<u32>().ok()) {
                                            face.push(i);
                                        }
                                    }
                                    if face.len() >= 3 {
                                        for w in face[1..].windows(2) {
                                            indices.extend_from_slice(&[face[0], w[0], w[1]]);
                                        }
                                    }
                                }
                                Prop::Scalar(_, _) => {
                                    let _ = toks.next();
                                }
                            }
                        }
                    }
                }
            }
        }
        PlyFormat::BinaryLe => {
            let mut off = data_off;
            for el in &elements {
                for _ in 0..el.count {
                    if el.name == "vertex" {
                        let mut p = [0.0f32; 3];
                        let mut n = [0.0f32, 1.0, 0.0];
                        let mut uv = [0.0f32; 2];
                        let mut col = [1.0f32; 4];
                        for prop in &el.props {
                            match prop {
                                Prop::Scalar(ty, name) => {
                                    let v = ty.read_le(&bytes, &mut off)?;
                                    apply_vertex_prop(name, *ty, v, &mut p, &mut n, &mut uv, &mut col, &mut any_norm, &mut any_uv, &mut any_color);
                                }
                                Prop::List(ct, it) => {
                                    let cnt = ct.read_le(&bytes, &mut off)? as usize;
                                    for _ in 0..cnt {
                                        let _ = it.read_le(&bytes, &mut off)?;
                                    }
                                }
                            }
                        }
                        positions.push(p);
                        normals.push(n);
                        uvs.push(uv);
                        colors.push(col);
                    } else if el.name == "face" {
                        for prop in &el.props {
                            match prop {
                                Prop::List(ct, it) => {
                                    let cnt = ct.read_le(&bytes, &mut off)? as usize;
                                    let mut face = Vec::with_capacity(cnt);
                                    for _ in 0..cnt {
                                        face.push(it.read_le(&bytes, &mut off)? as u32);
                                    }
                                    if face.len() >= 3 {
                                        for w in face[1..].windows(2) {
                                            indices.extend_from_slice(&[face[0], w[0], w[1]]);
                                        }
                                    }
                                }
                                Prop::Scalar(ty, _) => {
                                    let _ = ty.read_le(&bytes, &mut off)?;
                                }
                            }
                        }
                    } else {
                        for prop in &el.props {
                            match prop {
                                Prop::Scalar(ty, _) => {
                                    let _ = ty.read_le(&bytes, &mut off)?;
                                }
                                Prop::List(ct, it) => {
                                    let cnt = ct.read_le(&bytes, &mut off)? as usize;
                                    for _ in 0..cnt {
                                        let _ = it.read_le(&bytes, &mut off)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if positions.is_empty() || indices.len() < 3 {
        return Err(format!("No mesh faces found in PLY '{}'", path.display()));
    }

    let mut doc = ModelDoc::empty();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("PLY Mesh")
        .to_string();
    let id = doc.alloc_id();
    let mut part = MeshPart {
        id,
        name,
        visible: true,
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        positions,
        normals,
        uvs: if any_uv { uvs } else { Vec::new() },
        tangents: None,
        colors: any_color.then_some(colors),
        joints: None,
        weights: None,
        indices,
        material: 0,
        orig_mesh_group: None,
        revision: 1,
    };
    if !any_norm {
        part.recompute_normals_smooth();
    }
    doc.parts.push(part);
    Ok(doc)
}

#[allow(clippy::too_many_arguments)]
fn apply_vertex_prop(
    name: &str,
    ty: ScalarTy,
    v: f64,
    p: &mut [f32; 3],
    n: &mut [f32; 3],
    uv: &mut [f32; 2],
    col: &mut [f32; 4],
    any_norm: &mut bool,
    any_uv: &mut bool,
    any_color: &mut bool,
) {
    let norm_c = if ty.is_uchar() { (v / 255.0) as f32 } else { v as f32 };
    match name {
        "x" => p[0] = v as f32,
        "y" => p[1] = v as f32,
        "z" => p[2] = v as f32,
        "nx" => { n[0] = v as f32; *any_norm = true; }
        "ny" => { n[1] = v as f32; *any_norm = true; }
        "nz" => { n[2] = v as f32; *any_norm = true; }
        "s" | "u" | "texture_u" => { uv[0] = v as f32; *any_uv = true; }
        "t" | "v" | "texture_v" => { uv[1] = v as f32; *any_uv = true; }
        "red" | "r" => { col[0] = norm_c; *any_color = true; }
        "green" | "g" => { col[1] = norm_c; *any_color = true; }
        "blue" | "b" => { col[2] = norm_c; *any_color = true; }
        "alpha" | "a" => { col[3] = norm_c; *any_color = true; }
        _ => {}
    }
}

/// Exports all visible parts to a binary little-endian `.ply` file.
pub fn save(doc: &ModelDoc, path: &Path) -> Result<(), String> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::new();

    for part in &doc.parts {
        if !part.visible {
            continue;
        }
        let base = positions.len() as u32;
        let m = part.model_matrix();
        let nm = m.inverse().transpose();
        for (i, &p) in part.positions.iter().enumerate() {
            positions.push(m.transform_point3(Vec3::from_array(p)).to_array());
            let n = part.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let wn = nm.transform_vector3(Vec3::from_array(n)).normalize_or_zero();
            normals.push(if wn == Vec3::ZERO { [0.0, 1.0, 0.0] } else { wn.to_array() });
            uvs.push(part.uvs.get(i).copied().unwrap_or([0.0, 0.0]));
        }
        for tri in part.indices.chunks_exact(3) {
            faces.push([base + tri[0], base + tri[1], base + tri[2]]);
        }
    }

    let header = format!(
        "ply\nformat binary_little_endian 1.0\ncomment Exported by Ink-cognito Model Editor\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\nproperty float s\nproperty float t\nelement face {}\nproperty list uchar int vertex_indices\nend_header\n",
        positions.len(),
        faces.len()
    );

    let mut out = Vec::with_capacity(header.len() + positions.len() * 32 + faces.len() * 13);
    out.extend_from_slice(header.as_bytes());
    for i in 0..positions.len() {
        for f in positions[i] {
            out.extend_from_slice(&f.to_le_bytes());
        }
        for f in normals[i] {
            out.extend_from_slice(&f.to_le_bytes());
        }
        for f in uvs[i] {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    for f in faces {
        out.push(3u8);
        for idx in f {
            out.extend_from_slice(&(idx as i32).to_le_bytes());
        }
    }
    std::fs::write(path, out).map_err(|e| format!("Couldn't write PLY '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ply_round_trips_cube() {
        let doc = ModelDoc::starter_cube();
        let tmp = std::env::temp_dir().join("ink_model_editor_test_ply.ply");
        save(&doc, &tmp).expect("save ply");
        let loaded = load(&tmp).expect("load ply");
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(loaded.triangle_count(), 12);
        assert!(loaded.parts[0].has_uvs());
    }
}
