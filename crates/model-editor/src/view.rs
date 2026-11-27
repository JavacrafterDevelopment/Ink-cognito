//! 3D viewport: Orbit/Pan/Zoom + optional Right-mouse WASD camera, exact 3D
//! picking, Paint & Sculpt strokes, Mesh sub-selection, keyboard shortcuts,
//! text input, File Drag-and-Drop, and 3D gizmo overlays.

use crate::{
    files,
    gizmo::{self, GizmoState},
    io,
    mesh_ops::{self, SubSelMode},
    model::{RayHit, srgb_f32_to_u8, u8_to_srgb_f32},
    paint::{self, PaintTool},
    state::{
        CameraRequest, Editor, EditorMode, FocusField, GizmoMode, MeshTool, Modal, TexMapKind,
    },
    ui,
};
use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
        mouse::{MouseMotion, MouseScrollUnit, MouseWheel},
    },
    prelude::*,
    window::{FileDragAndDrop, PrimaryWindow},
};

pub struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewportInteract>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (
                    handle_file_drop,
                    text_input,
                    shortcuts,
                    orbit_camera,
                    gizmo::gizmo_input,
                    viewport_interact,
                    draw_overlay,
                )
                    .chain(),
            );
    }
}

#[derive(Component)]
pub struct OrbitCam {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl OrbitCam {
    pub fn apply_to(&self, t: &mut Transform) {
        let rot = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
        let offset = rot * Vec3::new(0.0, 0.0, self.distance);
        t.translation = self.target + offset;
        t.look_at(self.target, Vec3::Y);
    }
}

#[derive(Resource, Default)]
pub struct ViewportInteract {
    /// Current 3D surface hit under cursor (for brush cursor preview).
    pub hover: Option<(usize, RayHit)>,
    /// Active paint stroke state.
    pub painting: bool,
    pub paint_part: Option<usize>,
    pub last_paint_local: Option<Vec3>,
    pub stroke_seed: u64,
    /// Active sculpt stroke state.
    pub sculpting: bool,
    pub sculpt_part: Option<usize>,
}

fn spawn_camera(mut commands: Commands) {
    let cam = OrbitCam {
        target: Vec3::new(0.0, 0.5, 0.0),
        yaw: 0.65,
        pitch: -0.42,
        distance: 2.8,
    };
    let mut tf = Transform::default();
    cam.apply_to(&mut tf);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 50f32.to_radians(),
            near: 0.02,
            far: 500.0,
            ..default()
        }),
        tf,
        cam,
    ));
}

/// Whether `cursor` lies over the 3D viewport (outside side/top/bottom panels
/// and not blocked by a modal dialog).
pub fn in_view(window: &Window, cursor: Vec2, editor: &Editor) -> bool {
    if editor.modal.is_some() {
        return false;
    }
    let w = window.width();
    let h = window.height();
    cursor.x > ui::LEFT_W
        && cursor.x < w - ui::RIGHT_W
        && cursor.y > ui::TOP_H
        && cursor.y < h - ui::BOTTOM_H
}

pub fn cursor_ray(
    window: &Window,
    editor: &Editor,
    cam: (&Camera, &GlobalTransform),
) -> Option<Ray3d> {
    let c = window.cursor_position()?;
    if !in_view(window, c, editor) {
        return None;
    }
    cam.0.viewport_to_world(cam.1, c).ok()
}

fn handle_file_drop(
    mut drops: MessageReader<FileDragAndDrop>,
    mut editor: ResMut<Editor>,
) {
    for ev in drops.read() {
        if let FileDragAndDrop::DroppedFile { path_buf, .. } = ev {
            if io::is_model_path(path_buf) {
                files::request_open_path(&mut editor, path_buf.clone());
            } else if io::is_image_path(path_buf) {
                let mat_idx = editor
                    .selected_part
                    .and_then(|pi| editor.doc.parts.get(pi))
                    .map(|p| p.material)
                    .unwrap_or(editor.selected_mat);
                files::load_texture_now(&mut editor, mat_idx, TexMapKind::BaseColor, path_buf);
            } else {
                editor.status = format!(
                    "Dropped '{}' is not a supported model or image.",
                    path_buf.display()
                );
                editor.bump_ui();
            }
        }
    }
}

fn text_input(mut keys: MessageReader<KeyboardInput>, mut editor: ResMut<Editor>) {
    let Some(field) = editor.focus else {
        return;
    };
    let mut changed = false;
    let mut submit = false;
    let mut cancel = false;

    for ev in keys.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Enter => submit = true,
            Key::Escape | Key::Tab => cancel = true,
            Key::Backspace => {
                if let Some(buf) = editor.focused_text_mut() {
                    buf.pop();
                    changed = true;
                }
            }
            _ => {
                if let Some(text) = &ev.text {
                    let limit = match field {
                        FocusField::HexColor => 7,
                        FocusField::BrowserFileName => 120,
                        _ => 48,
                    };
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        if let Some(buf) = editor.focused_text_mut()
                            && buf.chars().count() < limit
                        {
                            buf.push(ch);
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    if changed {
        if matches!(field, FocusField::PartName | FocusField::MaterialName) {
            editor.dirty = true;
        }
        editor.bump_ui();
    }
    if submit {
        editor.focus = None;
        match field {
            FocusField::HexColor => editor.apply_hex_color(),
            FocusField::BrowserFileName => files::browser_submit_save(&mut editor),
            _ => editor.bump_ui(),
        }
    } else if cancel {
        editor.focus = None;
        editor.bump_ui();
    }
}

fn shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut editor: ResMut<Editor>,
) {
    if editor.focus.is_some() {
        return;
    }

    // Esc closes modals cleanly (cancelling Overwrite modal NEVER deletes work!).
    if keys.just_pressed(KeyCode::Escape) {
        match &editor.modal {
            Some(Modal::ConfirmOverwrite { .. }) => {
                files::cancel_overwrite(&mut editor);
                return;
            }
            Some(_) => {
                editor.modal = None;
                editor.bump_ui();
                return;
            }
            None => {
                if !editor.mesh_sel.is_empty() {
                    editor.mesh_sel.clear();
                    editor.bump_ui();
                } else {
                    editor.select_part(None);
                }
                return;
            }
        }
    }

    if editor.modal.is_some() {
        return;
    }

    let ctrl = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if ctrl {
        if keys.just_pressed(KeyCode::KeyS) {
            if shift {
                files::request_save_as_native(&mut editor);
            } else {
                files::request_save(&mut editor);
            }
        }
        if keys.just_pressed(KeyCode::KeyO) {
            files::request_open_native(&mut editor);
        }
        if keys.just_pressed(KeyCode::KeyN) {
            files::request_new(&mut editor);
        }
        if keys.just_pressed(KeyCode::KeyZ) {
            if shift {
                editor.redo();
            } else {
                editor.undo();
            }
        }
        if keys.just_pressed(KeyCode::KeyY) {
            editor.redo();
        }
        if keys.just_pressed(KeyCode::KeyD) {
            editor.duplicate_selected();
        }
        return;
    }

    // Don't hijack WASD while flying with right mouse held.
    if mouse.pressed(MouseButton::Right) {
        return;
    }

    if keys.just_pressed(KeyCode::Digit1) {
        editor.mode = EditorMode::Object;
        editor.bump_ui();
    }
    if keys.just_pressed(KeyCode::Digit2) {
        editor.mode = EditorMode::Mesh;
        editor.bump_ui();
    }
    if keys.just_pressed(KeyCode::Digit3) {
        editor.mode = EditorMode::Paint;
        editor.bump_ui();
    }
    if keys.just_pressed(KeyCode::Digit4) {
        editor.mode = EditorMode::Material;
        editor.bump_ui();
    }

    if keys.just_pressed(KeyCode::KeyF) {
        editor.camera_request = Some(if shift {
            CameraRequest::FrameSelected
        } else {
            CameraRequest::FrameAll
        });
    }
    if keys.just_pressed(KeyCode::KeyW) {
        editor.show_wireframe = !editor.show_wireframe;
        editor.bump_ui();
    }

    // Brush / sculpt size hotkeys
    if keys.just_pressed(KeyCode::BracketLeft) {
        if editor.mode == EditorMode::Paint {
            editor.brush.radius = (editor.brush.radius * 0.8).clamp(0.01, 3.0);
        } else {
            editor.sculpt_radius = (editor.sculpt_radius * 0.8).clamp(0.02, 3.0);
        }
        editor.bump_ui();
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        if editor.mode == EditorMode::Paint {
            editor.brush.radius = (editor.brush.radius * 1.25).clamp(0.01, 3.0);
        } else {
            editor.sculpt_radius = (editor.sculpt_radius * 1.25).clamp(0.02, 3.0);
        }
        editor.bump_ui();
    }

    match editor.mode {
        EditorMode::Object => {
            if keys.just_pressed(KeyCode::KeyG) {
                editor.gizmo = GizmoMode::Move;
                editor.bump_ui();
            }
            if keys.just_pressed(KeyCode::KeyR) {
                editor.gizmo = GizmoMode::Rotate;
                editor.bump_ui();
            }
            if keys.just_pressed(KeyCode::KeyS) {
                editor.gizmo = GizmoMode::Scale;
                editor.bump_ui();
            }
            if keys.any_just_pressed([KeyCode::Delete, KeyCode::Backspace]) {
                editor.delete_selected_part();
            }
        }
        EditorMode::Mesh => {
            if keys.just_pressed(KeyCode::KeyE)
                && !editor.mesh_sel.faces.is_empty()
                && let Some(pi) = editor.selected_part
            {
                editor.snapshot();
                let faces = editor.mesh_sel.faces.clone();
                let cap = mesh_ops::extrude_faces(&mut editor.doc.parts[pi], &faces, 0.25);
                editor.mesh_sel.faces = cap;
                editor.mesh_sel.verts.clear();
                editor.status = "Extruded selected faces. Drag the gizmo arrows to adjust.".into();
                editor.bump_scene();
            }
            if keys.any_just_pressed([KeyCode::Delete, KeyCode::Backspace])
                && !editor.mesh_sel.faces.is_empty()
                && let Some(pi) = editor.selected_part
            {
                editor.snapshot();
                let faces = editor.mesh_sel.faces.clone();
                mesh_ops::delete_faces(&mut editor.doc.parts[pi], &faces);
                editor.mesh_sel.clear();
                editor.status = "Deleted selected faces.".into();
                editor.bump_scene();
            }
        }
        EditorMode::Paint => {
            if keys.just_pressed(KeyCode::KeyB) {
                editor.brush.tool = PaintTool::Brush;
                editor.bump_ui();
            }
            if keys.just_pressed(KeyCode::KeyE) {
                editor.brush.tool = PaintTool::Eraser;
                editor.bump_ui();
            }
            if keys.just_pressed(KeyCode::KeyI) {
                editor.brush.tool = PaintTool::Eyedropper;
                editor.bump_ui();
            }
        }
        EditorMode::Material => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn orbit_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    mut editor: ResMut<Editor>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    mut cam_q: Query<(&mut Transform, &mut OrbitCam)>,
) {
    let Ok((mut tf, mut cam)) = cam_q.single_mut() else {
        return;
    };

    // Handle Frame / View Preset camera requests first.
    if let Some(req) = editor.camera_request.take() {
        match req {
            CameraRequest::FrameAll => {
                if let Some((lo, hi)) = editor.doc.bounds() {
                    let center = (lo + hi) * 0.5;
                    let radius = ((hi - lo).length() * 0.5).max(0.25);
                    cam.target = center;
                    cam.distance = (radius * 2.5).clamp(0.4, 80.0);
                } else {
                    cam.target = Vec3::ZERO;
                    cam.distance = 3.0;
                }
            }
            CameraRequest::FrameSelected => {
                let bounds = editor
                    .selected_part
                    .and_then(|i| editor.doc.parts.get(i))
                    .and_then(|p| p.world_bounds())
                    .or_else(|| editor.doc.bounds());
                if let Some((lo, hi)) = bounds {
                    let center = (lo + hi) * 0.5;
                    let radius = ((hi - lo).length() * 0.5).max(0.15);
                    cam.target = center;
                    cam.distance = (radius * 2.4).clamp(0.3, 80.0);
                }
            }
            CameraRequest::Front => {
                cam.yaw = 0.0;
                cam.pitch = -0.05;
            }
            CameraRequest::Side => {
                cam.yaw = std::f32::consts::FRAC_PI_2;
                cam.pitch = -0.05;
            }
            CameraRequest::Top => {
                cam.yaw = 0.0;
                cam.pitch = -1.52;
            }
            CameraRequest::ResetIso => {
                cam.yaw = 0.65;
                cam.pitch = -0.42;
            }
        }
        cam.apply_to(&mut tf);
    }

    let mut delta = Vec2::ZERO;
    for m in motion.read() {
        delta += m.delta;
    }
    let mut scroll = 0.0f32;
    for w in wheel.read() {
        scroll += match w.unit {
            MouseScrollUnit::Line => w.y,
            MouseScrollUnit::Pixel => w.y / 40.0,
        };
    }

    let Ok(window) = window_q.single() else { return };
    let over_view = window
        .cursor_position()
        .is_some_and(|c| in_view(window, c, &editor));

    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let rmb = mouse.pressed(MouseButton::Right);
    let mmb = mouse.pressed(MouseButton::Middle);

    if (rmb || mmb) && editor.modal.is_none() {
        if shift {
            // Pan target parallel to camera screen plane
            let right = tf.right().as_vec3();
            let up = tf.up().as_vec3();
            let scale = cam.distance * 0.0018;
            cam.target -= right * delta.x * scale;
            cam.target += up * delta.y * scale;
        } else {
            // Smooth orbit around target
            cam.yaw -= delta.x * 0.0065;
            cam.pitch = (cam.pitch - delta.y * 0.0065).clamp(-1.54, 1.54);
        }

        // Right-mouse + WASD / Q / E also pans/flies the target (familiar from map-editor!)
        if rmb {
            let speed = (cam.distance * 0.9).clamp(0.8, 18.0) * time.delta_secs();
            let fwd = tf.forward().as_vec3();
            let right = tf.right().as_vec3();
            let mut move_v = Vec3::ZERO;
            if keys.pressed(KeyCode::KeyW) { move_v += fwd; }
            if keys.pressed(KeyCode::KeyS) { move_v -= fwd; }
            if keys.pressed(KeyCode::KeyD) { move_v += right; }
            if keys.pressed(KeyCode::KeyA) { move_v -= right; }
            if keys.pressed(KeyCode::KeyE) { move_v += Vec3::Y; }
            if keys.pressed(KeyCode::KeyQ) { move_v -= Vec3::Y; }
            if move_v != Vec3::ZERO {
                cam.target += move_v.normalize() * speed;
            }
        }
    }

    if over_view && scroll.abs() > 1e-4 {
        let factor = (0.88f32).powf(scroll);
        cam.distance = (cam.distance * factor).clamp(0.12, 120.0);
    }

    cam.apply_to(&mut tf);
}

#[allow(clippy::too_many_arguments)]
fn viewport_interact(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut editor: ResMut<Editor>,
    mut interact: ResMut<ViewportInteract>,
    gizmo_state: Res<GizmoState>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    cam_q: Query<(&Camera, &GlobalTransform)>,
) {
    if !mouse.pressed(MouseButton::Left) {
        interact.painting = false;
        interact.paint_part = None;
        interact.sculpting = false;
        interact.sculpt_part = None;
    }

    if editor.modal.is_some() || gizmo_state.busy() {
        interact.hover = None;
        return;
    }

    let (Ok(window), Ok(cam)) = (window_q.single(), cam_q.single()) else {
        return;
    };
    let Some(ray) = cursor_ray(window, &editor, cam) else {
        interact.hover = None;
        return;
    };

    let hit = editor.doc.raycast(ray.origin, ray.direction.as_vec3());
    interact.hover = hit;

    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    match editor.mode {
        EditorMode::Object | EditorMode::Material => {
            if mouse.just_pressed(MouseButton::Left) {
                editor.focus = None;
                editor.select_part(hit.map(|(i, _)| i));
            }
        }
        EditorMode::Mesh => match editor.mesh_tool {
            MeshTool::Select(sub) => {
                if mouse.just_pressed(MouseButton::Left) {
                    editor.focus = None;
                    let Some((pi, rh)) = hit else {
                        if !shift {
                            editor.mesh_sel.clear();
                            editor.bump_ui();
                        }
                        return;
                    };
                    if editor.selected_part != Some(pi) {
                        editor.select_part(Some(pi));
                    }
                    let ed = &mut *editor;
                    let part = &ed.doc.parts[pi];
                    if !shift {
                        ed.mesh_sel.clear();
                    }
                    match sub {
                        SubSelMode::Vertex => {
                            let picked = mesh_ops::pick_vertex_on_tri(part, rh.tri_index, rh.local_pos);
                            for v in picked {
                                if shift && ed.mesh_sel.verts.contains(&v) {
                                    ed.mesh_sel.verts.remove(&v);
                                } else {
                                    ed.mesh_sel.verts.insert(v);
                                }
                            }
                        }
                        SubSelMode::Edge => {
                            let picked = mesh_ops::pick_edge_on_tri(part, rh.tri_index, rh.local_pos);
                            for v in picked {
                                ed.mesh_sel.verts.insert(v);
                            }
                        }
                        SubSelMode::Face => {
                            let faces = if alt {
                                [rh.tri_index].into_iter().collect()
                            } else {
                                mesh_ops::select_coplanar_faces(part, rh.tri_index)
                            };
                            for f in faces {
                                if shift && ed.mesh_sel.faces.contains(&f) {
                                    ed.mesh_sel.faces.remove(&f);
                                } else {
                                    ed.mesh_sel.faces.insert(f);
                                }
                            }
                        }
                    }
                    ed.bump_ui();
                }
            }
            MeshTool::Sculpt(sculpt_tool) => {
                if mouse.just_pressed(MouseButton::Left)
                    && let Some((pi, _)) = hit
                {
                    editor.focus = None;
                    if editor.selected_part != Some(pi) {
                        editor.select_part(Some(pi));
                    }
                    editor.snapshot();
                    interact.sculpting = true;
                    interact.sculpt_part = Some(pi);
                }
                if interact.sculpting
                    && mouse.pressed(MouseButton::Left)
                    && let Some(pi) = interact.sculpt_part
                    && let Some((hpi, rh)) = hit
                    && hpi == pi
                {
                    let scale_avg = Vec3::from_array(editor.doc.parts[pi].scale).abs().max_element().max(1e-4);
                    let local_r = editor.sculpt_radius / scale_avg;
                    let strength = editor.sculpt_strength;
                    let mirror = editor.sculpt_mirror_x;
                    mesh_ops::apply_sculpt_brush(
                        &mut editor.doc.parts[pi],
                        rh.local_pos,
                        rh.local_normal,
                        local_r,
                        strength,
                        sculpt_tool,
                        mirror,
                    );
                    editor.dirty = true;
                }
            }
        },
        EditorMode::Paint => {
            if mouse.just_pressed(MouseButton::Left) {
                editor.focus = None;
                let Some((pi, rh)) = hit else {
                    return;
                };
                if editor.selected_part != Some(pi) {
                    editor.select_part(Some(pi));
                }

                // Alt+Click or Eyedropper picks color straight from the 3D surface!
                if alt || editor.brush.tool == PaintTool::Eyedropper {
                    let mat_idx = editor.doc.parts[pi].material;
                    let sampled = if let Some(uv) = rh.uv
                        && let Some(ti) = editor.doc.materials.get(mat_idx).and_then(|m| m.base_color_texture)
                        && let Some(tex) = editor.doc.textures.get(ti)
                    {
                        u8_to_srgb_f32(tex.sample_uv(uv))
                    } else if let Some(mat) = editor.doc.materials.get(mat_idx) {
                        mat.base_color_srgb()
                    } else {
                        [1.0; 4]
                    };
                    editor.set_brush_color(sampled);
                    editor.status = format!("Picked color {}.", editor.hex_input);
                    if editor.brush.tool == PaintTool::Eyedropper {
                        editor.brush.tool = PaintTool::Brush;
                    }
                    editor.bump_ui();
                    return;
                }

                // One undo snapshot for the entire paint stroke or fill!
                editor.snapshot();
                let tex_size = editor.brush.new_tex_size;
                let Some((ti, created_tex, created_uvs)) = editor.doc.ensure_paintable_texture(pi, tex_size) else {
                    return;
                };
                if created_tex || created_uvs {
                    let part_name = editor.doc.parts[pi].name.clone();
                    editor.status = match (created_tex, created_uvs) {
                        (true, true) => format!("Created {tex_size}×{tex_size} paint texture & auto-UVs for '{part_name}'."),
                        (true, false) => format!("Created {tex_size}×{tex_size} paint texture for '{part_name}'."),
                        (false, true) => format!("Generated auto-UVs for '{part_name}'."),
                        _ => editor.status.clone(),
                    };
                    editor.bump_scene();
                }

                // Re-raycast if UVs were just created so UV coordinates are populated!
                let rh = if created_uvs {
                    editor.doc.parts[pi]
                        .raycast(ray.origin, ray.direction.as_vec3())
                        .unwrap_or(rh)
                } else {
                    rh
                };

                match editor.brush.tool {
                    PaintTool::FillAll => {
                        let rgba = srgb_f32_to_u8(editor.brush.color);
                        paint::fill_texture(&mut editor.doc.textures[ti], rgba);
                        editor.status = "Filled part texture with brush color.".into();
                        editor.bump_ui();
                    }
                    PaintTool::FloodFill => {
                        if let Some(uv) = rh.uv {
                            let rgba = srgb_f32_to_u8(editor.brush.color);
                            paint::flood_fill_uv(&mut editor.doc.textures[ti], uv, rgba, 28);
                            editor.status = "Bucket-filled region.".into();
                            editor.bump_ui();
                        }
                    }
                    PaintTool::Brush | PaintTool::Eraser => {
                        interact.painting = true;
                        interact.paint_part = Some(pi);
                        let start_local = if shift {
                            interact.last_paint_local.unwrap_or(rh.local_pos)
                        } else {
                            rh.local_pos
                        };
                        interact.stroke_seed = interact.stroke_seed.wrapping_add(1);
                        let seed = interact.stroke_seed;
                        let brush = editor.brush;
                        let scale_avg = Vec3::from_array(editor.doc.parts[pi].scale).abs().max_element().max(1e-4);
                        let local_r = brush.radius / scale_avg;
                        let doc = &mut editor.doc;
                        let part = &doc.parts[pi];
                        let tex = &mut doc.textures[ti];
                        paint::paint_stroke_3d(
                            part,
                            tex,
                            start_local,
                            rh.local_pos,
                            rh.local_normal,
                            local_r,
                            &brush,
                            seed,
                        );
                        interact.last_paint_local = Some(rh.local_pos);
                    }
                    PaintTool::Eyedropper => {}
                }
            } else if interact.painting
                && mouse.pressed(MouseButton::Left)
                && let Some(pi) = interact.paint_part
                && let Some((hpi, rh)) = hit
                && hpi == pi
            {
                let mat_idx = editor.doc.parts[pi].material;
                if let Some(ti) = editor.doc.materials.get(mat_idx).and_then(|m| m.base_color_texture)
                    && ti < editor.doc.textures.len()
                {
                    let from = interact.last_paint_local.unwrap_or(rh.local_pos);
                    let brush = editor.brush;
                    let scale_avg = Vec3::from_array(editor.doc.parts[pi].scale).abs().max_element().max(1e-4);
                    let local_r = brush.radius / scale_avg;
                    if from.distance(rh.local_pos) > local_r * 0.08 {
                        interact.stroke_seed = interact.stroke_seed.wrapping_add(1);
                        let seed = interact.stroke_seed;
                        let doc = &mut editor.doc;
                        let part = &doc.parts[pi];
                        let tex = &mut doc.textures[ti];
                        paint::paint_stroke_3d(
                            part,
                            tex,
                            from,
                            rh.local_pos,
                            rh.local_normal,
                            local_r,
                            &brush,
                            seed,
                        );
                        interact.last_paint_local = Some(rh.local_pos);
                    }
                }
            }
        }
    }
}

fn draw_overlay(
    editor: Res<Editor>,
    interact: Res<ViewportInteract>,
    gizmo_state: Res<GizmoState>,
    cam_q: Query<&GlobalTransform, With<Camera3d>>,
    mut gizmos: Gizmos,
) {
    // 1. Ground studio grid + origin axes
    if editor.show_grid {
        let half = 6;
        for i in -half..=half {
            let f = i as f32;
            let c = if i == 0 {
                continue;
            } else if i % 5 == 0 {
                Color::srgba(1.0, 1.0, 1.0, 0.22)
            } else {
                Color::srgba(1.0, 1.0, 1.0, 0.09)
            };
            gizmos.line(Vec3::new(f, 0.0, -half as f32), Vec3::new(f, 0.0, half as f32), c);
            gizmos.line(Vec3::new(-half as f32, 0.0, f), Vec3::new(half as f32, 0.0, f), c);
        }
        // X axis (red) & Z axis (blue)
        gizmos.line(
            Vec3::new(-half as f32, 0.002, 0.0),
            Vec3::new(half as f32, 0.002, 0.0),
            Color::srgba(0.95, 0.28, 0.28, 0.7),
        );
        gizmos.line(
            Vec3::new(0.0, 0.002, -half as f32),
            Vec3::new(0.0, 0.002, half as f32),
            Color::srgba(0.28, 0.56, 1.0, 0.7),
        );
    }

    // 2. Wireframe overlay
    if editor.show_wireframe {
        for (pi, part) in editor.doc.parts.iter().enumerate() {
            if !part.visible {
                continue;
            }
            let selected = editor.selected_part == Some(pi);
            // Keep framerate smooth on very dense meshes by drawing wireframe on selected or moderate parts
            if !selected && part.tri_count() > 8000 {
                continue;
            }
            let color = if selected {
                Color::srgba(0.35, 0.85, 1.0, 0.55)
            } else {
                Color::srgba(0.85, 0.90, 0.96, 0.22)
            };
            let m = part.model_matrix();
            for tri in part.indices.chunks_exact(3).take(16_000) {
                let p0 = m.transform_point3(Vec3::from_array(part.positions[tri[0] as usize]));
                let p1 = m.transform_point3(Vec3::from_array(part.positions[tri[1] as usize]));
                let p2 = m.transform_point3(Vec3::from_array(part.positions[tri[2] as usize]));
                gizmos.line(p0, p1, color);
                gizmos.line(p1, p2, color);
                gizmos.line(p2, p0, color);
            }
        }
    }

    // 3. Selection bounding box (Object / Material modes)
    if matches!(editor.mode, EditorMode::Object | EditorMode::Material)
        && let Some(pi) = editor.selected_part
        && let Some(part) = editor.doc.parts.get(pi)
        && let Some((lo, hi)) = part.world_bounds()
    {
        gizmo::draw_wire_box(&mut gizmos, lo, hi, Color::srgba(1.0, 0.85, 0.2, 0.85));
    }

    // 4. Mesh Edit Mode sub-selection highlights
    if editor.mode == EditorMode::Mesh
        && let Some(pi) = editor.selected_part
        && let Some(part) = editor.doc.parts.get(pi)
    {
        let m = part.model_matrix();
        let face_col = Color::srgb(1.0, 0.65, 0.15);
        for &f in &editor.mesh_sel.faces {
            if f * 3 + 2 < part.indices.len() {
                let p0 = m.transform_point3(Vec3::from_array(part.positions[part.indices[f * 3] as usize]));
                let p1 = m.transform_point3(Vec3::from_array(part.positions[part.indices[f * 3 + 1] as usize]));
                let p2 = m.transform_point3(Vec3::from_array(part.positions[part.indices[f * 3 + 2] as usize]));
                gizmos.line(p0, p1, face_col);
                gizmos.line(p1, p2, face_col);
                gizmos.line(p2, p0, face_col);
            }
        }
        let v_col = Color::srgb(1.0, 0.92, 0.2);
        let d = 0.025;
        for &vi in &editor.mesh_sel.verts {
            if let Some(&lp) = part.positions.get(vi) {
                let p = m.transform_point3(Vec3::from_array(lp));
                gizmos.line(p - Vec3::X * d, p + Vec3::X * d, v_col);
                gizmos.line(p - Vec3::Y * d, p + Vec3::Y * d, v_col);
                gizmos.line(p - Vec3::Z * d, p + Vec3::Z * d, v_col);
            }
        }
    }

    // 5. Transform gizmo handles
    if let Ok(cam) = cam_q.single() {
        gizmo::draw(&editor, &gizmo_state, cam, &mut gizmos);
    }

    // 6. Live 3D surface-aligned brush ring in Paint & Sculpt modes
    let brush_preview = match editor.mode {
        EditorMode::Paint => Some((
            editor.brush.radius,
            editor.brush.mirror_x,
            Color::srgb(editor.brush.color[0], editor.brush.color[1], editor.brush.color[2]),
        )),
        EditorMode::Mesh if matches!(editor.mesh_tool, MeshTool::Sculpt(_)) => Some((
            editor.sculpt_radius,
            editor.sculpt_mirror_x,
            Color::srgb(0.30, 0.90, 1.0),
        )),
        _ => None,
    };
    if let (Some((radius, mirror, col)), Some((_, rh))) = (brush_preview, interact.hover) {
        draw_surface_ring(&mut gizmos, rh.world_pos + rh.world_normal * 0.004, rh.world_normal, radius, col);
        if mirror {
            let mp = Vec3::new(-rh.world_pos.x, rh.world_pos.y, rh.world_pos.z);
            let mn = Vec3::new(-rh.world_normal.x, rh.world_normal.y, rh.world_normal.z);
            draw_surface_ring(&mut gizmos, mp + mn * 0.004, mn, radius, Color::srgba(1.0, 1.0, 1.0, 0.6));
        }
    }
}

fn draw_surface_ring(gizmos: &mut Gizmos, center: Vec3, normal: Vec3, r: f32, color: Color) {
    let n = if normal.length_squared() > 1e-6 { normal.normalize() } else { Vec3::Y };
    let rot = Quat::from_rotation_arc(Vec3::Y, n);
    let segs = 28;
    for i in 0..segs {
        let a = i as f32 / segs as f32 * std::f32::consts::TAU;
        let b = (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let p0 = center + rot * Vec3::new(a.cos() * r, 0.0, a.sin() * r);
        let p1 = center + rot * Vec3::new(b.cos() * r, 0.0, b.sin() * r);
        gizmos.line(p0, p1, color);
    }
}
