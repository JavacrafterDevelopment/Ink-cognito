//! The five fields. Each map is a list of models from the LowPoly Armory pack
//! (bunkers, props and the map kit) with the shapes that collide with them,
//! where each team spawns, and how the ground looks. The host picks the map;
//! everyone builds the same one when the match tells them which.
//!
//! - **Splat Park**: the classic airball field with inflatable bunkers.
//! - **Dockyard**: a small, tight container yard packed with cover.
//! - **Ring Road**: a big outer loop around two walled blocks, joined by
//!   cross-paths through the blocks and down the middle.
//! - **Three Lanes**: a wide middle lane and two flanks (the Yard and the
//!   Woods), linked by connectors at the center and near each base.
//! - **Pine Hollow**: woods with a raised skybridge in the middle and perches.

use crate::{
    arena::{Collider, Colliders, Shape},
    protocol::{MapId, PlayerId, Team},
    world::PLAYER_GROUND_Y,
};
use bevy::{
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    math::Affine2,
    prelude::*,
};
use std::f32::consts::{FRAC_PI_2, PI};

pub struct MapsPlugin;

impl Plugin for MapsPlugin {
    fn build(&self, app: &mut App) {
        let layout = layout(MapId::default());
        app.init_resource::<CurrentMap>()
            .insert_resource(Colliders(layout.colliders.clone()))
            .insert_resource(layout.info.clone())
            .add_systems(Startup, load_ground_textures)
            .add_systems(Update, rebuild_map.after(crate::world::WorldSystems::Net));
    }
}

/// Which map is built (the host's pick in game; the selected one on the menu).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentMap(pub MapId);

/// Everything spawned for the current map (despawned when it changes).
#[derive(Component)]
pub struct MapEntity;

/// What the rest of the game needs to know about the built map.
#[derive(Resource, Debug, Clone, Default)]
pub struct MapInfo {
    pub id: MapId,
    /// Half the playable size: x and z.
    pub half: Vec2,
    /// Blue and orange spawn spots (on the ground).
    pub spawns: [Vec<Vec3>; 2],
    /// Free-for-all spawn spots.
    pub ffa: Vec<Vec3>,
    /// Ground footprints of everything solid and its height, for the minimap.
    pub footprints: Vec<(Rect, f32)>,
}

impl MapInfo {
    /// Where player `id` should (re)spawn and which way to face. With
    /// `threats` (living enemies), the spot farthest from them is taken.
    pub fn spawn_for(&self, id: PlayerId, ffa: bool, threats: &[Vec3], salt: u32) -> (Vec3, f32) {
        let list: &[Vec3] = if ffa {
            &self.ffa
        } else {
            &self.spawns[Team::of(id).index()]
        };
        if list.is_empty() {
            return (Vec3::new(0.0, PLAYER_GROUND_Y, 0.0), 0.0);
        }
        let slot = if ffa {
            (id as usize).wrapping_mul(7) % list.len()
        } else {
            (id as usize / 2) % list.len()
        };
        let pick = if threats.is_empty() {
            slot
        } else {
            // Score each spot by its nearest threat; take one of the best
            // few so two players spawning together don't stack up.
            let mut scored: Vec<(usize, f32)> = list
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let near = threats
                        .iter()
                        .map(|t| t.with_y(0.0).distance(p.with_y(0.0)))
                        .fold(f32::MAX, f32::min);
                    (i, near)
                })
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top = scored.len().min(3);
            let choice = (id.wrapping_mul(31).wrapping_add(salt) as usize) % top;
            scored[choice].0
        };
        let p = list[pick];
        let pos = Vec3::new(p.x, PLAYER_GROUND_Y, p.z);
        (pos, face_center(pos))
    }
}

/// The yaw that looks from `pos` toward the middle of the field (0 looks down -Z).
fn face_center(pos: Vec3) -> f32 {
    let d = -pos.with_y(0.0);
    if d.length_squared() < 1e-4 {
        return 0.0;
    }
    // Mostly face down the field toward the other team, like a real start.
    if d.z.abs() >= d.x.abs() * 0.6 {
        if d.z < 0.0 { 0.0 } else { PI }
    } else {
        (-d.x).atan2(-d.z)
    }
}

/// Name and blurb for the map picker.
pub struct MapMeta {
    pub name: &'static str,
    pub style: &'static str,
    pub blurb: &'static str,
    pub size: &'static str,
}

pub fn meta(id: MapId) -> MapMeta {
    match id {
        MapId::SplatPark => MapMeta {
            name: "Splat Park",
            style: "Classic airball",
            blurb: "The original field: inflatable bunkers, cones and nets.",
            size: "Large",
        },
        MapId::Dockyard => MapMeta {
            name: "Dockyard",
            style: "Close quarters",
            blurb: "A tight container yard stacked with crates, barriers and spools. Fast and loud.",
            size: "Small",
        },
        MapId::RingRoad => MapMeta {
            name: "Ring Road",
            style: "Loops",
            blurb: "A wide outer loop around two walled blocks, joined by cross-paths. No dead ends.",
            size: "Large",
        },
        MapId::ThreeLanes => MapMeta {
            name: "Three Lanes",
            style: "Lanes",
            blurb: "Mid lane flanked by the Yard and the Woods, linked by connectors. Clear callouts.",
            size: "Medium",
        },
        MapId::PineHollow => MapMeta {
            name: "Pine Hollow",
            style: "Woods",
            blurb: "Pines, rocks and logs, a raised skybridge in the middle and perches to hold.",
            size: "Medium",
        },
    }
}

// --- Pieces -------------------------------------------------------------

/// One collision part of a piece, in the model's own space.
#[derive(Clone, Copy)]
enum Part {
    Box([f32; 3], [f32; 3]),
    /// x, z, radius, bottom, height.
    Cyl(f32, f32, f32, f32, f32),
    /// A slope rising toward the model's -Z.
    Ramp([f32; 3], [f32; 3]),
}

struct Piece {
    model: &'static str,
    parts: &'static [Part],
    decals: bool,
}

macro_rules! piece {
    ($model:expr, decals: $d:expr, [$($part:expr),* $(,)?]) => {
        Piece { model: $model, parts: &[$($part),*], decals: $d }
    };
}

// Inflatable bunkers and the original props.
const CAN: Piece =
    piece!("models/bunkers/can.glb", decals: true, [Part::Cyl(0.0, 0.0, 0.537, 0.0, 1.31)]);
const MINI_CAN: Piece =
    piece!("models/bunkers/mini_can.glb", decals: true, [Part::Cyl(0.0, 0.0, 0.44, 0.0, 0.9)]);
const CAKE: Piece =
    piece!("models/bunkers/cake.glb", decals: true, [Part::Cyl(0.0, 0.0, 0.85, 0.0, 0.76)]);
const BUNKER_TREE: Piece =
    piece!("models/bunkers/tree.glb", decals: false, [Part::Cyl(0.0, 0.0, 0.5, 0.0, 1.95)]);
const BRICK: Piece = piece!("models/bunkers/brick.glb", decals: true, [Part::Box([-0.806, 0.0, -0.456], [0.806, 0.9, 0.456])]);
const DORITO: Piece = piece!("models/bunkers/dorito.glb", decals: false, [Part::Box([-0.675, 0.0, -0.39], [0.675, 1.15, 0.78])]);
const SNAKE: Piece = piece!("models/bunkers/snake.glb", decals: true, [Part::Box([-1.8, 0.0, -0.356], [1.8, 0.75, 0.356])]);
const TALL_WALL: Piece = piece!("models/bunkers/tall_wall.glb", decals: true, [Part::Box([-1.006, 0.0, -0.306], [1.006, 2.0, 0.306])]);
const TEMPLE: Piece = piece!("models/bunkers/temple.glb", decals: false, [Part::Box([-0.7, 0.0, -0.606], [0.7, 1.6, 0.606])]);
const BARREL: Piece =
    piece!("models/props/barrel.glb", decals: true, [Part::Cyl(0.0, 0.0, 0.295, 0.0, 0.885)]);
const CRATE: Piece =
    piece!("models/props/crate.glb", decals: true, [Part::Box([-0.4, 0.0, -0.4], [0.4, 0.8, 0.4])]);
const SANDBAGS: Piece = piece!("models/props/sandbag_wall.glb", decals: true, [Part::Box([-1.244, 0.0, -0.185], [1.244, 0.59, 0.185])]);

// Map kit: walls.
const WALL_2M: Piece = piece!("models/map-kit/walls/wall_2m.glb", decals: true, [Part::Box([-1.0, 0.0, -0.08], [1.0, 2.4, 0.08])]);
const WALL_LOW: Piece = piece!("models/map-kit/walls/wall_low_cover.glb", decals: true, [Part::Box([-1.0, 0.0, -0.08], [1.0, 1.1, 0.08])]);
const WALL_WINDOW: Piece = piece!("models/map-kit/walls/wall_window.glb", decals: true, [
    Part::Box([-1.0, 0.0, -0.08], [1.0, 1.0, 0.08]),
    Part::Box([-1.0, 1.6, -0.08], [1.0, 2.4, 0.08]),
    Part::Box([-1.0, 1.0, -0.08], [-0.7, 1.6, 0.08]),
    Part::Box([0.7, 1.0, -0.08], [1.0, 1.6, 0.08]),
]);
const WALL_DOOR: Piece = piece!("models/map-kit/walls/wall_doorway.glb", decals: true, [
    Part::Box([-1.0, 0.0, -0.08], [-0.45, 2.4, 0.08]),
    Part::Box([0.45, 0.0, -0.08], [1.0, 2.4, 0.08]),
    Part::Box([-0.45, 2.0, -0.08], [0.45, 2.4, 0.08]),
]);
const CONCRETE: Piece = piece!("models/map-kit/walls/concrete_wall_2m.glb", decals: true, [Part::Box([-1.0, 0.0, -0.15], [1.0, 2.4, 0.15])]);
const JERSEY: Piece = piece!("models/map-kit/walls/jersey_barrier.glb", decals: true, [Part::Box([-1.0, 0.0, -0.3], [1.0, 0.8, 0.3])]);

// Map kit: structures.
const CONTAINER_BOX: Part = Part::Box([-3.04, 0.0, -1.28], [3.1, 2.59, 1.28]);
const CONT_GREEN: Piece =
    piece!("models/map-kit/structures/shipping_container_green.glb", decals: true, [CONTAINER_BOX]);
const CONT_RUST: Piece =
    piece!("models/map-kit/structures/shipping_container_rust.glb", decals: true, [CONTAINER_BOX]);
const CONT_BLUE: Piece =
    piece!("models/map-kit/structures/shipping_container_blue.glb", decals: true, [CONTAINER_BOX]);
const PLATFORM: Piece = piece!("models/map-kit/structures/platform_2x2.glb", decals: true, [Part::Box([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0])]);
const RAMP: Piece = piece!("models/map-kit/structures/ramp_2m.glb", decals: false, [Part::Ramp([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0])]);
const STAIRS: Piece = piece!("models/map-kit/structures/stairs_1m.glb", decals: false, [Part::Ramp([-0.6, 0.0, -1.0], [0.6, 1.0, 1.0])]);
const BRIDGE: Piece = piece!("models/map-kit/structures/bridge_4m.glb", decals: true, [Part::Box([-2.0, 0.0, -0.61], [2.0, 1.0, 0.61])]);
const SHACK: Piece = piece!("models/map-kit/structures/field_shack.glb", decals: true, [Part::Box([-1.7, 0.0, -1.8], [1.7, 3.0, 1.8])]);
const TOWER: Piece = piece!("models/map-kit/structures/watch_tower.glb", decals: true, [
    Part::Cyl(-1.1, -1.1, 0.12, 0.0, 3.0),
    Part::Cyl(1.1, -1.1, 0.12, 0.0, 3.0),
    Part::Cyl(-1.1, 1.1, 0.12, 0.0, 3.0),
    Part::Cyl(1.1, 1.1, 0.12, 0.0, 3.0),
    Part::Box([-1.3, 2.9, -1.3], [1.3, 4.1, 1.3]),
]);
const FENCE: Piece = piece!("models/map-kit/structures/fence_wood_2m.glb", decals: false, []);
const SCOREBOARD: Piece =
    piece!("models/map-kit/structures/scoreboard_stand.glb", decals: false, []);

// Map kit: props.
const PALLET_STACK: Piece = piece!("models/map-kit/props/pallet_stack.glb", decals: true, [Part::Box([-0.6, 0.0, -0.5], [0.6, 0.46, 0.5])]);
const PALLET: Piece = piece!("models/map-kit/props/pallet.glb", decals: false, []);
const TIRES: Piece = piece!("models/map-kit/props/tire_stack.glb", decals: true, [Part::Cyl(0.0, 0.0, 0.36, 0.0, 0.88)]);
const SPOOL: Piece = piece!("models/map-kit/props/cable_spool.glb", decals: true, [Part::Box([-0.68, 0.0, -0.48], [0.68, 1.38, 0.48])]);
const HAY: Piece = piece!("models/map-kit/props/hay_bale.glb", decals: true, [Part::Box([-0.5, 0.0, -0.24], [0.5, 0.51, 0.24])]);
const LOG_PILE: Piece = piece!("models/map-kit/props/log_pile.glb", decals: true, [Part::Box([-1.1, 0.0, -0.46], [1.1, 0.83, 0.46])]);
const FLOODLIGHT: Piece = piece!("models/map-kit/props/floodlight_pole.glb", decals: false, [Part::Cyl(0.0, 0.0, 0.12, 0.0, 6.25)]);
const SIGN: Piece = piece!("models/map-kit/props/sign_post.glb", decals: false, []);
const BOUNDARY_POST: Piece = piece!("models/map-kit/props/boundary_post.glb", decals: false, []);

// Map kit: nature.
const OAK: Piece = piece!("models/map-kit/nature/tree_oak.glb", decals: false, [Part::Cyl(0.0, 0.0, 0.3, 0.0, 4.0)]);
const PINE: Piece = piece!("models/map-kit/nature/tree_pine.glb", decals: false, [Part::Cyl(0.0, 0.0, 0.4, 0.0, 3.4)]);
const PINE_TALL: Piece = piece!("models/map-kit/nature/tree_pine_tall.glb", decals: false, [Part::Cyl(0.0, 0.0, 0.35, 0.0, 5.2)]);
const BUSH: Piece = piece!("models/map-kit/nature/bush.glb", decals: false, []);
const GRASS: Piece = piece!("models/map-kit/nature/grass_clump.glb", decals: false, []);
const ROCK_L: Piece = piece!("models/map-kit/nature/rock_large.glb", decals: false, [Part::Box([-1.6, 0.0, -1.0], [1.8, 1.9, 1.3])]);
const ROCK_M: Piece = piece!("models/map-kit/nature/rock_medium.glb", decals: false, [Part::Box([-0.65, 0.0, -0.6], [0.9, 0.85, 0.7])]);
const ROCK_S: Piece = piece!("models/map-kit/nature/rock_small.glb", decals: false, []);
const LOG: Piece = piece!("models/map-kit/nature/fallen_log.glb", decals: false, [Part::Box([-1.41, 0.0, -0.2], [1.41, 0.6, 0.43])]);
const STUMP: Piece = piece!("models/map-kit/nature/tree_stump.glb", decals: false, [Part::Cyl(0.0, 0.0, 0.3, 0.0, 0.46)]);

// Gadgets as cover and scenery.
const BARRICADE: Piece = piece!("models/weapons/gadgets/deployable_barricade.glb", decals: true, [Part::Box([-0.6, 0.0, -0.39], [0.6, 1.0, 0.03])]);
const GRENADE_CRATE: Piece = piece!("models/weapons/gadgets/grenade_crate.glb", decals: false, []);

// --- Layouts ------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GroundTex {
    Turf,
    Dirt,
}

/// A flat patch of ground: center, size, texture, tint.
#[derive(Clone, Copy, Debug)]
struct Patch {
    center: Vec2,
    size: Vec2,
    tex: GroundTex,
    tint: Color,
    tile: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Perimeter {
    Nets,
    Concrete,
    Fence,
}

/// A map, ready to spawn.
pub struct Layout {
    models: Vec<(&'static str, Transform)>,
    colliders: Vec<Collider>,
    info: MapInfo,
    patches: Vec<Patch>,
    sky: Color,
}

impl Layout {
    fn new(id: MapId, half: Vec2, sky: Color) -> Self {
        Self {
            models: Vec::new(),
            colliders: Vec::new(),
            info: MapInfo {
                id,
                half,
                ..default()
            },
            patches: Vec::new(),
            sky,
        }
    }

    fn patch(&mut self, center: Vec2, size: Vec2, tex: GroundTex, tint: Color, tile: f32) {
        self.patches.push(Patch {
            center,
            size,
            tex,
            tint,
            tile,
        });
    }

    fn model(&mut self, path: &'static str, transform: Transform) {
        self.models.push((path, transform));
    }

    /// Places `piece` at (x, z) turned `turns` quarter turns, lifted by `y`.
    fn place_y(&mut self, piece: &Piece, x: f32, y: f32, z: f32, turns: f32) {
        let rot = Quat::from_rotation_y(turns * FRAC_PI_2);
        let at = Vec3::new(x, y, z);
        self.model(
            piece.model,
            Transform::from_translation(at).with_rotation(rot),
        );
        for part in piece.parts {
            let shape = match *part {
                Part::Box(min, max) => {
                    let (lo, hi) = turned(rot, Vec3::from_array(min), Vec3::from_array(max));
                    Shape::Box {
                        min: lo + at,
                        max: hi + at,
                    }
                }
                Part::Cyl(cx, cz, r, y0, h) => Shape::Cylinder {
                    base: at + rot * Vec3::new(cx, y0, cz),
                    radius: r,
                    height: h,
                },
                Part::Ramp(min, max) => {
                    let (lo, hi) = turned(rot, Vec3::from_array(min), Vec3::from_array(max));
                    let up = rot * Vec3::NEG_Z;
                    Shape::Ramp {
                        min: lo + at,
                        max: hi + at,
                        rise: Vec2::new(up.x, up.z),
                    }
                }
            };
            self.colliders.push(Collider {
                shape,
                decals: piece.decals,
            });
        }
    }

    fn place(&mut self, piece: &Piece, x: f32, z: f32, turns: f32) {
        self.place_y(piece, x, 0.0, z, turns);
    }

    /// Places a piece and its twin turned half way round the middle, so both
    /// halves of the field are the same for each team.
    fn mirrored(&mut self, piece: &Piece, x: f32, z: f32, turns: f32) {
        self.place(piece, x, z, turns);
        self.place(piece, -x, -z, turns + 2.0);
    }

    /// Places a piece and its reflection across the center line (z -> -z).
    fn reflected(&mut self, piece: &Piece, x: f32, z: f32, turns: f32) {
        self.place(piece, x, z, turns);
        self.place(piece, x, -z, -turns);
    }

    /// A straight run of 2 m wall pieces from `a` to `b` (along x or z).
    fn wall_run(&mut self, pieces: &[&Piece], a: Vec2, b: Vec2) {
        let d = b - a;
        let len = d.length();
        let n = (len / 2.0).round().max(1.0) as usize;
        let along_x = d.x.abs() > d.y.abs();
        for i in 0..n {
            let t = (i as f32 + 0.5) / n as f32;
            let p = a + d * t;
            let piece = pieces[i % pieces.len()];
            self.place(piece, p.x, p.y, if along_x { 0.0 } else { 1.0 });
        }
    }

    fn spawns(&mut self, team: Team, points: &[(f32, f32)]) {
        for &(x, z) in points {
            self.info.spawns[team.index()].push(Vec3::new(x, 0.0, z));
        }
    }

    /// Blue spawns as given; orange gets them turned round the middle.
    fn mirrored_spawns(&mut self, points: &[(f32, f32)]) {
        self.spawns(Team::Blue, points);
        let other: Vec<_> = points.iter().map(|&(x, z)| (-x, -z)).collect();
        self.spawns(Team::Orange, &other);
    }

    fn ffa(&mut self, points: &[(f32, f32)]) {
        for &(x, z) in points {
            self.info.ffa.push(Vec3::new(x, 0.0, z));
        }
    }

    /// The edge: what it looks like, plus invisible walls that stop players and paint.
    fn perimeter(&mut self, kind: Perimeter) {
        let h = self.info.half;
        match kind {
            Perimeter::Nets => {
                let section = 2.474;
                for (len, along_x) in [(h.x * 2.0, true), (h.y * 2.0, false)] {
                    let count = (len / section).ceil() as i32;
                    let scale = len / (count as f32 * section);
                    for i in 0..count {
                        let along = -len * 0.5 + (i as f32 + 0.5) * section * scale;
                        for side in [-1.0, 1.0] {
                            let (x, z, rot) = if along_x {
                                (along, side * h.y, 0.0)
                            } else {
                                (side * h.x, along, FRAC_PI_2)
                            };
                            self.model(
                                "models/props/netting_fence_section.glb",
                                Transform::from_xyz(x, 0.0, z)
                                    .with_rotation(Quat::from_rotation_y(rot))
                                    .with_scale(Vec3::new(scale, 1.0, 1.0)),
                            );
                        }
                    }
                }
            }
            Perimeter::Concrete | Perimeter::Fence => {
                let (path, depth) = match kind {
                    Perimeter::Concrete => (CONCRETE.model, 0.15),
                    _ => (FENCE.model, 0.06),
                };
                for (len, along_x) in [(h.x * 2.0, true), (h.y * 2.0, false)] {
                    let count = (len / 2.0).round().max(1.0) as i32;
                    let scale = len / (count as f32 * 2.0);
                    for i in 0..count {
                        let along = -len * 0.5 + (i as f32 + 0.5) * 2.0 * scale;
                        for side in [-1.0, 1.0] {
                            let (x, z, rot) = if along_x {
                                (along, side * (h.y + depth), 0.0)
                            } else {
                                (side * (h.x + depth), along, FRAC_PI_2)
                            };
                            self.model(
                                path,
                                Transform::from_xyz(x, 0.0, z)
                                    .with_rotation(Quat::from_rotation_y(rot))
                                    .with_scale(Vec3::new(scale, 1.0, 1.0)),
                            );
                        }
                    }
                }
                // Corner posts.
                for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
                    self.model(
                        BOUNDARY_POST.model,
                        Transform::from_xyz(sx * (h.x + depth), 0.0, sz * (h.y + depth)),
                    );
                }
            }
        }
        let wall = 2.4;
        for (min, max) in [
            (
                Vec3::new(-h.x - 1.0, 0.0, -h.y - 1.0),
                Vec3::new(h.x + 1.0, wall, -h.y),
            ),
            (
                Vec3::new(-h.x - 1.0, 0.0, h.y),
                Vec3::new(h.x + 1.0, wall, h.y + 1.0),
            ),
            (Vec3::new(-h.x - 1.0, 0.0, -h.y), Vec3::new(-h.x, wall, h.y)),
            (Vec3::new(h.x, 0.0, -h.y), Vec3::new(h.x + 1.0, wall, h.y)),
        ] {
            self.colliders.push(Collider {
                shape: Shape::Box { min, max },
                decals: false,
            });
        }
    }

    /// The supply props (paint, pods, air) behind each team's spawn, at z = ±`z`.
    fn team_supplies(&mut self, z: f32) {
        for side in [1.0, -1.0] {
            self.model(
                "models/pickups/paint_case.glb",
                Transform::from_xyz(-5.5, 0.0, side * (z + 1.0))
                    .with_rotation(Quat::from_rotation_y(0.4)),
            );
            self.model(
                "models/pickups/pod_pickup.glb",
                Transform::from_xyz(-5.0, 0.3, side * (z + 1.2)),
            );
            self.model(
                "models/pickups/air_refill_station.glb",
                Transform::from_xyz(5.5, 0.0, side * (z + 1.0))
                    .with_rotation(Quat::from_rotation_y(if side > 0.0 { PI } else { 0.0 })),
            );
        }
    }

    fn finish(mut self) -> Self {
        self.info.footprints = self
            .colliders
            .iter()
            .filter(|c| {
                // Skip the invisible edge walls: the minimap frame shows those.
                let r = c.shape.footprint();
                r.width() < self.info.half.x * 1.5 && r.height() < self.info.half.y * 1.5
            })
            .map(|c| (c.shape.footprint(), c.shape.height()))
            .collect();
        self
    }
}

fn turned(rot: Quat, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for x in [min.x, max.x] {
        for z in [min.z, max.z] {
            let p = rot * Vec3::new(x, 0.0, z);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    // Quarter turns: snap away float noise.
    let snap = |v: Vec3| (v * 1000.0).round() / 1000.0;
    (snap(lo).with_y(min.y), snap(hi).with_y(max.y))
}

const TURF_TINT: Color = Color::WHITE;
const DIRT_TINT: Color = Color::WHITE;

pub fn layout(id: MapId) -> Layout {
    match id {
        MapId::SplatPark => splat_park(),
        MapId::Dockyard => dockyard(),
        MapId::RingRoad => ring_road(),
        MapId::ThreeLanes => three_lanes(),
        MapId::PineHollow => pine_hollow(),
    }
}

/// The original airball field.
fn splat_park() -> Layout {
    let mut l = Layout::new(
        MapId::SplatPark,
        Vec2::splat(30.0),
        Color::srgb(0.55, 0.76, 0.96),
    );
    l.patch(
        Vec2::ZERO,
        Vec2::splat(60.0),
        GroundTex::Turf,
        TURF_TINT,
        3.0,
    );
    l.patch(
        Vec2::ZERO,
        Vec2::new(60.0, 2.0),
        GroundTex::Dirt,
        DIRT_TINT,
        2.0,
    );

    // One half; the other is the same turned round the middle.
    for &(piece, x, z, turns) in &[
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
        (&BUNKER_TREE, -9.0, 3.0, 0.0),
        (&CRATE, 22.0, 9.0, 0.0),
        (&CRATE, 22.9, 9.3, 0.0),
        (&BARREL, 21.6, 10.2, 0.0),
        (&SANDBAGS, -24.0, 19.0, 0.0),
        (&SANDBAGS, 24.0, 19.0, 0.0),
        (&BRICK, 8.0, 21.0, 0.0),
        (&BRICK, -8.0, 21.0, 0.0),
    ] {
        l.mirrored(piece, x, z, turns);
    }
    for &(piece, x, z, turns) in &[
        (&TEMPLE, 0.0, 0.0, 1.0),
        (&CAN, -13.0, 0.0, 0.0),
        (&CAN, 13.0, 0.0, 0.0),
        (&SNAKE, -6.5, 0.0, 0.0),
        (&SNAKE, 6.5, 0.0, 0.0),
        (&MINI_CAN, -24.0, 0.0, 0.0),
        (&MINI_CAN, 24.0, 0.0, 0.0),
    ] {
        l.place(piece, x, z, turns);
    }
    for i in [-4.0f32, -3.0, -1.0, 1.0, 3.0, 4.0] {
        l.model(
            "models/props/field_cone.glb",
            Transform::from_xyz(i * 3.0, 0.0, 0.0),
        );
    }
    l.team_supplies(24.0);
    l.perimeter(Perimeter::Nets);
    l.place(&SCOREBOARD, 0.0, -33.5, 0.0);

    let mut blue = Vec::new();
    for slot in 0..10 {
        let s = slot as f32;
        blue.push(((s % 5.0 - 2.0) * 1.6, 24.0 + (s / 5.0).floor() * 1.4));
    }
    l.mirrored_spawns(&blue);
    let mut all: Vec<(f32, f32)> = blue.iter().step_by(2).copied().collect();
    all.extend(blue.iter().step_by(2).map(|&(x, z)| (-x, -z)));
    all.extend([
        (27.0, 0.0),
        (-27.0, 0.0),
        (22.0, 16.0),
        (-22.0, 16.0),
        (22.0, -16.0),
        (-22.0, -16.0),
        (10.0, 4.0),
        (-10.0, -4.0),
    ]);
    l.ffa(&all);
    l.finish()
}

/// A small, tight container yard: lots of hard cover, short sightlines.
fn dockyard() -> Layout {
    let mut l = Layout::new(
        MapId::Dockyard,
        Vec2::splat(17.0),
        Color::srgb(0.93, 0.72, 0.56),
    );
    let asphalt = Color::srgb(0.52, 0.53, 0.56);
    l.patch(Vec2::ZERO, Vec2::splat(34.0), GroundTex::Dirt, asphalt, 4.0);
    // Painted lanes where the trucks drive.
    l.patch(
        Vec2::new(0.0, 0.0),
        Vec2::new(34.0, 0.25),
        GroundTex::Dirt,
        Color::srgb(0.95, 0.82, 0.25),
        1.0,
    );

    for &(piece, x, z, turns) in &[
        (&CONT_GREEN, -9.0, 8.0, 0.0),
        (&CONT_RUST, 9.5, 9.0, 1.0),
        (&CONT_BLUE, 4.5, 5.0, 0.0),
        (&CONT_RUST, -13.5, 1.0, 1.0),
        (&JERSEY, -4.0, 4.5, 0.0),
        (&JERSEY, 6.0, 12.0, 0.0),
        (&JERSEY, -6.0, 12.5, 0.0),
        (&SPOOL, -3.5, 9.0, 0.0),
        (&TIRES, 8.0, 1.5, 0.0),
        (&TIRES, 8.6, 2.1, 0.0),
        (&PALLET_STACK, -8.6, 4.2, 1.0),
        (&PALLET_STACK, -7.5, 4.2, 1.0),
        (&CRATE, 13.0, 5.5, 0.0),
        (&CRATE, 13.85, 5.7, 0.0),
        (&BARREL, 13.4, 6.8, 0.0),
        (&SANDBAGS, 0.0, 9.5, 0.0),
        (&WALL_LOW, -14.5, 9.5, 1.0),
        (&BARRICADE, 3.0, 1.5, 0.0),
        (&BARREL, -1.9, 4.6, 0.0),
        (&CRATE, 15.6, 12.5, 0.0),
    ] {
        l.mirrored(piece, x, z, turns);
    }
    // A two-high stack on each side: a landmark you can't see over.
    l.place_y(&CONT_GREEN, -9.0, 2.59, 8.0, 0.0);
    l.place_y(&CONT_GREEN, 9.0, 2.59, -8.0, 2.0);
    // The middle: one container across the center line.
    l.place(&CONT_BLUE, 0.0, 0.0, 1.0);
    for (x, z) in [(-16.2, -16.2), (16.2, 16.2), (-16.2, 16.2), (16.2, -16.2)] {
        l.place(&FLOODLIGHT, x, z, if x * z > 0.0 { 0.5 } else { 1.5 });
    }
    l.model(
        PALLET.model,
        Transform::from_xyz(-11.5, 0.0, 12.0).with_rotation(Quat::from_rotation_y(0.3)),
    );
    l.model(
        PALLET.model,
        Transform::from_xyz(11.5, 0.0, -12.0).with_rotation(Quat::from_rotation_y(0.3)),
    );
    l.model(
        GRENADE_CRATE.model,
        Transform::from_xyz(12.3, 0.0, 14.2).with_rotation(Quat::from_rotation_y(0.6)),
    );
    l.model(
        GRENADE_CRATE.model,
        Transform::from_xyz(-12.3, 0.0, -14.2).with_rotation(Quat::from_rotation_y(0.6)),
    );
    l.perimeter(Perimeter::Concrete);

    let blue = [
        (-10.0, 15.2),
        (-5.0, 15.2),
        (0.0, 15.2),
        (5.0, 15.2),
        (10.0, 15.2),
        (-12.0, 13.8),
        (-2.5, 13.8),
        (2.5, 13.8),
        (12.0, 13.8),
    ];
    l.mirrored_spawns(&blue);
    let mut all = vec![
        (15.5, 0.0),
        (-15.5, 0.0),
        (6.0, 0.0),
        (-6.0, 0.0),
        (0.0, 7.0),
        (0.0, -7.0),
        (-15.4, 14.8),
        (15.4, -14.8),
        (11.5, 4.0),
        (-11.5, -4.0),
    ];
    all.extend(blue.iter().step_by(2).copied());
    all.extend(blue.iter().step_by(2).map(|&(x, z)| (-x, -z)));
    l.ffa(&all);
    l.finish()
}

/// Outer loop around two walled blocks; cross-paths through the blocks and
/// down the middle. Several ways in and out of everything, no dead ends.
fn ring_road() -> Layout {
    let mut l = Layout::new(
        MapId::RingRoad,
        Vec2::new(30.0, 24.0),
        Color::srgb(0.60, 0.78, 0.95),
    );
    l.patch(
        Vec2::ZERO,
        Vec2::new(60.0, 48.0),
        GroundTex::Turf,
        TURF_TINT,
        3.0,
    );
    let road = Color::srgb(0.80, 0.74, 0.66);
    // The ring: both sides and both ends.
    for x in [-24.5, 24.5] {
        l.patch(
            Vec2::new(x, 0.0),
            Vec2::new(9.0, 48.0),
            GroundTex::Dirt,
            road,
            3.0,
        );
    }
    for z in [-16.5, 16.5] {
        l.patch(
            Vec2::new(0.0, z),
            Vec2::new(60.0, 13.0),
            GroundTex::Dirt,
            road,
            3.0,
        );
    }
    // The middle cross-path.
    l.patch(
        Vec2::ZERO,
        Vec2::new(14.0, 18.0),
        GroundTex::Dirt,
        road,
        3.0,
    );

    // The east block (the west one is it turned round the middle).
    let block = |l: &mut Layout, sx: f32| {
        let x0 = 7.0 * sx;
        let x1 = 19.0 * sx;
        let z_side = |l: &mut Layout, x: f32| {
            for (z, piece) in [
                (-8.0, &WALL_2M),
                (-6.0, &WALL_WINDOW),
                (-4.0, &WALL_2M),
                (-2.0, &WALL_2M),
                (0.0, &WALL_DOOR),
                (2.0, &WALL_2M),
                (4.0, &WALL_2M),
                (6.0, &WALL_WINDOW),
                (8.0, &WALL_2M),
            ] {
                l.place(piece, x, z, 1.0);
            }
        };
        z_side(l, x0);
        z_side(l, x1);
        for z in [-9.0, 9.0] {
            for (i, piece) in [
                &WALL_2M,
                &WALL_2M,
                &WALL_WINDOW,
                &WALL_WINDOW,
                &WALL_2M,
                &WALL_2M,
            ]
            .into_iter()
            .enumerate()
            {
                let x = (8.0 + i as f32 * 2.0) * sx;
                l.place(piece, x, z, 0.0);
            }
        }
        // Inside: crates and spools to fight around.
        for &(piece, x, z, turns) in &[
            (&SPOOL, 11.0, -5.0, 0.0),
            (&CRATE, 16.0, -6.0, 0.0),
            (&CRATE, 16.85, -5.75, 0.0),
            (&TIRES, 13.0, 3.0, 0.0),
            (&TIRES, 13.7, 3.4, 0.0),
            (&PALLET_STACK, 16.0, 4.5, 0.0),
            (&BARREL, 10.0, 6.5, 0.0),
            (&HAY, 10.5, -1.6, 1.0),
        ] {
            l.place(piece, x * sx, z * sx, turns);
        }
    };
    block(&mut l, 1.0);
    block(&mut l, -1.0);

    // The middle: a raised platform with ramps at both ends.
    l.place(&PLATFORM, 0.0, 0.0, 0.0);
    l.place(&RAMP, 0.0, 2.0, 0.0);
    l.place(&RAMP, 0.0, -2.0, 2.0);
    for &(piece, x, z, turns) in &[
        (&JERSEY, 3.5, 6.0, 0.0),
        (&JERSEY, -3.5, 6.0, 0.0),
        (&WALL_LOW, 4.5, 0.0, 1.0),
        (&HAY, -5.2, 11.5, 0.0),
        (&HAY, 5.2, 11.5, 0.0),
        // The ring.
        (&CONT_GREEN, 25.0, 0.0, 1.0),
        (&JERSEY, 22.3, 13.0, 1.0),
        (&JERSEY, 27.6, -12.0, 1.0),
        (&TIRES, 28.0, 8.0, 0.0),
        (&TIRES, 28.6, 8.5, 0.0),
        (&LOG_PILE, 21.6, -7.0, 1.0),
        (&CONT_RUST, 26.0, 20.0, 0.0),
        (&CONT_RUST, -26.0, 20.0, 0.0),
        (&SANDBAGS, 13.0, 14.0, 0.0),
        (&SANDBAGS, -13.0, 14.0, 0.0),
        (&PALLET_STACK, 0.0, 14.0, 0.0),
        (&BARRICADE, 6.0, 16.0, 0.0),
        (&BARRICADE, -6.0, 16.0, 0.0),
        (&SPOOL, 19.5, 12.5, 0.0),
    ] {
        l.mirrored(piece, x, z, turns);
    }
    for (x, z) in [(-29.0, -23.0), (29.0, 23.0), (-29.0, 23.0), (29.0, -23.0)] {
        l.place(&FLOODLIGHT, x, z, if x * z > 0.0 { 0.5 } else { 1.5 });
    }
    for (x, z, t) in [(10.0, 9.8, 0.0), (-10.0, -9.8, 2.0)] {
        l.place(&SIGN, x, z, t);
    }
    l.team_supplies(21.0);
    l.perimeter(Perimeter::Concrete);

    let blue = [
        (-14.0, 21.5),
        (-7.0, 21.5),
        (0.0, 19.0),
        (7.0, 21.5),
        (14.0, 21.5),
        (-10.5, 19.5),
        (-3.5, 21.8),
        (3.5, 21.8),
        (10.5, 19.5),
    ];
    l.mirrored_spawns(&blue);
    let mut all = vec![
        (24.5, 8.0),
        (-24.5, -8.0),
        (24.5, -8.0),
        (-24.5, 8.0),
        (13.0, 0.0),
        (-13.0, 0.0),
        (0.0, 9.0),
        (0.0, -9.0),
        (21.0, 19.0),
        (-21.0, -19.0),
    ];
    all.extend(blue.iter().step_by(2).copied());
    all.extend(blue.iter().step_by(2).map(|&(x, z)| (-x, -z)));
    l.ffa(&all);
    l.finish()
}

/// A main lane and two flanks (the Yard, x > 0, and the Woods, x < 0),
/// linked by connectors at the middle and near each base.
fn three_lanes() -> Layout {
    let mut l = Layout::new(
        MapId::ThreeLanes,
        Vec2::new(22.0, 38.0),
        Color::srgb(0.66, 0.80, 0.95),
    );
    let quarry = Color::srgb(0.95, 0.86, 0.74);
    l.patch(
        Vec2::ZERO,
        Vec2::new(44.0, 76.0),
        GroundTex::Dirt,
        quarry,
        4.0,
    );
    // Mid lane is turf, the Woods are darker grass, the Yard stays dirt.
    l.patch(
        Vec2::ZERO,
        Vec2::new(17.0, 76.0),
        GroundTex::Turf,
        TURF_TINT,
        3.0,
    );
    l.patch(
        Vec2::new(-16.0, 0.0),
        Vec2::new(12.0, 60.0),
        GroundTex::Turf,
        Color::srgb(0.72, 0.86, 0.66),
        3.0,
    );

    // Lane dividers, gaps at z = 0 and z = ±15.5.
    for z in [5.0, 11.0, 20.5, 26.6] {
        l.reflected(&CONT_RUST, 10.0, z, 1.0);
    }
    for (a, b) in [(2.0, 14.0), (17.0, 30.0)] {
        for sz in [1.0, -1.0] {
            l.wall_run(
                &[&CONCRETE],
                Vec2::new(-10.0, a * sz),
                Vec2::new(-10.0, b * sz),
            );
        }
    }

    // Mid lane.
    l.place(&TEMPLE, 0.0, 0.0, 1.0);
    for &(piece, x, z, turns) in &[
        (&CAN, -4.0, 0.0, 0.0),
        (&CAN, 4.0, 0.0, 0.0),
        (&BRICK, -5.0, 8.5, 0.0),
        (&BRICK, 5.0, 8.5, 0.0),
        (&SNAKE, 0.0, 13.0, 0.0),
        (&CAKE, -3.5, 18.0, 0.0),
        (&MINI_CAN, 3.5, 18.5, 0.0),
        (&TALL_WALL, -4.5, 24.0, 0.0),
        (&TALL_WALL, 4.5, 24.0, 0.0),
        (&DORITO, 0.0, 27.0, 2.0),
        // The Yard.
        (&CRATE, 16.0, 4.0, 0.0),
        (&CRATE, 16.85, 4.25, 0.0),
        (&PALLET_STACK, 19.0, 9.0, 1.0),
        (&SPOOL, 14.0, 12.0, 0.0),
        (&TIRES, 18.0, 17.0, 0.0),
        (&TIRES, 18.7, 17.4, 0.0),
        (&JERSEY, 16.0, 22.0, 0.0),
        (&BARREL, 20.5, 26.0, 0.0),
        (&BARRICADE, 13.5, 1.5, 0.0),
        // The Woods.
        (&LOG, -16.0, 5.0, 0.0),
        (&ROCK_M, -19.0, 10.0, 0.0),
        (&PINE, -13.5, 12.0, 0.0),
        (&STUMP, -17.0, 15.5, 0.0),
        (&LOG_PILE, -15.0, 21.0, 1.0),
        (&ROCK_L, -18.0, 26.0, 0.0),
        (&PINE_TALL, -20.5, 18.5, 0.0),
        (&OAK, -14.0, 28.5, 0.0),
        // Base cover.
        (&SANDBAGS, -6.0, 31.0, 0.0),
        (&SANDBAGS, 6.0, 31.0, 0.0),
        (&JERSEY, -15.0, 31.0, 0.0),
        (&JERSEY, 15.0, 31.0, 0.0),
    ] {
        l.reflected(piece, x, z, turns);
    }
    for &(x, z) in &[(-20.0, 3.0), (-12.5, 19.0), (-20.5, 23.0), (-12.0, 6.5)] {
        for sz in [1.0, -1.0] {
            l.place(&BUSH, x, z * sz, 0.0);
        }
    }
    for &(x, z) in &[(-18.0, 1.0), (-13.0, 25.0)] {
        for sz in [1.0, -1.0] {
            l.place(&ROCK_S, x, z * sz, 0.0);
        }
    }
    // Callout signs at the connectors.
    for (x, z) in [
        (12.0, 1.8),
        (-12.0, -1.8),
        (12.0, 15.5),
        (-12.0, -15.5),
        (12.0, -15.5),
        (-12.0, 15.5),
    ] {
        l.place(&SIGN, x, z, 0.0);
    }
    for (x, z) in [(-21.0, -37.0), (21.0, 37.0), (-21.0, 37.0), (21.0, -37.0)] {
        l.place(&FLOODLIGHT, x, z, if x * z > 0.0 { 0.5 } else { 1.5 });
    }
    l.team_supplies(34.0);
    l.perimeter(Perimeter::Fence);

    let blue = [
        (-12.0, 35.0),
        (-6.0, 35.0),
        (0.0, 33.0),
        (6.0, 35.0),
        (12.0, 35.0),
        (-15.0, 33.5),
        (-3.0, 35.5),
        (3.0, 35.5),
        (15.0, 33.5),
    ];
    l.spawns(Team::Blue, &blue);
    let orange: Vec<_> = blue.iter().map(|&(x, z)| (x, -z)).collect();
    l.spawns(Team::Orange, &orange);
    let mut all = vec![
        (16.0, 0.0),
        (-16.0, 0.0),
        (0.0, 10.0),
        (0.0, -10.0),
        (19.0, 14.0),
        (19.0, -14.0),
        (-19.5, 14.0),
        (-19.5, -14.0),
        (8.0, 4.5),
        (-8.0, -4.5),
    ];
    all.extend(blue.iter().step_by(2).copied());
    all.extend(orange.iter().step_by(2).copied());
    l.ffa(&all);
    l.finish()
}

/// Woods with height: a skybridge in the middle, a perch on each side.
fn pine_hollow() -> Layout {
    let mut l = Layout::new(
        MapId::PineHollow,
        Vec2::splat(30.0),
        Color::srgb(0.70, 0.82, 0.90),
    );
    let forest = Color::srgb(0.74, 0.88, 0.70);
    l.patch(Vec2::ZERO, Vec2::splat(60.0), GroundTex::Turf, forest, 3.0);
    // A worn path between the bases.
    l.patch(
        Vec2::ZERO,
        Vec2::new(3.0, 60.0),
        GroundTex::Dirt,
        Color::srgb(0.85, 0.78, 0.68),
        2.0,
    );

    // The skybridge.
    l.place(&BRIDGE, 0.0, 0.0, 0.0);
    for &(piece, x, z, turns) in &[
        (&PLATFORM, 3.0, 0.0, 0.0),
        (&RAMP, 5.0, 0.0, 1.0),
        // Perch with stairs.
        (&PLATFORM, 12.0, 9.0, 0.0),
        (&STAIRS, 12.0, 11.0, 0.0),
        (&SHACK, -17.0, 12.0, 2.0),
        (&TOWER, 20.0, 20.0, 0.0),
        (&LOG, -6.0, 9.0, 0.0),
        (&LOG, 7.0, 4.0, 1.0),
        (&ROCK_L, -10.0, 4.0, 0.0),
        (&ROCK_M, -3.5, 6.0, 0.0),
        (&ROCK_M, 15.5, 3.0, 1.0),
        (&PINE, -22.0, 6.0, 0.0),
        (&PINE, -14.0, 2.0, 0.0),
        (&PINE, 16.0, 7.0, 0.0),
        (&PINE_TALL, 22.0, 12.0, 0.0),
        (&PINE, -8.0, 16.0, 0.0),
        (&PINE_TALL, 5.0, 17.0, 0.0),
        (&OAK, 0.0, 10.0, 0.0),
        (&OAK, -24.0, 18.0, 0.0),
        (&HAY, -2.0, 20.0, 0.0),
        (&HAY, 2.0, 20.0, 0.0),
        (&LOG_PILE, 10.0, 20.0, 0.0),
        (&STUMP, -12.0, 9.0, 0.0),
        (&STUMP, 14.0, 14.0, 0.0),
        (&BARRICADE, -6.0, 22.0, 0.0),
        (&BARRICADE, 6.0, 22.5, 0.0),
        (&WALL_LOW, -22.0, 24.0, 0.0),
        (&ROCK_M, 24.5, 4.5, 0.0),
    ] {
        l.mirrored(piece, x, z, turns);
    }
    for &(x, z) in &[
        (-20.0, 0.0),
        (18.0, -3.0),
        (-9.0, 12.0),
        (9.0, 13.5),
        (-25.0, 10.0),
        (26.0, 16.0),
    ] {
        l.place(&BUSH, x, z, 0.0);
        l.place(&BUSH, -x, -z, 0.0);
    }
    for &(x, z) in &[
        (-4.0, 14.0),
        (8.0, 7.0),
        (-15.0, 18.0),
        (19.0, 8.0),
        (-1.0, 4.0),
    ] {
        l.place(&GRASS, x, z, 0.0);
        l.place(&GRASS, -x, -z, 0.0);
    }
    for &(x, z) in &[(-11.0, 18.5), (17.0, 1.0)] {
        l.place(&ROCK_S, x, z, 0.0);
        l.place(&ROCK_S, -x, -z, 0.0);
    }
    // A treeline outside the fence.
    let mut i = 0;
    let mut along = -32.0;
    while along <= 32.0 {
        let jitter = ((i * 37) % 7) as f32 * 0.35;
        let tall = i % 3 == 0;
        let tree = if tall { &PINE_TALL } else { &PINE };
        for (x, z) in [
            (along, 33.0 + jitter),
            (along + 1.5, -33.0 - jitter),
            (33.0 + jitter, along + 0.8),
            (-33.0 - jitter, along - 0.8),
        ] {
            l.model(tree.model, Transform::from_xyz(x, 0.0, z));
        }
        along += 4.2;
        i += 1;
    }
    l.team_supplies(26.5);
    l.perimeter(Perimeter::Fence);

    let blue = [
        (-12.0, 26.0),
        (-6.0, 26.0),
        (0.0, 24.0),
        (6.0, 26.0),
        (12.0, 26.0),
        (-9.0, 24.0),
        (-3.0, 27.0),
        (3.0, 27.0),
        (9.0, 24.0),
    ];
    l.mirrored_spawns(&blue);
    let mut all = vec![
        (24.0, 0.0),
        (-24.0, 0.0),
        (0.0, 13.0),
        (0.0, -13.0),
        (13.0, -8.0),
        (-13.0, 8.0),
        (20.0, -15.0),
        (-20.0, 15.0),
        (8.0, -2.5),
        (-8.0, 2.5),
    ];
    all.extend(blue.iter().step_by(2).copied());
    all.extend(blue.iter().step_by(2).map(|&(x, z)| (-x, -z)));
    l.ffa(&all);
    l.finish()
}

// --- Spawning -----------------------------------------------------------

#[derive(Resource)]
struct GroundTextures {
    turf: Handle<Image>,
    dirt: Handle<Image>,
}

fn load_ground_textures(mut commands: Commands, assets: Res<AssetServer>) {
    let repeat = |s: &mut ImageLoaderSettings| {
        s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            ..default()
        });
    };
    commands.insert_resource(GroundTextures {
        turf: assets
            .load_builder()
            .with_settings(repeat)
            .load("textures/ground/turf_tile.png"),
        dirt: assets
            .load_builder()
            .with_settings(repeat)
            .load("textures/ground/dirt_tile.png"),
    });
}

/// Builds the current map whenever it changes (and the first time).
#[allow(clippy::too_many_arguments)]
fn rebuild_map(
    mut commands: Commands,
    current: Res<CurrentMap>,
    assets: Res<AssetServer>,
    ground: Option<Res<GroundTextures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<MapEntity>>,
    mut built: Local<Option<MapId>>,
) {
    let Some(ground) = ground else {
        return;
    };
    if *built == Some(current.0) {
        return;
    }
    *built = Some(current.0);
    for e in &existing {
        commands.entity(e).despawn();
    }
    let layout = layout(current.0);
    commands.insert_resource(ClearColor(layout.sky));

    let half = layout.info.half;
    // Dirt all around, beyond the edge.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(240.0, 240.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.86, 0.82, 0.76),
            base_color_texture: Some(ground.dirt.clone()),
            uv_transform: Affine2::from_scale(Vec2::splat(240.0 / 4.0)),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.03, 0.0),
        MapEntity,
    ));
    // The field: a slab with its top at y = 0 (so nothing shows under it).
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(half.x * 2.0, 1.0, half.y * 2.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.3, 0.3),
            ..default()
        })),
        Transform::from_xyz(0.0, -0.505, 0.0),
        MapEntity,
    ));
    for (i, p) in layout.patches.iter().enumerate() {
        let tex = match p.tex {
            GroundTex::Turf => ground.turf.clone(),
            GroundTex::Dirt => ground.dirt.clone(),
        };
        commands.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(p.size.x, p.size.y))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: p.tint,
                base_color_texture: Some(tex),
                uv_transform: Affine2::from_scale(p.size / p.tile),
                perceptual_roughness: 0.95,
                ..default()
            })),
            // Later patches sit a hair higher so they never flicker.
            Transform::from_xyz(p.center.x, i as f32 * 0.003, p.center.y),
            MapEntity,
        ));
    }
    for (path, transform) in &layout.models {
        commands.spawn((
            WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(path.to_string()))),
            *transform,
            MapEntity,
        ));
    }
    commands.insert_resource(Colliders(layout.colliders));
    commands.insert_resource(layout.info);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spawn_is_clear_and_inside() {
        for id in MapId::ALL {
            let l = layout(id);
            let colliders = Colliders(l.colliders.clone());
            let mut points: Vec<Vec3> = l.info.ffa.clone();
            points.extend(l.info.spawns[0].iter());
            points.extend(l.info.spawns[1].iter());
            assert!(l.info.spawns[0].len() >= 8, "{id:?}");
            assert_eq!(l.info.spawns[0].len(), l.info.spawns[1].len(), "{id:?}");
            assert!(l.info.ffa.len() >= 12, "{id:?}");
            for p in points {
                assert!(
                    p.x.abs() < l.info.half.x - 0.5 && p.z.abs() < l.info.half.y - 0.5,
                    "{id:?}: spawn {p} is outside"
                );
                let mut pos = Vec3::new(p.x, PLAYER_GROUND_Y, p.z);
                let floor = colliders.resolve_player(&mut pos, 0.42, 0.0, 1.8);
                assert!(
                    pos.distance(Vec3::new(p.x, PLAYER_GROUND_Y, p.z)) < 1e-4 && floor == 0.0,
                    "{id:?}: spawn {p} is inside something"
                );
            }
        }
    }

    #[test]
    fn teams_spawn_on_their_own_side() {
        for id in MapId::ALL {
            let l = layout(id);
            assert!(l.info.spawns[0].iter().all(|p| p.z > 10.0), "{id:?}");
            assert!(l.info.spawns[1].iter().all(|p| p.z < -10.0), "{id:?}");
        }
    }

    #[test]
    fn spawning_avoids_threats() {
        let l = layout(MapId::Dockyard);
        let threat = l.info.ffa[0];
        let (pos, _) = l.info.spawn_for(3, true, &[threat], 0);
        assert!(pos.with_y(0.0).distance(threat.with_y(0.0)) > 10.0);
        // Teams stay on their side even with threats about.
        let (pos, yaw) = l.info.spawn_for(2, false, &[Vec3::ZERO], 0);
        assert!(pos.z > 10.0);
        assert_eq!(yaw, 0.0);
        let (pos, yaw) = l.info.spawn_for(1, false, &[], 0);
        assert!(pos.z < -10.0);
        assert_eq!(yaw, PI);
    }

    #[test]
    fn ramps_rise_toward_their_platform() {
        let l = layout(MapId::RingRoad);
        let colliders = Colliders(l.colliders.clone());
        // Walking up the north ramp (z = 3 -> 1) ends on the platform deck.
        let mut pos = Vec3::new(0.0, 0.9 + 0.5, 2.0);
        let floor = colliders.resolve_player(&mut pos, 0.42, 0.45, 2.3);
        assert!((floor - 0.5).abs() < 0.05, "floor {floor}");
        let l = layout(MapId::PineHollow);
        let colliders = Colliders(l.colliders.clone());
        let mut pos = Vec3::new(4.5, 0.9 + 0.75, 0.0);
        let floor = colliders.resolve_player(&mut pos, 0.42, 0.7, 2.5);
        assert!((floor - 0.75).abs() < 0.05, "floor {floor}");
    }
}
