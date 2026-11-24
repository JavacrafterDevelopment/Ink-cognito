//! The 3D view: the free-fly camera, clicking and dragging in the field,
//! keyboard shortcuts, and the overlay lines (grid, selection, spawns).

use crate::{
    files,
    scene::CursorGround,
    state::{Editor, Field, Sel, SpawnKind, Tool},
    ui,
};
use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
        mouse::{MouseMotion, MouseWheel},
    },
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use ink_map::Shape;

pub struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Drag>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (
                    text_input,
                    shortcuts,
                    fly_camera,
                    cursor_ground,
                    clicks,
                    draw_overlay,
                )
                    .chain(),
            );
    }
}

/// The editor camera: where it looks.
#[derive(Component)]
pub struct FlyCam {
    yaw: f32,
    pitch: f32,
}

/// A drag in progress: the selection moves with the cursor, keeping this
/// offset from it. The bool: it has moved (and so taken its undo step).
#[derive(Resource, Default)]
struct Drag(Option<(Vec2, bool)>);

fn spawn_camera(mut commands: Commands) {
    let pos = Vec3::new(0.0, 32.0, 42.0);
    let t = Transform::from_translation(pos).looking_at(Vec3::ZERO, Vec3::Y);
    let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 60f32.to_radians(),
            near: 0.05,
            ..default()
        }),
        t,
        FlyCam { yaw, pitch },
    ));
}

/// Whether the cursor is over the 3D view (not a panel).
fn in_view(window: &Window, cursor: Vec2, editor: &Editor) -> bool {
    if editor.open_list.is_some() {
        return false;
    }
    let w = window.width();
    let h = window.height();
    cursor.x > ui::LEFT_W && cursor.x < w - ui::RIGHT_W && cursor.y > ui::TOP_H && cursor.y < h - ui::BOTTOM_H
}

/// Typing into the focused text box.
fn text_input(mut keys: MessageReader<KeyboardInput>, mut editor: ResMut<Editor>) {
    let Some(field) = editor.focus else {
        return;
    };
    let mut changed = false;
    let mut done = false;
    for ev in keys.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Enter | Key::Escape | Key::Tab => done = true,
            Key::Backspace => {
                editor.field_mut(field).pop();
                changed = true;
            }
            _ => {
                if let Some(text) = &ev.text {
                    let limit = if field == Field::Blurb { 160 } else { 40 };
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        if editor.field_mut(field).chars().count() < limit {
                            editor.field_mut(field).push(ch);
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    if changed && field != Field::FileName {
        // One undo step per text box visit would be nicer; this keeps it simple.
        editor.dirty = true;
    }
    if done {
        editor.focus = None;
    }
}

fn shortcuts(keys: Res<ButtonInput<KeyCode>>, mut editor: ResMut<Editor>, mouse: Res<ButtonInput<MouseButton>>) {
    if editor.focus.is_some() {
        return;
    }
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if ctrl {
        if keys.just_pressed(KeyCode::KeyS) {
            if shift {
                files::save_copy(&mut editor);
            } else {
                files::save(&mut editor);
            }
        }
        if keys.just_pressed(KeyCode::KeyZ) {
            if shift { editor.redo() } else { editor.undo() }
        }
        if keys.just_pressed(KeyCode::KeyY) {
            editor.redo();
        }
        if keys.just_pressed(KeyCode::KeyD) {
            editor.duplicate();
        }
        return;
    }
    // Movement keys belong to the camera while flying.
    if mouse.pressed(MouseButton::Right) {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        if editor.open_list.is_some() {
            editor.open_list = None;
        } else if editor.tool != Tool::Select {
            editor.tool = Tool::Select;
        } else {
            editor.sel = None;
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        editor.turn(if shift { -1.0 } else { 1.0 });
    }
    if keys.any_just_pressed([KeyCode::Delete, KeyCode::Backspace]) {
        editor.delete();
    }
    if keys.just_pressed(KeyCode::KeyM) {
        editor.mirror();
    }
    if keys.just_pressed(KeyCode::KeyG) {
        editor.snap = (editor.snap + 1) % crate::state::SNAPS.len();
    }
    if keys.just_pressed(KeyCode::PageUp) {
        editor.raise(0.25);
    }
    if keys.just_pressed(KeyCode::PageDown) {
        editor.raise(-0.25);
    }
}

/// Right mouse held: look around and fly with WASD (Q/E down/up, Shift fast).
/// The wheel moves forward and back.
#[allow(clippy::too_many_arguments)]
fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    editor: Res<Editor>,
    mut cursor_q: Query<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut cam_q: Query<(&mut Transform, &mut FlyCam)>,
) {
    let Ok((mut t, mut cam)) = cam_q.single_mut() else {
        return;
    };
    let Ok((window, mut cursor)) = cursor_q.single_mut() else {
        return;
    };
    let over_view = window.cursor_position().is_some_and(|c| in_view(window, c, &editor));
    if mouse.just_pressed(MouseButton::Right) && over_view {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    let flying = cursor.grab_mode != CursorGrabMode::None;
    if !mouse.pressed(MouseButton::Right) && flying {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
    let mut delta = Vec2::ZERO;
    for m in motion.read() {
        delta += m.delta;
    }
    if flying {
        cam.yaw -= delta.x * 0.003;
        cam.pitch = (cam.pitch - delta.y * 0.003).clamp(-1.54, 1.54);
    }
    t.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);

    let fast = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { 3.0 } else { 1.0 };
    let speed = 14.0 * fast * time.delta_secs();
    let mut go = Vec3::ZERO;
    if flying {
        let fwd = t.forward().as_vec3();
        let right = t.right().as_vec3();
        if keys.pressed(KeyCode::KeyW) {
            go += fwd;
        }
        if keys.pressed(KeyCode::KeyS) {
            go -= fwd;
        }
        if keys.pressed(KeyCode::KeyD) {
            go += right;
        }
        if keys.pressed(KeyCode::KeyA) {
            go -= right;
        }
        if keys.pressed(KeyCode::KeyE) {
            go += Vec3::Y;
        }
        if keys.pressed(KeyCode::KeyQ) {
            go -= Vec3::Y;
        }
    }
    let mut scroll = 0.0;
    for w in wheel.read() {
        scroll += match w.unit {
            bevy::input::mouse::MouseScrollUnit::Line => w.y,
            bevy::input::mouse::MouseScrollUnit::Pixel => w.y / 40.0,
        };
    }
    if over_view && scroll != 0.0 {
        let fwd = t.forward().as_vec3();
        t.translation += fwd * scroll * 2.0 * fast;
    }
    t.translation += go.normalize_or_zero() * speed;
    t.translation.y = t.translation.y.clamp(0.3, 150.0);
}

/// The cursor's ray, if it's over the 3D view.
fn cursor_ray(
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

/// Where a ray meets the ground (y = `height`).
fn ground_hit(ray: Ray3d, height: f32) -> Option<Vec2> {
    let d = ray.direction.as_vec3();
    if d.y.abs() < 1e-5 {
        return None;
    }
    let t = (height - ray.origin.y) / d.y;
    if t <= 0.0 {
        return None;
    }
    let p = ray.origin + d * t;
    Some(Vec2::new(p.x, p.z))
}

fn cursor_ground(
    editor: Res<Editor>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    cam_q: Query<(&Camera, &GlobalTransform)>,
    mut out: ResMut<CursorGround>,
) {
    let hit = (|| {
        let window = window_q.single().ok()?;
        let cam = cam_q.single().ok()?;
        ground_hit(cursor_ray(window, &editor, cam)?, 0.0)
    })();
    if out.0 != hit {
        out.0 = hit;
    }
}

/// The box to click on for each object and spawn.
fn pick_boxes(editor: &Editor) -> Vec<(Sel, Vec3, Vec3)> {
    let mut out = Vec::new();
    for (i, o) in editor.map.objects.iter().enumerate() {
        if let Some((lo, hi)) = object_bounds(o) {
            out.push((Sel::Object(i), lo, hi));
        }
    }
    for kind in SpawnKind::ALL {
        for (i, &[x, z]) in editor.spawns(kind).iter().enumerate() {
            out.push((Sel::Spawn(kind, i), Vec3::new(x - 0.45, 0.0, z - 0.45), Vec3::new(x + 0.45, 1.8, z + 0.45)));
        }
    }
    out
}

/// The box around an object: its collision shapes, or a small box for scenery.
pub fn object_bounds(o: &ink_map::Object) -> Option<(Vec3, Vec3)> {
    let piece = ink_map::piece(&o.piece)?;
    let at = Vec3::from_array(o.pos);
    let solids = ink_map::piece_solids(piece, at, o.rotation(), Vec3::from_array(o.scale));
    if solids.is_empty() {
        let s = Vec3::from_array(o.scale);
        return Some((at - Vec3::new(0.5, 0.0, 0.5) * s, at + Vec3::new(0.5, 1.0, 0.5) * s));
    }
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for s in solids {
        let (a, b) = match s.shape {
            Shape::Box { min, max } | Shape::Ramp { min, max, .. } => (min, max),
            Shape::Cylinder { base, radius, height } => (
                base - Vec3::new(radius, 0.0, radius),
                base + Vec3::new(radius, height, radius),
            ),
        };
        lo = lo.min(a);
        hi = hi.max(b);
    }
    Some((lo, hi))
}

/// Distance along a ray to a box, if it hits.
fn ray_box(ray: Ray3d, lo: Vec3, hi: Vec3) -> Option<f32> {
    let d = ray.direction.as_vec3();
    let o = ray.origin;
    let mut t0 = 0.0f32;
    let mut t1 = f32::MAX;
    for a in 0..3 {
        if d[a].abs() < 1e-8 {
            if o[a] < lo[a] || o[a] > hi[a] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[a];
        let (mut n, mut f) = ((lo[a] - o[a]) * inv, (hi[a] - o[a]) * inv);
        if n > f {
            std::mem::swap(&mut n, &mut f);
        }
        t0 = t0.max(n);
        t1 = t1.min(f);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// The top of whatever solid object is under `at` (for stacking), ignoring `skip`.
pub fn top_at(editor: &Editor, at: Vec2, skip: Option<usize>) -> f32 {
    let mut top: f32 = 0.0;
    for (i, o) in editor.map.objects.iter().enumerate() {
        if Some(i) == skip || !o.solid {
            continue;
        }
        if let Some((lo, hi)) = object_bounds(o)
            && at.x > lo.x
            && at.x < hi.x
            && at.y > lo.z
            && at.y < hi.z
        {
            top = top.max(hi.y);
        }
    }
    top
}

#[allow(clippy::too_many_arguments)]
fn clicks(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut editor: ResMut<Editor>,
    mut drag: ResMut<Drag>,
    cursor: Res<CursorGround>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    cam_q: Query<(&Camera, &GlobalTransform)>,
) {
    if mouse.just_released(MouseButton::Left) {
        drag.0 = None;
    }
    if let (Some((offset, moved)), Some(at)) = (drag.0, cursor.0)
        && mouse.pressed(MouseButton::Left)
    {
        let to = editor.snapped(at - offset);
        if editor.sel_pos().is_some_and(|p| Vec2::new(p.x, p.z) != to) {
            if !moved {
                // The first move of a drag is one undo step.
                editor.snapshot();
                drag.0 = Some((offset, true));
            }
            editor.move_sel_to(to);
        }
        return;
    }
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(at) = cursor.0 else {
        return;
    };
    editor.focus = None;
    let snapped = editor.snapped(at);
    match editor.tool {
        Tool::Place(piece) => {
            editor.place_piece(piece, snapped);
            // Shift: stack it on what's underneath.
            if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
                let n = editor.map.objects.len() - 1;
                let y = top_at(&editor, snapped, Some(n));
                editor.map.objects[n].pos[1] = y;
            }
        }
        Tool::Spawn(kind) => editor.place_spawn(kind, snapped),
        Tool::Select => {
            let (Ok(window), Ok(cam)) = (window_q.single(), cam_q.single()) else {
                return;
            };
            let Some(ray) = cursor_ray(window, &editor, cam) else {
                return;
            };
            let hit = pick_boxes(&editor)
                .into_iter()
                .filter_map(|(sel, lo, hi)| ray_box(ray, lo, hi).map(|t| (sel, t)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            editor.sel = hit.map(|(s, _)| s);
            if let Some(p) = editor.sel_pos() {
                drag.0 = Some((at - Vec2::new(p.x, p.z), false));
            }
        }
    }
}

fn draw_overlay(editor: Res<Editor>, cursor: Res<CursorGround>, mut gizmos: Gizmos) {
    let h = editor.map.half();
    let y = 0.03;
    // The field's edge and center line.
    let edge = Color::srgba(1.0, 1.0, 1.0, 0.8);
    let corners = [
        Vec3::new(-h.x, y, -h.y),
        Vec3::new(h.x, y, -h.y),
        Vec3::new(h.x, y, h.y),
        Vec3::new(-h.x, y, h.y),
    ];
    for i in 0..4 {
        gizmos.line(corners[i], corners[(i + 1) % 4], edge);
    }
    gizmos.line(Vec3::new(-h.x, y, 0.0), Vec3::new(h.x, y, 0.0), Color::srgba(1.0, 1.0, 1.0, 0.35));
    // A 1 m grid near the cursor (every 5 m brighter).
    if let Some(c) = cursor.0 {
        let r = 8.0;
        let x0 = (c.x - r).floor().max(-h.x);
        let x1 = (c.x + r).ceil().min(h.x);
        let z0 = (c.y - r).floor().max(-h.y);
        let z1 = (c.y + r).ceil().min(h.y);
        let mut x = x0.ceil();
        while x <= x1 {
            let a = if x as i32 % 5 == 0 { 0.35 } else { 0.15 };
            gizmos.line(Vec3::new(x, y, z0), Vec3::new(x, y, z1), Color::srgba(1.0, 1.0, 1.0, a));
            x += 1.0;
        }
        let mut z = z0.ceil();
        while z <= z1 {
            let a = if z as i32 % 5 == 0 { 0.35 } else { 0.15 };
            gizmos.line(Vec3::new(x0, y, z), Vec3::new(x1, y, z), Color::srgba(1.0, 1.0, 1.0, a));
            z += 1.0;
        }
    }
    // Spawns: a ring and a post, facing the middle.
    for kind in SpawnKind::ALL {
        let color = kind.color();
        for (i, &[x, z]) in editor.spawns(kind).iter().enumerate() {
            let p = Vec3::new(x, y, z);
            let selected = editor.sel == Some(Sel::Spawn(kind, i));
            ring(&mut gizmos, p, 0.45, if selected { Color::srgb(1.0, 0.9, 0.1) } else { color });
            gizmos.line(p, p + Vec3::Y * 1.8, color);
            let to_mid = (-Vec3::new(x, 0.0, z)).normalize_or_zero();
            if to_mid != Vec3::ZERO {
                gizmos.line(p + Vec3::Y * 0.05, p + Vec3::Y * 0.05 + to_mid * 0.9, color);
            }
        }
    }
    // The selection.
    if let Some(Sel::Object(i)) = editor.sel
        && let Some(o) = editor.map.objects.get(i)
        && let Some((lo, hi)) = object_bounds(o)
    {
        wire_box(&mut gizmos, lo, hi, Color::srgb(1.0, 0.9, 0.1));
        // Which way is its front (-Z).
        let at = Vec3::from_array(o.pos) + Vec3::Y * (hi.y - o.pos[1] + 0.2);
        gizmos.line(at, at + o.rotation() * Vec3::NEG_Z, Color::srgb(1.0, 0.9, 0.1));
    }
    // What's about to be placed.
    if let Some(c) = cursor.0 {
        let at = editor.snapped(c);
        match editor.tool {
            Tool::Place(piece) => {
                let mut o = ink_map::Object::new(piece.id, [at.x, 0.0, at.y], editor.place_yaw);
                o.solid = true;
                if let Some((lo, hi)) = object_bounds(&o) {
                    wire_box(&mut gizmos, lo, hi, Color::srgba(0.4, 1.0, 0.5, 0.9));
                }
            }
            Tool::Spawn(kind) => ring(&mut gizmos, Vec3::new(at.x, y, at.y), 0.45, kind.color()),
            Tool::Select => {}
        }
    }
}

fn ring(gizmos: &mut Gizmos, at: Vec3, r: f32, color: Color) {
    let n = 20;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let b = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
        gizmos.line(
            at + Vec3::new(a.cos(), 0.0, a.sin()) * r,
            at + Vec3::new(b.cos(), 0.0, b.sin()) * r,
            color,
        );
    }
}

fn wire_box(gizmos: &mut Gizmos, lo: Vec3, hi: Vec3, color: Color) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_boxes_in_front() {
        let ray = Ray3d::new(Vec3::new(0.0, 5.0, 10.0), Dir3::new(Vec3::new(0.0, -0.5, -1.0)).unwrap());
        assert!(ray_box(ray, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0)).is_some());
        assert!(ray_box(ray, Vec3::new(5.0, 0.0, -1.0), Vec3::new(6.0, 2.0, 1.0)).is_none());
    }

    #[test]
    fn stacking_finds_the_top() {
        let mut ed = Editor::default();
        ed.place_piece(ink_map::piece("container_green").unwrap(), Vec2::ZERO);
        assert!((top_at(&ed, Vec2::new(1.0, 0.0), None) - 2.59).abs() < 1e-4);
        assert_eq!(top_at(&ed, Vec2::new(20.0, 0.0), None), 0.0);
    }
}
