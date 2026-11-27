//! 3D model and texture I/O supporting GLB, glTF, OBJ (+ MTL), STL, and PLY,
//! plus PNG/JPEG/WebP/BMP/TGA texture import and PNG texture export.

pub mod glb;
pub mod obj;
pub mod ply;
pub mod stl;

use crate::model::{ModelDoc, TextureSlot};
use std::path::Path;

/// Supported 3D model file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Glb,
    Gltf,
    Obj,
    Stl,
    Ply,
}

impl ModelFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "glb" => Some(Self::Glb),
            "gltf" => Some(Self::Gltf),
            "obj" => Some(Self::Obj),
            "stl" => Some(Self::Stl),
            "ply" => Some(Self::Ply),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Glb => "glb",
            Self::Gltf => "gltf",
            Self::Obj => "obj",
            Self::Stl => "stl",
            Self::Ply => "ply",
        }
    }
}

pub fn is_model_path(path: &Path) -> bool {
    ModelFormat::from_path(path).is_some()
}

pub fn is_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tga"
    )
}

/// Loads any supported 3D model file into a `ModelDoc`.
pub fn load_model(path: &Path) -> Result<ModelDoc, String> {
    let format = ModelFormat::from_path(path).ok_or_else(|| {
        format!(
            "Unsupported model format for '{}' (supported: .glb, .gltf, .obj, .stl, .ply)",
            path.display()
        )
    })?;
    let mut doc = match format {
        ModelFormat::Glb | ModelFormat::Gltf => glb::load(path)?,
        ModelFormat::Obj => obj::load(path)?,
        ModelFormat::Stl => stl::load(path)?,
        ModelFormat::Ply => ply::load(path)?,
    };
    if doc.materials.is_empty() {
        doc.ensure_default_material();
    }
    for p in &mut doc.parts {
        if p.material >= doc.materials.len() {
            p.material = 0;
        }
    }
    Ok(doc)
}

/// Saves a `ModelDoc` to disk. Format is selected by file extension (defaulting
/// to `.glb` if unknown).
pub fn save_model(doc: &ModelDoc, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Couldn't create directory {}: {e}", parent.display()))?;
    }
    let format = ModelFormat::from_path(path).unwrap_or(ModelFormat::Glb);
    match format {
        ModelFormat::Glb => glb::save_glb(doc, path),
        ModelFormat::Gltf => glb::save_gltf(doc, path),
        ModelFormat::Obj => obj::save(doc, path),
        ModelFormat::Stl => stl::save(doc, path),
        ModelFormat::Ply => ply::save(doc, path),
    }
}

/// Loads an image file (PNG, JPEG, WebP, BMP, TGA) into an RGBA8 `TextureSlot`.
pub fn load_texture(path: &Path) -> Result<TextureSlot, String> {
    let img = image::open(path)
        .map_err(|e| format!("Couldn't open image '{}': {e}", path.display()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("texture")
        .to_string();
    Ok(TextureSlot::from_rgba(name, w, h, rgba.into_raw()))
}

/// Exports a `TextureSlot` to a PNG file on disk.
pub fn save_texture_png(tex: &TextureSlot, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Couldn't create directory {}: {e}", parent.display()))?;
    }
    let img = image::RgbaImage::from_raw(tex.width, tex.height, (*tex.pixels).clone())
        .ok_or_else(|| "Invalid texture dimensions".to_string())?;
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| format!("Couldn't write PNG '{}': {e}", path.display()))
}

/// Encodes a `TextureSlot` into in-memory PNG bytes (used for GLB embedding).
pub fn encode_texture_png(tex: &TextureSlot) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(tex.width, tex.height, (*tex.pixels).clone())
        .ok_or_else(|| "Invalid texture dimensions".to_string())?;
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(bytes)
}
