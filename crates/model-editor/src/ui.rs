//! Bevy UI panels for the 3D Model Editor: top file bar, left mode/tools and
//! scrollable parts outliner, right context inspector (with color swatches &
//! live interactive 2D texture paint canvas), bottom help bar, and modal
//! dialogs (including the First-Save Overwrite Warning and In-App File Browser).

use crate::{
    files,
    io::ModelFormat,
    mesh_ops::{self, SculptTool, SubSelMode},
    model::{PrimitiveKind, srgb_f32_to_u8},
    paint::{self, BrushTip, PaintTool, ProceduralPattern},
    scene::RenderCache,
    state::{
        BrowserPurpose, CameraRequest, Editor, EditorMode, FocusField, GizmoMode, MeshTool, Modal,
        NativeDialogRequest, SWATCHES, TexMapKind, rgba_to_hex,
    },
};
use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    picking::prelude::*,
    prelude::*,
    window::PrimaryWindow,
};

pub const TOP_H: f32 = 46.0;
pub const LEFT_W: f32 = 248.0;
pub const RIGHT_W: f32 = 296.0;
pub const BOTTOM_H: f32 = 26.0;

const PANEL: Color = Color::srgba(0.07, 0.10, 0.15, 0.95);
const CARD: Color = Color::srgba(0.09, 0.13, 0.20, 0.98);
const BTN: Color = Color::srgb(0.17, 0.22, 0.30);
const BTN_HOVER: Color = Color::srgb(0.25, 0.32, 0.43);
const BTN_ON: Color = Color::srgb(0.18, 0.52, 0.90);
const BTN_ON_HOVER: Color = Color::srgb(0.24, 0.60, 0.98);
const BTN_WARN: Color = Color::srgb(0.76, 0.24, 0.20);
const BTN_WARN_HOVER: Color = Color::srgb(0.88, 0.30, 0.24);
const BTN_OFF: Color = Color::srgb(0.12, 0.15, 0.20);
const TEXT: Color = Color::srgb(0.93, 0.95, 0.98);
const MUTED: Color = Color::srgb(0.62, 0.68, 0.76);
const ACCENT: Color = Color::srgb(1.0, 0.84, 0.20);
const FIELD_BG: Color = Color::srgb(0.04, 0.06, 0.09);
const FIELD_FOCUS: Color = Color::srgb(0.10, 0.16, 0.26);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScrollState>()
            .init_resource::<Canvas2dStroke>()
            .add_systems(Update, (rebuild_ui, scroll_lists, paint_on_2d_canvas))
            .add_observer(on_click)
            .add_observer(on_over)
            .add_observer(on_out);
    }
}

#[derive(Component)]
struct UiRoot;

#[derive(Component)]
struct Btn {
    normal: Color,
    hover: Color,
}

#[derive(Component)]
struct PartsList;

#[derive(Component)]
struct BrowserList;

#[derive(Component)]
struct UvCanvasNode;

#[derive(Resource, Default)]
struct ScrollState {
    parts_y: f32,
    browser_y: f32,
}

#[derive(Resource, Default)]
struct Canvas2dStroke {
    active: bool,
}

#[derive(Component, Clone, Copy, Debug)]
enum Action {
    // Top bar
    New,
    OpenNative,
    OpenBrowser,
    ImportNative,
    Save,
    SaveAsNative,
    Undo,
    Redo,
    ToggleWireframe,
    ToggleGrid,
    CycleLight,
    Camera(CameraRequest),
    // Modes
    Mode(EditorMode),
    Gizmo(GizmoMode),
    // Object mode
    AddPrimitive(PrimitiveKind),
    SelectPart(usize),
    TogglePartVis(usize),
    DuplicatePart,
    MirrorPart(usize),
    DeletePart,
    MergeNext,
    CenterAndGround,
    NudgePart(usize, f32),
    RotatePart(usize, f32),
    ScalePart(f32),
    ResetPartPos,
    ResetPartRot,
    ResetPartScale,
    AutoUvPart,
    SmoothNormals,
    FlatNormals,
    FlipNormals,
    BakeTransform,
    CyclePartMaterial(i32),
    // Mesh mode
    MeshTool(MeshTool),
    MeshSelectAll,
    MeshClearSel,
    MeshExtrude(f32),
    MeshInset,
    MeshSubdivide,
    MeshSmooth,
    MeshFlatten,
    MeshScale(f32),
    MeshDeleteFaces,
    MeshSplitMaterial,
    SculptRadius(f32),
    SculptStrength(f32),
    ToggleSculptMirror,
    // Paint mode
    PaintTool(PaintTool),
    BrushTip(BrushTip),
    BrushRadius(f32),
    BrushOpacity(f32),
    BrushHardness(f32),
    TogglePaintMirror,
    SetTexRes(u32),
    NewPaintCanvas,
    ExportPaintPng,
    LoadTexNative(usize, TexMapKind),
    LoadTexBrowser(usize, TexMapKind),
    ClearTex(usize, TexMapKind),
    Procedural(ProceduralPattern),
    PickSwatch(usize),
    SwapBrushColors,
    AdjustBrushRgb(usize, f32),
    // Material mode
    SelectMat(usize),
    NewMat,
    DuplicateMat,
    AssignMatToSelected,
    AdjustMatRgb(usize, f32),
    AdjustMatAlpha(f32),
    AdjustMatMetallic(f32),
    AdjustMatRoughness(f32),
    AdjustMatEmissive(f32),
    CycleMatAlpha,
    ToggleMatDoubleSided,
    // Text focus
    Focus(FocusField),
    // Modals
    ConfirmOverwrite,
    OverwriteToSaveAs,
    CancelOverwrite,
    ConfirmDiscard,
    CloseModal,
    // File browser
    BrowserUp,
    BrowserGoModels,
    BrowserGoHome,
    BrowserEntry(usize),
    BrowserFormat(ModelFormat),
    BrowserMode(BrowserPurpose),
    BrowserSubmitSave,
    BrowserSwitchNative,
}

fn on_over(over: On<PointerOver>, mut q: Query<(&Btn, &mut BackgroundColor)>) {
    if let Ok((b, mut bg)) = q.get_mut(over.entity) {
        bg.0 = b.hover;
    }
}

fn on_out(out: On<PointerOut>, mut q: Query<(&Btn, &mut BackgroundColor)>) {
    if let Ok((b, mut bg)) = q.get_mut(out.entity) {
        bg.0 = b.normal;
    }
}

fn on_click(click: On<PointerClick>, actions: Query<&Action>, mut editor: ResMut<Editor>) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(&action) = actions.get(click.entity) else {
        return;
    };
    let ed = &mut *editor;
    if !matches!(action, Action::Focus(_)) {
        ed.focus = None;
    }

    match action {
        Action::New => files::request_new(ed),
        Action::OpenNative => files::request_open_native(ed),
        Action::OpenBrowser => files::request_open_browser(ed),
        Action::ImportNative => ed.pending_dialog = Some(NativeDialogRequest::ImportPart),
        Action::Save => files::request_save(ed),
        Action::SaveAsNative => files::request_save_as_native(ed),
        Action::Undo => ed.undo(),
        Action::Redo => ed.redo(),
        Action::ToggleWireframe => {
            ed.show_wireframe = !ed.show_wireframe;
            ed.bump_ui();
        }
        Action::ToggleGrid => {
            ed.show_grid = !ed.show_grid;
            ed.bump_ui();
        }
        Action::CycleLight => {
            ed.light_preset = ed.light_preset.next();
            ed.bump_ui();
        }
        Action::Camera(req) => ed.camera_request = Some(req),

        Action::Mode(m) => {
            ed.mode = m;
            if m == EditorMode::Material
                && let Some(mat) = ed.doc.materials.get(ed.selected_mat)
            {
                ed.hex_input = rgba_to_hex(mat.base_color_srgb());
            } else if m == EditorMode::Paint {
                ed.hex_input = rgba_to_hex(ed.brush.color);
            }
            ed.bump_ui();
        }
        Action::Gizmo(g) => {
            ed.gizmo = g;
            ed.bump_ui();
        }

        // --- Object mode ---
        Action::AddPrimitive(k) => ed.add_primitive(k),
        Action::SelectPart(i) => ed.select_part(Some(i)),
        Action::TogglePartVis(i) => {
            if let Some(p) = ed.doc.parts.get_mut(i) {
                p.visible = !p.visible;
                ed.bump_scene();
            }
        }
        Action::DuplicatePart => ed.duplicate_selected(),
        Action::MirrorPart(axis) => ed.mirror_selected(axis),
        Action::DeletePart => ed.delete_selected_part(),
        Action::MergeNext => {
            if let Some(pi) = ed.selected_part
                && ed.doc.parts.len() > 1
            {
                let other = if pi + 1 < ed.doc.parts.len() { pi + 1 } else { pi - 1 };
                ed.snapshot();
                let dst = pi.min(other);
                let src = pi.max(other);
                if ed.doc.merge_into(dst, src) {
                    ed.select_part(Some(dst));
                    ed.status = "Merged two parts together.".into();
                    ed.bump_scene();
                }
            }
        }
        Action::CenterAndGround => {
            ed.snapshot();
            if ed.doc.center_and_ground() {
                ed.camera_request = Some(CameraRequest::FrameAll);
                ed.status = "Centered model at X=0, Z=0 and dropped bottom onto Y=0.".into();
                ed.bump_scene();
            }
        }
        Action::NudgePart(axis, d) => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].translation[axis] =
                    ((ed.doc.parts[pi].translation[axis] + d) * 1000.0).round() / 1000.0;
                ed.bump_ui();
            }
        }
        Action::RotatePart(axis, deg) => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                let q = match axis {
                    0 => Quat::from_rotation_x(deg.to_radians()),
                    1 => Quat::from_rotation_y(deg.to_radians()),
                    _ => Quat::from_rotation_z(deg.to_radians()),
                };
                let next = q * ed.doc.parts[pi].quat();
                ed.doc.parts[pi].set_quat(next);
                ed.bump_ui();
            }
        }
        Action::ScalePart(f) => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                for v in &mut ed.doc.parts[pi].scale {
                    *v = ((*v * f).clamp(0.02, 50.0) * 1000.0).round() / 1000.0;
                }
                ed.bump_ui();
            }
        }
        Action::ResetPartPos => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].translation = [0.0, 0.0, 0.0];
                ed.bump_ui();
            }
        }
        Action::ResetPartRot => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].rotation = [0.0, 0.0, 0.0, 1.0];
                ed.bump_ui();
            }
        }
        Action::ResetPartScale => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].scale = [1.0, 1.0, 1.0];
                ed.bump_ui();
            }
        }
        Action::AutoUvPart => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].generate_box_uvs();
                ed.status = "Generated non-overlapping 6-side Box-Atlas UVs.".into();
                ed.bump_scene();
            }
        }
        Action::SmoothNormals => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].recompute_normals_smooth();
                ed.status = "Recomputed smooth vertex normals.".into();
                ed.bump_scene();
            }
        }
        Action::FlatNormals => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].recompute_normals_flat();
                ed.status = "Recomputed crisp flat face normals.".into();
                ed.bump_scene();
            }
        }
        Action::FlipNormals => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].flip_normals();
                ed.status = "Flipped triangle winding and normals.".into();
                ed.bump_scene();
            }
        }
        Action::BakeTransform => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                ed.doc.parts[pi].bake_transform();
                ed.status = "Baked transform into vertex positions.".into();
                ed.bump_scene();
            }
        }
        Action::CyclePartMaterial(dir) => {
            if let Some(pi) = ed.selected_part
                && !ed.doc.materials.is_empty()
            {
                ed.snapshot();
                let n = ed.doc.materials.len() as i32;
                let cur = ed.doc.parts[pi].material as i32;
                let next = (cur + dir).rem_euclid(n) as usize;
                ed.doc.parts[pi].material = next;
                ed.selected_mat = next;
                ed.bump_scene();
            }
        }

        // --- Mesh mode ---
        Action::MeshTool(t) => {
            ed.mesh_tool = t;
            ed.bump_ui();
        }
        Action::MeshSelectAll => {
            if let Some(pi) = ed.selected_part
                && let Some(p) = ed.doc.parts.get(pi)
            {
                ed.mesh_sel.faces = (0..p.tri_count()).collect();
                ed.mesh_sel.verts = (0..p.positions.len()).collect();
                ed.bump_ui();
            }
        }
        Action::MeshClearSel => {
            ed.mesh_sel.clear();
            ed.bump_ui();
        }
        Action::MeshExtrude(dist) => {
            if let Some(pi) = ed.selected_part
                && !ed.mesh_sel.faces.is_empty()
            {
                ed.snapshot();
                let faces = ed.mesh_sel.faces.clone();
                let cap = mesh_ops::extrude_faces(&mut ed.doc.parts[pi], &faces, dist);
                ed.mesh_sel.faces = cap;
                ed.mesh_sel.verts.clear();
                ed.status = format!("Extruded faces by {dist:+.2}.");
                ed.bump_scene();
            } else {
                ed.status = "Select one or more faces first (click a face in Mesh mode).".into();
                ed.bump_ui();
            }
        }
        Action::MeshInset => {
            if let Some(pi) = ed.selected_part
                && !ed.mesh_sel.faces.is_empty()
            {
                ed.snapshot();
                let faces = ed.mesh_sel.faces.clone();
                let cap = mesh_ops::inset_faces(&mut ed.doc.parts[pi], &faces, 0.28);
                ed.mesh_sel.faces = cap;
                ed.mesh_sel.verts.clear();
                ed.status = "Inset selected faces.".into();
                ed.bump_scene();
            } else {
                ed.status = "Select faces first to inset.".into();
                ed.bump_ui();
            }
        }
        Action::MeshSubdivide => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                let faces = ed.mesh_sel.faces.clone();
                mesh_ops::subdivide_faces(&mut ed.doc.parts[pi], &faces);
                ed.mesh_sel.clear();
                ed.status = format!("Subdivided ({} tris now).", ed.doc.parts[pi].tri_count());
                ed.bump_scene();
            }
        }
        Action::MeshSmooth => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                let verts = ed.mesh_sel.effective_verts(&ed.doc.parts[pi]);
                mesh_ops::smooth_vertices(&mut ed.doc.parts[pi], &verts, 0.45);
                ed.status = "Smoothed mesh vertices.".into();
                ed.bump_scene();
            }
        }
        Action::MeshFlatten => {
            if let Some(pi) = ed.selected_part {
                let verts = ed.mesh_sel.effective_verts(&ed.doc.parts[pi]);
                if verts.len() >= 3 {
                    ed.snapshot();
                    mesh_ops::flatten_vertices(&mut ed.doc.parts[pi], &verts);
                    ed.status = "Flattened selection onto its average plane.".into();
                    ed.bump_scene();
                } else {
                    ed.status = "Select at least 3 vertices or a face to flatten.".into();
                    ed.bump_ui();
                }
            }
        }
        Action::MeshScale(factor) => {
            if let Some(pi) = ed.selected_part {
                let verts = ed.mesh_sel.effective_verts(&ed.doc.parts[pi]);
                if !verts.is_empty() {
                    ed.snapshot();
                    mesh_ops::scale_vertices(&mut ed.doc.parts[pi], &verts, factor);
                    ed.bump_scene();
                }
            }
        }
        Action::MeshDeleteFaces => {
            if let Some(pi) = ed.selected_part
                && !ed.mesh_sel.faces.is_empty()
            {
                ed.snapshot();
                let faces = ed.mesh_sel.faces.clone();
                mesh_ops::delete_faces(&mut ed.doc.parts[pi], &faces);
                ed.mesh_sel.clear();
                ed.status = "Deleted selected faces.".into();
                ed.bump_scene();
            }
        }
        Action::MeshSplitMaterial => {
            if let Some(pi) = ed.selected_part
                && !ed.mesh_sel.faces.is_empty()
            {
                ed.snapshot();
                let n = ed.doc.materials.len() + 1;
                let new_mat = ed.doc.add_material(format!("Material {n}"));
                let faces = ed.mesh_sel.faces.clone();
                if let Some(new_pi) = mesh_ops::split_faces_to_new_part(&mut ed.doc, pi, &faces, new_mat) {
                    ed.select_part(Some(new_pi));
                    ed.status = "Split selected faces to a new part with its own material.".into();
                    ed.bump_scene();
                }
            } else {
                ed.status = "Select faces first to split off a separate material.".into();
                ed.bump_ui();
            }
        }
        Action::SculptRadius(mult) => {
            ed.sculpt_radius = (ed.sculpt_radius * mult).clamp(0.02, 3.0);
            ed.bump_ui();
        }
        Action::SculptStrength(d) => {
            ed.sculpt_strength = (ed.sculpt_strength + d).clamp(0.05, 1.5);
            ed.bump_ui();
        }
        Action::ToggleSculptMirror => {
            ed.sculpt_mirror_x = !ed.sculpt_mirror_x;
            ed.bump_ui();
        }

        // --- Paint mode ---
        Action::PaintTool(t) => {
            ed.brush.tool = t;
            ed.bump_ui();
        }
        Action::BrushTip(tip) => {
            ed.brush.tip = tip;
            ed.bump_ui();
        }
        Action::BrushRadius(mult) => {
            ed.brush.radius = (ed.brush.radius * mult).clamp(0.01, 3.0);
            ed.bump_ui();
        }
        Action::BrushOpacity(d) => {
            ed.brush.opacity = (ed.brush.opacity + d).clamp(0.05, 1.0);
            ed.bump_ui();
        }
        Action::BrushHardness(d) => {
            ed.brush.hardness = (ed.brush.hardness + d).clamp(0.0, 0.99);
            ed.bump_ui();
        }
        Action::TogglePaintMirror => {
            ed.brush.mirror_x = !ed.brush.mirror_x;
            ed.bump_ui();
        }
        Action::SetTexRes(res) => {
            ed.brush.new_tex_size = res;
            ed.bump_ui();
        }
        Action::NewPaintCanvas => {
            if let Some(pi) = ed.selected_part {
                ed.snapshot();
                let res = ed.brush.new_tex_size;
                let mat_idx = ed.doc.parts[pi].material;
                let fill = srgb_f32_to_u8(ed.brush.erase_color);
                let name = format!("{}_tex", ed.doc.parts[pi].name);
                let ti = ed.doc.textures.len();
                ed.doc.textures.push(crate::model::TextureSlot::new_solid(name, res, res, fill));
                let made_uvs = ed.doc.parts[pi].ensure_uvs();
                if let Some(m) = ed.doc.materials.get_mut(mat_idx) {
                    m.base_color_texture = Some(ti);
                    m.base_color = [1.0, 1.0, 1.0, m.base_color[3]];
                }
                ed.status = if made_uvs {
                    format!("Created fresh {res}×{res} canvas & auto-UVs.")
                } else {
                    format!("Created fresh {res}×{res} paint canvas.")
                };
                ed.bump_scene();
            }
        }
        Action::ExportPaintPng => {
            if let Some(ti) = ed.active_paint_texture() {
                ed.pending_dialog = Some(NativeDialogRequest::ExportTexture { tex_idx: ti });
            } else {
                ed.status = "No texture on the selected part yet to export.".into();
                ed.bump_ui();
            }
        }
        Action::LoadTexNative(mat_idx, slot) => {
            ed.pending_dialog = Some(NativeDialogRequest::LoadTexture { mat_idx, slot });
        }
        Action::LoadTexBrowser(mat_idx, slot) => {
            files::open_file_browser(ed, BrowserPurpose::LoadTexture(mat_idx, slot));
        }
        Action::ClearTex(mat_idx, slot) => {
            if let Some(m) = ed.doc.materials.get(mat_idx) {
                let had = match slot {
                    TexMapKind::BaseColor => m.base_color_texture.is_some(),
                    TexMapKind::Normal => m.normal_texture.is_some(),
                    TexMapKind::MetallicRoughness => m.metallic_roughness_texture.is_some(),
                    TexMapKind::Emissive => m.emissive_texture.is_some(),
                };
                if had {
                    ed.snapshot();
                    let m = &mut ed.doc.materials[mat_idx];
                    match slot {
                        TexMapKind::BaseColor => m.base_color_texture = None,
                        TexMapKind::Normal => m.normal_texture = None,
                        TexMapKind::MetallicRoughness => m.metallic_roughness_texture = None,
                        TexMapKind::Emissive => m.emissive_texture = None,
                    }
                    ed.status = format!("Cleared {} texture.", slot.name());
                    ed.bump_scene();
                }
            }
        }
        Action::Procedural(pat) => ed.apply_procedural_to_selected(pat),
        Action::PickSwatch(idx) => {
            if let Some(&c) = SWATCHES.get(idx) {
                match ed.mode {
                    EditorMode::Material => {
                        if ed.selected_mat < ed.doc.materials.len() {
                            ed.snapshot();
                            let a = ed.doc.materials[ed.selected_mat].base_color[3];
                            ed.doc.materials[ed.selected_mat].set_base_color_srgb([c[0], c[1], c[2], a]);
                            ed.hex_input = rgba_to_hex(c);
                            ed.bump_scene();
                        }
                    }
                    _ => ed.set_brush_color(c),
                }
            }
        }
        Action::SwapBrushColors => {
            std::mem::swap(&mut ed.brush.color, &mut ed.brush.erase_color);
            ed.hex_input = rgba_to_hex(ed.brush.color);
            ed.bump_ui();
        }
        Action::AdjustBrushRgb(ch, d) => {
            ed.brush.color[ch] = (ed.brush.color[ch] + d).clamp(0.0, 1.0);
            ed.hex_input = rgba_to_hex(ed.brush.color);
            ed.bump_ui();
        }

        // --- Material mode ---
        Action::SelectMat(i) => {
            if i < ed.doc.materials.len() {
                ed.selected_mat = i;
                ed.hex_input = rgba_to_hex(ed.doc.materials[i].base_color_srgb());
                ed.bump_ui();
            }
        }
        Action::NewMat => {
            ed.snapshot();
            let n = ed.doc.materials.len() + 1;
            let i = ed.doc.add_material(format!("Material {n}"));
            ed.selected_mat = i;
            ed.bump_scene();
        }
        Action::DuplicateMat => {
            if let Some(src) = ed.doc.materials.get(ed.selected_mat).cloned() {
                ed.snapshot();
                let mut copy = src;
                copy.name = format!("{} Copy", copy.name);
                ed.doc.materials.push(copy);
                ed.selected_mat = ed.doc.materials.len() - 1;
                ed.bump_scene();
            }
        }
        Action::AssignMatToSelected => {
            if let Some(pi) = ed.selected_part
                && ed.selected_mat < ed.doc.materials.len()
            {
                ed.snapshot();
                ed.doc.parts[pi].material = ed.selected_mat;
                ed.status = format!(
                    "Assigned '{}' to '{}'.",
                    ed.doc.materials[ed.selected_mat].name, ed.doc.parts[pi].name
                );
                ed.bump_scene();
            }
        }
        Action::AdjustMatRgb(ch, d) => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                let mut srgb = ed.doc.materials[ed.selected_mat].base_color_srgb();
                srgb[ch] = (srgb[ch] + d).clamp(0.0, 1.0);
                ed.doc.materials[ed.selected_mat].set_base_color_srgb(srgb);
                ed.hex_input = rgba_to_hex(srgb);
                ed.bump_scene();
            }
        }
        Action::AdjustMatAlpha(d) => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                let m = &mut ed.doc.materials[ed.selected_mat];
                m.base_color[3] = (m.base_color[3] + d).clamp(0.0, 1.0);
                ed.bump_scene();
            }
        }
        Action::AdjustMatMetallic(d) => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                let m = &mut ed.doc.materials[ed.selected_mat];
                m.metallic = ((m.metallic + d) * 20.0).round() / 20.0;
                m.metallic = m.metallic.clamp(0.0, 1.0);
                ed.bump_scene();
            }
        }
        Action::AdjustMatRoughness(d) => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                let m = &mut ed.doc.materials[ed.selected_mat];
                m.roughness = ((m.roughness + d) * 20.0).round() / 20.0;
                m.roughness = m.roughness.clamp(0.05, 1.0);
                ed.bump_scene();
            }
        }
        Action::AdjustMatEmissive(d) => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                let m = &mut ed.doc.materials[ed.selected_mat];
                let base = m.base_color_srgb();
                let cur = m.emissive[0].max(m.emissive[1]).max(m.emissive[2]);
                let next = (cur + d).clamp(0.0, 2.0);
                m.emissive = [base[0] * next, base[1] * next, base[2] * next];
                ed.bump_scene();
            }
        }
        Action::CycleMatAlpha => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                ed.doc.materials[ed.selected_mat].alpha_mode =
                    ed.doc.materials[ed.selected_mat].alpha_mode.next();
                ed.bump_scene();
            }
        }
        Action::ToggleMatDoubleSided => {
            if ed.selected_mat < ed.doc.materials.len() {
                ed.snapshot();
                ed.doc.materials[ed.selected_mat].double_sided =
                    !ed.doc.materials[ed.selected_mat].double_sided;
                ed.bump_scene();
            }
        }

        // --- Focus ---
        Action::Focus(f) => {
            ed.focus = Some(f);
            ed.bump_ui();
        }

        // --- Modals ---
        Action::ConfirmOverwrite => files::confirm_overwrite(ed),
        Action::OverwriteToSaveAs => files::request_save_as_native(ed),
        Action::CancelOverwrite => files::cancel_overwrite(ed),
        Action::ConfirmDiscard => files::confirm_discard(ed),
        Action::CloseModal => {
            ed.modal = None;
            ed.bump_ui();
        }

        // --- In-app File Browser ---
        Action::BrowserUp => {
            if let Some(Modal::FileBrowser(fb)) = &ed.modal
                && let Some(parent) = fb.dir.parent()
            {
                files::browser_navigate(ed, parent.to_path_buf());
            }
        }
        Action::BrowserGoModels => {
            files::browser_navigate(ed, files::default_models_dir());
        }
        Action::BrowserGoHome => {
            if let Some(home) = files::home_dir() {
                files::browser_navigate(ed, home);
            }
        }
        Action::BrowserEntry(i) => {
            let clicked = match &ed.modal {
                Some(Modal::FileBrowser(fb)) => fb.entries.get(i).cloned().map(|e| (fb.purpose, e)),
                _ => None,
            };
            if let Some((purpose, entry)) = clicked {
                if entry.is_dir {
                    files::browser_navigate(ed, entry.path);
                } else {
                    match purpose {
                        BrowserPurpose::OpenModel => {
                            ed.modal = None;
                            files::open_path_now(ed, &entry.path);
                        }
                        BrowserPurpose::ImportPart => {
                            ed.modal = None;
                            files::import_part_now(ed, &entry.path);
                        }
                        BrowserPurpose::LoadTexture(mat_idx, slot) => {
                            ed.modal = None;
                            files::load_texture_now(ed, mat_idx, slot, &entry.path);
                        }
                        BrowserPurpose::SaveAs => {
                            if let Some(Modal::FileBrowser(fb)) = &mut ed.modal {
                                fb.file_name = entry.name;
                                ed.bump_ui();
                            }
                        }
                    }
                }
            }
        }
        Action::BrowserFormat(fmt) => {
            if let Some(Modal::FileBrowser(fb)) = &mut ed.modal {
                fb.save_format = fmt;
                let stem = std::path::Path::new(&fb.file_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("model")
                    .to_string();
                fb.file_name = format!("{stem}.{}", fmt.extension());
                ed.bump_ui();
            }
        }
        Action::BrowserMode(purpose) => {
            files::open_file_browser(ed, purpose);
        }
        Action::BrowserSubmitSave => files::browser_submit_save(ed),
        Action::BrowserSwitchNative => {
            let purpose = match &ed.modal {
                Some(Modal::FileBrowser(fb)) => Some(fb.purpose),
                _ => None,
            };
            ed.modal = None;
            if let Some(p) = purpose {
                ed.pending_dialog = Some(match p {
                    BrowserPurpose::OpenModel => NativeDialogRequest::OpenModel,
                    BrowserPurpose::ImportPart => NativeDialogRequest::ImportPart,
                    BrowserPurpose::SaveAs => NativeDialogRequest::SaveModelAs,
                    BrowserPurpose::LoadTexture(mat_idx, slot) => {
                        NativeDialogRequest::LoadTexture { mat_idx, slot }
                    }
                });
            }
            ed.bump_ui();
        }
    }
}

/// Allows painting directly on the live 2D Texture Preview inside the Paint panel!
fn paint_on_2d_canvas(
    mouse: Res<ButtonInput<MouseButton>>,
    mut editor: ResMut<Editor>,
    mut stroke: ResMut<Canvas2dStroke>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    canvas_q: Query<(&ComputedNode, &UiGlobalTransform), With<UvCanvasNode>>,
) {
    if !mouse.pressed(MouseButton::Left) {
        stroke.active = false;
        return;
    }
    if editor.mode != EditorMode::Paint || editor.modal.is_some() {
        stroke.active = false;
        return;
    }
    let (Ok(window), Ok((node, gtf))) = (window_q.single(), canvas_q.single()) else {
        return;
    };
    let Some(cursor) = window.physical_cursor_position() else {
        return;
    };
    let Some(norm) = node.normalize_point(*gtf, cursor) else {
        return;
    };
    let rel = norm + Vec2::splat(0.5);
    if !(0.0..=1.0).contains(&rel.x) || !(0.0..=1.0).contains(&rel.y) {
        return;
    }

    let Some(ti) = editor.active_paint_texture() else {
        return;
    };
    if mouse.just_pressed(MouseButton::Left) {
        editor.snapshot();
        stroke.active = true;
    }
    if stroke.active {
        let brush = editor.brush;
        let tex = &mut editor.doc.textures[ti];
        let px_r = (brush.radius * tex.width as f32 * 0.14).clamp(2.0, 96.0);
        paint::paint_dab_2d(tex, rel, px_r, &brush);
    }
}

fn scroll_lists(
    mut wheel: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut parts_q: Query<(&mut ScrollPosition, &ComputedNode), (With<PartsList>, Without<BrowserList>)>,
    mut browser_q: Query<(&mut ScrollPosition, &ComputedNode), (With<BrowserList>, Without<PartsList>)>,
    mut state: ResMut<ScrollState>,
    editor: Res<Editor>,
) {
    let mut lines = 0.0;
    for w in wheel.read() {
        lines += match w.unit {
            MouseScrollUnit::Line => w.y,
            MouseScrollUnit::Pixel => w.y / 30.0,
        };
    }
    let cursor = windows.single().ok().and_then(|w| w.cursor_position());

    if let Ok((mut pos, node)) = browser_q.single_mut() {
        if lines != 0.0 && matches!(editor.modal, Some(Modal::FileBrowser(_))) {
            let room = ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0);
            state.browser_y = (state.browser_y - lines * 32.0).clamp(0.0, room);
        }
        if pos.0.y != state.browser_y {
            pos.0.y = state.browser_y;
        }
    }
    if let Ok((mut pos, node)) = parts_q.single_mut() {
        let over_left = cursor.is_some_and(|c| c.x < LEFT_W && c.y > TOP_H);
        if lines != 0.0 && over_left && editor.modal.is_none() {
            let room = ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0);
            state.parts_y = (state.parts_y - lines * 28.0).clamp(0.0, room);
        }
        if pos.0.y != state.parts_y {
            pos.0.y = state.parts_y;
        }
    }
}

fn rebuild_ui(
    mut commands: Commands,
    editor: Res<Editor>,
    cache: Res<RenderCache>,
    scroll: Res<ScrollState>,
    roots: Query<Entity, With<UiRoot>>,
    time: Res<Time>,
    mut last: Local<(u64, u64, usize, bool)>,
) {
    let blink = editor.focus.is_some() && ((time.elapsed_secs() * 2.0) as u32).is_multiple_of(2);
    let key = (editor.ui_version, editor.scene_version, cache.srgb_textures.len(), blink);
    if *last == key {
        return;
    }
    *last = key;

    for e in &roots {
        commands.entity(e).despawn();
    }

    let ed = &*editor;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            Pickable::IGNORE,
            UiRoot,
        ))
        .with_children(|root| {
            top_bar(root, ed);
            root.spawn((
                Node {
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    min_height: Val::Px(0.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|mid| {
                left_panel(mid, ed, &scroll);
                right_panel(mid, ed, &cache, blink);
            });
            help_bar(root, ed);
        });

    if let Some(modal) = &ed.modal {
        match modal {
            Modal::ConfirmOverwrite { target } => {
                overwrite_warning_modal(&mut commands, target);
            }
            Modal::ConfirmDiscard(_) => {
                discard_warning_modal(&mut commands);
            }
            Modal::FileBrowser(fb) => {
                file_browser_modal(&mut commands, ed, fb, &scroll, blink);
            }
        }
    }
}

// --- Widgets ----------------------------------------------------------------

fn label(p: &mut ChildSpawnerCommands, text: impl Into<String>, size: f32, color: Color) {
    p.spawn((
        Node {
            flex_shrink: 0.0,
            ..default()
        },
        Text::new(text.into()),
        TextFont::from_font_size(size),
        TextColor(color),
        Pickable::IGNORE,
    ));
}

fn button_styled(
    p: &mut ChildSpawnerCommands,
    text: impl Into<String>,
    action: Action,
    normal: Color,
    hover: Color,
    enabled: bool,
    grow: bool,
) {
    let (n, h) = if enabled { (normal, hover) } else { (BTN_OFF, BTN_OFF) };
    let mut e = p.spawn((
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_grow: if grow { 1.0 } else { 0.0 },
            flex_shrink: 0.0,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(n),
        Btn { normal: n, hover: h },
    ));
    if enabled {
        e.insert(action);
    }
    e.with_children(|b| label(b, text, 12.5, if enabled { TEXT } else { MUTED }));
}

fn button(p: &mut ChildSpawnerCommands, text: impl Into<String>, action: Action, on: bool) {
    let (n, h) = if on { (BTN_ON, BTN_ON_HOVER) } else { (BTN, BTN_HOVER) };
    button_styled(p, text, action, n, h, true, false);
}

fn button_grow(p: &mut ChildSpawnerCommands, text: impl Into<String>, action: Action, on: bool) {
    let (n, h) = if on { (BTN_ON, BTN_ON_HOVER) } else { (BTN, BTN_HOVER) };
    button_styled(p, text, action, n, h, true, true);
}

fn row(p: &mut ChildSpawnerCommands, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(4.0),
            row_gap: Val::Px(4.0),
            align_items: AlignItems::Center,
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(f);
}

fn heading(p: &mut ChildSpawnerCommands, text: &str) {
    p.spawn((
        Node {
            margin: UiRect::top(Val::Px(7.0)),
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(|h| label(h, text.to_uppercase(), 11.5, MUTED));
}

fn text_field(
    p: &mut ChildSpawnerCommands,
    ed: &Editor,
    field: FocusField,
    value: &str,
    blink: bool,
    width: Val,
) {
    let focused = ed.focus == Some(field);
    let shown = if focused {
        format!("{value}{}", if blink { "|" } else { " " })
    } else if value.is_empty() {
        "(click to type)".to_string()
    } else {
        value.to_string()
    };
    let bg = if focused { FIELD_FOCUS } else { FIELD_BG };
    p.spawn((
        Node {
            width,
            flex_shrink: 0.0,
            padding: UiRect::axes(Val::Px(7.0), Val::Px(5.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(bg),
        BorderColor::all(if focused { BTN_ON } else { Color::srgb(0.25, 0.30, 0.38) }),
        Btn { normal: bg, hover: FIELD_FOCUS },
        Action::Focus(field),
    ))
    .with_children(|b| label(b, shown, 12.5, if value.is_empty() && !focused { MUTED } else { TEXT }));
}

fn panel_node(width: f32) -> Node {
    Node {
        width: Val::Px(width),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        padding: UiRect::all(Val::Px(10.0)),
        row_gap: Val::Px(5.0),
        overflow: Overflow::clip(),
        ..default()
    }
}

fn color_swatches(p: &mut ChildSpawnerCommands, active_srgb: [f32; 4]) {
    row(p, |r| {
        for (i, &c) in SWATCHES.iter().enumerate() {
            let col = Color::srgb(c[0], c[1], c[2]);
            let close = (c[0] - active_srgb[0]).abs()
                + (c[1] - active_srgb[1]).abs()
                + (c[2] - active_srgb[2]).abs()
                < 0.06;
            r.spawn((
                Node {
                    width: Val::Px(28.0),
                    height: Val::Px(22.0),
                    border: UiRect::all(Val::Px(if close { 2.0 } else { 1.0 })),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(col),
                BorderColor::all(if close { ACCENT } else { Color::srgba(1.0, 1.0, 1.0, 0.2) }),
                Btn { normal: col, hover: col },
                Action::PickSwatch(i),
            ));
        }
    });
}

// --- Panels -----------------------------------------------------------------

fn top_bar(root: &mut ChildSpawnerCommands, ed: &Editor) {
    root.spawn((
        Node {
            height: Val::Px(TOP_H),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(5.0),
            padding: UiRect::horizontal(Val::Px(10.0)),
            ..default()
        },
        BackgroundColor(PANEL),
    ))
    .with_children(|bar| {
        label(bar, "MODEL EDITOR", 14.5, ACCENT);
        bar.spawn((Node { width: Val::Px(6.0), ..default() }, Pickable::IGNORE));
        button(bar, "New", Action::New, false);
        button(bar, "Open", Action::OpenNative, false);
        button(bar, "Browse", Action::OpenBrowser, false);
        button(bar, "Import", Action::ImportNative, false);
        button(bar, if ed.dirty { "Save*" } else { "Save" }, Action::Save, ed.dirty);
        button(bar, "Save As", Action::SaveAsNative, false);
        bar.spawn((Node { width: Val::Px(4.0), ..default() }, Pickable::IGNORE));
        button_styled(bar, "Undo", Action::Undo, BTN, BTN_HOVER, ed.can_undo(), false);
        button_styled(bar, "Redo", Action::Redo, BTN, BTN_HOVER, ed.can_redo(), false);
        bar.spawn((Node { width: Val::Px(4.0), ..default() }, Pickable::IGNORE));
        button(bar, "Wire", Action::ToggleWireframe, ed.show_wireframe);
        button(bar, "Grid", Action::ToggleGrid, ed.show_grid);
        button(bar, format!("Light: {}", ed.light_preset.name()), Action::CycleLight, false);
        button(bar, "Frame", Action::Camera(CameraRequest::FrameAll), false);
        bar.spawn((Node { width: Val::Px(8.0), ..default() }, Pickable::IGNORE));
        label(
            bar,
            format!("{}{}", ed.display_name, if ed.dirty { "*" } else { "" }),
            12.5,
            if ed.dirty { ACCENT } else { TEXT },
        );
        bar.spawn((
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                margin: UiRect::left(Val::Px(10.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|s| label(s, ed.status.clone(), 11.5, MUTED));
    });
}

fn left_panel(mid: &mut ChildSpawnerCommands, ed: &Editor, scroll: &ScrollState) {
    mid.spawn((panel_node(LEFT_W), BackgroundColor(PANEL)))
        .with_children(|p| {
            heading(p, "Mode (1-4)");
            row(p, |r| {
                for (idx, m) in EditorMode::ALL.into_iter().enumerate() {
                    button_grow(r, format!("{} {}", idx + 1, m.name()), Action::Mode(m), ed.mode == m);
                }
            });

            match ed.mode {
                EditorMode::Object => {
                    heading(p, "Transform Gizmo");
                    row(p, |r| {
                        button_grow(r, "Move (G)", Action::Gizmo(GizmoMode::Move), ed.gizmo == GizmoMode::Move);
                        button_grow(r, "Rot (R)", Action::Gizmo(GizmoMode::Rotate), ed.gizmo == GizmoMode::Rotate);
                        button_grow(r, "Scale (S)", Action::Gizmo(GizmoMode::Scale), ed.gizmo == GizmoMode::Scale);
                    });
                    heading(p, "Add Primitive");
                    row(p, |r| {
                        for k in PrimitiveKind::ALL {
                            button(r, k.name(), Action::AddPrimitive(k), false);
                        }
                    });
                    heading(p, "Scene");
                    row(p, |r| {
                        button_grow(r, "Center & Ground", Action::CenterAndGround, false);
                    });
                    row(p, |r| {
                        button(r, "Front", Action::Camera(CameraRequest::Front), false);
                        button(r, "Side", Action::Camera(CameraRequest::Side), false);
                        button(r, "Top", Action::Camera(CameraRequest::Top), false);
                        button(r, "Iso", Action::Camera(CameraRequest::ResetIso), false);
                    });
                }
                EditorMode::Mesh => {
                    heading(p, "Select Geometry");
                    row(p, |r| {
                        for sub in [SubSelMode::Vertex, SubSelMode::Edge, SubSelMode::Face] {
                            button_grow(
                                r,
                                sub.name(),
                                Action::MeshTool(MeshTool::Select(sub)),
                                ed.mesh_tool == MeshTool::Select(sub),
                            );
                        }
                    });
                    heading(p, "3D Sculpt Brushes");
                    row(p, |r| {
                        for st in SculptTool::ALL {
                            button_grow(
                                r,
                                st.name(),
                                Action::MeshTool(MeshTool::Sculpt(st)),
                                ed.mesh_tool == MeshTool::Sculpt(st),
                            );
                        }
                    });
                    row(p, |r| {
                        label(r, format!("Radius {:.2}", ed.sculpt_radius), 12.0, TEXT);
                        button(r, "-", Action::SculptRadius(0.8), false);
                        button(r, "+", Action::SculptRadius(1.25), false);
                    });
                    row(p, |r| {
                        label(r, format!("Power {:.0}%", ed.sculpt_strength * 100.0), 12.0, TEXT);
                        button(r, "-", Action::SculptStrength(-0.1), false);
                        button(r, "+", Action::SculptStrength(0.1), false);
                        button(r, "Mirror X", Action::ToggleSculptMirror, ed.sculpt_mirror_x);
                    });
                }
                EditorMode::Paint => {
                    heading(p, "Paint Tools");
                    row(p, |r| {
                        for t in PaintTool::ALL {
                            button(r, t.name(), Action::PaintTool(t), ed.brush.tool == t);
                        }
                    });
                    heading(p, "Brush Tip & Feel");
                    row(p, |r| {
                        for tip in BrushTip::ALL {
                            button(r, tip.name(), Action::BrushTip(tip), ed.brush.tip == tip);
                        }
                    });
                    row(p, |r| {
                        label(r, format!("Size {:.2}", ed.brush.radius), 12.0, TEXT);
                        button(r, "-", Action::BrushRadius(0.8), false);
                        button(r, "+", Action::BrushRadius(1.25), false);
                        button(r, "Mirror X", Action::TogglePaintMirror, ed.brush.mirror_x);
                    });
                    row(p, |r| {
                        label(r, format!("Opacity {:.0}%", ed.brush.opacity * 100.0), 11.5, TEXT);
                        button(r, "-", Action::BrushOpacity(-0.1), false);
                        button(r, "+", Action::BrushOpacity(0.1), false);
                    });
                    row(p, |r| {
                        label(r, format!("Hard {:.0}%", ed.brush.hardness * 100.0), 11.5, TEXT);
                        button(r, "-", Action::BrushHardness(-0.15), false);
                        button(r, "+", Action::BrushHardness(0.15), false);
                    });
                    heading(p, "Texture Canvas");
                    row(p, |r| {
                        for res in [256u32, 512, 1024, 2048] {
                            button(r, format!("{res}"), Action::SetTexRes(res), ed.brush.new_tex_size == res);
                        }
                    });
                    row(p, |r| {
                        button_grow(r, "New Canvas", Action::NewPaintCanvas, false);
                        button_grow(r, "Export PNG", Action::ExportPaintPng, false);
                    });
                    heading(p, "Procedural Texture");
                    row(p, |r| {
                        for pat in ProceduralPattern::ALL {
                            button(r, pat.name(), Action::Procedural(pat), false);
                        }
                    });
                }
                EditorMode::Material => {
                    heading(p, "Materials");
                    row(p, |r| {
                        button_grow(r, "+ New Material", Action::NewMat, false);
                        button(r, "Copy", Action::DuplicateMat, false);
                    });
                    for (i, m) in ed.doc.materials.iter().enumerate() {
                        row(p, |r| {
                            let srgb = m.base_color_srgb();
                            r.spawn((
                                Node {
                                    width: Val::Px(14.0),
                                    height: Val::Px(14.0),
                                    border_radius: BorderRadius::all(Val::Px(3.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgb(srgb[0], srgb[1], srgb[2])),
                                Pickable::IGNORE,
                            ));
                            button_grow(r, m.name.clone(), Action::SelectMat(i), ed.selected_mat == i);
                        });
                    }
                    if ed.selected_part.is_some() {
                        row(p, |r| button_grow(r, "Assign to Selected Part", Action::AssignMatToSelected, false));
                    }
                }
            }

            // Scrollable Parts Outliner
            heading(p, &format!("Parts ({})", ed.doc.parts.len()));
            p.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    flex_basis: Val::Px(0.0),
                    min_height: Val::Px(64.0),
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollPosition(Vec2::new(0.0, scroll.parts_y)),
                PartsList,
            ))
            .with_children(|list| {
                for (i, part) in ed.doc.parts.iter().enumerate() {
                    let sel = ed.selected_part == Some(i);
                    row(list, |r| {
                        button(r, if part.visible { "V" } else { "-" }, Action::TogglePartVis(i), part.visible);
                        button_grow(
                            r,
                            format!("{} ({}t)", part.name, part.tri_count()),
                            Action::SelectPart(i),
                            sel,
                        );
                    });
                }
            });
        });
}

fn right_panel(
    mid: &mut ChildSpawnerCommands,
    ed: &Editor,
    cache: &RenderCache,
    blink: bool,
) {
    mid.spawn((panel_node(RIGHT_W), BackgroundColor(PANEL)))
        .with_children(|p| match ed.mode {
            EditorMode::Object => right_object_inspector(p, ed, blink),
            EditorMode::Mesh => right_mesh_inspector(p, ed),
            EditorMode::Paint => right_paint_inspector(p, ed, cache, blink),
            EditorMode::Material => right_material_inspector(p, ed, blink),
        });
}

fn right_object_inspector(p: &mut ChildSpawnerCommands, ed: &Editor, blink: bool) {
    heading(p, "Model Summary");
    label(
        p,
        format!(
            "{} parts  |  {} verts  |  {} tris",
            ed.doc.parts.len(),
            ed.doc.vertex_count(),
            ed.doc.triangle_count()
        ),
        12.0,
        TEXT,
    );
    label(
        p,
        format!(
            "{} materials  |  {} textures",
            ed.doc.materials.len(),
            ed.doc.textures.len()
        ),
        11.5,
        MUTED,
    );
    if let Some(rig) = &ed.doc.preserved_rig {
        label(
            p,
            format!(
                "Rig preserved: {} skin(s), {} animation(s)",
                rig.skins.len(),
                rig.animations.len()
            ),
            11.5,
            ACCENT,
        );
    }

    heading(p, "Selected Part");
    let Some(pi) = ed.selected_part else {
        label(p, "Click a part in the 3D view or Parts list.", 12.0, MUTED);
        return;
    };
    let Some(part) = ed.doc.parts.get(pi) else { return };

    text_field(p, ed, FocusField::PartName, &part.name, blink, Val::Percent(100.0));
    label(
        p,
        format!(
            "Pos  X {:.2}  Y {:.2}  Z {:.2}",
            part.translation[0], part.translation[1], part.translation[2]
        ),
        11.5,
        TEXT,
    );
    row(p, |r| {
        button(r, "X-", Action::NudgePart(0, -0.25), false);
        button(r, "X+", Action::NudgePart(0, 0.25), false);
        button(r, "Y-", Action::NudgePart(1, -0.25), false);
        button(r, "Y+", Action::NudgePart(1, 0.25), false);
        button(r, "Z-", Action::NudgePart(2, -0.25), false);
        button(r, "Z+", Action::NudgePart(2, 0.25), false);
        button(r, "0", Action::ResetPartPos, false);
    });

    row(p, |r| {
        label(r, "Turn", 11.5, MUTED);
        button(r, "X 90", Action::RotatePart(0, 90.0), false);
        button(r, "Y 90", Action::RotatePart(1, 90.0), false);
        button(r, "Z 90", Action::RotatePart(2, 90.0), false);
        button(r, "Reset", Action::ResetPartRot, false);
    });

    label(
        p,
        format!("Scale  {:.2}  {:.2}  {:.2}", part.scale[0], part.scale[1], part.scale[2]),
        11.5,
        TEXT,
    );
    row(p, |r| {
        button(r, "Smaller", Action::ScalePart(0.8), false);
        button(r, "Bigger", Action::ScalePart(1.25), false);
        button(r, "Reset 1.0", Action::ResetPartScale, false);
    });

    heading(p, "Part Material");
    let mat_name = ed
        .doc
        .materials
        .get(part.material)
        .map(|m| m.name.as_str())
        .unwrap_or("Default");
    row(p, |r| {
        button(r, "<", Action::CyclePartMaterial(-1), false);
        label(r, mat_name, 12.5, ACCENT);
        button(r, ">", Action::CyclePartMaterial(1), false);
    });
    row(p, |r| {
        button_grow(r, "Paint This Part", Action::Mode(EditorMode::Paint), false);
        button_grow(r, "Edit Material", Action::Mode(EditorMode::Material), false);
    });

    heading(p, "Geometry Tools");
    row(p, |r| {
        button(r, "Auto-UVs", Action::AutoUvPart, !part.has_uvs());
        button(r, "Smooth N", Action::SmoothNormals, false);
        button(r, "Flat N", Action::FlatNormals, false);
        button(r, "Flip N", Action::FlipNormals, false);
    });
    row(p, |r| {
        button(r, "Bake TRS", Action::BakeTransform, false);
        button_styled(r, "Merge Next", Action::MergeNext, BTN, BTN_HOVER, ed.doc.parts.len() > 1, false);
    });

    heading(p, "Organize");
    row(p, |r| {
        button(r, "Duplicate", Action::DuplicatePart, false);
        button(r, "Mirror X", Action::MirrorPart(0), false);
        button(r, "Mirror Z", Action::MirrorPart(2), false);
    });
    row(p, |r| {
        button_styled(r, "Delete Part", Action::DeletePart, BTN_WARN, BTN_WARN_HOVER, true, true);
    });
}

fn right_mesh_inspector(p: &mut ChildSpawnerCommands, ed: &Editor) {
    heading(p, "Mesh Edit Inspector");
    let Some(pi) = ed.selected_part else {
        label(p, "Select a part first.", 12.0, MUTED);
        return;
    };
    let Some(part) = ed.doc.parts.get(pi) else { return };
    label(p, format!("Editing: {}", part.name), 13.5, ACCENT);
    label(
        p,
        format!(
            "Selected: {} faces, {} verts",
            ed.mesh_sel.faces.len(),
            ed.mesh_sel.verts.len()
        ),
        12.0,
        TEXT,
    );
    row(p, |r| {
        button_grow(r, "Select All", Action::MeshSelectAll, false);
        button_grow(r, "Clear", Action::MeshClearSel, false);
    });

    heading(p, "Face Operations");
    row(p, |r| {
        button_grow(r, "Extrude +0.25 (E)", Action::MeshExtrude(0.25), false);
        button(r, "In -0.15", Action::MeshExtrude(-0.15), false);
    });
    row(p, |r| {
        button_grow(r, "Inset Face", Action::MeshInset, false);
        button_grow(r, "Subdivide", Action::MeshSubdivide, false);
    });
    row(p, |r| {
        button_grow(r, "Split -> New Material", Action::MeshSplitMaterial, false);
    });
    row(p, |r| {
        button_styled(
            r,
            "Delete Selected Faces",
            Action::MeshDeleteFaces,
            BTN_WARN,
            BTN_WARN_HOVER,
            !ed.mesh_sel.faces.is_empty(),
            true,
        );
    });

    heading(p, "Vertex Operations");
    row(p, |r| {
        button_grow(r, "Smooth", Action::MeshSmooth, false);
        button_grow(r, "Flatten", Action::MeshFlatten, false);
    });
    row(p, |r| {
        button_grow(r, "Scale 0.85x", Action::MeshScale(0.85), false);
        button_grow(r, "Scale 1.15x", Action::MeshScale(1.15), false);
    });

    heading(p, "Tips");
    label(
        p,
        "Click faces/edges/verts in 3D (Shift to multi-select, Alt for single tri). Drag the RGB arrows to move selection. Switch to Push/Pull/Smooth/Flatten on the left to sculpt!",
        11.5,
        MUTED,
    );
}

fn right_paint_inspector(
    p: &mut ChildSpawnerCommands,
    ed: &Editor,
    cache: &RenderCache,
    blink: bool,
) {
    heading(p, "Brush Color");
    let c = ed.brush.color;
    let e = ed.brush.erase_color;
    row(p, |r| {
        r.spawn((
            Node {
                width: Val::Px(36.0),
                height: Val::Px(26.0),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(c[0], c[1], c[2])),
            BorderColor::all(ACCENT),
            Pickable::IGNORE,
        ));
        r.spawn((
            Node {
                width: Val::Px(22.0),
                height: Val::Px(22.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(e[0], e[1], e[2])),
            BorderColor::all(MUTED),
            Pickable::IGNORE,
        ));
        text_field(r, ed, FocusField::HexColor, &ed.hex_input, blink, Val::Px(84.0));
        button(r, "Swap", Action::SwapBrushColors, false);
    });

    color_swatches(p, c);

    // Fine RGB sliders via clean +/- buttons
    for (ch, name) in [(0, "R"), (1, "G"), (2, "B")] {
        let val = (c[ch] * 255.0).round() as u8;
        row(p, |r| {
            label(r, format!("{name}: {val:3}"), 11.5, TEXT);
            button(r, "-25", Action::AdjustBrushRgb(ch, -0.10), false);
            button(r, "-5", Action::AdjustBrushRgb(ch, -0.02), false);
            button(r, "+5", Action::AdjustBrushRgb(ch, 0.02), false);
            button(r, "+25", Action::AdjustBrushRgb(ch, 0.10), false);
        });
    }

    heading(p, "Texture Image & 2D Canvas");
    let mat_idx = ed
        .selected_part
        .and_then(|pi| ed.doc.parts.get(pi))
        .map(|part| part.material)
        .unwrap_or(ed.selected_mat);
    row(p, |r| {
        button_grow(r, "Load Image...", Action::LoadTexNative(mat_idx, TexMapKind::BaseColor), false);
        button(r, "Browse", Action::LoadTexBrowser(mat_idx, TexMapKind::BaseColor), false);
    });

    if let Some(ti) = ed.active_paint_texture()
        && let Some(handle) = cache.preview_handle(ti)
    {
        let tex = &ed.doc.textures[ti];
        label(p, format!("{} ({}×{}) — drag here to paint 2D!", tex.name, tex.width, tex.height), 11.0, MUTED);
        p.spawn((
            Node {
                width: Val::Px(240.0),
                height: Val::Px(240.0),
                border: UiRect::all(Val::Px(1.5)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BorderColor::all(Color::srgb(0.32, 0.38, 0.48)),
            ImageNode::new(handle),
            UvCanvasNode,
        ));
    } else {
        label(
            p,
            "Paint directly on the 3D model — a texture canvas and UVs are created automatically on your first brushstroke!",
            11.5,
            MUTED,
        );
    }
}

fn right_material_inspector(p: &mut ChildSpawnerCommands, ed: &Editor, blink: bool) {
    heading(p, "Material Properties");
    let Some(mat) = ed.doc.materials.get(ed.selected_mat) else {
        label(p, "No material selected.", 12.0, MUTED);
        return;
    };
    let mi = ed.selected_mat;
    text_field(p, ed, FocusField::MaterialName, &mat.name, blink, Val::Percent(100.0));

    let srgb = mat.base_color_srgb();
    row(p, |r| {
        r.spawn((
            Node {
                width: Val::Px(34.0),
                height: Val::Px(24.0),
                border: UiRect::all(Val::Px(1.5)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(srgb[0], srgb[1], srgb[2])),
            BorderColor::all(ACCENT),
            Pickable::IGNORE,
        ));
        text_field(r, ed, FocusField::HexColor, &ed.hex_input, blink, Val::Px(90.0));
        label(r, format!("Alpha {:.0}%", srgb[3] * 100.0), 11.5, MUTED);
    });
    color_swatches(p, srgb);

    for (ch, name) in [(0, "R"), (1, "G"), (2, "B")] {
        let val = (srgb[ch] * 255.0).round() as u8;
        row(p, |r| {
            label(r, format!("{name} {val:3}"), 11.5, TEXT);
            button(r, "-", Action::AdjustMatRgb(ch, -0.08), false);
            button(r, "+", Action::AdjustMatRgb(ch, 0.08), false);
        });
    }
    row(p, |r| {
        label(r, format!("Alpha {:.0}%", mat.base_color[3] * 100.0), 11.5, TEXT);
        button(r, "-", Action::AdjustMatAlpha(-0.1), false);
        button(r, "+", Action::AdjustMatAlpha(0.1), false);
        button(r, mat.alpha_mode.name(), Action::CycleMatAlpha, false);
    });

    heading(p, "PBR Surface");
    row(p, |r| {
        label(r, format!("Metallic  {:.0}%", mat.metallic * 100.0), 12.0, TEXT);
        button(r, "-", Action::AdjustMatMetallic(-0.1), false);
        button(r, "+", Action::AdjustMatMetallic(0.1), false);
    });
    row(p, |r| {
        label(r, format!("Roughness {:.0}%", mat.roughness * 100.0), 12.0, TEXT);
        button(r, "-", Action::AdjustMatRoughness(-0.1), false);
        button(r, "+", Action::AdjustMatRoughness(0.1), false);
    });
    let em = mat.emissive[0].max(mat.emissive[1]).max(mat.emissive[2]);
    row(p, |r| {
        label(r, format!("Glow      {:.0}%", em * 100.0), 12.0, TEXT);
        button(r, "-", Action::AdjustMatEmissive(-0.2), false);
        button(r, "+", Action::AdjustMatEmissive(0.2), false);
        button(r, "2-Sided", Action::ToggleMatDoubleSided, mat.double_sided);
    });

    heading(p, "Texture Maps (Embedded in GLB)");
    let slots = [
        (TexMapKind::BaseColor, mat.base_color_texture),
        (TexMapKind::Normal, mat.normal_texture),
        (TexMapKind::MetallicRoughness, mat.metallic_roughness_texture),
        (TexMapKind::Emissive, mat.emissive_texture),
    ];
    for (slot, tex_opt) in slots {
        let desc = tex_opt
            .and_then(|ti| ed.doc.textures.get(ti))
            .map(|t| format!("{} ({}×{})", t.name, t.width, t.height))
            .unwrap_or_else(|| "None".into());
        label(p, format!("{}: {desc}", slot.name()), 11.5, if tex_opt.is_some() { ACCENT } else { MUTED });
        row(p, |r| {
            button(r, "Load...", Action::LoadTexNative(mi, slot), false);
            button(r, "Browse", Action::LoadTexBrowser(mi, slot), false);
            if tex_opt.is_some() {
                button(r, "Clear", Action::ClearTex(mi, slot), false);
            }
        });
    }
}

fn help_bar(root: &mut ChildSpawnerCommands, ed: &Editor) {
    let tip = match ed.mode {
        EditorMode::Object => "Click part to select. G/R/S: Move/Rotate/Scale gizmo (Shift snaps). Drag & drop .glb/.obj/.stl/.ply or texture images onto the window!",
        EditorMode::Mesh => "Click Vertex/Edge/Face (Shift multi-select, Alt single tri). E: extrude. Switch to Push/Pull/Smooth/Flatten to sculpt.",
        EditorMode::Paint => "Left-drag on 3D model or 2D canvas to paint! [ / ]: brush size. Alt+click: eyedropper. Shift+click: straight line.",
        EditorMode::Material => "Edit PBR Base Color, Metallic, Roughness, Glow, and attach PNG/JPG textures (embedded automatically when saving .glb).",
    };
    root.spawn((
        Node {
            height: Val::Px(BOTTOM_H),
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(10.0)),
            ..default()
        },
        BackgroundColor(PANEL),
        Pickable::IGNORE,
    ))
    .with_children(|b| {
        label(
            b,
            format!("{tip}   |   Right/Mid drag: orbit  Shift+drag: pan  Wheel: zoom  F: frame  Ctrl+S: save  Ctrl+Shift+S: save as"),
            11.5,
            MUTED,
        );
    });
}

// --- Modal Dialogs ----------------------------------------------------------

/// First-Save Overwrite Warning Modal.
/// Clicking "No, Keep Editing" preserves ALL work in the editor and keeps the
/// warning armed so clicking Save again will prompt again.
fn overwrite_warning_modal(commands: &mut Commands, target: &std::path::Path) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
            GlobalZIndex(20),
            UiRoot,
        ))
        .with_children(|bg| {
            bg.spawn((
                Node {
                    width: Val::Px(520.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(18.0)),
                    row_gap: Val::Px(10.0),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(CARD),
                BorderColor::all(ACCENT),
            ))
            .with_children(|p| {
                label(p, "WARNING: OVERWRITE EXISTING FILE?", 16.0, ACCENT);
                label(
                    p,
                    "Pressing Save will overwrite the original file on disk:",
                    13.0,
                    TEXT,
                );
                p.spawn((
                    Node {
                        padding: UiRect::all(Val::Px(8.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(FIELD_BG),
                    Pickable::IGNORE,
                ))
                .with_children(|box_| label(box_, target.display().to_string(), 12.5, ACCENT));

                label(
                    p,
                    "If you click No, your changes will NOT be lost — you can keep editing, click Save again to see this prompt, or use Save As to save a separate copy.",
                    12.0,
                    MUTED,
                );
                p.spawn((Node { height: Val::Px(4.0), ..default() }, Pickable::IGNORE));
                row(p, |r| {
                    button_styled(
                        r,
                        "Yes, Overwrite File",
                        Action::ConfirmOverwrite,
                        BTN_WARN,
                        BTN_WARN_HOVER,
                        true,
                        true,
                    );
                    button_styled(
                        r,
                        "Save As Copy...",
                        Action::OverwriteToSaveAs,
                        BTN_ON,
                        BTN_ON_HOVER,
                        true,
                        true,
                    );
                    button_styled(
                        r,
                        "No, Keep Editing",
                        Action::CancelOverwrite,
                        BTN,
                        BTN_HOVER,
                        true,
                        true,
                    );
                });
            });
        });
}

fn discard_warning_modal(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
            GlobalZIndex(20),
            UiRoot,
        ))
        .with_children(|bg| {
            bg.spawn((
                Node {
                    width: Val::Px(440.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(18.0)),
                    row_gap: Val::Px(10.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(CARD),
                BorderColor::all(ACCENT),
            ))
            .with_children(|p| {
                label(p, "UNSAVED CHANGES", 16.0, ACCENT);
                label(
                    p,
                    "You have unsaved edits to this model. Discard them and continue?",
                    13.0,
                    TEXT,
                );
                row(p, |r| {
                    button_styled(
                        r,
                        "Discard & Continue",
                        Action::ConfirmDiscard,
                        BTN_WARN,
                        BTN_WARN_HOVER,
                        true,
                        true,
                    );
                    button_styled(r, "Cancel", Action::CloseModal, BTN, BTN_HOVER, true, true);
                });
            });
        });
}

fn file_browser_modal(
    commands: &mut Commands,
    ed: &Editor,
    fb: &crate::state::FileBrowserState,
    scroll: &ScrollState,
    blink: bool,
) {
    let title = match fb.purpose {
        BrowserPurpose::OpenModel => "BROWSE & OPEN 3D MODEL",
        BrowserPurpose::ImportPart => "IMPORT 3D MODEL PART",
        BrowserPurpose::SaveAs => "SAVE 3D MODEL AS",
        BrowserPurpose::LoadTexture(_, slot) => match slot {
            TexMapKind::BaseColor => "LOAD BASE COLOR TEXTURE",
            TexMapKind::Normal => "LOAD NORMAL MAP",
            TexMapKind::MetallicRoughness => "LOAD METALLIC / ROUGHNESS MAP",
            TexMapKind::Emissive => "LOAD EMISSIVE MAP",
        },
    };
    let is_model_browser = !matches!(fb.purpose, BrowserPurpose::LoadTexture(..));

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.62)),
            GlobalZIndex(20),
            UiRoot,
        ))
        .with_children(|bg| {
            bg.spawn((
                Node {
                    width: Val::Px(580.0),
                    height: Val::Px(520.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(14.0)),
                    row_gap: Val::Px(6.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(CARD),
                BorderColor::all(Color::srgb(0.28, 0.36, 0.48)),
            ))
            .with_children(|p| {
                row(p, |r| {
                    label(r, title, 15.0, ACCENT);
                    button(r, "OS Dialog...", Action::BrowserSwitchNative, false);
                });
                if is_model_browser {
                    row(p, |r| {
                        button(r, "Open", Action::BrowserMode(BrowserPurpose::OpenModel), fb.purpose == BrowserPurpose::OpenModel);
                        button(r, "Import Part", Action::BrowserMode(BrowserPurpose::ImportPart), fb.purpose == BrowserPurpose::ImportPart);
                        button(r, "Save As Copy", Action::BrowserMode(BrowserPurpose::SaveAs), fb.purpose == BrowserPurpose::SaveAs);
                    });
                }
                row(p, |r| {
                    button(r, "Up (..)", Action::BrowserUp, false);
                    button(r, "Models Folder", Action::BrowserGoModels, false);
                    button(r, "Home", Action::BrowserGoHome, false);
                });
                label(p, fb.dir.display().to_string(), 11.5, MUTED);

                p.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(3.0),
                        flex_grow: 1.0,
                        flex_shrink: 1.0,
                        flex_basis: Val::Px(0.0),
                        padding: UiRect::all(Val::Px(6.0)),
                        overflow: Overflow::scroll_y(),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(FIELD_BG),
                    ScrollPosition(Vec2::new(0.0, scroll.browser_y)),
                    BrowserList,
                ))
                .with_children(|list| {
                    if fb.entries.is_empty() {
                        label(list, "No matching files or folders here.", 12.0, MUTED);
                    }
                    for (i, entry) in fb.entries.iter().enumerate() {
                        row(list, |r| {
                            button_grow(r, entry.name.clone(), Action::BrowserEntry(i), false);
                        });
                    }
                });

                if fb.purpose == BrowserPurpose::SaveAs {
                    row(p, |r| {
                        label(r, "File:", 12.0, TEXT);
                        text_field(r, ed, FocusField::BrowserFileName, &fb.file_name, blink, Val::Px(360.0));
                        button_styled(r, "Save", Action::BrowserSubmitSave, BTN_ON, BTN_ON_HOVER, true, true);
                    });
                    row(p, |r| {
                        label(r, "Format:", 11.5, MUTED);
                        for fmt in [
                            ModelFormat::Glb,
                            ModelFormat::Gltf,
                            ModelFormat::Obj,
                            ModelFormat::Stl,
                            ModelFormat::Ply,
                        ] {
                            button(r, format!(".{}", fmt.extension()), Action::BrowserFormat(fmt), fb.save_format == fmt);
                        }
                    });
                }

                row(p, |r| {
                    button(r, "Cancel", Action::CloseModal, false);
                });
            });
        });
}
