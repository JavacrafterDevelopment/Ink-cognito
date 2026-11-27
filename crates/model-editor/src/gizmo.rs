//! Interactive 3D transform handles (Move arrows, Rotate rings, Scale handles)
//! working in both Object Mode (transforming a whole part) and Mesh Edit Mode
//! (translating selected vertices, edges, or faces).

use crate::{
    mesh_ops,
    state::{Editor, EditorMode, GizmoMode, MeshTool},
    view,
};
use bevy::{prelude::*, window::PrimaryWindow};

pub struct GizmoPlugin;

impl Plugin for GizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GizmoState>()
            .add_systems(Startup, draw_on_top);
    }
}

/// Overlay gizmos render on top of geometry so handles are never obscured.
fn draw_on_top(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    config.depth_bias = -1.0;
}

pub const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub axis: usize,
}

#[derive(Clone, Debug)]
enum GrabTarget {
    Part {
        part_idx: usize,
        start_pos: Vec3,
        start_rot: Quat,
        start_scale: Vec3,
    },
    MeshVerts {
        part_idx: usize,
        start_positions: Vec<(usize, [f32; 3])>,
    },
}

#[derive(Clone, Debug)]
struct Grab {
    handle: Handle,
    mode: GizmoMode,
    origin: Vec3,
    start_s: f32,
    start_dir: Vec3,
    moved: bool,
    target: GrabTarget,
}

#[derive(Resource, Default)]
pub struct GizmoState {
    pub hover: Option<Handle>,
    grab: Option<Grab>,
}

impl GizmoState {
    pub fn busy(&self) -> bool {
        self.grab.is_some()
    }
}

pub fn axis_color(axis: usize, hot: bool) -> Color {
    let base = match axis {
        0 => Vec3::new(0.95, 0.25, 0.25),
        1 => Vec3::new(0.28, 0.90, 0.34),
        _ => Vec3::new(0.28, 0.54, 1.0),
    };
    let c = if hot { base.lerp(Vec3::ONE, 0.58) } else { base };
    Color::srgb(c.x, c.y, c.z)
}

/// Where the transform gizmo sits and which gizmo mode applies right now.
pub fn active_gizmo(editor: &Editor) -> Option<(Vec3, GizmoMode)> {
    let pi = editor.selected_part?;
    let part = editor.doc.parts.get(pi)?;
    match editor.mode {
        EditorMode::Object => {
            let center = part
                .world_bounds()
                .map_or(Vec3::from_array(part.translation), |(lo, hi)| (lo + hi) * 0.5);
            Some((center, editor.gizmo))
        }
        EditorMode::Mesh => {
            if !matches!(editor.mesh_tool, MeshTool::Select(_)) || editor.mesh_sel.is_empty() {
                return None;
            }
            let center = editor.mesh_sel.world_centroid(part)?;
            Some((center, GizmoMode::Move))
        }
        _ => None,
    }
}

pub fn gizmo_size(cam: &GlobalTransform, at: Vec3) -> f32 {
    (cam.translation().distance(at) * 0.16).clamp(0.15, 8.0)
}

fn ring_points(center: Vec3, axis: Vec3, r: f32, n: usize) -> Vec<Vec3> {
    let rot = Quat::from_rotation_arc(Vec3::Y, axis);
    (0..=n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            center + rot * Vec3::new(a.cos() * r, 0.0, a.sin() * r)
        })
        .collect()
}

fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn handle_at(editor: &Editor, cam: (&Camera, &GlobalTransform), cursor: Vec2) -> Option<Handle> {
    let (center, mode) = active_gizmo(editor)?;
    let len = gizmo_size(cam.1, center);
    let screen = |p: Vec3| cam.0.world_to_viewport(cam.1, p).ok();
    let mut best: Option<(f32, Handle)> = None;
    for axis in 0..3 {
        let d = match mode {
            GizmoMode::Move | GizmoMode::Scale => {
                let (Some(a), Some(b)) = (screen(center), screen(center + AXES[axis] * len)) else {
                    continue;
                };
                dist_to_segment(cursor, a, b)
            }
            GizmoMode::Rotate => {
                let pts: Vec<Vec2> = ring_points(center, AXES[axis], len, 48)
                    .into_iter()
                    .filter_map(screen)
                    .collect();
                pts.windows(2)
                    .map(|w| dist_to_segment(cursor, w[0], w[1]))
                    .fold(f32::MAX, f32::min)
            }
        };
        if d < 10.0 && best.is_none_or(|b| d < b.0) {
            best = Some((d, Handle { axis }));
        }
    }
    best.map(|b| b.1)
}

fn along_axis(o: Vec3, a: Vec3, ray: Ray3d) -> Option<f32> {
    let d = ray.direction.as_vec3();
    let w = o - ray.origin;
    let b = a.dot(d);
    let denom = 1.0 - b * b;
    if denom < 1e-4 {
        return None;
    }
    Some((b * d.dot(w) - a.dot(w)) / denom)
}

fn on_plane(o: Vec3, a: Vec3, ray: Ray3d) -> Option<Vec3> {
    let d = ray.direction.as_vec3();
    let denom = d.dot(a);
    if denom.abs() < 1e-3 {
        return None;
    }
    let t = (o - ray.origin).dot(a) / denom;
    (t > 0.0)
        .then(|| (ray.origin + d * t - o).normalize_or_zero())
        .filter(|v| *v != Vec3::ZERO)
}

pub fn gizmo_input(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut editor: ResMut<Editor>,
    mut state: ResMut<GizmoState>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    cam_q: Query<(&Camera, &GlobalTransform)>,
) {
    if editor.modal.is_some() {
        state.grab = None;
        state.hover = None;
        return;
    }
    if mouse.just_released(MouseButton::Left) {
        state.grab = None;
    }
    let (Ok(window), Ok(cam)) = (window_q.single(), cam_q.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        state.hover = None;
        return;
    };
    let ray = view::cursor_ray(window, &editor, cam);
    let snap = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if let Some(mut grab) = state.grab.clone() {
        if !mouse.pressed(MouseButton::Left) {
            state.grab = None;
            return;
        }
        let Some(ray) = ray else { return };
        let axis = AXES[grab.handle.axis];

        match &grab.target {
            GrabTarget::Part {
                part_idx,
                start_pos,
                start_rot,
                start_scale,
            } => {
                let pi = *part_idx;
                if pi >= editor.doc.parts.len() {
                    state.grab = None;
                    return;
                }
                match grab.mode {
                    GizmoMode::Move => {
                        let Some(s) = along_axis(grab.origin, axis, ray) else { return };
                        let mut delta = s - grab.start_s;
                        if snap {
                            delta = (delta / 0.1).round() * 0.1;
                        }
                        let to = *start_pos + axis * delta;
                        if Vec3::from_array(editor.doc.parts[pi].translation).distance(to) > 1e-5 {
                            if !grab.moved {
                                editor.snapshot();
                                grab.moved = true;
                            }
                            editor.doc.parts[pi].translation = to.to_array();
                            editor.dirty = true;
                            editor.bump_ui();
                        }
                    }
                    GizmoMode::Rotate => {
                        let Some(dir) = on_plane(grab.origin, axis, ray) else { return };
                        let mut angle = grab.start_dir.cross(dir).dot(axis).atan2(grab.start_dir.dot(dir));
                        if snap {
                            let step = 15f32.to_radians();
                            angle = (angle / step).round() * step;
                        }
                        let rot = Quat::from_axis_angle(axis, angle) * *start_rot;
                        if editor.doc.parts[pi].quat().angle_between(rot) > 1e-4 {
                            if !grab.moved {
                                editor.snapshot();
                                grab.moved = true;
                            }
                            editor.doc.parts[pi].set_quat(rot);
                            editor.dirty = true;
                            editor.bump_ui();
                        }
                    }
                    GizmoMode::Scale => {
                        let Some(s) = along_axis(grab.origin, axis, ray) else { return };
                        let len = gizmo_size(cam.1, grab.origin).max(0.1);
                        let factor = (1.0 + (s - grab.start_s) / len).clamp(0.05, 20.0);
                        let factor = if snap { (factor / 0.1).round() * 0.1 } else { factor };
                        let mut next = *start_scale;
                        next[grab.handle.axis] = (start_scale[grab.handle.axis] * factor).clamp(0.01, 50.0);
                        if Vec3::from_array(editor.doc.parts[pi].scale).distance(next) > 1e-5 {
                            if !grab.moved {
                                editor.snapshot();
                                grab.moved = true;
                            }
                            editor.doc.parts[pi].scale = next.to_array();
                            editor.dirty = true;
                            editor.bump_ui();
                        }
                    }
                }
            }
            GrabTarget::MeshVerts {
                part_idx,
                start_positions,
                ..
            } => {
                let pi = *part_idx;
                if pi >= editor.doc.parts.len() {
                    state.grab = None;
                    return;
                }
                let Some(s) = along_axis(grab.origin, axis, ray) else { return };
                let mut delta = s - grab.start_s;
                if snap {
                    delta = (delta / 0.05).round() * 0.05;
                }
                let world_delta = axis * delta;
                let inv = editor.doc.parts[pi].inv_model_matrix();
                let local_delta = inv.transform_vector3(world_delta);
                if local_delta.length() > 1e-5 {
                    if !grab.moved {
                        editor.snapshot();
                        grab.moved = true;
                    }
                    let part = &mut editor.doc.parts[pi];
                    for &(vi, sp) in start_positions {
                        if let Some(p) = part.positions.get_mut(vi) {
                            *p = [sp[0] + local_delta.x, sp[1] + local_delta.y, sp[2] + local_delta.z];
                        }
                    }
                    part.recompute_normals_smooth();
                    editor.dirty = true;
                }
            }
        }
        state.grab = Some(grab);
        return;
    }

    let over = ray.is_some().then(|| handle_at(&editor, cam, cursor)).flatten();
    if state.hover != over {
        state.hover = over;
    }

    if let Some(handle) = over
        && mouse.just_pressed(MouseButton::Left)
        && let Some(ray) = ray
        && let Some((center, mode)) = active_gizmo(&editor)
        && let Some(pi) = editor.selected_part
        && let Some(part) = editor.doc.parts.get(pi)
    {
        let axis = AXES[handle.axis];
        let (start_s, start_dir) = match mode {
            GizmoMode::Move | GizmoMode::Scale => (along_axis(center, axis, ray).unwrap_or(0.0), Vec3::ZERO),
            GizmoMode::Rotate => (0.0, on_plane(center, axis, ray).unwrap_or(Vec3::X)),
        };
        let target = match editor.mode {
            EditorMode::Object => GrabTarget::Part {
                part_idx: pi,
                start_pos: Vec3::from_array(part.translation),
                start_rot: part.quat(),
                start_scale: Vec3::from_array(part.scale),
            },
            EditorMode::Mesh => {
                let verts = editor.mesh_sel.effective_verts(part);
                let expanded = mesh_ops::expand_coincident(part, &verts);
                let start_positions = expanded
                    .iter()
                    .filter_map(|&vi| part.positions.get(vi).copied().map(|p| (vi, p)))
                    .collect();
                GrabTarget::MeshVerts {
                    part_idx: pi,
                    start_positions,
                }
            }
            _ => return,
        };
        editor.focus = None;
        state.grab = Some(Grab {
            handle,
            mode,
            origin: center,
            start_s,
            start_dir,
            moved: false,
            target,
        });
    }
}

pub fn draw(editor: &Editor, state: &GizmoState, cam: &GlobalTransform, gizmos: &mut Gizmos) {
    let Some((center, mode)) = active_gizmo(editor) else {
        return;
    };
    let len = gizmo_size(cam, center);
    let held = state.grab.as_ref().map(|g| g.handle);
    for axis in 0..3 {
        let hot = state.hover == Some(Handle { axis }) || held == Some(Handle { axis });
        let color = axis_color(axis, hot);
        match mode {
            GizmoMode::Move => {
                gizmos
                    .arrow(center, center + AXES[axis] * len, color)
                    .with_tip_length(len * 0.22);
            }
            GizmoMode::Rotate => {
                let pts = ring_points(center, AXES[axis], len, 48);
                for w in pts.windows(2) {
                    gizmos.line(w[0], w[1], color);
                }
            }
            GizmoMode::Scale => {
                let tip = center + AXES[axis] * len;
                gizmos.line(center, tip, color);
                // Small cube handle at the tip
                let s = len * 0.08;
                let lo = tip - Vec3::splat(s);
                let hi = tip + Vec3::splat(s);
                draw_wire_box(gizmos, lo, hi, color);
            }
        }
    }
}

pub fn draw_wire_box(gizmos: &mut Gizmos, lo: Vec3, hi: Vec3, color: Color) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { hi.x } else { lo.x },
            if y { hi.y } else { lo.y },
            if z { hi.z } else { lo.z },
        )
    };
    for &(a, b) in &[
        ((false, false, false), (true, false, false)),
        ((false, false, true), (true, false, true)),
        ((false, true, false), (true, true, false)),
        ((false, true, true), (true, true, true)),
        ((false, false, false), (false, false, true)),
        ((true, false, false), (true, false, true)),
        ((false, true, false), (false, true, true)),
        ((true, true, false), (true, true, true)),
        ((false, false, false), (false, true, false)),
        ((true, false, false), (true, true, false)),
        ((false, false, true), (false, true, true)),
        ((true, false, true), (true, true, true)),
    ] {
        gizmos.line(c(a.0, a.1, a.2), c(b.0, b.1, b.2), color);
    }
}
