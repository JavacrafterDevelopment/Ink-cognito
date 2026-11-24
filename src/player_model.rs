//! The rigged blue / orange player models: Idle and Walk animations from the
//! pack, an upper body that aims where the player looks, and the player's gun
//! in their hands.

use crate::{
    loadout::{GunAssets, GunModel, Loadout, spawn_gun_parts},
    protocol::Team,
};
use bevy::{
    app::AnimationSystems, prelude::*, transform::TransformSystems,
    world_serialization::WorldInstanceReady,
};
use std::{collections::HashMap, f32::consts::FRAC_PI_2};

pub struct PlayerModelPlugin;

impl Plugin for PlayerModelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_player_assets)
            .add_observer(on_model_ready)
            .add_systems(Update, (drive_animation, sync_held_guns))
            .add_systems(
                PostUpdate,
                pose_upper_body
                    .after(AnimationSystems)
                    .before(TransformSystems::Propagate),
            );
    }
}

pub const BLUE_MODEL: &str = "models/players/blue_player_rigged.glb";
pub const ORANGE_MODEL: &str = "models/players/orange_player_rigged.glb";

pub fn model_path(team: Team) -> &'static str {
    match team {
        Team::Blue => BLUE_MODEL,
        Team::Orange => ORANGE_MODEL,
    }
}

#[derive(Resource)]
pub struct PlayerModelAssets {
    pub blue: Handle<WorldAsset>,
    pub orange: Handle<WorldAsset>,
    graph: Handle<AnimationGraph>,
    idle: AnimationNodeIndex,
    walk: AnimationNodeIndex,
}

impl PlayerModelAssets {
    pub fn scene(&self, team: Team) -> Handle<WorldAsset> {
        match team {
            Team::Blue => self.blue.clone(),
            Team::Orange => self.orange.clone(),
        }
    }
}

fn load_player_assets(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    // Both models share the same skeleton and clips.
    let (graph, nodes) = AnimationGraph::from_clips([
        assets.load(GltfAssetLabel::Animation(0).from_asset(BLUE_MODEL)),
        assets.load(GltfAssetLabel::Animation(1).from_asset(BLUE_MODEL)),
    ]);
    commands.insert_resource(PlayerModelAssets {
        blue: assets.load(GltfAssetLabel::Scene(0).from_asset(BLUE_MODEL)),
        orange: assets.load(GltfAssetLabel::Scene(0).from_asset(ORANGE_MODEL)),
        graph: graphs.add(graph),
        idle: nodes[0],
        walk: nodes[1],
    });
}

/// A rigged model whose bones get looked up once it's spawned.
#[derive(Component)]
pub struct RiggedModel;

/// What a living player's model shows: where they aim, how fast they move,
/// and what they hold.
#[derive(Component, Default)]
pub struct PlayerPose {
    pub pitch: f32,
    pub speed: f32,
    pub loadout: u32,
}

/// The model's bones by name, once spawned.
#[derive(Component)]
pub struct Rig {
    pub bones: HashMap<String, Entity>,
    pub anim_player: Option<Entity>,
    pub gun_holder: Option<Entity>,
}

impl Rig {
    pub fn bone(&self, name: &str) -> Option<Entity> {
        self.bones.get(name).copied()
    }
}

/// Spawns a player model (feet at its origin, facing +Z).
pub fn spawn_player_model(
    commands: &mut Commands,
    asset_server: &AssetServer,
    team: Team,
) -> Entity {
    commands
        .spawn((
            WorldAssetRoot(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(model_path(team))),
            ),
            RiggedModel,
        ))
        .id()
}

fn on_model_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    models: Query<Has<PlayerPose>, With<RiggedModel>>,
    children: Query<&Children>,
    names: Query<&Name>,
    anim_players: Query<(), With<AnimationPlayer>>,
    player_assets: Res<PlayerModelAssets>,
) {
    let root = ready.entity;
    let Ok(posed) = models.get(root) else {
        return;
    };
    let mut bones = HashMap::new();
    let mut anim_player = None;
    for e in children.iter_descendants(root) {
        if let Ok(name) = names.get(e) {
            bones.insert(name.as_str().to_string(), e);
        }
        if anim_player.is_none() && anim_players.contains(e) {
            anim_player = Some(e);
        }
    }
    let mut gun_holder = None;
    if posed {
        if let Some(player) = anim_player {
            commands
                .entity(player)
                .insert(AnimationGraphHandle(player_assets.graph.clone()));
        }
        // The gun rides on the chest, barrel forward, stock at the right shoulder.
        if let Some(chest) = bones.get("Spine2") {
            let holder = commands
                .spawn((
                    Transform::from_translation(GUN_IN_CHEST)
                        .with_rotation(Quat::from_rotation_y(-FRAC_PI_2)),
                    Visibility::default(),
                    GunModel { packed: u32::MAX },
                ))
                .id();
            commands.entity(*chest).add_child(holder);
            gun_holder = Some(holder);
        }
    } else {
        anim_player = None;
    }
    commands.entity(root).insert(Rig {
        bones,
        anim_player,
        gun_holder,
    });
}

/// Where the gun's origin sits in the chest bone's space (the model faces +Z,
/// its right hand is on -X).
const GUN_IN_CHEST: Vec3 = Vec3::new(-0.15, 0.06, 0.3);
/// Hand targets in the chest bone's space: trigger grip and foregrip.
const RIGHT_HAND: Vec3 = Vec3::new(-0.15, -0.03, 0.2);
const LEFT_HAND: Vec3 = Vec3::new(-0.12, 0.0, 0.42);
const UPPER_ARM: f32 = 0.28;
const FOREARM: f32 = 0.25;

fn drive_animation(
    player_assets: Res<PlayerModelAssets>,
    models: Query<(&Rig, &PlayerPose)>,
    mut players: Query<&mut AnimationPlayer>,
) {
    for (rig, pose) in &models {
        let Some(mut player) = rig.anim_player.and_then(|e| players.get_mut(e).ok()) else {
            continue;
        };
        if !player.is_playing_animation(player_assets.idle) {
            player.play(player_assets.idle).repeat();
        }
        if !player.is_playing_animation(player_assets.walk) {
            player.play(player_assets.walk).repeat();
        }
        let walk = (pose.speed / 5.0).clamp(0.0, 1.0);
        if let Some(anim) = player.animation_mut(player_assets.walk) {
            anim.set_weight(walk);
            anim.set_speed((pose.speed / 6.5).clamp(0.6, 1.7));
        }
        if let Some(anim) = player.animation_mut(player_assets.idle) {
            anim.set_weight(1.0 - walk);
        }
    }
}

fn sync_held_guns(
    mut commands: Commands,
    guns: Option<Res<GunAssets>>,
    models: Query<(&Rig, &PlayerPose)>,
    mut holders: Query<&mut GunModel>,
) {
    let Some(guns) = guns else {
        return;
    };
    for (rig, pose) in &models {
        let Some(holder) = rig.gun_holder else {
            continue;
        };
        let Ok(mut gun) = holders.get_mut(holder) else {
            continue;
        };
        if gun.packed != pose.loadout {
            gun.packed = pose.loadout;
            spawn_gun_parts(&mut commands, &guns, holder, &Loadout::unpack(pose.loadout));
        }
    }
}

/// Two-bone IK: where the elbow goes for a shoulder reaching a target.
fn elbow_for(shoulder: Vec3, target: Vec3, pole: Vec3) -> (Vec3, Vec3) {
    let to = target - shoulder;
    let d = to.length().clamp(
        (UPPER_ARM - FOREARM).abs() + 0.01,
        UPPER_ARM + FOREARM - 0.002,
    );
    let dir = to.normalize_or(Vec3::Z);
    let target = shoulder + dir * d;
    let cos_a = ((UPPER_ARM * UPPER_ARM + d * d - FOREARM * FOREARM) / (2.0 * UPPER_ARM * d))
        .clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let perp = (pole - dir * pole.dot(dir)).normalize_or(Vec3::NEG_Y);
    (
        shoulder + dir * UPPER_ARM * cos_a + perp * UPPER_ARM * sin_a,
        target,
    )
}

/// After the animation: lean the chest toward the aim and put both hands on the gun.
fn pose_upper_body(models: Query<(&Rig, &PlayerPose)>, mut transforms: Query<&mut Transform>) {
    for (rig, pose) in &models {
        if let Some(mut chest) = rig.bone("Spine2").and_then(|e| transforms.get_mut(e).ok()) {
            chest.rotation = Quat::from_rotation_x(-pose.pitch * 0.85);
        }
        if let Some(mut head) = rig.bone("Head").and_then(|e| transforms.get_mut(e).ok()) {
            head.rotation = Quat::from_rotation_x(-pose.pitch * 0.15);
        }
        for (arm, fore, shoulder, target, rest, pole) in [
            (
                "RightArm",
                "RightForeArm",
                Vec3::new(-0.19, 0.10, 0.0),
                RIGHT_HAND,
                Vec3::NEG_X,
                Vec3::new(-0.6, -1.0, 0.0),
            ),
            (
                "LeftArm",
                "LeftForeArm",
                Vec3::new(0.19, 0.10, 0.0),
                LEFT_HAND,
                Vec3::X,
                Vec3::new(0.4, -1.0, 0.0),
            ),
        ] {
            let (elbow, hand) = elbow_for(shoulder, target, pole);
            let upper = Quat::from_rotation_arc(rest, (elbow - shoulder).normalize_or(rest));
            let fore_world =
                Quat::from_rotation_arc(upper * rest, (hand - elbow).normalize_or(rest)) * upper;
            if let Some(mut t) = rig.bone(arm).and_then(|e| transforms.get_mut(e).ok()) {
                t.rotation = upper;
            }
            if let Some(mut t) = rig.bone(fore).and_then(|e| transforms.get_mut(e).ok()) {
                t.rotation = upper.inverse() * fore_world;
            }
        }
    }
}
