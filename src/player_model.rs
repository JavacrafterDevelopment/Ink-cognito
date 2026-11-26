//! The rigged blue / orange player models: the pack's animations (idle,
//! walk, run, aim, crouch, jump, reload, hit react and victory) blended from
//! what each player is doing, an upper body that aims where the player looks,
//! and the player's gun in their hands.
//!
//! The pack's Eliminated clip is never used: eliminated players ragdoll.

use crate::{
    loadout::{GunAssets, GunModel, Loadout, spawn_gun_parts},
    locomotion::{self, Locomotion},
    protocol::{FLAG_AIMING, FLAG_AIRBORNE, FLAG_CROUCH, FLAG_RELOADING, Team},
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
            .add_plugins(locomotion::LocomotionPlugin)
            .add_systems(
                PostUpdate,
                (
                    locomotion::sample_model_transforms,
                    locomotion::drive_legs,
                    pose_upper_body,
                )
                    .chain()
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
    clips: Clips,
}

/// Graph nodes for the clips we use.
#[derive(Clone, Copy)]
struct Clips {
    idle: AnimationNodeIndex,
    walk: AnimationNodeIndex,
    run: AnimationNodeIndex,
    aim_idle: AnimationNodeIndex,
    aim_walk: AnimationNodeIndex,
    reload: AnimationNodeIndex,
    crouch_idle: AnimationNodeIndex,
    crouch_walk: AnimationNodeIndex,
    jump: AnimationNodeIndex,
    hit: AnimationNodeIndex,
    victory: AnimationNodeIndex,
}

impl Clips {
    /// The looping clips blended by movement.
    fn looping(&self) -> [AnimationNodeIndex; 9] {
        [
            self.idle,
            self.walk,
            self.run,
            self.aim_idle,
            self.aim_walk,
            self.reload,
            self.crouch_idle,
            self.crouch_walk,
            self.victory,
        ]
    }
}

/// Clip numbers in the rigged player files.
const CLIP_IDLE: usize = 0;
const CLIP_WALK: usize = 1;
const CLIP_RUN: usize = 2;
const CLIP_AIM_IDLE: usize = 3;
const CLIP_AIM_WALK: usize = 4;
const CLIP_RELOAD: usize = 6;
const CLIP_CROUCH_IDLE: usize = 7;
const CLIP_CROUCH_WALK: usize = 8;
const CLIP_JUMP: usize = 9;
const CLIP_HIT_REACT: usize = 10;
// 11 is Eliminated: not used, players ragdoll instead.
const CLIP_VICTORY: usize = 12;

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
    let order = [
        CLIP_IDLE,
        CLIP_WALK,
        CLIP_RUN,
        CLIP_AIM_IDLE,
        CLIP_AIM_WALK,
        CLIP_RELOAD,
        CLIP_CROUCH_IDLE,
        CLIP_CROUCH_WALK,
        CLIP_JUMP,
        CLIP_HIT_REACT,
        CLIP_VICTORY,
    ];
    let (graph, n) = AnimationGraph::from_clips(
        order.map(|i| assets.load(GltfAssetLabel::Animation(i).from_asset(BLUE_MODEL))),
    );
    commands.insert_resource(PlayerModelAssets {
        blue: assets.load(GltfAssetLabel::Scene(0).from_asset(BLUE_MODEL)),
        orange: assets.load(GltfAssetLabel::Scene(0).from_asset(ORANGE_MODEL)),
        graph: graphs.add(graph),
        clips: Clips {
            idle: n[0],
            walk: n[1],
            run: n[2],
            aim_idle: n[3],
            aim_walk: n[4],
            reload: n[5],
            crouch_idle: n[6],
            crouch_walk: n[7],
            jump: n[8],
            hit: n[9],
            victory: n[10],
        },
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
    /// `protocol::FLAG_*` bits: crouching, airborne, aiming, reloading.
    pub flags: u8,
    pub health: f32,
    /// Celebrating a won match.
    pub victory: bool,
    /// Counts down after taking a hit (hit react).
    pub hit_timer: f32,
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
    if posed {
        // Legs and hips are procedural; the clips keep the upper body.
        commands.entity(root).insert(Locomotion::default());
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
    time: Res<Time>,
    player_assets: Res<PlayerModelAssets>,
    mut models: Query<(&Rig, &mut PlayerPose)>,
    mut players: Query<&mut AnimationPlayer>,
    mut last_health: Local<HashMap<Entity, f32>>,
) {
    let c = player_assets.clips;
    let dt = time.delta_secs();
    let k = 1.0 - (-10.0 * dt).exp();
    for (rig, mut pose) in &mut models {
        let Some(anim_e) = rig.anim_player else {
            continue;
        };
        let Ok(mut player) = players.get_mut(anim_e) else {
            continue;
        };
        for node in c.looping() {
            if !player.is_playing_animation(node) {
                player.play(node).repeat().set_weight(0.0);
            }
        }
        // A hit (health went down while alive): flinch.
        let prev = last_health
            .insert(anim_e, pose.health)
            .unwrap_or(pose.health);
        if pose.health < prev - 0.5 && pose.health > 0.0 {
            pose.hit_timer = 0.4;
            player.play(c.hit).replay();
        }
        pose.hit_timer = (pose.hit_timer - dt).max(0.0);

        let crouch = pose.flags & FLAG_CROUCH != 0;
        let airborne = pose.flags & FLAG_AIRBORNE != 0;
        let aiming = pose.flags & FLAG_AIMING != 0;
        let reloading = pose.flags & FLAG_RELOADING != 0;
        let moving = (pose.speed / 3.0).clamp(0.0, 1.0);
        let run = ((pose.speed - 6.5) / 3.0).clamp(0.0, 1.0);

        // Target weights for each clip.
        let mut w: HashMap<AnimationNodeIndex, f32> = HashMap::new();
        if pose.victory {
            w.insert(c.victory, 1.0);
        } else if airborne {
            w.insert(c.jump, 1.0);
        } else if crouch {
            w.insert(c.crouch_idle, 1.0 - moving);
            w.insert(c.crouch_walk, moving);
        } else if aiming {
            w.insert(c.aim_idle, 1.0 - moving);
            w.insert(c.aim_walk, moving);
        } else {
            w.insert(c.idle, 1.0 - moving);
            w.insert(c.walk, moving * (1.0 - run));
            w.insert(c.run, moving * run);
        }
        if reloading && !pose.victory {
            // Mixed over the legs' clip: the arms do the work.
            for v in w.values_mut() {
                *v *= 0.5;
            }
            w.insert(c.reload, 0.5);
        }
        let hit = (pose.hit_timer / 0.4) * 0.6;

        for node in c.looping().into_iter().chain([c.jump]) {
            let target = w.get(&node).copied().unwrap_or(0.0) * (1.0 - hit);
            if node == c.jump && target > 0.0 && !player.is_playing_animation(node) {
                player.play(node).repeat();
            }
            if let Some(anim) = player.animation_mut(node) {
                let cur = anim.weight();
                anim.set_weight(cur + (target - cur) * k);
                if node == c.walk || node == c.crouch_walk || node == c.aim_walk {
                    anim.set_speed((pose.speed / 6.5).clamp(0.6, 1.7));
                }
            }
        }
        if let Some(anim) = player.animation_mut(c.hit) {
            anim.set_weight(hit);
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
fn pose_upper_body(
    models: Query<(&Rig, &PlayerPose, Option<&Locomotion>)>,
    mut transforms: Query<&mut Transform>,
) {
    for (rig, pose, loco) in &models {
        // Celebrating: the victory clip moves freely.
        if pose.victory {
            continue;
        }
        let reloading = pose.flags & FLAG_RELOADING != 0;
        if let Some(mut chest) = rig.bone("Spine2").and_then(|e| transforms.get_mut(e).ok()) {
            // Undo the hips' procedural twist and lean so the gun stays on aim.
            let hips = loco
                .filter(|l| l.active)
                .map_or(Quat::IDENTITY, |l| l.hips_rot);
            chest.rotation = hips.inverse() * Quat::from_rotation_x(-pose.pitch * 0.85);
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
            // Reloading: the left hand leaves the foregrip for the hopper.
            if reloading && arm == "LeftArm" {
                continue;
            }
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
