//! The maps. Each one is a map file (the format is in the `ink-map` crate):
//! the field's size and ground, every placed piece, and where teams spawn.
//! The host picks the map; everyone builds the same one when the match tells
//! them which.
//!
//! The five that come with the game live in `assets/maps` and are built into
//! the game, so every player has exactly the same ones:
//!
//! - **Splat Park**: the classic airball field with inflatable bunkers.
//! - **Dockyard**: a small, tight container yard packed with cover.
//! - **Ring Road**: a big outer loop around two walled blocks, joined by
//!   cross-paths through the blocks and down the middle.
//! - **Three Lanes**: a wide middle lane and two flanks (the Yard and the
//!   Woods), linked by connectors at the center and near each base.
//! - **Pine Hollow**: woods with a raised skybridge in the middle and perches.
//!
//! Custom maps made with the map editor are read at startup from the maps
//! folder in the game's settings folder (see [`custom_maps_dir`]) and from
//! `assets/maps`. Everyone in a match needs the same file: a player who
//! doesn't have the host's map gets Splat Park's layout in its place.

use crate::{
    arena::{Collider, Colliders},
    protocol::{MapId, PlayerId, Team},
    world::{MainCamera, PLAYER_GROUND_Y, Sun},
};
use bevy::{
    asset::io::file::FileAssetReader,
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    math::Affine2,
    prelude::*,
};
use ink_map::{Daylight, GroundTex, MapFile, Patch};
use std::{f32::consts::PI, path::PathBuf};

pub struct MapsPlugin;

impl Plugin for MapsPlugin {
    fn build(&self, app: &mut App) {
        let library = MapLibrary::load();
        let layout = library.layout(MapId::default());
        app.init_resource::<CurrentMap>()
            .insert_resource(Colliders(layout.colliders.clone()))
            .insert_resource(layout.info.clone())
            .insert_resource(library)
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

// --- The map library ------------------------------------------------------

/// The built-in maps, compiled in so every player has the same ones.
const BUILT_IN_FILES: [(MapId, &str); 6] = [
    (MapId::SPLAT_PARK, include_str!("../assets/maps/SplatPark.json")),
    (MapId::DOCKYARD, include_str!("../assets/maps/Dockyard.json")),
    (MapId::RING_ROAD, include_str!("../assets/maps/RingRoad.json")),
    (MapId::THREE_LANES, include_str!("../assets/maps/ThreeLanes.json")),
    (MapId::PINE_HOLLOW, include_str!("../assets/maps/PineHollow.json")),
    (MapId::CITY, include_str!("../assets/maps/City.json")),
];

/// Where the map editor saves custom maps and the game looks for them.
pub fn custom_maps_dir() -> Option<PathBuf> {
    crate::loadout::config_file("maps")
}

/// Every map this player has: the built-in ones first, then custom maps.
#[derive(Resource, Debug, Clone)]
pub struct MapLibrary {
    maps: Vec<(MapId, MapFile)>,
}

impl MapLibrary {
    /// Only the maps that come with the game.
    pub fn built_in() -> Self {
        let maps = BUILT_IN_FILES
            .iter()
            .map(|&(id, json)| {
                let file = MapFile::from_json(json)
                    .unwrap_or_else(|e| panic!("built-in map {id} is broken: {e}"));
                (id, file)
            })
            .collect();
        Self { maps }
    }

    /// The built-in maps plus every custom map in the maps folders.
    pub fn load() -> Self {
        let mut library = Self::built_in();
        let dirs = [
            Some(FileAssetReader::get_base_path().join("assets").join("maps")),
            custom_maps_dir(),
        ];
        for dir in dirs.into_iter().flatten() {
            library.add_dir(&dir);
        }
        library
    }

    /// Adds the map files in `dir` (ids already taken are skipped).
    fn add_dir(&mut self, dir: &std::path::Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == ink_map::EXTENSION))
            .collect();
        paths.sort();
        for path in paths {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let id = MapId::new(&ink_map::id_from_stem(stem));
            if self.get(id).is_some() {
                continue;
            }
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| MapFile::from_json(&t)) {
                Ok(file) => {
                    info!("Loaded map {id} from {}", path.display());
                    self.maps.push((id, file));
                }
                Err(e) => warn!("Skipping map {}: {e}", path.display()),
            }
        }
    }

    /// Every map id, built-in ones first.
    pub fn ids(&self) -> impl Iterator<Item = MapId> + '_ {
        self.maps.iter().map(|(id, _)| *id)
    }

    pub fn get(&self, id: MapId) -> Option<&MapFile> {
        self.maps.iter().find(|(i, _)| *i == id).map(|(_, f)| f)
    }

    /// Name and blurb for the map picker.
    pub fn meta(&self, id: MapId) -> MapMeta {
        match self.get(id) {
            Some(f) => MapMeta {
                name: f.name.clone(),
                style: f.style.clone(),
                blurb: f.blurb.clone(),
                size: f.size.clone(),
            },
            None => MapMeta {
                name: id.to_string(),
                style: "Missing".into(),
                blurb: "You don't have this map. Ask the host for its file.".into(),
                size: "Unknown".into(),
            },
        }
    }

    /// The map ready to spawn. A map this player doesn't have is built as
    /// Splat Park (still under the asked-for id, so the match goes on).
    pub fn layout(&self, id: MapId) -> Layout {
        let file = match self.get(id) {
            Some(f) => f,
            None => {
                warn!("Map {id} isn't in the maps folder; building Splat Park in its place");
                self.get(MapId::SPLAT_PARK).expect("Splat Park is built in")
            }
        };
        Layout::from_file(id, file)
    }
}

/// Name and blurb for the map picker.
pub struct MapMeta {
    pub name: String,
    pub style: String,
    pub blurb: String,
    pub size: String,
}

/// A map, ready to spawn.
pub struct Layout {
    models: Vec<ink_map::Model>,
    colliders: Vec<Collider>,
    info: MapInfo,
    patches: Vec<Patch>,
    daylight: Daylight,
}

impl Layout {
    fn from_file(id: MapId, file: &MapFile) -> Self {
        let built = ink_map::build(file);
        let half = file.half();
        let colliders: Vec<Collider> = built
            .solids
            .iter()
            .map(|s| Collider {
                shape: s.shape.clone(),
                decals: s.decals,
            })
            .collect();
        let footprints = colliders
            .iter()
            .filter(|c| {
                // Skip the invisible edge walls: the minimap frame shows those.
                let r = c.shape.footprint();
                r.width() < half.x * 1.5 && r.height() < half.y * 1.5
            })
            .map(|c| (c.shape.footprint(), c.shape.height()))
            .collect();
        Self {
            models: built.models,
            colliders,
            info: MapInfo {
                id,
                half,
                spawns: built.spawns,
                ffa: built.ffa,
                footprints,
            },
            patches: file.ground.clone(),
            daylight: Daylight::for_sky(file.sky),
        }
    }
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

/// Lights the world for the map's time of day: sky, sun (or moon), ambient
/// light and fog. The map editor lights the same way, from the same table.
fn light_for_time_of_day(
    commands: &mut Commands,
    d: &Daylight,
    ambient: &mut GlobalAmbientLight,
    sun_q: &mut Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    camera_q: &Query<Entity, With<MainCamera>>,
) {
    let rgb = |[r, g, b]: [f32; 3]| Color::srgb(r, g, b);
    commands.insert_resource(ClearColor(rgb(d.sky)));
    ambient.color = rgb(d.ambient_color);
    ambient.brightness = d.ambient_brightness;
    for (mut sun, mut t) in sun_q.iter_mut() {
        sun.color = rgb(d.sun_color);
        sun.illuminance = d.sun_lux;
        *t = Transform::from_translation(d.sun_position()).looking_at(Vec3::ZERO, Vec3::Y);
    }
    for cam in camera_q {
        match d.fog {
            Some(f) => {
                commands.entity(cam).insert(DistanceFog {
                    color: rgb(f.color),
                    falloff: FogFalloff::Linear {
                        start: f.start,
                        end: f.end,
                    },
                    ..default()
                });
            }
            None => {
                commands.entity(cam).remove::<DistanceFog>();
            }
        }
    }
}

/// Builds the current map whenever it changes (and the first time).
#[allow(clippy::too_many_arguments)]
fn rebuild_map(
    mut commands: Commands,
    current: Res<CurrentMap>,
    library: Res<MapLibrary>,
    assets: Res<AssetServer>,
    ground: Option<Res<GroundTextures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<MapEntity>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun_q: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    camera_q: Query<Entity, With<MainCamera>>,
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
    let layout = library.layout(current.0);
    light_for_time_of_day(
        &mut commands,
        &layout.daylight,
        &mut ambient,
        &mut sun_q,
        &camera_q,
    );

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
        let tex = match p.texture {
            GroundTex::Turf => ground.turf.clone(),
            GroundTex::Dirt => ground.dirt.clone(),
        };
        let size = Vec2::from_array(p.size);
        let [r, g, b] = p.tint;
        commands.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(r, g, b),
                base_color_texture: Some(tex),
                uv_transform: Affine2::from_scale(size / p.tile.max(0.01)),
                perceptual_roughness: 0.95,
                ..default()
            })),
            // Later patches sit a hair higher so they never flicker.
            Transform::from_xyz(p.center[0], i as f32 * 0.003, p.center[1]),
            MapEntity,
        ));
    }
    for m in &layout.models {
        commands.spawn((
            WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(m.path.to_string()))),
            Transform {
                translation: m.translation,
                rotation: m.rotation,
                scale: m.scale,
            },
            MapEntity,
        ));
    }
    commands.insert_resource(Colliders(layout.colliders));
    commands.insert_resource(layout.info);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(id: MapId) -> Layout {
        MapLibrary::built_in().layout(id)
    }

    #[test]
    fn built_in_maps_have_no_problems() {
        let library = MapLibrary::built_in();
        assert_eq!(library.ids().count(), 6);
        for id in MapId::BUILT_IN {
            let file = library.get(id).unwrap();
            assert!(file.problems().is_empty(), "{id:?}: {:?}", file.problems());
        }
    }

    #[test]
    fn every_spawn_is_clear_and_inside() {
        for id in MapId::BUILT_IN {
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
        for id in MapId::BUILT_IN {
            let l = layout(id);
            assert!(l.info.spawns[0].iter().all(|p| p.z > 10.0), "{id:?}");
            assert!(l.info.spawns[1].iter().all(|p| p.z < -10.0), "{id:?}");
        }
    }

    #[test]
    fn spawning_avoids_threats() {
        let l = layout(MapId::DOCKYARD);
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
        let l = layout(MapId::RING_ROAD);
        let colliders = Colliders(l.colliders.clone());
        // Walking up the north ramp (z = 3 -> 1) ends on the platform deck.
        let mut pos = Vec3::new(0.0, 0.9 + 0.5, 2.0);
        let floor = colliders.resolve_player(&mut pos, 0.42, 0.45, 2.3);
        assert!((floor - 0.5).abs() < 0.05, "floor {floor}");
        let l = layout(MapId::PINE_HOLLOW);
        let colliders = Colliders(l.colliders.clone());
        let mut pos = Vec3::new(4.5, 0.9 + 0.75, 0.0);
        let floor = colliders.resolve_player(&mut pos, 0.42, 0.7, 2.5);
        assert!((floor - 0.75).abs() < 0.05, "floor {floor}");
    }

    #[test]
    fn pine_hollow_bridge_collides_as_it_looks() {
        let l = layout(MapId::PINE_HOLLOW);
        let colliders = Colliders(l.colliders.clone());
        // Paint flies under the deck (the old box filled it to the ground)...
        assert!(colliders.segment_hit(Vec3::new(0.0, 0.5, -3.0), Vec3::new(0.0, 0.5, 3.0)).is_none());
        // ...stops on the deck from above...
        let hit = colliders.segment_hit(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 0.5, 0.0)).unwrap();
        assert!((hit.point.y - 1.05).abs() < 0.06, "{}", hit.point);
        // ...and players stand on the deck but can't walk off through the railing.
        let mut pos = Vec3::new(0.0, 1.9, 0.0);
        let floor = colliders.resolve_player(&mut pos, 0.42, 1.04, 2.84);
        assert!((floor - 1.05).abs() < 0.06, "floor {floor}");
        let mut pos = Vec3::new(0.0, 1.9, 0.4);
        colliders.resolve_player(&mut pos, 0.42, 1.04, 2.84);
        assert!(pos.z < 0.4, "{pos}");
    }

    #[test]
    fn a_missing_map_still_builds_under_its_own_id() {
        let id = MapId::new("NotAMap");
        let l = MapLibrary::built_in().layout(id);
        assert_eq!(l.info.id, id);
        assert!(!l.colliders.is_empty());
    }

    #[test]
    fn built_in_ids_match_the_map_crate() {
        let ours: Vec<&str> = MapId::BUILT_IN.iter().map(|id| id.as_str()).collect();
        assert_eq!(ours, ink_map::BUILT_IN_IDS);
    }

    #[test]
    fn map_ids_look_like_the_old_enum_on_the_wire() {
        assert_eq!(serde_json::to_string(&MapId::THREE_LANES).unwrap(), "\"ThreeLanes\"");
        let back: MapId = serde_json::from_str("\"PineHollow\"").unwrap();
        assert_eq!(back, MapId::PINE_HOLLOW);
    }
}
