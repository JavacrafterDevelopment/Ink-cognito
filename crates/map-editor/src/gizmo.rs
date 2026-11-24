//! Handles on the selection, like other game editors: three arrows (red X,
//! green Y, blue Z) to drag it along one axis, or three rings to turn it
//! around one. 1 picks the arrows, 2 the rings.

use crate::{
    state::{Editor, GizmoMode, Sel, Tool},
    view,
};
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

pub struct GizmoPlugin;

impl Plugin for GizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GizmoState>()
            .add_systems(Startup, draw_on_top);
    }
}

/// The overlay lines (handles, selection, spawns) show through pieces and the
/// ground, so a handle is never hidden.
fn draw_on_top(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    config.depth_bias = -1.0;
}

/// One handle: which axis (0 x, 1 y, 2 z).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub axis: usize,
}

/// A drag on a handle in progress.
#[derive(Clone, Copy, Debug)]
struct Grab {
    handle: Handle,
    mode: GizmoMode,
    origin: Vec3,
    start_pos: Vec3,
    start_rot: Quat,
    /// Move: where along the axis the grab started. Rotate: the start direction.
    start_s: f32,
    start_dir: Vec3,
    moved: bool,
}

#[derive(Resource, Default)]
pub struct GizmoState {
    pub hover: Option<Handle>,
    grab: Option<Grab>,
}

impl GizmoState {
    /// Whether a handle is being dragged (the view leaves the mouse alone).
    pub fn busy(&self) -> bool {
        self.grab.is_some()
    }
}

pub const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

pub fn axis_color(axis: usize, hot: bool) -> Color {
    let base = match axis {
        0 => Vec3::new(0.95, 0.25, 0.25),
        1 => Vec3::new(0.3, 0.9, 0.3),
        _ => Vec3::new(0.3, 0.5, 1.0),
    };
    let c = if hot { base.lerp(Vec3::ONE, 0.6) } else { base };
    Color::srgb(c.x, c.y, c.z)
}

/// Where the handles sit, and which modes apply (spawns only move flat).
pub fn origin(editor: &Editor) -> Option<(Vec3, bool)> {
    if editor.tool != Tool::Select {
        return None;
    }
    match editor.sel? {
        Sel::Object(i) => {
            let o = editor.map.objects.get(i)?;
            // The middle of the piece, so the handles don't sink into the ground.
            let center = view::object_bounds(o).map_or(Vec3::from_array(o.pos), |(lo, hi)| (lo + hi) * 0.5);
            Some((center, true))
        }
        Sel::Spawn(..) => editor.sel_pos().map(|p| (p + Vec3::Y * 0.9, false)),
    }
}

/// Handle size: about the same on screen however far away.
pub fn size(cam: &GlobalTransform, at: Vec3) -> f32 {
    (cam.translation().distance(at) * 0.13).clamp(0.6, 12.0)
}

/// The axes a selection can use.
fn axes_for(full: bool, mode: GizmoMode) -> &'static [usize] {
    match (full, mode) {
        (true, _) => &[0, 1, 2],
        (false, GizmoMode::Move) => &[0, 2],
        (false, GizmoMode::Rotate) => &[],
    }
}

/// Points along a ring around `axis`.
pub fn ring_points(center: Vec3, axis: Vec3, r: f32, n: usize) -> Vec<Vec3> {
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

/// The handle under the cursor (within a few pixels), if any.
fn handle_at(editor: &Editor, cam: (&Camera, &GlobalTransform), cursor: Vec2) -> Option<Handle> {
    let (center, full) = origin(editor)?;
    let len = size(cam.1, center);
    let screen = |p: Vec3| cam.0.world_to_viewport(cam.1, p).ok();
    let mut best: Option<(f32, Handle)> = None;
    for &axis in axes_for(full, editor.gizmo) {
        let d = match editor.gizmo {
            GizmoMode::Move => {
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
        if d < 9.0 && best.is_none_or(|b| d < b.0) {
            best = Some((d, Handle { axis }));
        }
    }
    best.map(|b| b.1)
}

/// Where along the line `o + s * a` comes closest to the ray.
fn along_axis(o: Vec3, a: Vec3, ray: Ray3d) -> Option<f32> {
    let d = ray.direction.as_vec3();
    let w = o - ray.origin;
    let b = a.dot(d);
    let denom = 1.0 - b * b;
    if denom < 1e-4 {
        // Looking straight down the axis.
        return None;
    }
    Some((b * d.dot(w) - a.dot(w)) / denom)
}

/// Where the ray meets the plane through `o` facing `a`, as a direction from `o`.
fn on_plane(o: Vec3, a: Vec3, ray: Ray3d) -> Option<Vec3> {
    let d = ray.direction.as_vec3();
    let denom = d.dot(a);
    if denom.abs() < 1e-3 {
        return None;
    }
    let t = (o - ray.origin).dot(a) / denom;
    (t > 0.0).then(|| (ray.origin + d * t - o).normalize_or_zero()).filter(|v| *v != Vec3::ZERO)
}

/// Hovering, grabbing and dragging the handles. Runs before the view's own
/// clicks, which stand aside while a handle is held.
pub fn gizmo_input(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut editor: ResMut<Editor>,
    mut state: ResMut<GizmoState>,
    window_q: Query<(&Window, &CursorOptions), With<PrimaryWindow>>,
    cam_q: Query<(&Camera, &GlobalTransform)>,
) {
    if editor.focus.is_none() {
        if keys.just_pressed(KeyCode::Digit1) {
            editor.gizmo = GizmoMode::Move;
        }
        if keys.just_pressed(KeyCode::Digit2) {
            editor.gizmo = GizmoMode::Rotate;
        }
    }
    if mouse.just_released(MouseButton::Left) {
        state.grab = None;
    }
    let (Ok((window, cursor_opts)), Ok(cam)) = (window_q.single(), cam_q.single()) else {
        return;
    };
    let flying = cursor_opts.grab_mode != CursorGrabMode::None;
    let Some(cursor) = window.cursor_position().filter(|_| !flying) else {
        state.hover = None;
        return;
    };
    let ray = view::cursor_ray(window, &editor, cam);

    if let Some(mut grab) = state.grab {
        if !mouse.pressed(MouseButton::Left) {
            state.grab = None;
            return;
        }
        let Some(ray) = ray else { return };
        let axis = AXES[grab.handle.axis];
        match grab.mode {
            GizmoMode::Move => {
                let Some(s) = along_axis(grab.origin, axis, ray) else { return };
                let mut delta = s - grab.start_s;
                let snap = editor.snap_value();
                if snap > 0.0 {
                    delta = (delta / snap).round() * snap;
                }
                let to = grab.start_pos + axis * delta;
                if editor.sel_pos().is_some_and(|p| p.distance(to) > 1e-5) {
                    if !grab.moved {
                        editor.snapshot();
                        grab.moved = true;
                    }
                    editor.move_sel_to3(to);
                }
            }
            GizmoMode::Rotate => {
                let Some(dir) = on_plane(grab.origin, axis, ray) else { return };
                let mut angle = grab.start_dir.cross(dir).dot(axis).atan2(grab.start_dir.dot(dir));
                let step = editor.turn_value().to_radians();
                angle = (angle / step).round() * step;
                let rot = Quat::from_axis_angle(axis, angle) * grab.start_rot;
                if editor.sel_rotation().is_some_and(|r| r.angle_between(rot) > 1e-4) {
                    if !grab.moved {
                        editor.snapshot();
                        grab.moved = true;
                    }
                    editor.set_sel_rotation(rot);
                    // Turning around a tilted piece's middle moves its base: keep the middle still.
                    if let Some(Sel::Object(i)) = editor.sel {
                        let o = &editor.map.objects[i];
                        if let Some((lo, hi)) = view::object_bounds(o) {
                            let drift = (lo + hi) * 0.5 - grab.origin;
                            let p = Vec3::from_array(o.pos) - drift;
                            editor.move_sel_to3(p);
                        }
                    }
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
        && let Some((center, _)) = origin(&editor)
    {
        let axis = AXES[handle.axis];
        let start_pos = editor.sel_pos().unwrap_or(center);
        let start_rot = editor.sel_rotation().unwrap_or_default();
        let (start_s, start_dir) = match editor.gizmo {
            GizmoMode::Move => (along_axis(center, axis, ray).unwrap_or(0.0), Vec3::ZERO),
            GizmoMode::Rotate => (0.0, on_plane(center, axis, ray).unwrap_or(Vec3::X)),
        };
        editor.focus = None;
        state.grab = Some(Grab {
            handle,
            mode: editor.gizmo,
            origin: center,
            start_pos,
            start_rot,
            start_s,
            start_dir,
            moved: false,
        });
    }
}

/// Draws the handles (and the one under the cursor brighter).
pub fn draw(editor: &Editor, state: &GizmoState, cam: &GlobalTransform, gizmos: &mut Gizmos) {
    let Some((center, full)) = origin(editor) else {
        return;
    };
    let len = size(cam, center);
    let held = state.grab.map(|g| g.handle);
    for &axis in axes_for(full, editor.gizmo) {
        let hot = state.hover == Some(Handle { axis }) || held == Some(Handle { axis });
        let color = axis_color(axis, hot);
        match editor.gizmo {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_point_on_an_axis() {
        // A ray straight down onto x = 3 meets the x axis line at s = 3.
        let ray = Ray3d::new(Vec3::new(3.0, 10.0, 2.0), Dir3::NEG_Y);
        let s = along_axis(Vec3::ZERO, Vec3::X, ray).unwrap();
        assert!((s - 3.0).abs() < 1e-4, "{s}");
        // Looking straight down the axis gives nothing.
        assert!(along_axis(Vec3::ZERO, Vec3::Y, ray).is_none());
    }
}
