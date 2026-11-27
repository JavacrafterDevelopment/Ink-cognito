//! Editor state, modes, tool settings, undo/redo history (with lightweight
//! copy-on-write textures), modal dialogs, and the first-save overwrite
//! confirmation state machine.

use crate::{
    io::ModelFormat,
    mesh_ops::{MeshSelection, SculptTool, SubSelMode},
    model::{ModelDoc, PrimitiveKind},
    paint::{BrushSettings, ProceduralPattern},
};
use bevy::prelude::*;
use std::path::PathBuf;

/// Top-level editing workflow mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Object,
    Mesh,
    Paint,
    Material,
}

impl EditorMode {
    pub const ALL: [EditorMode; 4] = [
        EditorMode::Object,
        EditorMode::Mesh,
        EditorMode::Paint,
        EditorMode::Material,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EditorMode::Object => "Object",
            EditorMode::Mesh => "Mesh",
            EditorMode::Paint => "Paint",
            EditorMode::Material => "Material",
        }
    }
}

/// Transform gizmo handle mode in Object / Mesh modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoMode {
    Move,
    Rotate,
    Scale,
}

/// Active sub-tool inside Mesh Mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshTool {
    Select(SubSelMode),
    Sculpt(SculptTool),
}

/// Studio lighting presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightPreset {
    Studio,
    Daylight,
    Bright,
    Dark,
}

impl LightPreset {
    pub fn name(self) -> &'static str {
        match self {
            LightPreset::Studio => "Studio",
            LightPreset::Daylight => "Daylight",
            LightPreset::Bright => "Bright",
            LightPreset::Dark => "Dark",
        }
    }

    pub fn next(self) -> Self {
        match self {
            LightPreset::Studio => LightPreset::Daylight,
            LightPreset::Daylight => LightPreset::Bright,
            LightPreset::Bright => LightPreset::Dark,
            LightPreset::Dark => LightPreset::Studio,
        }
    }
}

/// Which material texture slot is targeted when loading an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexMapKind {
    BaseColor,
    Normal,
    MetallicRoughness,
    Emissive,
}

impl TexMapKind {
    pub fn name(self) -> &'static str {
        match self {
            TexMapKind::BaseColor => "Base Color",
            TexMapKind::Normal => "Normal Map",
            TexMapKind::MetallicRoughness => "Metal/Rough",
            TexMapKind::Emissive => "Emissive",
        }
    }
}

/// Editable text boxes in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusField {
    PartName,
    MaterialName,
    BrowserFileName,
    HexColor,
}

/// What the user wants to do after confirming discarding unsaved changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAfterDiscard {
    NewModel,
    OpenNativeDialog,
    OpenBrowser,
    OpenPath(PathBuf),
}

/// Purpose of the built-in in-app file browser modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserPurpose {
    OpenModel,
    ImportPart,
    SaveAs,
    LoadTexture(usize, TexMapKind),
}

#[derive(Clone, Debug)]
pub struct BrowserEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct FileBrowserState {
    pub purpose: BrowserPurpose,
    pub dir: PathBuf,
    pub entries: Vec<BrowserEntry>,
    pub file_name: String,
    pub save_format: ModelFormat,
}

/// Active modal dialog blocking 3D viewport input.
#[derive(Clone, Debug)]
pub enum Modal {
    /// First-time overwrite warning when pressing Save on an opened existing
    /// file (or when choosing an existing file in the in-app Save As browser).
    /// Clicking "No" closes this modal without deleting or reverting ANY edits,
    /// leaving `overwrite_confirmed = false` so clicking Save again shows this
    /// prompt again.
    ConfirmOverwrite { target: PathBuf },
    /// Warns before discarding unsaved edits on New / Open.
    ConfirmDiscard(PendingAfterDiscard),
    /// Built-in cross-platform file browser (works alongside native OS dialogs).
    FileBrowser(FileBrowserState),
}

/// Native OS dialog requests processed on the main thread (macOS-safe).
#[derive(Clone, Debug)]
pub enum NativeDialogRequest {
    OpenModel,
    ImportPart,
    SaveModelAs,
    LoadTexture { mat_idx: usize, slot: TexMapKind },
    ExportTexture { tex_idx: usize },
}

/// Camera view requests consumed by `view.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraRequest {
    FrameAll,
    FrameSelected,
    Front,
    Side,
    Top,
    ResetIso,
}

/// Curated vibrant + neutral color swatches for fast painting & materials.
pub const SWATCHES: [[f32; 4]; 16] = [
    [0.95, 0.96, 0.98, 1.0], // White
    [0.72, 0.75, 0.80, 1.0], // Light Gray
    [0.36, 0.39, 0.44, 1.0], // Slate
    [0.10, 0.11, 0.14, 1.0], // Charcoal
    [0.95, 0.22, 0.22, 1.0], // Red
    [1.00, 0.52, 0.12, 1.0], // Ink Orange
    [1.00, 0.84, 0.18, 1.0], // Gold Yellow
    [0.32, 0.84, 0.30, 1.0], // Lime Green
    [0.14, 0.52, 0.26, 1.0], // Forest Green
    [0.14, 0.80, 0.85, 1.0], // Cyan
    [0.16, 0.54, 1.00, 1.0], // Ink Blue
    [0.24, 0.26, 0.78, 1.0], // Indigo
    [0.64, 0.26, 0.92, 1.0], // Purple
    [0.95, 0.30, 0.68, 1.0], // Hot Pink
    [0.58, 0.36, 0.20, 1.0], // Wood Brown
    [0.86, 0.72, 0.52, 1.0], // Sand Tan
];

/// Complete editor resource.
#[derive(Resource)]
pub struct Editor {
    pub doc: ModelDoc,
    /// Currently open / saved file on disk (`None` for an unsaved new model).
    pub source_path: Option<PathBuf>,
    /// Display name for title bar & suggested filenames.
    pub display_name: String,
    /// `true` once the user has confirmed overwriting `source_path` at least
    /// once during this open session (or saved a new copy via Save As).
    /// Always starts `false` whenever an existing file is opened!
    pub overwrite_confirmed: bool,
    /// Unsaved edits flag.
    pub dirty: bool,

    pub mode: EditorMode,
    pub gizmo: GizmoMode,
    pub mesh_tool: MeshTool,
    pub selected_part: Option<usize>,
    pub selected_mat: usize,
    pub mesh_sel: MeshSelection,

    pub brush: BrushSettings,
    pub sculpt_radius: f32,
    pub sculpt_strength: f32,
    pub sculpt_mirror_x: bool,

    pub show_wireframe: bool,
    pub show_grid: bool,
    pub light_preset: LightPreset,

    pub focus: Option<FocusField>,
    pub hex_input: String,
    pub modal: Option<Modal>,
    pub pending_dialog: Option<NativeDialogRequest>,
    pub camera_request: Option<CameraRequest>,
    pub last_dir: PathBuf,
    pub status: String,

    /// Bumped when parts/materials are added, removed, or replaced so the 3D
    /// scene syncs immediately.
    pub scene_version: u64,
    /// Bumped for UI signature changes (lightweight counter).
    pub ui_version: u64,

    undo: Vec<ModelDoc>,
    redo: Vec<ModelDoc>,
}

impl Default for Editor {
    fn default() -> Self {
        let doc = ModelDoc::starter_cube();
        let brush = BrushSettings::default();
        let hex_input = rgba_to_hex(brush.color);
        Self {
            doc,
            source_path: None,
            display_name: "untitled.glb".into(),
            overwrite_confirmed: false,
            dirty: false,
            mode: EditorMode::Object,
            gizmo: GizmoMode::Move,
            mesh_tool: MeshTool::Select(SubSelMode::Face),
            selected_part: Some(0),
            selected_mat: 0,
            mesh_sel: MeshSelection::default(),
            brush,
            sculpt_radius: 0.25,
            sculpt_strength: 0.55,
            sculpt_mirror_x: false,
            show_wireframe: false,
            show_grid: true,
            light_preset: LightPreset::Studio,
            focus: None,
            hex_input,
            modal: None,
            pending_dialog: None,
            camera_request: Some(CameraRequest::FrameAll),
            last_dir: crate::files::default_models_dir(),
            status: "Ready. Open a .glb/.obj/.stl/.ply model, or switch to Mesh / Paint / Material to edit.".into(),
            scene_version: 1,
            ui_version: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl Editor {
    pub fn bump_scene(&mut self) {
        self.scene_version = self.scene_version.wrapping_add(1);
        self.ui_version = self.ui_version.wrapping_add(1);
    }

    pub fn bump_ui(&mut self) {
        self.ui_version = self.ui_version.wrapping_add(1);
    }

    /// Saves a snapshot before a mutation so Undo restores it.
    pub fn snapshot(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        self.bump_ui();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let cur = std::mem::replace(&mut self.doc, prev);
            self.redo.push(cur);
            self.after_history("Undid last change.");
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let cur = std::mem::replace(&mut self.doc, next);
            self.undo.push(cur);
            self.after_history("Redid change.");
        }
    }

    fn after_history(&mut self, msg: &str) {
        if let Some(i) = self.selected_part
            && i >= self.doc.parts.len()
        {
            self.selected_part = (!self.doc.parts.is_empty()).then_some(0);
        }
        if self.selected_mat >= self.doc.materials.len() {
            self.selected_mat = 0;
        }
        self.mesh_sel.clear();
        self.dirty = true;
        self.status = msg.into();
        self.bump_scene();
    }

    /// Replaces the open document (on New or Open). Resets undo history and
    /// sets `overwrite_confirmed = false` whenever an existing file is opened!
    pub fn replace_document(&mut self, doc: ModelDoc, source_path: Option<PathBuf>, status: String) {
        let display_name = source_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "untitled.glb".into());
        if let Some(parent) = source_path.as_ref().and_then(|p| p.parent())
            && parent.is_dir()
        {
            self.last_dir = parent.to_path_buf();
        }
        self.doc = doc;
        self.source_path = source_path;
        self.display_name = display_name;
        // CRITICAL: Opening an existing file ALWAYS requires a first-time
        // overwrite confirmation before Save overwrites it!
        self.overwrite_confirmed = false;
        self.dirty = false;
        self.selected_part = (!self.doc.parts.is_empty()).then_some(0);
        self.selected_mat = self
            .selected_part
            .and_then(|i| self.doc.parts.get(i))
            .map(|p| p.material)
            .unwrap_or(0);
        self.mesh_sel.clear();
        self.focus = None;
        self.modal = None;
        self.undo.clear();
        self.redo.clear();
        self.status = status;
        self.camera_request = Some(CameraRequest::FrameAll);
        self.bump_scene();
    }

    /// Selects a part and syncs `selected_mat` to that part's material.
    pub fn select_part(&mut self, idx: Option<usize>) {
        self.selected_part = idx.filter(|&i| i < self.doc.parts.len());
        self.mesh_sel.clear();
        if let Some(i) = self.selected_part {
            self.selected_mat = self.doc.parts[i].material;
        }
        self.bump_ui();
    }

    pub fn add_primitive(&mut self, kind: PrimitiveKind) {
        self.snapshot();
        let idx = self.doc.add_primitive(kind);
        self.select_part(Some(idx));
        self.status = format!("Added {}.", kind.name());
        self.bump_scene();
    }

    pub fn duplicate_selected(&mut self) {
        let Some(i) = self.selected_part else { return };
        self.snapshot();
        if let Some(new_i) = self.doc.duplicate_part(i) {
            self.select_part(Some(new_i));
            self.status = format!("Duplicated '{}'.", self.doc.parts[new_i].name);
            self.bump_scene();
        }
    }

    pub fn mirror_selected(&mut self, axis: usize) {
        let Some(i) = self.selected_part else { return };
        self.snapshot();
        if let Some(new_i) = self.doc.mirror_part(i, axis) {
            self.select_part(Some(new_i));
            self.status = format!("Mirrored '{}'.", self.doc.parts[new_i].name);
            self.bump_scene();
        }
    }

    pub fn delete_selected_part(&mut self) {
        let Some(i) = self.selected_part else { return };
        if i >= self.doc.parts.len() {
            return;
        }
        self.snapshot();
        let name = self.doc.parts[i].name.clone();
        self.doc.parts.remove(i);
        self.selected_part = if self.doc.parts.is_empty() {
            None
        } else {
            Some(i.min(self.doc.parts.len() - 1))
        };
        self.mesh_sel.clear();
        self.status = format!("Deleted '{name}'.");
        self.bump_scene();
    }

    pub fn set_brush_color(&mut self, rgba: [f32; 4]) {
        self.brush.color = rgba;
        self.hex_input = rgba_to_hex(rgba);
        self.bump_ui();
    }

    pub fn apply_hex_color(&mut self) {
        if let Some(rgba) = parse_hex_rgba(&self.hex_input) {
            match self.mode {
                EditorMode::Material => {
                    if self.selected_mat < self.doc.materials.len() {
                        self.snapshot();
                        let m = &mut self.doc.materials[self.selected_mat];
                        m.set_base_color_srgb(rgba);
                        self.bump_scene();
                    }
                }
                _ => {
                    self.brush.color = rgba;
                }
            }
            self.hex_input = rgba_to_hex(rgba);
            self.bump_ui();
        }
    }

    pub fn apply_procedural_to_selected(&mut self, pattern: ProceduralPattern) {
        let Some(pi) = self.selected_part else {
            self.status = "Select a part first to generate a texture.".into();
            return;
        };
        self.snapshot();
        let size = self.brush.new_tex_size;
        let Some((ti, _, made_uvs)) = self.doc.ensure_paintable_texture(pi, size) else {
            return;
        };
        let c1 = crate::model::srgb_f32_to_u8(self.brush.color);
        let c2 = crate::model::srgb_f32_to_u8(self.brush.erase_color);
        crate::paint::generate_procedural(&mut self.doc.textures[ti], pattern, c1, c2);
        self.status = if made_uvs {
            format!("Generated {} texture ({size}×{size}) with auto-UVs.", pattern.name())
        } else {
            format!("Generated {} texture.", pattern.name())
        };
        self.bump_scene();
    }

    /// Returns the active material's base color texture index, if any.
    pub fn active_paint_texture(&self) -> Option<usize> {
        let mat_idx = self
            .selected_part
            .and_then(|pi| self.doc.parts.get(pi))
            .map(|p| p.material)
            .unwrap_or(self.selected_mat);
        self.doc
            .materials
            .get(mat_idx)
            .and_then(|m| m.base_color_texture)
            .filter(|&ti| ti < self.doc.textures.len())
    }

    /// Mutable text buffer for whichever `FocusField` is active.
    pub fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.focus? {
            FocusField::PartName => self
                .selected_part
                .and_then(|i| self.doc.parts.get_mut(i))
                .map(|p| &mut p.name),
            FocusField::MaterialName => self
                .doc
                .materials
                .get_mut(self.selected_mat)
                .map(|m| &mut m.name),
            FocusField::BrowserFileName => match &mut self.modal {
                Some(Modal::FileBrowser(fb)) => Some(&mut fb.file_name),
                _ => None,
            },
            FocusField::HexColor => Some(&mut self.hex_input),
        }
    }
}

pub fn rgba_to_hex(c: [f32; 4]) -> String {
    let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{r:02X}{g:02X}{b:02X}")
}

pub fn parse_hex_rgba(s: &str) -> Option<[f32; 4]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
}
