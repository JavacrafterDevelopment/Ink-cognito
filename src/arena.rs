//! The paintball field: turf, inflatable bunkers, props and spawn pads from
//! the LowPoly Armory pack, and the simple shapes used to collide with them.

use crate::protocol::SPAWN_PAD_Z;
use bevy::{
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    math::Affine2,
    prelude::*,
};
use std::f32::consts::{FRAC_PI_2, PI};

pub const FIELD_SIZE: f32 = 60.0;
pub const FIELD_HALF: f32 = FIELD_SIZE * 0.5;

/// A solid thing on the field.
#[derive(Debug, Clone, Copy)]
pub enum Shape {
    Box {
        min: Vec3,
        max: Vec3,
    },
    /// Upright cylinder standing on `base`.
    Cylinder {
        base: Vec3,
        radius: f32,
        height: f32,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Collider {
    pub shape: Shape,
    /// Whether paint splats stick to it (false for odd shapes, where a flat
    /// decal on its bounding box would float in the air).
    pub decals: bool,
}

#[derive(Resource, Default)]
pub struct Colliders(pub Vec<Collider>);

/// A hit along a segment.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    /// 0..=1 along the segment.
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    pub decals: bool,
}

impl Colliders {
    /// The first thing (ground included) the segment `from -> to` hits.
    pub fn segment_hit(&self, from: Vec3, to: Vec3) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        let mut consider = |hit: RayHit| {
            if best.is_none_or(|b| hit.t < b.t) {
                best = Some(hit);
            }
        };
        if to.y <= 0.0 && from.y > 0.0 {
            let t = from.y / (from.y - to.y);
            consider(RayHit {
                t,
                point: from.lerp(to, t).with_y(0.0),
                normal: Vec3::Y,
                decals: true,
            });
        }
        for c in &self.0 {
            let hit = match c.shape {
                Shape::Box { min, max } => segment_box(from, to, min, max),
                Shape::Cylinder {
                    base,
                    radius,
                    height,
                } => segment_cylinder(from, to, base, radius, height),
            };
            if let Some((t, normal)) = hit {
                consider(RayHit {
                    t,
                    point: from.lerp(to, t),
                    normal,
                    decals: c.decals,
                });
            }
        }
        best
    }

    /// Pushes a standing cylinder (a player) out of everything it overlaps.
    /// `feet` is the bottom, `top` the head. Returns the height of the
    /// highest surface right under the feet, to stand on.
    pub fn resolve_player(&self, pos: &mut Vec3, radius: f32, feet: f32, top: f32) -> f32 {
        let mut floor = 0.0f32;
        for c in &self.0 {
            match c.shape {
                Shape::Box { min, max } => {
                    let closest = Vec2::new(pos.x.clamp(min.x, max.x), pos.z.clamp(min.z, max.z));
                    let p = Vec2::new(pos.x, pos.z);
                    let d = p - closest;
                    let dist2 = d.length_squared();
                    let overlap = dist2 < radius * radius
                        || (p.x > min.x && p.x < max.x && p.y > min.z && p.y < max.z);
                    if !overlap {
                        continue;
                    }
                    if feet >= max.y - 0.25 {
                        floor = floor.max(max.y);
                    } else if top > min.y {
                        let push = if dist2 > 1e-8 {
                            let dist = dist2.sqrt();
                            d / dist * (radius - dist)
                        } else {
                            // Center inside the box: leave by the nearest side.
                            let exits = [
                                (p.x - min.x, Vec2::new(-(p.x - min.x + radius), 0.0)),
                                (max.x - p.x, Vec2::new(max.x - p.x + radius, 0.0)),
                                (p.y - min.z, Vec2::new(0.0, -(p.y - min.z + radius))),
                                (max.z - p.y, Vec2::new(0.0, max.z - p.y + radius)),
                            ];
                            exits
                                .into_iter()
                                .min_by(|a, b| a.0.total_cmp(&b.0))
                                .map(|e| e.1)
                                .unwrap_or_default()
                        };
                        pos.x += push.x;
                        pos.z += push.y;
                    }
                }
                Shape::Cylinder {
                    base,
                    radius: r,
                    height,
                } => {
                    let d = Vec2::new(pos.x - base.x, pos.z - base.z);
                    let reach = r + radius;
                    let dist2 = d.length_squared();
                    if dist2 >= reach * reach {
                        continue;
                    }
                    let top_y = base.y + height;
                    if feet >= top_y - 0.25 {
                        if dist2 < r * r {
                            floor = floor.max(top_y);
                        }
                    } else if top > base.y {
                        let dist = dist2.sqrt().max(1e-4);
                        let dir = if dist2 > 1e-8 { d / dist } else { Vec2::X };
                        let push = dir * (reach - dist);
                        pos.x += push.x;
                        pos.z += push.y;
                    }
                }
            }
        }
        floor
    }

    /// Keeps a small sphere (a ragdoll joint) outside every shape.
    pub fn push_out_point(&self, p: &mut Vec3, radius: f32) -> bool {
        let mut touched = false;
        for c in &self.0 {
            match c.shape {
                Shape::Box { min, max } => {
                    let lo = min - Vec3::splat(radius);
                    let hi = max + Vec3::splat(radius);
                    if p.x > lo.x
                        && p.x < hi.x
                        && p.y > lo.y
                        && p.y < hi.y
                        && p.z > lo.z
                        && p.z < hi.z
                    {
                        let exits = [
                            (p.x - lo.x, Vec3::new(lo.x, p.y, p.z)),
                            (hi.x - p.x, Vec3::new(hi.x, p.y, p.z)),
                            (hi.y - p.y, Vec3::new(p.x, hi.y, p.z)),
                            (p.z - lo.z, Vec3::new(p.x, p.y, lo.z)),
                            (hi.z - p.z, Vec3::new(p.x, p.y, hi.z)),
                        ];
                        if let Some(e) = exits.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
                            *p = e.1;
                            touched = true;
                        }
                    }
                }
                Shape::Cylinder {
                    base,
                    radius: r,
                    height,
                } => {
                    let top = base.y + height + radius;
                    if p.y > top || p.y < base.y {
                        continue;
                    }
                    let d = Vec2::new(p.x - base.x, p.z - base.z);
                    let reach = r + radius;
                    let dist = d.length();
                    if dist >= reach {
                        continue;
                    }
                    if top - p.y < reach - dist {
                        p.y = top;
                    } else {
                        let dir = if dist > 1e-4 { d / dist } else { Vec2::X };
                        p.x = base.x + dir.x * reach;
                        p.z = base.z + dir.y * reach;
                    }
                    touched = true;
                }
            }
        }
        touched
    }
}

/// Slab test. Returns (t, outward normal) of where the segment enters the box.
fn segment_box(from: Vec3, to: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let d = to - from;
    let mut t_enter = 0.0f32;
    let mut t_exit = 1.0f32;
    let mut normal = Vec3::ZERO;
    for axis in 0..3 {
        let (o, dir, lo, hi) = (from[axis], d[axis], min[axis], max[axis]);
        if dir.abs() < 1e-8 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let mut t0 = (lo - o) / dir;
        let mut t1 = (hi - o) / dir;
        let mut n = Vec3::ZERO;
        n[axis] = -1.0;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
            n[axis] = 1.0;
        }
        if t0 > t_enter {
            t_enter = t0;
            normal = n;
        }
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }
    // Starting inside counts as no hit (the ball just left a surface).
    (normal != Vec3::ZERO).then_some((t_enter, normal))
}

fn segment_cylinder(
    from: Vec3,
    to: Vec3,
    base: Vec3,
    radius: f32,
    height: f32,
) -> Option<(f32, Vec3)> {
    let top = base.y + height;
    let d = to - from;
    // Side wall.
    let o = Vec2::new(from.x - base.x, from.z - base.z);
    let v = Vec2::new(d.x, d.z);
    let a = v.length_squared();
    let mut best: Option<(f32, Vec3)> = None;
    if a > 1e-10 {
        let b = 2.0 * o.dot(v);
        let c = o.length_squared() - radius * radius;
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 && c > 0.0 {
            let t = (-b - disc.sqrt()) / (2.0 * a);
            if (0.0..=1.0).contains(&t) {
                let p = from + d * t;
                if p.y >= base.y && p.y <= top {
                    let n = Vec3::new(p.x - base.x, 0.0, p.z - base.z).normalize_or(Vec3::X);
                    best = Some((t, n));
                }
            }
        }
    }
    // Top cap.
    if from.y > top && to.y <= top {
        let t = (from.y - top) / (from.y - to.y);
        let p = from + d * t;
        if Vec2::new(p.x - base.x, p.z - base.z).length_squared() <= radius * radius
            && best.is_none_or(|b| t < b.0)
        {
            best = Some((t, Vec3::Y));
        }
    }
    best
}

/// Bunkers and props: model, footprint shape, whether decals stick.
struct Piece {
    model: &'static str,
    /// Local bounds (from the model): min, max.
    bounds: (Vec3, Vec3),
    round: Option<f32>,
    decals: bool,
}

const fn piece(model: &'static str, min: [f32; 3], max: [f32; 3], decals: bool) -> Piece {
    Piece {
        model,
        bounds: (
            Vec3::new(min[0], min[1], min[2]),
            Vec3::new(max[0], max[1], max[2]),
        ),
        round: None,
        decals,
    }
}

const fn round(model: &'static str, radius: f32, height: f32, decals: bool) -> Piece {
    Piece {
        model,
        bounds: (
            Vec3::new(-radius, 0.0, -radius),
            Vec3::new(radius, height, radius),
        ),
        round: Some(radius),
        decals,
    }
}

const CAN: Piece = round("models/bunkers/can.glb", 0.537, 1.31, true);
const MINI_CAN: Piece = round("models/bunkers/mini_can.glb", 0.44, 0.9, true);
const CAKE: Piece = round("models/bunkers/cake.glb", 0.85, 0.76, true);
const TREE: Piece = round("models/bunkers/tree.glb", 0.5, 1.95, false);
const BRICK: Piece = piece(
    "models/bunkers/brick.glb",
    [-0.806, 0.0, -0.456],
    [0.806, 0.9, 0.456],
    true,
);
const DORITO: Piece = piece(
    "models/bunkers/dorito.glb",
    [-0.675, 0.0, -0.39],
    [0.675, 1.15, 0.78],
    false,
);
const SNAKE: Piece = piece(
    "models/bunkers/snake.glb",
    [-1.8, 0.0, -0.356],
    [1.8, 0.75, 0.356],
    true,
);
const TALL_WALL: Piece = piece(
    "models/bunkers/tall_wall.glb",
    [-1.006, 0.0, -0.306],
    [1.006, 2.0, 0.306],
    true,
);
const TEMPLE: Piece = piece(
    "models/bunkers/temple.glb",
    [-0.7, 0.0, -0.606],
    [0.7, 1.6, 0.606],
    false,
);
const BARREL: Piece = round("models/props/barrel.glb", 0.295, 0.885, true);
const CRATE: Piece = piece(
    "models/props/crate.glb",
    [-0.4, 0.0, -0.4],
    [0.4, 0.8, 0.4],
    true,
);
const SANDBAGS: Piece = piece(
    "models/props/sandbag_wall.glb",
    [-1.244, 0.0, -0.185],
    [1.244, 0.59, 0.185],
    true,
);

/// One half of the field (the blue half, +Z). The other half is the same
/// turned around the middle, so neither team has the better side.
const HALF_LAYOUT: &[(&Piece, f32, f32, f32)] = &[
    // (piece, x, z, quarter turns)
    (&TALL_WALL, -6.0, 16.0, 0.0),
    (&TALL_WALL, 6.0, 16.0, 0.0),
    (&CAN, 0.0, 13.0, 0.0),
    (&DORITO, -11.0, 11.0, 2.0),
    (&DORITO, 11.0, 11.0, 2.0),
    (&CAKE, -4.0, 7.5, 0.0),
    (&MINI_CAN, 4.5, 8.0, 0.0),
    (&BRICK, -17.0, 14.0, 1.0),
    (&BRICK, 17.0, 15.0, 1.0),
    (&SNAKE, -21.0, 5.0, 1.0),
    (&TEMPLE, 15.0, 4.0, 0.0),
    (&TREE, -9.0, 3.0, 0.0),
    (&CRATE, 22.0, 9.0, 0.0),
    (&CRATE, 22.9, 9.3, 0.0),
    (&BARREL, 21.6, 10.2, 0.0),
    (&SANDBAGS, -24.0, 19.0, 0.0),
    (&SANDBAGS, 24.0, 19.0, 0.0),
    (&BRICK, 8.0, 21.0, 0.0),
    (&BRICK, -8.0, 21.0, 0.0),
];

/// Pieces on the center line (not mirrored).
const CENTER_LAYOUT: &[(&Piece, f32, f32, f32)] = &[
    (&TEMPLE, 0.0, 0.0, 1.0),
    (&CAN, -13.0, 0.0, 0.0),
    (&CAN, 13.0, 0.0, 0.0),
    (&SNAKE, -6.5, 0.0, 0.0),
    (&SNAKE, 6.5, 0.0, 0.0),
    (&MINI_CAN, -24.0, 0.0, 0.0),
    (&MINI_CAN, 24.0, 0.0, 0.0),
];

fn turned_bounds(bounds: (Vec3, Vec3), turns: f32) -> (Vec3, Vec3) {
    let rot = Quat::from_rotation_y(turns * FRAC_PI_2);
    let (min, max) = bounds;
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for x in [min.x, max.x] {
        for z in [min.z, max.z] {
            let p = rot * Vec3::new(x, 0.0, z);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    (lo.with_y(min.y), hi.with_y(max.y))
}

/// Spawns the field (visible everywhere: it's also the menu's backdrop) and
/// fills `Colliders`.
pub fn setup_arena(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let repeat = |s: &mut ImageLoaderSettings| {
        s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            ..default()
        });
    };
    let turf: Handle<Image> = assets
        .load_builder()
        .with_settings(repeat)
        .load("textures/ground/turf_tile.png");
    let dirt: Handle<Image> = assets
        .load_builder()
        .with_settings(repeat)
        .load("textures/ground/dirt_tile.png");

    // Turf slab with its top at y = 0.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(FIELD_SIZE, 1.0, FIELD_SIZE))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(turf),
            uv_transform: Affine2::from_scale(Vec2::splat(FIELD_SIZE / 3.0)),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    // Dirt surround outside the nets.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(220.0, 220.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(dirt.clone()),
            uv_transform: Affine2::from_scale(Vec2::splat(220.0 / 4.0)),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.02, 0.0),
    ));
    // A dirt strip down the center line.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(FIELD_SIZE, 2.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(dirt),
            uv_transform: Affine2::from_scale(Vec2::new(FIELD_SIZE / 2.0, 1.0)),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.004, 0.0),
    ));

    let glb = |path: &str| {
        WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(path.to_string())))
    };
    let mut colliders = Vec::new();
    let mut place = |commands: &mut Commands, piece: &Piece, x: f32, z: f32, turns: f32| {
        commands.spawn((
            glb(piece.model),
            Transform::from_xyz(x, 0.0, z).with_rotation(Quat::from_rotation_y(turns * FRAC_PI_2)),
        ));
        let shape = match piece.round {
            Some(radius) => Shape::Cylinder {
                base: Vec3::new(x, 0.0, z),
                radius,
                height: piece.bounds.1.y,
            },
            None => {
                let (lo, hi) = turned_bounds(piece.bounds, turns);
                Shape::Box {
                    min: lo + Vec3::new(x, 0.0, z),
                    max: hi + Vec3::new(x, 0.0, z),
                }
            }
        };
        colliders.push(Collider {
            shape,
            decals: piece.decals,
        });
    };
    for &(piece, x, z, turns) in HALF_LAYOUT {
        place(&mut commands, piece, x, z, turns);
        place(&mut commands, piece, -x, -z, turns + 2.0);
    }
    for &(piece, x, z, turns) in CENTER_LAYOUT {
        place(&mut commands, piece, x, z, turns);
    }

    // Spawn pads and flags behind them.
    for (pad, flag, side) in [
        (
            "models/props/spawn_pad_blue.glb",
            "models/props/flag_blue.glb",
            1.0,
        ),
        (
            "models/props/spawn_pad_orange.glb",
            "models/props/flag_orange.glb",
            -1.0,
        ),
    ] {
        for x in [-2.0, 2.0] {
            commands.spawn((
                glb(pad),
                Transform::from_xyz(x * 1.25, 0.0, side * SPAWN_PAD_Z),
            ));
        }
        commands.spawn((
            glb(flag),
            Transform::from_xyz(0.0, 0.0, side * (FIELD_HALF - 2.0)),
        ));
        // Pickups by the pads (scenery for now).
        commands.spawn((
            glb("models/pickups/paint_case.glb"),
            Transform::from_xyz(-5.5, 0.0, side * (SPAWN_PAD_Z + 1.0))
                .with_rotation(Quat::from_rotation_y(0.4)),
        ));
        commands.spawn((
            glb("models/pickups/pod_pickup.glb"),
            Transform::from_xyz(-5.0, 0.3, side * (SPAWN_PAD_Z + 1.2)),
        ));
        commands.spawn((
            glb("models/pickups/air_refill_station.glb"),
            Transform::from_xyz(5.5, 0.0, side * (SPAWN_PAD_Z + 1.0))
                .with_rotation(Quat::from_rotation_y(if side > 0.0 { PI } else { 0.0 })),
        ));
    }

    // Cones along the center line.
    for i in -4..=4 {
        if i == 0 {
            continue;
        }
        let x = i as f32 * 3.0;
        if x.abs() == 6.0 || x.abs() == 12.0 {
            continue;
        }
        commands.spawn((
            glb("models/props/field_cone.glb"),
            Transform::from_xyz(x, 0.0, 0.0),
        ));
    }

    // Netting all the way around (sections are about 2.47 m wide).
    let section = 2.474;
    let count = (FIELD_SIZE / section).ceil() as i32;
    let scale = FIELD_SIZE / (count as f32 * section);
    for i in 0..count {
        let along = -FIELD_HALF + (i as f32 + 0.5) * section * scale;
        for (x, z, rot) in [
            (along, -FIELD_HALF, 0.0),
            (along, FIELD_HALF, 0.0),
            (-FIELD_HALF, along, FRAC_PI_2),
            (FIELD_HALF, along, FRAC_PI_2),
        ] {
            commands.spawn((
                glb("models/props/netting_fence_section.glb"),
                Transform::from_xyz(x, 0.0, z)
                    .with_rotation(Quat::from_rotation_y(rot))
                    .with_scale(Vec3::new(scale, 1.0, 1.0)),
            ));
        }
    }
    // The nets stop balls too.
    let h = 2.4;
    for (min, max) in [
        (
            Vec3::new(-FIELD_HALF - 1.0, 0.0, -FIELD_HALF - 1.0),
            Vec3::new(FIELD_HALF + 1.0, h, -FIELD_HALF),
        ),
        (
            Vec3::new(-FIELD_HALF - 1.0, 0.0, FIELD_HALF),
            Vec3::new(FIELD_HALF + 1.0, h, FIELD_HALF + 1.0),
        ),
        (
            Vec3::new(-FIELD_HALF - 1.0, 0.0, -FIELD_HALF),
            Vec3::new(-FIELD_HALF, h, FIELD_HALF),
        ),
        (
            Vec3::new(FIELD_HALF, 0.0, -FIELD_HALF),
            Vec3::new(FIELD_HALF + 1.0, h, FIELD_HALF),
        ),
    ] {
        colliders.push(Collider {
            shape: Shape::Box { min, max },
            decals: false,
        });
    }

    commands.insert_resource(Colliders(colliders));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_hit_boxes_cylinders_and_ground() {
        let c = Colliders(vec![
            Collider {
                shape: Shape::Box {
                    min: Vec3::new(-1.0, 0.0, -1.0),
                    max: Vec3::new(1.0, 2.0, 1.0),
                },
                decals: true,
            },
            Collider {
                shape: Shape::Cylinder {
                    base: Vec3::new(10.0, 0.0, 0.0),
                    radius: 1.0,
                    height: 2.0,
                },
                decals: true,
            },
        ]);
        let hit = c
            .segment_hit(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0))
            .unwrap();
        assert!((hit.point.x + 1.0).abs() < 1e-4);
        assert_eq!(hit.normal, Vec3::NEG_X);
        let hit = c
            .segment_hit(Vec3::new(5.0, 1.0, 0.0), Vec3::new(15.0, 1.0, 0.0))
            .unwrap();
        assert!((hit.point.x - 9.0).abs() < 1e-4);
        let hit = c
            .segment_hit(Vec3::new(5.0, 1.0, 5.0), Vec3::new(5.0, -1.0, 5.0))
            .unwrap();
        assert_eq!(hit.normal, Vec3::Y);
        assert!(
            c.segment_hit(Vec3::new(5.0, 1.0, 5.0), Vec3::new(6.0, 1.0, 5.0))
                .is_none()
        );

        let mut p = Vec3::new(1.2, 0.9, 0.0);
        c.resolve_player(&mut p, 0.42, 0.0, 1.8);
        assert!(p.x >= 1.42 - 1e-4);
        let mut p = Vec3::new(0.0, 3.0, 0.0);
        assert_eq!(c.resolve_player(&mut p, 0.42, 2.1, 3.9), 2.0);
    }
}
