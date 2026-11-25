//! The 3D field: ground, models and the piece about to be placed.

use crate::state::{Editor, Tool};
use bevy::{
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    math::Affine2,
    prelude::*,
};
use ink_map::{Daylight, GroundTex};

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.88, 0.93, 1.0),
            brightness: 380.0,
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (rebuild, sync_objects, update_ghost, update_sky).chain(),
        );
    }
}

/// Everything spawned for the map (despawned on rebuild).
#[derive(Component)]
pub struct MapItem;

/// The model of `MapFile::objects[i]`.
#[derive(Component)]
pub struct ObjectModel(pub usize);

/// The piece about to be placed, following the cursor.
#[derive(Component)]
struct Ghost(&'static str);

/// The sun (or moon) light.
#[derive(Component)]
struct Sun;

/// Where the cursor meets the ground, if it's over the field view.
#[derive(Resource, Default)]
pub struct CursorGround(pub Option<Vec2>);

#[derive(Resource)]
struct GroundTextures {
    turf: Handle<Image>,
    dirt: Handle<Image>,
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.init_resource::<CursorGround>();
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.97, 0.90),
            illuminance: 14_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(24.0, 38.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        Sun,
    ));
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

fn model(assets: &AssetServer, path: &str) -> WorldAssetRoot {
    WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(path.to_string())))
}

/// Rebuilds everything when pieces are added or removed, or the field changes.
#[allow(clippy::too_many_arguments)]
fn rebuild(
    mut commands: Commands,
    editor: Res<Editor>,
    assets: Res<AssetServer>,
    ground: Res<GroundTextures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<MapItem>>,
    mut built: Local<Option<u64>>,
) {
    if *built == Some(editor.structure) {
        return;
    }
    *built = Some(editor.structure);
    for e in &existing {
        commands.entity(e).despawn();
    }
    let map = &editor.map;
    let half = map.half();
    // Dirt all around, beyond the edge.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(400.0, 400.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.86, 0.82, 0.76),
            base_color_texture: Some(ground.dirt.clone()),
            uv_transform: Affine2::from_scale(Vec2::splat(400.0 / 4.0)),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.03, 0.0),
        MapItem,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(half.x * 2.0, 1.0, half.y * 2.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.3, 0.3),
            ..default()
        })),
        Transform::from_xyz(0.0, -0.505, 0.0),
        MapItem,
    ));
    for (i, p) in map.ground.iter().enumerate() {
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
            Transform::from_xyz(p.center[0], i as f32 * 0.003, p.center[1]),
            MapItem,
        ));
    }
    for m in ink_map::build(map).models {
        let mut e = commands.spawn((
            model(&assets, m.path),
            Transform {
                translation: m.translation,
                rotation: m.rotation,
                scale: m.scale,
            },
            MapItem,
        ));
        if let Some(i) = m.object {
            e.insert(ObjectModel(i));
        }
    }
}

/// Moves and turns follow the map without a rebuild.
fn sync_objects(editor: Res<Editor>, mut q: Query<(&ObjectModel, &mut Transform)>) {
    if !editor.is_changed() {
        return;
    }
    for (ObjectModel(i), mut t) in &mut q {
        let Some(o) = editor.map.objects.get(*i) else {
            continue;
        };
        let want = Transform {
            translation: Vec3::from_array(o.pos),
            rotation: o.rotation(),
            scale: Vec3::from_array(o.scale),
        };
        if *t != want {
            *t = want;
        }
    }
}

fn update_ghost(
    mut commands: Commands,
    editor: Res<Editor>,
    cursor: Res<CursorGround>,
    assets: Res<AssetServer>,
    mut ghost_q: Query<(Entity, &Ghost, &mut Transform, &mut Visibility)>,
) {
    let want = match editor.tool {
        Tool::Place(p) => Some(p.model),
        _ => None,
    };
    let mut have = false;
    for (e, g, mut t, mut vis) in &mut ghost_q {
        if Some(g.0) != want {
            commands.entity(e).despawn();
            continue;
        }
        have = true;
        match cursor.0 {
            Some(at) => {
                let at = editor.snapped(at);
                t.translation = Vec3::new(at.x, 0.0, at.y);
                t.rotation = Quat::from_rotation_y(ink_map::yaw_radians(editor.place_yaw));
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
    if !have && let Some(path) = want {
        commands.spawn((model(&assets, path), Transform::default(), Visibility::Hidden, Ghost(path)));
    }
}

/// Lights the field for the map's time of day: sky, sun, ambient light and
/// fog, the same as the game does (see `ink_map::Daylight`).
fn update_sky(
    mut commands: Commands,
    editor: Res<Editor>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun_q: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    cameras: Query<Entity, With<Camera3d>>,
    mut lit: Local<Option<[f32; 3]>>,
) {
    let sky = editor.map.sky;
    if *lit == Some(sky) {
        return;
    }
    let Ok((mut sun, mut sun_t)) = sun_q.single_mut() else {
        return;
    };
    let cams: Vec<Entity> = cameras.iter().collect();
    if cams.is_empty() {
        return;
    }
    *lit = Some(sky);
    let d = Daylight::for_sky(sky);
    let rgb = |[r, g, b]: [f32; 3]| Color::srgb(r, g, b);
    clear.0 = rgb(d.sky);
    ambient.color = rgb(d.ambient_color);
    ambient.brightness = d.ambient_brightness;
    sun.color = rgb(d.sun_color);
    sun.illuminance = d.sun_lux;
    *sun_t = Transform::from_translation(d.sun_position()).looking_at(Vec3::ZERO, Vec3::Y);
    for cam in cams {
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
