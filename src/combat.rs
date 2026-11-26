//! Shooting, health and eliminations.
//!
//! Every player's shots are simulated visually on every screen via their
//! `shots` counter, while damage is confirmed by the shooter (`HitEvent`):
//! when your paintball hits an enemy on your screen and shows a hitmarker,
//! the hit is reliably relayed to the victim (deduplicated per shot) so
//! moving targets always take damage that matches what the shooter saw.

use crate::{
    AppState, Overlay,
    arena::Colliders,
    armory_v2_fx::Hopper,
    game_mode::respawn_local,
    loadout::{
        GunAssets, GunKind, GunModel, Loadout, WeaponStats, is_canted_sight, sight_eye,
        spawn_gun_parts,
    },
    maps::MapInfo,
    net::NetworkState,
    player_model::{PlayerModelAssets, Rig},
    protocol::{FLAG_CROUCH, HitEvent, MAX_HEALTH, PlayerId, PlayerState, Team, enemies},
    ragdoll::{BodyHit, Ragdoll, RagdollStart, spawn_ragdoll},
    settings::{AudioSettings, SfxAudio},
    world::{
        CROUCH_DROP, CameraRig, EYE_OFFSET, LocalPlayer, MainCamera, PLAYER_GROUND_Y, RemotePlayer,
        WorldSystems,
    },
};
use bevy::prelude::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    f32::consts::TAU,
};

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalState>()
            .init_resource::<Combat>()
            .init_resource::<HitBookkeeping>()
            .init_resource::<Killfeed>()
            .add_systems(Startup, load_fx)
            // After the camera exists.
            .add_systems(PostStartup, spawn_viewmodel)
            .add_systems(OnEnter(AppState::InGame), reset_hit_bookkeeping)
            .add_systems(
                OnExit(AppState::InGame),
                (hide_viewmodel, clear_killfeed, reset_hit_bookkeeping),
            )
            .add_systems(
                Update,
                (
                    apply_loadout,
                    aim_and_reload,
                    fire_local,
                    fire_remote,
                    move_projectiles,
                    process_incoming_hits,
                    remote_eliminations,
                    regen_and_respawn,
                    update_viewmodel,
                    footsteps,
                    remote_footsteps,
                )
                    .chain()
                    .in_set(CombatSystems)
                    .after(WorldSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            )
            .init_resource::<BakedPaint>()
            .add_systems(OnExit(AppState::InGame), clear_baked_paint)
            .add_systems(Update, update_particles)
            .add_systems(Update, bake_decals.run_if(in_state(AppState::InGame)))
            .add_systems(
                Update,
                sync_hopper_balls
                    .after(CombatSystems)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CombatSystems;

/// Seconds without taking damage before health starts coming back.
pub const REGEN_DELAY: f32 = 7.0;
/// Health per second while regenerating.
pub const REGEN_RATE: f32 = 14.0;
/// Seconds from elimination to respawn.
pub const RESPAWN_TIME: f32 = 5.0;
const PLAYER_HIT_RADIUS: f32 = 0.44;
/// The top of the body capsule that counts as the head.
const HEAD_ZONE: f32 = 0.35;
/// Damage multiplier for a ball that lands in the head zone.
pub const HEADSHOT_MULTIPLIER: f32 = 1.5;
/// How long paint from a headshot stays on the victim's screen.
pub const HEAD_PAINT_SECS: f32 = 4.0;
/// Hit marker lengths (seconds): a normal or head hit, and an elimination.
const HITMARKER_SECS: f32 = 0.25;
const KILLMARKER_SECS: f32 = 0.5;
/// Paint is baked into merged meshes once this many fresh splats pile up,
/// or once the oldest has sat for `BAKE_AFTER_SECS`.
const BAKE_AT_COUNT: usize = 192;
const BAKE_AFTER_SECS: f32 = 0.5;
/// Splats per merged mesh before a new one is started, so rebuilding a mesh
/// after new paint lands stays cheap.
const QUADS_PER_BATCH: usize = 512;
/// Merged paint is grouped into cells this wide (meters), so transparent
/// sorting stays local and each rebuild only touches the area that was hit.
const BAKE_CELL: f32 = 12.0;

/// Tracks outbound hit IDs and deduplicates inbound UDP hit retries on the client.
#[derive(Resource, Default)]
struct HitBookkeeping {
    next_hit_id: u32,
    seen: HashSet<(PlayerId, u32)>,
    order: VecDeque<(PlayerId, u32)>,
}

impl HitBookkeeping {
    const CAPACITY: usize = 512;

    fn alloc_id(&mut self) -> u32 {
        self.next_hit_id = self.next_hit_id.wrapping_add(1).max(1);
        self.next_hit_id
    }

    fn record_incoming(&mut self, attacker: PlayerId, hit_id: u32) -> bool {
        let key = (attacker, hit_id);
        if !self.seen.insert(key) {
            return false;
        }
        self.order.push_back(key);
        while self.order.len() > Self::CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }
}

fn reset_hit_bookkeeping(mut hits: ResMut<HitBookkeeping>) {
    *hits = HitBookkeeping::default();
}

/// What we send others about ourselves (health, gun, counters).
#[derive(Resource, Default)]
pub struct LocalState(pub PlayerState);

/// The local player's weapon and health bookkeeping.
#[derive(Resource)]
pub struct Combat {
    pub stats: WeaponStats,
    pub kind: GunKind,
    pub ammo: u32,
    /// Seconds left on a reload in progress.
    pub reloading: Option<f32>,
    cooldown: f32,
    /// `Time::elapsed_secs` of the last hit we took.
    pub last_damage: f32,
    pub aiming: bool,
    /// 0 (hip) to 1 (fully aimed), eased.
    pub aim_blend: f32,
    /// Counts down after we hit someone (hit marker).
    pub hitmarker: f32,
    /// What the current hit marker is for (white hit, gold headshot, red kill).
    pub hitmarker_kind: HitmarkerKind,
    /// Counts down after we got hit (red flash).
    pub damage_flash: f32,
    /// Counts down from `HEAD_PAINT_SECS` after we took a headshot: paint
    /// over part of the screen that fades away.
    pub head_paint: f32,
    /// The colour of that paint (the shooter's team, or water).
    pub head_paint_tint: Color,
    /// Bumped on every headshot taken so the HUD re-scatters the splats.
    pub head_paint_seed: u32,
    /// Which way (on the ground) the last hit came from, and how long to show it.
    pub hit_from: Vec3,
    pub hit_arc: f32,
    /// Where we went down, while eliminated.
    pub death_cam: Option<Vec3>,
    pub dead_for: f32,
    recoil: f32,
    step_timer: f32,
    dry_fired: bool,
}

impl Default for Combat {
    fn default() -> Self {
        let stats = Loadout::default().stats();
        Self {
            stats,
            kind: GunKind::Paintball,
            ammo: stats.capacity,
            reloading: None,
            cooldown: 0.0,
            last_damage: -100.0,
            aiming: false,
            aim_blend: 0.0,
            hitmarker: 0.0,
            hitmarker_kind: HitmarkerKind::Hit,
            damage_flash: 0.0,
            head_paint: 0.0,
            head_paint_tint: Color::WHITE,
            head_paint_seed: 0,
            hit_from: Vec3::Z,
            hit_arc: 0.0,
            death_cam: None,
            dead_for: 0.0,
            recoil: 0.0,
            step_timer: 0.0,
            dry_fired: false,
        }
    }
}

impl Combat {
    pub fn reset(&mut self, stats: &WeaponStats) {
        *self = Self {
            stats: *stats,
            kind: if stats.water {
                GunKind::Water
            } else {
                GunKind::Paintball
            },
            ammo: stats.capacity,
            ..default()
        };
    }

    pub fn respawn_in(&self) -> Option<f32> {
        self.death_cam
            .map(|_| (RESPAWN_TIME - self.dead_for).max(0.0))
    }
}

/// Which hit marker to show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HitmarkerKind {
    #[default]
    Hit,
    Headshot,
    Kill,
}

impl HitmarkerKind {
    pub fn color(self) -> Color {
        match self {
            HitmarkerKind::Hit => Color::srgb(1.0, 1.0, 1.0),
            HitmarkerKind::Headshot => Color::srgb(1.0, 0.78, 0.15),
            HitmarkerKind::Kill => Color::srgb(1.0, 0.18, 0.15),
        }
    }
}

/// Recent eliminations for the HUD: (killer, victim, when).
#[derive(Resource, Default)]
pub struct Killfeed {
    pub entries: Vec<(PlayerId, PlayerId, f32)>,
}

fn clear_killfeed(mut feed: ResMut<Killfeed>) {
    feed.entries.clear();
}

/// What colour paint is: a team's, or water.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Paint {
    Team(Team),
    Water,
}

impl Paint {
    fn color(self) -> Color {
        match self {
            Paint::Team(team) => {
                let [r, g, b] = team.srgb();
                Color::srgb(r, g, b)
            }
            Paint::Water => Color::srgba(0.62, 0.89, 1.0, 0.75),
        }
    }
}

#[derive(Resource)]
pub struct Fx {
    billboard: Handle<Mesh>,
    decal: Handle<Mesh>,
    /// A paint bomb ball.
    shrapnel: Handle<Mesh>,
    splats: Vec<Handle<Image>>,
    drips: Vec<Handle<Image>>,
    droplet: Handle<Image>,
    puff: Handle<Image>,
    ring: Handle<Image>,
    balls: HashMap<Team, Handle<WorldAsset>>,
    mats: HashMap<(Paint, u8), Handle<StandardMaterial>>,
    pub sounds: Sounds,
    rng: u32,
    /// The footstep sound played last, so the next step picks another one.
    last_step: usize,
}

pub struct Sounds {
    pub shots: Vec<Handle<AudioSource>>,
    pub splats: Vec<Handle<AudioSource>>,
    pub player_hit: Handle<AudioSource>,
    pub hit_tick: Handle<AudioSource>,
    /// The "ding" layered on top of the tick for a headshot.
    pub headshot: Handle<AudioSource>,
    pub dry_fire: Handle<AudioSource>,
    pub pod_reload: Handle<AudioSource>,
    pub water_pump: Handle<AudioSource>,
    pub water_squirt: Handle<AudioSource>,
    pub water_splash: Handle<AudioSource>,
    pub eliminated: Handle<AudioSource>,
    pub confirm: Handle<AudioSource>,
    pub round_start: Handle<AudioSource>,
    pub steps: Vec<Handle<AudioSource>>,
    pub bunker_hit: Handle<AudioSource>,
}

const DROPLET: u8 = 100;
const PUFF: u8 = 101;
const RING: u8 = 102;
/// Solid paint for a paint bomb's balls (no texture).
const SHRAPNEL: u8 = 103;
/// One in this many paint bomb balls leaves a splat and effects where it lands.
const SHRAPNEL_SPLAT_EVERY: u32 = 8;
/// Of the rest, one in this many still leaves a plain splat (no particles or
/// sound), so a bomb paints the area it covers. Cheap now that settled paint
/// is merged into a few meshes (see `bake_decals`).
const SHRAPNEL_DECAL_EVERY: u32 = 3;
/// Splat sounds from paint bomb balls per frame, at most.
const SHRAPNEL_SOUNDS_PER_FRAME: u32 = 2;
const DRIP: u8 = 50;

impl Fx {
    fn rand(&mut self) -> f32 {
        self.rng = xorshift(self.rng);
        self.rng as f32 / u32::MAX as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rand()
    }
    fn pick(&mut self, len: usize) -> usize {
        ((self.rand() * len as f32) as usize).min(len.saturating_sub(1))
    }
    /// A random footstep sound, never the same one twice in a row.
    fn step_sound(&mut self) -> Handle<AudioSource> {
        let n = self.sounds.steps.len();
        let mut i = self.pick(n);
        if n > 1 && i == self.last_step {
            // Shift by 1..n-1 so every other sound stays equally likely.
            i = (i + 1 + self.pick(n - 1)) % n;
        }
        self.last_step = i;
        self.sounds.steps[i].clone()
    }
    fn material(
        &mut self,
        paint: Paint,
        kind: u8,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        if let Some(h) = self.mats.get(&(paint, kind)) {
            return h.clone();
        }
        if kind == SHRAPNEL {
            let handle = materials.add(StandardMaterial {
                base_color: paint.color().with_alpha(1.0),
                perceptual_roughness: 0.35,
                ..default()
            });
            self.mats.insert((paint, kind), handle.clone());
            return handle;
        }
        let (texture, unlit, roughness, depth_bias) = match kind {
            DROPLET => (self.droplet.clone(), true, 1.0, 0.0),
            PUFF => (self.puff.clone(), true, 1.0, 0.0),
            RING => (self.ring.clone(), true, 1.0, 0.0),
            i if i >= DRIP => (self.drips[(i - DRIP) as usize].clone(), false, 0.3, 10.0),
            i => (self.splats[i as usize].clone(), false, 0.3, 10.0),
        };
        let handle = materials.add(StandardMaterial {
            base_color: paint.color(),
            base_color_texture: Some(texture),
            alpha_mode: AlphaMode::Blend,
            unlit,
            perceptual_roughness: roughness,
            cull_mode: None,
            depth_bias,
            ..default()
        });
        self.mats.insert((paint, kind), handle.clone());
        handle
    }
}

fn xorshift(mut x: u32) -> u32 {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x
}

fn load_fx(mut commands: Commands, assets: Res<AssetServer>, mut meshes: ResMut<Assets<Mesh>>) {
    let ogg = |p: &str| assets.load::<AudioSource>(format!("audio/{p}.ogg"));
    commands.insert_resource(Fx {
        billboard: meshes.add(Rectangle::new(1.0, 1.0)),
        decal: meshes.add(Plane3d::default().mesh().size(1.0, 1.0)),
        shrapnel: meshes.add(Sphere::new(0.035).mesh().uv(8, 6)),
        splats: (1..=8)
            .map(|i| assets.load(format!("textures/splats/splat_{i:02}.png")))
            .collect(),
        drips: (1..=4)
            .map(|i| assets.load(format!("textures/splats/drip_{i:02}.png")))
            .collect(),
        droplet: assets.load("textures/particles/droplet.png"),
        puff: assets.load("textures/particles/soft_puff.png"),
        ring: assets.load("textures/particles/ring_shockwave.png"),
        balls: [
            (
                Team::Blue,
                assets.load(
                    GltfAssetLabel::Scene(0).from_asset("models/projectiles/paintball_blue.glb"),
                ),
            ),
            (
                Team::Orange,
                assets.load(
                    GltfAssetLabel::Scene(0).from_asset("models/projectiles/paintball_orange.glb"),
                ),
            ),
        ]
        .into_iter()
        .collect(),
        mats: HashMap::new(),
        sounds: Sounds {
            shots: vec![assets.load("audio/weapons/basicshot.flac")],
            splats: (1..=3)
                .map(|i| ogg(&format!("impacts/splat_surface_{i:02}")))
                .collect(),
            player_hit: ogg("impacts/splat_player_hit"),
            hit_tick: ogg("ui/hit_marker_tick"),
            headshot: ogg("impacts/hit_metal"),
            dry_fire: ogg("weapons/dry_fire"),
            pod_reload: ogg("weapons/pod_reload"),
            water_pump: ogg("water/water_pump"),
            water_squirt: ogg("water/water_squirt"),
            water_splash: ogg("water/water_splash"),
            eliminated: assets.load("audio/game/elimination.flac"),
            confirm: ogg("game/flag_captured"),
            round_start: ogg("game/round_start_horn"),
            steps: (1..=4)
                .map(|i| assets.load(format!("audio/footsteps/footstep_{i:02}.flac")))
                .collect(),
            bunker_hit: ogg("impacts/hit_bunker_inflatable"),
        },
        rng: 0x9E37_79B9,
        last_step: usize::MAX,
    });
}

pub fn play_sfx(
    commands: &mut Commands,
    sound: &Handle<AudioSource>,
    base_volume: f32,
    settings: &AudioSettings,
) {
    let volume = base_volume * settings.effective_sfx();
    if volume <= 0.005 {
        return;
    }
    commands.spawn((
        AudioPlayer::new(sound.clone()),
        PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(volume)),
        SfxAudio(base_volume),
    ));
}

/// Spatial sounds use inverse-square falloff on positions scaled by this, so
/// they are at full volume within `1 / SPATIAL_SCALE` meters (4 m) and fade
/// out over the next couple dozen.
const SPATIAL_SCALE: f32 = 0.25;

/// Plays a sound at a point in the world: panned left/right and quieter with
/// distance, relative to the camera's `SpatialListener`.
pub fn play_sfx_at(
    commands: &mut Commands,
    sound: &Handle<AudioSource>,
    base_volume: f32,
    at: Vec3,
    settings: &AudioSettings,
) {
    let volume = base_volume * settings.effective_sfx();
    if volume <= 0.005 {
        return;
    }
    commands.spawn((
        AudioPlayer::new(sound.clone()),
        PlaybackSettings {
            spatial: true,
            spatial_scale: Some(bevy::audio::SpatialScale::new(SPATIAL_SCALE)),
            ..PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(volume))
        },
        Transform::from_translation(at),
        DespawnOnExit(AppState::InGame),
    ));
}

/// Quieter the further it is from the listener.
fn falloff(listener: Vec3, at: Vec3) -> f32 {
    (1.0 - listener.distance(at) / 70.0).clamp(0.08, 1.0)
}

#[derive(Component)]
struct Projectile {
    owner: PlayerId,
    team: Team,
    water: bool,
    velocity: Vec3,
    gravity: f32,
    damage: f32,
    age: f32,
    /// Despawned after this many seconds in the air.
    max_age: f32,
    /// One of a paint bomb's many balls: a plain sphere, only some of them
    /// splat with full effects and sound, and their hits on a player are
    /// summed into one `HitEvent` per frame instead of one each.
    shrapnel: bool,
}

#[derive(Component)]
pub struct PaintDecal {
    born: f32,
    /// Water dries up.
    fade: Option<f32>,
}

/// Paint that has landed and stays for the rest of the match, merged into a
/// handful of meshes (one per splat texture, team and area) instead of one
/// entity per splat, so a paint bomb's hundreds of balls don't cost hundreds
/// of draw calls forever.
#[derive(Resource, Default)]
struct BakedPaint {
    batches: HashMap<(AssetId<StandardMaterial>, IVec3), Vec<PaintBatch>>,
}

struct PaintBatch {
    entity: Entity,
    material: Handle<StandardMaterial>,
    /// Where the batch's mesh is anchored (the first splat in it); vertices are
    /// stored relative to this so transparent sorting has a sensible center.
    origin: Vec3,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl PaintBatch {
    fn quads(&self) -> usize {
        self.positions.len() / 4
    }

    /// Adds one splat: the unit decal plane (normal +Y, texture top at -Z)
    /// under the splat's transform, matching `Plane3d`'s UVs.
    fn push(&mut self, tf: &Transform) {
        let base = self.positions.len() as u32;
        let n = (tf.rotation * Vec3::Y).normalize_or(Vec3::Y).to_array();
        for (x, z) in [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)] {
            let p = tf.transform_point(Vec3::new(x, 0.0, z)) - self.origin;
            self.positions.push(p.to_array());
            self.normals.push(n);
            self.uvs.push([x + 0.5, z + 0.5]);
        }
        // Counter-clockwise seen from +Y (corners: 0 = -x-z, 1 = +x-z, 2 = -x+z, 3 = +x+z).
        self.indices
            .extend_from_slice(&[base, base + 2, base + 3, base, base + 3, base + 1]);
    }

    fn mesh(&self) -> Mesh {
        Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs.clone())
        .with_inserted_indices(bevy::mesh::Indices::U32(self.indices.clone()))
    }
}

#[derive(Component)]
struct Particle {
    vel: Vec3,
    life: f32,
    max: f32,
    s0: f32,
    s1: f32,
    gravity: f32,
}

/// The first-person gun, a child of the camera.
#[derive(Component)]
pub struct ViewModel;

fn spawn_viewmodel(mut commands: Commands, camera_q: Query<Entity, With<MainCamera>>) {
    let Ok(camera) = camera_q.single() else {
        return;
    };
    let vm = commands
        .spawn((
            ViewModel,
            GunModel { packed: u32::MAX },
            Transform::from_translation(HIP_POS),
            Visibility::Hidden,
        ))
        .id();
    commands.entity(camera).add_child(vm);
}

fn hide_viewmodel(mut vm_q: Query<&mut Visibility, With<ViewModel>>) {
    for mut v in &mut vm_q {
        *v = Visibility::Hidden;
    }
}

const HIP_POS: Vec3 = Vec3::new(0.24, -0.27, -0.62);

/// How far the gun rolls (about its barrel) when fully aimed: canted sights
/// sit on a 45° side mount, so aiming through one rolls the gun to bring it on top.
fn aim_cant(kind: GunKind, sight: u8) -> f32 {
    if kind == GunKind::Paintball && is_canted_sight(sight) && sight_eye(sight).is_some() {
        std::f32::consts::FRAC_PI_4
    } else {
        0.0
    }
}

/// The view model's rotation before sprint / bob / recoil: barrel forward
/// (gun +X to camera -Z), rolled by `cant` about the barrel.
fn gun_base_rotation(cant: f32) -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(-cant)
}

fn aim_pos(kind: GunKind, sight: u8) -> Vec3 {
    match kind {
        GunKind::Water => Vec3::new(0.06, -0.14, -0.5),
        // Looking through a sight: its eye point sits on the camera, so the
        // reticle is dead centre (where the shots go).
        GunKind::Paintball => match sight_eye(sight) {
            Some(eye) => -(gun_base_rotation(aim_cant(kind, sight)) * eye),
            // No sight / laser: gun just under the middle of the screen.
            None if sight == 0 => Vec3::new(0.1, -0.12, -0.5),
            None => Vec3::new(0.1, -0.14, -0.46),
        },
    }
}

/// The balls in our own hopper (first person) follow the ammo count: they
/// drain as we shoot and drop back in on a reload.
fn sync_hopper_balls(
    combat: Res<Combat>,
    vm_q: Query<(), With<ViewModel>>,
    parents: Query<&ChildOf>,
    mut hoppers: Query<(Entity, &mut Hopper)>,
) {
    let capacity = combat.stats.capacity.max(1);
    for (entity, mut hopper) in &mut hoppers {
        if !parents.iter_ancestors(entity).any(|e| vm_q.contains(e)) {
            continue;
        }
        let total = hopper.ball_count();
        let n = (total * combat.ammo.min(capacity) as usize).div_ceil(capacity as usize);
        if n != hopper.visible_balls {
            hopper.set_fill(n);
        }
    }
}

/// The gun follows the loadout: rebuild the view model, tell others, refill.
fn apply_loadout(
    mut commands: Commands,
    loadout: Res<Loadout>,
    guns: Option<Res<GunAssets>>,
    mut combat: ResMut<Combat>,
    mut local_state: ResMut<LocalState>,
    mut vm_q: Query<(Entity, &mut GunModel), With<ViewModel>>,
) {
    let Some(guns) = guns else {
        return;
    };
    let packed = loadout.pack();
    if local_state.0.loadout != packed {
        local_state.0.loadout = packed;
        let death_cam = combat.death_cam;
        let dead_for = combat.dead_for;
        let last_damage = combat.last_damage;
        let (head_paint, tint, seed) = (
            combat.head_paint,
            combat.head_paint_tint,
            combat.head_paint_seed,
        );
        combat.reset(&loadout.stats());
        combat.head_paint = head_paint;
        combat.head_paint_tint = tint;
        combat.head_paint_seed = seed;
        combat.death_cam = death_cam;
        combat.dead_for = dead_for;
        combat.last_damage = last_damage;
    }
    for (entity, mut gun) in &mut vm_q {
        if gun.packed != packed {
            gun.packed = packed;
            spawn_gun_parts(&mut commands, &guns, entity, &loadout);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn aim_and_reload(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<crate::settings::KeybindSettings>,
    overlay: Res<Overlay>,
    fx: Res<Fx>,
    settings: Res<AudioSettings>,
    local_state: Res<LocalState>,
    mut combat: ResMut<Combat>,
) {
    use crate::settings::KeyAction;
    let dt = time.delta_secs();
    let alive = local_state.0.health > 0.0;
    let can_act = alive && !overlay.blocks_input();
    combat.aiming =
        can_act && keybinds.pressed(KeyAction::Aim, &keys, &mouse) && combat.reloading.is_none();
    let target = if combat.aiming { 1.0 } else { 0.0 };
    let k = 1.0 - (-14.0 * dt).exp();
    combat.aim_blend += (target - combat.aim_blend) * k;
    combat.hitmarker = (combat.hitmarker - dt).max(0.0);
    combat.damage_flash = (combat.damage_flash - dt).max(0.0);
    combat.head_paint = (combat.head_paint - dt).max(0.0);
    combat.hit_arc = (combat.hit_arc - dt).max(0.0);
    // May dip just below zero: `fire_local` carries the leftover into the next
    // shot, so the fire rate doesn't round up to whole frames (slower at some
    // frame rates than others). Capped at one frame so idling saves nothing.
    combat.cooldown = (combat.cooldown - dt).max(-dt);
    combat.recoil = (combat.recoil - dt * 0.4).max(0.0) * (-12.0 * dt).exp();

    if let Some(left) = combat.reloading {
        let left = left - dt;
        if left <= 0.0 {
            combat.reloading = None;
            combat.ammo = combat.stats.capacity;
        } else {
            combat.reloading = Some(left);
        }
    } else if can_act
        && keybinds.just_pressed(KeyAction::Reload, &keys, &mouse)
        && combat.ammo < combat.stats.capacity
    {
        start_reload(&mut commands, &fx, &settings, &mut combat);
    }
}

fn start_reload(commands: &mut Commands, fx: &Fx, settings: &AudioSettings, combat: &mut Combat) {
    combat.reloading = Some(combat.stats.reload);
    let sound = if combat.stats.water {
        &fx.sounds.water_pump
    } else {
        &fx.sounds.pod_reload
    };
    play_sfx(commands, sound, 0.7, settings);
}

/// A shot's direction: where the shooter looks, nudged inside the spread
/// cone. Seeded by (shooter, shot number) so every screen agrees.
fn shot_direction(forward: Vec3, spread: f32, owner: PlayerId, shot: u32) -> Vec3 {
    let mut r = xorshift(owner.wrapping_mul(0x85EB_CA6B) ^ shot.wrapping_mul(0xC2B2_AE35) | 1);
    r = xorshift(r);
    let a = r as f32 / u32::MAX as f32 * TAU;
    r = xorshift(r);
    let m = (r as f32 / u32::MAX as f32).sqrt() * spread;
    let (t1, t2) = forward.any_orthonormal_pair();
    (forward + (t1 * a.cos() + t2 * a.sin()) * m).normalize_or(forward)
}

fn look_rotation(yaw: f32, pitch: f32) -> Quat {
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
}

#[allow(clippy::too_many_arguments)]
fn fire_local(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<crate::settings::KeybindSettings>,
    overlay: Res<Overlay>,
    net: Res<NetworkState>,
    settings: Res<AudioSettings>,
    mut fx: ResMut<Fx>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut combat: ResMut<Combat>,
    mut local_state: ResMut<LocalState>,
    player_q: Query<&LocalPlayer>,
    camera_q: Query<&Transform, With<MainCamera>>,
) {
    use crate::settings::KeyAction;
    let (Ok(player), Ok(cam)) = (player_q.single(), camera_q.single()) else {
        return;
    };
    let alive = local_state.0.health > 0.0;
    if !alive || overlay.blocks_input() || net.match_state.frozen() {
        return;
    }
    if !keybinds.pressed(KeyAction::Shoot, &keys, &mouse) {
        combat.dry_fired = false;
        return;
    }
    if combat.reloading.is_some() || combat.cooldown > 0.0 {
        return;
    }
    if combat.ammo == 0 {
        if !combat.dry_fired {
            combat.dry_fired = true;
            play_sfx(&mut commands, &fx.sounds.dry_fire, 0.7, &settings);
            start_reload(&mut commands, &fx, &settings, &mut combat);
        }
        return;
    }

    let stats = combat.stats;
    combat.ammo -= 1;
    combat.cooldown = combat.cooldown.min(0.0) + stats.fire_interval;
    combat.recoil = (combat.recoil + 0.35).min(1.0);
    local_state.0.shots = local_state.0.shots.wrapping_add(1);
    let shot = local_state.0.shots;

    let moving = player.speed > 0.5;
    let mut spread = stats.spread * if moving { 1.0 } else { 0.6 };
    spread *= 1.0 + (stats.aim_spread - 1.0) * combat.aim_blend;
    let forward = cam.forward().as_vec3();
    let dir = shot_direction(forward, spread, player.id, shot);
    let origin = cam.translation + forward * 0.45 + cam.right() * 0.06 * (1.0 - combat.aim_blend)
        - cam.up() * 0.05;
    let team = Team::of(net.local_player_id.unwrap_or(player.id));
    for pellet in 0..stats.pellets {
        let dir = if pellet == 0 {
            dir
        } else {
            shot_direction(forward, spread, player.id, pellet_seed(shot, pellet))
        };
        spawn_projectile(
            &mut commands,
            &mut fx,
            &mut materials,
            player.id,
            team,
            &stats,
            origin,
            dir,
        );
    }

    // Muzzle puff / spray at the barrel tip.
    let muzzle = cam.transform_point(
        HIP_POS.lerp(aim_pos(combat.kind, 0), combat.aim_blend) + Vec3::new(0.0, 0.02, -0.5),
    );
    muzzle_fx(
        &mut commands,
        &mut fx,
        &mut materials,
        muzzle,
        dir,
        team,
        stats.water,
    );
    shot_sound(
        &mut commands,
        &mut fx,
        &settings,
        &stats,
        1.0,
        time.elapsed_secs(),
    );
}

fn shot_sound(
    commands: &mut Commands,
    fx: &mut Fx,
    settings: &AudioSettings,
    stats: &WeaponStats,
    volume: f32,
    _now: f32,
) {
    if stats.water {
        let s = fx.sounds.water_squirt.clone();
        play_sfx(commands, &s, 0.35 * volume * stats.loudness, settings);
    } else {
        let n = fx.sounds.shots.len();
        let i = fx.pick(n);
        let s = fx.sounds.shots[i].clone();
        play_sfx(commands, &s, 0.8 * volume * stats.loudness, settings);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_projectile(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    owner: PlayerId,
    team: Team,
    stats: &WeaponStats,
    origin: Vec3,
    dir: Vec3,
) {
    let projectile = Projectile {
        owner,
        team,
        water: stats.water,
        velocity: dir * stats.speed,
        gravity: stats.gravity,
        damage: stats.damage,
        age: 0.0,
        max_age: 3.0,
        shrapnel: false,
    };
    if stats.water {
        let mat = fx.material(Paint::Water, DROPLET, materials);
        commands.spawn((
            Mesh3d(fx.billboard.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(origin).with_scale(Vec3::splat(0.09)),
            projectile,
            DespawnOnExit(AppState::InGame),
        ));
    } else {
        commands.spawn((
            WorldAssetRoot(fx.balls[&team].clone()),
            Transform::from_translation(origin).with_scale(Vec3::splat(2.2)),
            projectile,
            DespawnOnExit(AppState::InGame),
        ));
    }
}

/// One of a paint bomb's balls: it hits and splats like any other, and the
/// owner's screen reports its hits. A cheap shared sphere rather than the
/// gun's paintball model, since a bomb throws hundreds of them at once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_paintball(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    owner: PlayerId,
    team: Team,
    origin: Vec3,
    velocity: Vec3,
    gravity: f32,
    damage: f32,
    max_age: f32,
) {
    let mat = fx.material(Paint::Team(team), SHRAPNEL, materials);
    commands.spawn((
        Mesh3d(fx.shrapnel.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(origin),
        Projectile {
            owner,
            team,
            water: false,
            velocity,
            gravity,
            damage,
            age: 0.0,
            max_age,
            shrapnel: true,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

/// A burst of team paint in the air (grenades going off).
pub(crate) fn paint_burst_fx(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    point: Vec3,
    team: Team,
    size: f32,
) {
    let paint = Paint::Team(team);
    player_hit_fx(commands, fx, materials, point, Vec3::Y, paint);
    player_hit_fx(commands, fx, materials, point, Vec3::NEG_Y, paint);
    let puff = fx.material(paint, PUFF, materials);
    spawn_particle(
        commands,
        fx,
        puff,
        point,
        Vec3::Y * 0.5,
        0.3 * size,
        2.2 * size,
        0.6,
        0.0,
    );
    let ring = fx.material(paint, RING, materials);
    spawn_particle(
        commands,
        fx,
        ring,
        point,
        Vec3::ZERO,
        0.2 * size,
        3.0 * size,
        0.35,
        0.0,
    );
}

impl Fx {
    /// The paint splat textures (for paint over the screen).
    pub(crate) fn splat_images(&self) -> &[Handle<Image>] {
        &self.splats
    }
}

fn muzzle_fx(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    at: Vec3,
    dir: Vec3,
    team: Team,
    water: bool,
) {
    if water {
        let mat = fx.material(Paint::Water, DROPLET, materials);
        for _ in 0..3 {
            let vel = dir * fx.range(3.0, 6.0)
                + Vec3::new(fx.range(-0.5, 0.5), fx.range(0.0, 0.6), fx.range(-0.5, 0.5));
            let s = fx.range(0.015, 0.03);
            let life = fx.range(0.15, 0.3);
            spawn_particle(commands, fx, mat.clone(), at, vel, s, 0.0, life, 9.81);
        }
    } else {
        let _ = team;
        let mat = fx.material(Paint::Water, PUFF, materials);
        spawn_particle(commands, fx, mat, at, dir * 1.5, 0.04, 0.16, 0.12, 0.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_particle(
    commands: &mut Commands,
    fx: &Fx,
    mat: Handle<StandardMaterial>,
    at: Vec3,
    vel: Vec3,
    s0: f32,
    s1: f32,
    life: f32,
    gravity: f32,
) {
    commands.spawn((
        Mesh3d(fx.billboard.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(at).with_scale(Vec3::splat(s0.max(0.001))),
        Particle {
            vel,
            life: 0.0,
            max: life,
            s0,
            s1,
            gravity,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

/// Other players' new shots, fired from where they stand and look.
fn fire_remote(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<AudioSettings>,
    mut fx: ResMut<Fx>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera_q: Query<&GlobalTransform, With<MainCamera>>,
    mut remote_q: Query<(&Transform, &mut RemotePlayer)>,
) {
    let listener = camera_q
        .single()
        .map(|g| g.translation())
        .unwrap_or_default();
    for (_transform, mut remote) in &mut remote_q {
        let shots = remote.state.shots;
        let new = shots.wrapping_sub(remote.seen_shots);
        // Way out of order (or a wrapped counter): just catch up.
        if new == 0 || new > 50 {
            remote.seen_shots = shots;
            continue;
        }
        let first = remote.seen_shots.wrapping_add(1);
        remote.seen_shots = shots;
        if !remote.alive() {
            continue;
        }
        let stats = Loadout::unpack(remote.state.loadout).stats();
        let rot = look_rotation(remote.target_yaw, remote.state.pitch);
        let forward = rot * Vec3::NEG_Z;
        let crouched = remote.state.flags & FLAG_CROUCH != 0;
        let eye =
            remote.target_pos + Vec3::Y * (EYE_OFFSET - if crouched { CROUCH_DROP } else { 0.0 });
        let origin = eye + forward * 0.45 + rot * Vec3::new(0.06, -0.05, 0.0);
        let team = Team::of(remote.id);
        for i in 0..new.min(8) {
            let shot = first.wrapping_add(i);
            for pellet in 0..stats.pellets {
                let seed = if pellet == 0 {
                    shot
                } else {
                    pellet_seed(shot, pellet)
                };
                let dir = shot_direction(forward, stats.spread * 0.8, remote.id, seed);
                spawn_projectile(
                    &mut commands,
                    &mut fx,
                    &mut materials,
                    remote.id,
                    team,
                    &stats,
                    origin,
                    dir,
                );
            }
        }
        muzzle_fx(
            &mut commands,
            &mut fx,
            &mut materials,
            origin + forward * 0.1,
            forward,
            team,
            stats.water,
        );
        let volume = falloff(listener, origin);
        shot_sound(
            &mut commands,
            &mut fx,
            &settings,
            &stats,
            volume,
            time.elapsed_secs(),
        );
    }
}

/// Closest distance between segment p1-q1 and segment p2-q2, and where along
/// the first one it is (0..=1).
fn segment_segment(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (f32, f32) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-9 && e <= 1e-9 {
        return (r.length(), 0.0);
    }
    if a <= 1e-9 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-9 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-9 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    (c1.distance(c2), s)
}

/// Where along `from -> to` (0..=1) it passes through a standing player.
/// A different (but repeatable) spread seed for each extra shotgun ball.
fn pellet_seed(shot: u32, pellet: u32) -> u32 {
    shot ^ pellet.wrapping_mul(0x9E37_79B9)
}

/// Where along `from -> to` (0..=1) it passes through a player's body
/// capsule, and whether that is in the head zone (the top `HEAD_ZONE` m).
fn hits_player(from: Vec3, to: Vec3, center: Vec3, crouched: bool) -> Option<(f32, bool)> {
    // Crouching drops the top of the body; the feet stay put.
    let drop = if crouched { CROUCH_DROP } else { 0.0 };
    let axis = PLAYER_GROUND_Y - PLAYER_HIT_RADIUS - drop * 0.5;
    let center = center - Vec3::Y * drop * 0.5;
    let (dist, s) = segment_segment(from, to, center - Vec3::Y * axis, center + Vec3::Y * axis);
    if dist > PLAYER_HIT_RADIUS {
        return None;
    }
    let top = center.y + axis + PLAYER_HIT_RADIUS;
    let at = from.lerp(to, s).y;
    Some((s, at >= top - HEAD_ZONE))
}

enum Target {
    Local,
    Remote(PlayerId),
    /// An eliminated player's body.
    Body(Entity, BodyHit),
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn move_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    colliders: Res<Colliders>,
    net: Res<NetworkState>,
    settings: Res<AudioSettings>,
    mut fx: ResMut<Fx>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut combat: ResMut<Combat>,
    mut hits: ResMut<HitBookkeeping>,
    local_state: Res<LocalState>,
    camera_q: Query<&GlobalTransform, With<MainCamera>>,
    local_q: Query<(&Transform, &LocalPlayer), Without<Projectile>>,
    remote_q: Query<(&Transform, &RemotePlayer), Without<Projectile>>,
    mut balls: Query<(Entity, &mut Transform, &mut Projectile)>,
    mut bodies: Query<(Entity, &mut Ragdoll)>,
) {
    // Capped only for hitches: balls sweep segments, so long steps can't skip walls.
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    let listener = camera_q
        .single()
        .map(|g| g.translation())
        .unwrap_or_default();
    let local = local_q.single().ok();
    let local_id = local.map(|(_, p)| p.id);
    // Paint bomb hits this frame, per (thrower, victim): (damage, velocity, point).
    let mut shrapnel_hits: HashMap<(PlayerId, PlayerId), (f32, Vec3, Vec3)> = HashMap::new();
    let mut shrapnel_sounds = 0;

    for (entity, mut tf, mut ball) in &mut balls {
        ball.age += dt;
        if ball.age > ball.max_age {
            commands.entity(entity).despawn();
            continue;
        }
        // Exact under constant gravity, so the arc is the same at any frame rate.
        let g = ball.gravity;
        let from = tf.translation;
        let to = from + ball.velocity * dt - Vec3::Y * (0.5 * g * dt * dt);
        ball.velocity.y -= g * dt;

        let world_hit = colliders.segment_hit(from, to);
        let mut best_t = world_hit.map(|h| h.t).unwrap_or(f32::MAX);
        let mut target = None;
        let mut headshot = false;

        // Players: enemies only (paint passes through teammates).
        if let Some((lt, lp)) = local
            && ball.owner != lp.id
            && enemies(ball.owner, lp.id)
            && local_state.0.health > 0.0
            && let Some((t, _)) = hits_player(from, to, lt.translation, lp.crouch > 0.5)
            && t < best_t
        {
            best_t = t;
            target = Some(Target::Local);
        }
        for (rt, remote) in &remote_q {
            if !enemies(ball.owner, remote.id) || !remote.alive() {
                continue;
            }
            let crouched = remote.state.flags & FLAG_CROUCH != 0;
            if let Some((t, head)) = hits_player(from, to, rt.translation, crouched)
                && t < best_t
            {
                best_t = t;
                headshot = head;
                target = Some(Target::Remote(remote.id));
            }
        }
        // Bodies: any paint knocks them around.
        for (body, rd) in &bodies {
            if let Some(hit) = rd.ball_hit(from, to)
                && hit.t < best_t
            {
                best_t = hit.t;
                target = Some(Target::Body(body, hit));
            }
        }

        let paint = if ball.water {
            Paint::Water
        } else {
            Paint::Team(ball.team)
        };
        match target {
            Some(Target::Local) => {
                // Visual cleanup only: damage is applied from shooter-confirmed HitEvents
                // in `process_incoming_hits` so moving players never miss hits or take double damage.
                commands.entity(entity).despawn();
            }
            Some(Target::Remote(victim_id)) if ball.shrapnel => {
                // Summed up and reported once after the loop.
                let point = from.lerp(to, best_t);
                if Some(ball.owner) == local_id {
                    let damage = if headshot {
                        ball.damage * HEADSHOT_MULTIPLIER
                    } else {
                        ball.damage
                    };
                    let e = shrapnel_hits.entry((ball.owner, victim_id)).or_insert((
                        0.0,
                        Vec3::ZERO,
                        point,
                    ));
                    e.0 += damage;
                    e.1 += ball.velocity;
                } else {
                    shrapnel_hits.entry((ball.owner, victim_id)).or_insert((
                        0.0,
                        ball.velocity,
                        point,
                    ));
                }
                commands.entity(entity).despawn();
            }
            Some(Target::Remote(victim_id)) => {
                let point = from.lerp(to, best_t);
                let normal = -ball.velocity.normalize_or(Vec3::Z);
                player_hit_fx(&mut commands, &mut fx, &mut materials, point, normal, paint);
                if Some(ball.owner) == local_id {
                    // A kill marker that is still showing wins over a new hit.
                    if combat.hitmarker_kind != HitmarkerKind::Kill || combat.hitmarker <= 0.0 {
                        combat.hitmarker_kind = if headshot {
                            HitmarkerKind::Headshot
                        } else {
                            HitmarkerKind::Hit
                        };
                        combat.hitmarker = HITMARKER_SECS;
                    }
                    let s = fx.sounds.hit_tick.clone();
                    play_sfx(&mut commands, &s, 0.8, &settings);
                    if headshot {
                        let s = fx.sounds.headshot.clone();
                        play_sfx(&mut commands, &s, 0.75, &settings);
                    }
                    let hit_id = hits.alloc_id();
                    net.send_hit(HitEvent {
                        hit_id,
                        attacker: ball.owner,
                        victim: victim_id,
                        damage: if headshot {
                            ball.damage * HEADSHOT_MULTIPLIER
                        } else {
                            ball.damage
                        },
                        velocity: ball.velocity.to_array(),
                        point: point.to_array(),
                        water: ball.water,
                        headshot,
                        flash: false,
                    });
                }
                let s = fx.sounds.player_hit.clone();
                play_sfx(&mut commands, &s, 0.6 * falloff(listener, point), &settings);
                commands.entity(entity).despawn();
            }
            Some(Target::Body(body, hit)) => {
                let point = from.lerp(to, best_t);
                let normal = -ball.velocity.normalize_or(Vec3::Z);
                let full = !ball.shrapnel || fx.pick(SHRAPNEL_SPLAT_EVERY as usize) == 0;
                if full {
                    player_hit_fx(&mut commands, &mut fx, &mut materials, point, normal, paint);
                }
                // The host moves the body for everyone (and a body the host
                // hasn't sent us yet moves here).
                if let Ok((_, mut rd)) = bodies.get_mut(body) {
                    rd.shove(&hit, ball.velocity, ball.damage);
                }
                if full {
                    let s = fx.sounds.player_hit.clone();
                    play_sfx(
                        &mut commands,
                        &s,
                        0.45 * falloff(listener, point),
                        &settings,
                    );
                }
                commands.entity(entity).despawn();
            }
            None => {
                if let Some(hit) = world_hit {
                    // Most of a paint bomb's balls just vanish into the
                    // splats the rest leave, and only a couple a frame make
                    // a sound, so a burst doesn't flood the scene.
                    let full = !ball.shrapnel || fx.pick(SHRAPNEL_SPLAT_EVERY as usize) == 0;
                    if !full && hit.decals && fx.pick(SHRAPNEL_DECAL_EVERY as usize) == 0 {
                        // Paint now stays for the match (merged by
                        // `bake_decals`), so more balls leave a plain splat
                        // with no particles or sound.
                        let n = hit.normal.normalize_or(Vec3::Y);
                        spawn_decal(
                            &mut commands,
                            &mut fx,
                            &mut materials,
                            now,
                            hit.point,
                            n,
                            paint,
                        );
                    }
                    if full {
                        let mut volume = falloff(listener, hit.point);
                        if ball.shrapnel {
                            if shrapnel_sounds < SHRAPNEL_SOUNDS_PER_FRAME {
                                shrapnel_sounds += 1;
                            } else {
                                volume = 0.0;
                            }
                        }
                        impact_fx(
                            &mut commands,
                            &mut fx,
                            &mut materials,
                            &settings,
                            now,
                            hit.point,
                            hit.normal,
                            paint,
                            hit.decals,
                            volume,
                        );
                    }
                    commands.entity(entity).despawn();
                } else {
                    tf.translation = to;
                }
            }
        }
    }

    // One splash, one marker and one `HitEvent` per player a paint bomb got
    // this frame, however many of its balls hit them.
    for ((owner, victim_id), (damage, velocity, point)) in shrapnel_hits {
        let normal = -velocity.normalize_or(Vec3::Z);
        let paint = Paint::Team(Team::of(owner));
        player_hit_fx(&mut commands, &mut fx, &mut materials, point, normal, paint);
        let s = fx.sounds.player_hit.clone();
        play_sfx(&mut commands, &s, 0.6 * falloff(listener, point), &settings);
        if damage <= 0.0 {
            continue;
        }
        if combat.hitmarker_kind != HitmarkerKind::Kill || combat.hitmarker <= 0.0 {
            combat.hitmarker_kind = HitmarkerKind::Hit;
            combat.hitmarker = HITMARKER_SECS;
        }
        let s = fx.sounds.hit_tick.clone();
        play_sfx(&mut commands, &s, 0.8, &settings);
        let hit_id = hits.alloc_id();
        net.send_hit(HitEvent {
            hit_id,
            attacker: owner,
            victim: victim_id,
            damage,
            velocity: velocity.normalize_or(Vec3::Z).to_array().map(|v| v * 13.0),
            point: point.to_array(),
            water: false,
            headshot: false,
            flash: false,
        });
    }
}

/// Applies shooter-confirmed hits relayed over the network to the local player.
#[allow(clippy::too_many_arguments)]
fn process_incoming_hits(
    mut commands: Commands,
    time: Res<Time>,
    player_assets: Res<PlayerModelAssets>,
    settings: Res<AudioSettings>,
    mut net: ResMut<NetworkState>,
    mut fx: ResMut<Fx>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut combat: ResMut<Combat>,
    mut hits: ResMut<HitBookkeeping>,
    mut local_state: ResMut<LocalState>,
    mut feed: ResMut<Killfeed>,
    local_q: Query<(&Transform, &LocalPlayer)>,
) {
    if net.pending_hits.is_empty() {
        return;
    }
    // Flashbang hits stay queued for the grenades module.
    let (flashes, incoming): (Vec<_>, Vec<_>) = std::mem::take(&mut net.pending_hits)
        .into_iter()
        .partition(|h| h.flash);
    net.pending_hits = flashes;
    if incoming.is_empty() {
        return;
    }
    let Ok((lt, lp)) = local_q.single() else {
        return;
    };
    let now = time.elapsed_secs();

    for hit in incoming {
        if hit.victim != lp.id
            || hit.attacker == lp.id
            || !enemies(hit.attacker, lp.id)
            || !hit.damage.is_finite()
            || hit.damage <= 0.0
        {
            continue;
        }
        if !hits.record_incoming(hit.attacker, hit.hit_id) {
            continue;
        }
        if local_state.0.health <= 0.0 {
            continue;
        }
        let velocity = Vec3::from_array(hit.velocity);
        let normal = -velocity.normalize_or(Vec3::Z);
        combat.hit_from = (-velocity).with_y(0.0).normalize_or(Vec3::Z);
        combat.hit_arc = 1.2;
        let point = lt.translation + Vec3::Y * 0.2;
        let paint = if hit.water {
            Paint::Water
        } else {
            Paint::Team(Team::of(hit.attacker))
        };
        player_hit_fx(&mut commands, &mut fx, &mut materials, point, normal, paint);
        let s = fx.sounds.player_hit.clone();
        play_sfx(&mut commands, &s, 0.9, &settings);
        if hit.headshot {
            let s = fx.sounds.headshot.clone();
            play_sfx(&mut commands, &s, 0.6, &settings);
            combat.head_paint = HEAD_PAINT_SECS;
            combat.head_paint_tint = paint.color().with_alpha(1.0);
            combat.head_paint_seed = combat.head_paint_seed.wrapping_add(1);
        }
        take_damage(
            &mut commands,
            &mut fx,
            &player_assets,
            &settings,
            &mut combat,
            &mut local_state,
            &mut feed,
            lt,
            lp,
            hit.attacker,
            hit.damage.min(150.0),
            velocity,
            now,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn take_damage(
    commands: &mut Commands,
    fx: &mut Fx,
    player_assets: &PlayerModelAssets,
    settings: &AudioSettings,
    combat: &mut Combat,
    local_state: &mut LocalState,
    feed: &mut Killfeed,
    transform: &Transform,
    player: &LocalPlayer,
    from: PlayerId,
    damage: f32,
    velocity: Vec3,
    now: f32,
) {
    let state = &mut local_state.0;
    if state.health <= 0.0 {
        return;
    }
    state.health = (state.health - damage).max(0.0);
    combat.last_damage = now;
    combat.damage_flash = 0.35;
    if state.health > 0.0 {
        return;
    }
    // Eliminated.
    state.deaths = state.deaths.wrapping_add(1);
    state.killer = from;
    combat.reloading = None;
    combat.dead_for = 0.0;
    let feet = transform.translation - Vec3::Y * PLAYER_GROUND_Y;
    combat.death_cam = Some(feet);
    feed.entries.push((from, player.id, now));
    let s = fx.sounds.eliminated.clone();
    play_sfx(commands, &s, 0.8, settings);
    let model = Transform::from_translation(feet)
        .with_rotation(Quat::from_rotation_y(player.yaw + std::f32::consts::PI));
    spawn_ragdoll(
        commands,
        player_assets,
        player.id,
        state.deaths,
        RagdollStart::from_rest(&model),
        velocity,
    );
}

/// Others' eliminations: a ragdoll where they fell, a killfeed line, and
/// their model hidden until they respawn.
#[allow(clippy::too_many_arguments)]
fn remote_eliminations(
    mut commands: Commands,
    time: Res<Time>,
    player_assets: Res<PlayerModelAssets>,
    fx: Res<Fx>,
    settings: Res<AudioSettings>,
    net: Res<NetworkState>,
    mut feed: ResMut<Killfeed>,
    mut remote_q: Query<(&Transform, &mut RemotePlayer)>,
    local_q: Query<(&Transform, &LocalPlayer), Without<RemotePlayer>>,
    mut models: Query<(&GlobalTransform, Option<&Rig>, &mut Visibility)>,
    bones: Query<&GlobalTransform>,
    mut combat: ResMut<Combat>,
    mut splats: MessageWriter<crate::progression::LocalSplat>,
) {
    let local_id = net.local_player_id;
    let positions: HashMap<PlayerId, Vec3> = remote_q
        .iter()
        .map(|(t, r)| (r.id, t.translation))
        .collect();
    for (transform, mut remote) in &mut remote_q {
        let Ok((model_tf, rig, mut vis)) = models.get_mut(remote.model) else {
            continue;
        };
        let deaths = remote.state.deaths;
        if deaths != remote.seen_deaths {
            let fresh = deaths.wrapping_sub(remote.seen_deaths) < 10;
            remote.seen_deaths = deaths;
            if fresh {
                let killer = remote.state.killer;
                feed.entries.push((killer, remote.id, time.elapsed_secs()));
                if Some(killer) == local_id {
                    play_sfx(&mut commands, &fx.sounds.confirm, 0.6, &settings);
                    if enemies(killer, remote.id) {
                        // Kill confirm: a red hit marker.
                        combat.hitmarker_kind = HitmarkerKind::Kill;
                        combat.hitmarker = KILLMARKER_SECS;
                    }
                    if enemies(killer, remote.id)
                        && let Ok((me, player)) = local_q.single()
                    {
                        splats.write(crate::progression::LocalSplat {
                            distance: me.translation.distance(transform.translation),
                            crouched: player.crouch > 0.5,
                            aiming: combat.aim_blend > 0.5,
                        });
                    }
                }
                let start = match rig {
                    Some(rig) => RagdollStart::from_rig(rig, model_tf, &bones),
                    None => RagdollStart::from_rest(&model_tf.compute_transform()),
                };
                let from = positions.get(&killer).copied().or_else(|| {
                    local_q
                        .single()
                        .ok()
                        .filter(|(_, p)| p.id == killer)
                        .map(|(t, _)| t.translation)
                });
                let push = from
                    .map(|k| transform.translation - k)
                    .unwrap_or(transform.forward().as_vec3() * -1.0);
                spawn_ragdoll(
                    &mut commands,
                    &player_assets,
                    remote.id,
                    deaths,
                    start,
                    push,
                );
            }
        }
        let want = if remote.alive() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn regen_and_respawn(
    mut commands: Commands,
    time: Res<Time>,
    fx: Res<Fx>,
    settings: Res<AudioSettings>,
    loadout: Res<Loadout>,
    map: Res<MapInfo>,
    net: Res<NetworkState>,
    mut combat: ResMut<Combat>,
    mut local_state: ResMut<LocalState>,
    mut feed: ResMut<Killfeed>,
    mut player_q: Query<(&mut Transform, &mut LocalPlayer)>,
    mut rig_q: Query<&mut CameraRig, With<MainCamera>>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    feed.entries.retain(|e| now - e.2 < 6.0);

    let state = &mut local_state.0;
    if state.health > 0.0 {
        if now - combat.last_damage >= REGEN_DELAY && state.health < MAX_HEALTH {
            state.health = (state.health + REGEN_RATE * dt).min(MAX_HEALTH);
        }
        return;
    }
    combat.dead_for += dt;
    // Elimination: out until the next round.
    if combat.dead_for < RESPAWN_TIME || !net.match_state.respawns() {
        return;
    }
    // Back in: full health and paint, at a spawn away from enemies.
    let Ok((mut transform, mut player)) = player_q.single_mut() else {
        return;
    };
    let mut rig = rig_q.single_mut().ok();
    respawn_local(
        &map,
        &net,
        &loadout,
        &mut combat,
        &mut local_state,
        &mut transform,
        &mut player,
        rig.as_deref_mut(),
    );
    play_sfx(&mut commands, &fx.sounds.round_start, 0.35, &settings);
}

#[allow(clippy::too_many_arguments)]
fn update_viewmodel(
    net: Res<NetworkState>,
    combat: Res<Combat>,
    local_state: Res<LocalState>,
    overlay: Res<Overlay>,
    loadout: Res<Loadout>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<crate::settings::KeybindSettings>,
    player_q: Query<&LocalPlayer>,
    mut vm_q: Query<(&mut Transform, &mut Visibility), With<ViewModel>>,
    mut sprint_blend: Local<f32>,
    mut bob_phase: Local<f32>,
    mut bob_amount: Local<f32>,
) {
    let Ok((mut tf, mut vis)) = vm_q.single_mut() else {
        return;
    };
    let dt = time.delta_secs().min(0.05);
    let alive = local_state.0.health > 0.0;
    let podium = net.match_state.phase == crate::protocol::Phase::MatchOver;
    *vis = if alive && !podium && !overlay.loadout && !overlay.settings {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let (speed, grounded, crouch) = player_q
        .single()
        .map(|p| (p.speed, p.grounded, p.crouch))
        .unwrap_or((0.0, true, 0.0));

    // Sprinting: holding sprint, actually moving fast, on the ground, not
    // aiming, crouching or reloading. Eased so the gun swings in and out.
    let sprinting = alive
        && !overlay.blocks_input()
        && keybinds.pressed(crate::settings::KeyAction::Sprint, &keys, &mouse)
        && speed > 4.0
        && grounded
        && crouch < 0.5
        && !combat.aiming
        && combat.reloading.is_none();
    let sprint_target = if sprinting { 1.0 } else { 0.0 };
    let sprint_rate = if sprinting { 9.0 } else { 12.0 };
    *sprint_blend += (sprint_target - *sprint_blend) * (1.0 - (-sprint_rate * dt).exp());
    let sb = *sprint_blend;

    // Weapon bob (Battlefield / CoD style): the phase advances with how fast
    // we move, so steps and gun bounce stay in sync; it fades out in the air
    // and while aiming down sights.
    let moving = (speed / 10.0).min(1.0);
    let bob_target = if grounded {
        moving * (1.0 - combat.aim_blend * 0.85)
    } else {
        0.0
    };
    *bob_amount += (bob_target - *bob_amount) * (1.0 - (-10.0 * dt).exp());
    let freq = 1.55 + sb * 0.55; // steps per metre-ish, quicker cadence when sprinting
    *bob_phase = (*bob_phase + speed * freq * dt) % TAU;
    let ph = *bob_phase;
    let amt = *bob_amount;
    // Figure-eight: side to side once per stride, down on every footfall.
    let side = ph.sin();
    let vert = -(ph * 2.0).cos() * 0.5 - 0.5; // 0 at mid-stride, -1 at footfall
    let walk_amp = 1.0 - sb;
    let sway = Vec3::new(
        side * (0.012 * walk_amp + 0.032 * sb),
        vert * (0.010 * walk_amp + 0.026 * sb),
        vert * -0.012 * sb,
    ) * amt;
    let bob_yaw = side * (0.015 * walk_amp + 0.07 * sb) * amt;
    let bob_roll = side * (0.02 * walk_amp + 0.09 * sb) * amt;
    let bob_pitch = vert * (0.01 * walk_amp + 0.05 * sb) * amt;

    let mut pos = HIP_POS.lerp(aim_pos(combat.kind, loadout.sight), combat.aim_blend) + sway;
    // Sprint pose: pull the gun in toward the chest and lower it, barrel
    // swung across to the left.
    pos += Vec3::new(-0.07, -0.06, 0.08) * sb;
    pos.z += combat.recoil * 0.035;
    // Reloading: dip the gun.
    let reload_dip = if let Some(left) = combat.reloading {
        let total = combat.stats.reload.max(0.01);
        let k = 1.0 - left / total;
        (k * std::f32::consts::PI).sin()
    } else {
        0.0
    };
    pos.y -= reload_dip * 0.12;
    tf.translation = pos;
    // Camera-space sprint tilt: yaw left (barrel points left), roll the top
    // of the gun inward, and dip the muzzle a little.
    let sprint_rot = Quat::from_rotation_y(0.95 * sb + bob_yaw)
        * Quat::from_rotation_x(-0.22 * sb + bob_pitch)
        * Quat::from_rotation_z(0.35 * sb + bob_roll);
    let cant = aim_cant(combat.kind, loadout.sight) * combat.aim_blend;
    tf.rotation = sprint_rot
        * gun_base_rotation(cant)
        * Quat::from_rotation_z(combat.recoil * 0.06 - reload_dip * 0.5);
}

/// How loud a footstep is: crouched steps are softer. The footstep files
/// are normalized to about -1 dB peak, and this sits a bit above 1.0 so
/// steps cut through gunfire (others' steps also fade with distance).
fn step_volume(crouched: bool) -> f32 {
    if crouched { 0.6 } else { 1.3 }
}

/// Our own footsteps, and the step counter others use to hear them.
fn footsteps(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<AudioSettings>,
    mut fx: ResMut<Fx>,
    mut combat: ResMut<Combat>,
    mut local_state: ResMut<LocalState>,
    player_q: Query<&LocalPlayer>,
) {
    let Ok(player) = player_q.single() else {
        return;
    };
    if local_state.0.health <= 0.0 || !player.grounded || player.speed < 1.0 {
        combat.step_timer = 0.15;
        return;
    }
    combat.step_timer -= time.delta_secs();
    if combat.step_timer <= 0.0 {
        let crouched = player.crouch > 0.5;
        // Added to the leftover so the step cadence doesn't depend on frame rate.
        combat.step_timer += if crouched {
            0.55
        } else if player.speed > 8.0 {
            0.3
        } else {
            0.42
        };
        combat.step_timer = combat.step_timer.max(0.1);
        local_state.0.steps = local_state.0.steps.wrapping_add(1);
        let s = fx.step_sound();
        play_sfx(&mut commands, &s, step_volume(crouched), &settings);
    }
}

/// Other players' footsteps, heard from where they walk: their step counter
/// went up, so play one step at their feet (panned and faded by distance).
fn remote_footsteps(
    mut commands: Commands,
    settings: Res<AudioSettings>,
    mut fx: ResMut<Fx>,
    mut remote_q: Query<(&Transform, &mut RemotePlayer)>,
) {
    for (transform, mut remote) in &mut remote_q {
        let steps = remote.state.steps;
        let new = steps.wrapping_sub(remote.seen_steps);
        remote.seen_steps = steps;
        // Nothing new, or way out of order (a rejoin / wrapped counter).
        if new == 0 || new > 20 || !remote.alive() {
            continue;
        }
        let crouched = remote.state.flags & FLAG_CROUCH != 0;
        let feet = transform.translation - Vec3::Y * PLAYER_GROUND_Y;
        let s = fx.step_sound();
        // Several steps can arrive in one snapshot; one sound is enough.
        play_sfx_at(&mut commands, &s, step_volume(crouched), feet, &settings);
    }
}

fn player_hit_fx(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    point: Vec3,
    normal: Vec3,
    paint: Paint,
) {
    let droplet = fx.material(paint, DROPLET, materials);
    let (t1, t2) = normal.any_orthonormal_pair();
    for _ in 0..20 {
        let a = fx.range(0.0, TAU);
        let vel = normal * fx.range(1.5, 3.5) + (t1 * a.cos() + t2 * a.sin()) * fx.range(0.8, 2.8);
        let s = fx.range(0.02, 0.05);
        let life = fx.range(0.35, 0.7);
        spawn_particle(
            commands,
            fx,
            droplet.clone(),
            point,
            vel,
            s,
            0.0,
            life,
            9.81,
        );
    }
    let ring = fx.material(paint, RING, materials);
    spawn_particle(commands, fx, ring, point, Vec3::ZERO, 0.1, 0.6, 0.25, 0.0);
}

/// Leaves one paint splat (or a drip on a wall) at `point`, which `bake_decals`
/// later merges into the match's lasting paint.
fn spawn_decal(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    now: f32,
    point: Vec3,
    n: Vec3,
    paint: Paint,
) {
    let water = paint == Paint::Water;
    let wall = n.y.abs() < 0.5;
    let (kind, rotation) = if wall && fx.rand() < 0.6 {
        // Drips run down walls.
        let count = fx.drips.len();
        let i = DRIP + fx.pick(count) as u8;
        let up = (Vec3::Y - n * n.y).normalize_or(Vec3::Y);
        // The decal plane's +Y is the normal; its -Z (texture top) points up.
        let rot = Quat::from_mat3(&Mat3::from_cols(up.cross(n).normalize_or(Vec3::X), n, -up));
        (i, rot)
    } else {
        let count = fx.splats.len();
        let i = fx.pick(count) as u8;
        let rot = Quat::from_rotation_arc(Vec3::Y, n) * Quat::from_rotation_y(fx.range(0.0, TAU));
        (i, rot)
    };
    let mat = fx.material(paint, kind, materials);
    let size = if water {
        fx.range(0.3, 0.5)
    } else {
        fx.range(0.25, 0.45)
    };
    commands.spawn((
        Mesh3d(fx.decal.clone()),
        MeshMaterial3d(mat),
        Transform {
            translation: point + n * 0.006,
            rotation,
            scale: Vec3::splat(size),
        },
        PaintDecal {
            born: now,
            fade: water.then_some(5.0),
        },
        DespawnOnExit(AppState::InGame),
    ));
}

#[allow(clippy::too_many_arguments)]
fn impact_fx(
    commands: &mut Commands,
    fx: &mut Fx,
    materials: &mut Assets<StandardMaterial>,
    settings: &AudioSettings,
    now: f32,
    point: Vec3,
    normal: Vec3,
    paint: Paint,
    decal: bool,
    volume: f32,
) {
    let n = normal.normalize_or(Vec3::Y);
    let water = paint == Paint::Water;

    if decal {
        spawn_decal(commands, fx, materials, now, point, n, paint);
    }

    let droplet = fx.material(paint, DROPLET, materials);
    let (t1, t2) = n.any_orthonormal_pair();
    let count = if water { 10 } else { 14 };
    for _ in 0..count {
        let a = fx.range(0.0, TAU);
        let vel = n * fx.range(1.0, 3.0) + (t1 * a.cos() + t2 * a.sin()) * fx.range(0.8, 2.8);
        let s = fx.range(0.02, 0.05);
        let life = fx.range(0.35, 0.7);
        spawn_particle(
            commands,
            fx,
            droplet.clone(),
            point + n * 0.02,
            vel,
            s,
            0.0,
            life,
            9.81,
        );
    }
    let puff = fx.material(paint, PUFF, materials);
    spawn_particle(
        commands,
        fx,
        puff,
        point + n * 0.05,
        n * 0.4,
        0.1,
        0.45,
        0.35,
        0.0,
    );

    if water {
        let s = fx.sounds.water_splash.clone();
        play_sfx(commands, &s, 0.25 * volume, settings);
    } else {
        let count = fx.sounds.splats.len();
        let i = fx.pick(count);
        let s = fx.sounds.splats[i].clone();
        play_sfx(commands, &s, 0.5 * volume, settings);
        if decal && n.y.abs() < 0.9 && fx.rand() < 0.5 {
            let s = fx.sounds.bunker_hit.clone();
            play_sfx(commands, &s, 0.3 * volume, settings);
        }
    }
}

fn update_particles(
    mut commands: Commands,
    time: Res<Time>,
    cameras: Query<&GlobalTransform, With<MainCamera>>,
    mut particles: Query<(Entity, &mut Transform, &mut Particle)>,
) {
    let dt = time.delta_secs();
    let cam = cameras.single().map(|g| g.translation()).ok();
    for (entity, mut tf, mut p) in &mut particles {
        p.life += dt;
        if p.life >= p.max {
            commands.entity(entity).despawn();
            continue;
        }
        let g = p.gravity;
        tf.translation += p.vel * dt - Vec3::Y * (0.5 * g * dt * dt);
        p.vel.y -= g * dt;
        let k = p.life / p.max;
        tf.scale = Vec3::splat((p.s0 + (p.s1 - p.s0) * k).max(0.001));
        if let Some(c) = cam {
            tf.look_at(c, Vec3::Y);
        }
    }
}

/// Dries up water, and merges settled paint into `BakedPaint` so splats on
/// walls and floors stay for the whole match.
fn bake_decals(
    mut commands: Commands,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut baked: ResMut<BakedPaint>,
    decals: Query<(
        Entity,
        &PaintDecal,
        &Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let now = time.elapsed_secs();
    let mut fresh = 0;
    let mut oldest = now;
    for (e, d, _, _) in &decals {
        match d.fade {
            Some(fade) if now - d.born > fade => commands.entity(e).despawn(),
            Some(_) => {}
            None => {
                fresh += 1;
                oldest = oldest.min(d.born);
            }
        }
    }
    if fresh == 0 || (fresh < BAKE_AT_COUNT && now - oldest < BAKE_AFTER_SECS) {
        return;
    }

    let mut touched: HashSet<(AssetId<StandardMaterial>, IVec3, usize)> = HashSet::new();
    for (e, d, tf, mat) in &decals {
        if d.fade.is_some() {
            continue;
        }
        let cell = (tf.translation / BAKE_CELL).floor().as_ivec3();
        let key = (mat.id(), cell);
        let list = baked.batches.entry(key).or_default();
        if list.last().is_none_or(|b| b.quads() >= QUADS_PER_BATCH) {
            list.push(PaintBatch {
                entity: Entity::PLACEHOLDER,
                material: mat.0.clone(),
                origin: tf.translation,
                positions: Vec::with_capacity(QUADS_PER_BATCH * 4),
                normals: Vec::with_capacity(QUADS_PER_BATCH * 4),
                uvs: Vec::with_capacity(QUADS_PER_BATCH * 4),
                indices: Vec::with_capacity(QUADS_PER_BATCH * 6),
            });
        }
        let idx = list.len() - 1;
        list[idx].push(tf);
        touched.insert((key.0, key.1, idx));
        commands.entity(e).despawn();
    }

    // Respawn each changed batch with its rebuilt mesh, so its bounds (for
    // culling) are worked out fresh. Batches are few and rebuilt rarely.
    for (mat_id, cell, idx) in touched {
        let Some(batch) = baked
            .batches
            .get_mut(&(mat_id, cell))
            .and_then(|l| l.get_mut(idx))
        else {
            continue;
        };
        if batch.entity != Entity::PLACEHOLDER {
            commands.entity(batch.entity).try_despawn();
        }
        batch.entity = commands
            .spawn((
                Mesh3d(meshes.add(batch.mesh())),
                MeshMaterial3d(batch.material.clone()),
                Transform::from_translation(batch.origin),
                DespawnOnExit(AppState::InGame),
            ))
            .id();
    }
}

/// The merged paint entities go with the match; forget them too.
fn clear_baked_paint(mut baked: ResMut<BakedPaint>) {
    baked.batches.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everyone_agrees_on_a_shots_direction() {
        let f = Vec3::new(0.3, 0.1, -1.0).normalize();
        let a = shot_direction(f, 0.02, 3, 17);
        let b = shot_direction(f, 0.02, 3, 17);
        assert_eq!(a, b);
        assert!(a.angle_between(f) <= 0.021);
        assert_ne!(a, shot_direction(f, 0.02, 3, 18));
    }

    #[test]
    fn balls_hit_standing_players() {
        let center = Vec3::new(0.0, PLAYER_GROUND_Y, 0.0);
        assert!(
            hits_player(
                Vec3::new(-2.0, 1.4, 0.0),
                Vec3::new(2.0, 1.4, 0.0),
                center,
                false
            )
            .is_some_and(|(_, head)| !head)
        );
        assert!(
            hits_player(
                Vec3::new(-2.0, 1.4, 1.0),
                Vec3::new(2.0, 1.4, 1.0),
                center,
                false
            )
            .is_none()
        );
        assert!(
            hits_player(
                Vec3::new(-2.0, 2.2, 0.0),
                Vec3::new(2.0, 2.2, 0.0),
                center,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn top_of_the_body_is_the_head() {
        let center = Vec3::new(0.0, PLAYER_GROUND_Y, 0.0);
        let top = PLAYER_GROUND_Y * 2.0;
        let across = |y: f32, crouched: bool| {
            hits_player(
                Vec3::new(-2.0, y, 0.0),
                Vec3::new(2.0, y, 0.0),
                center,
                crouched,
            )
        };
        assert!(matches!(across(top - 0.1, false), Some((_, true))));
        assert!(matches!(
            across(top - HEAD_ZONE - 0.1, false),
            Some((_, false))
        ));
        // Crouched, the head drops with the top of the body.
        let crouched_top = top - CROUCH_DROP;
        assert!(matches!(across(crouched_top - 0.1, true), Some((_, true))));
        assert!(across(top - 0.1, true).is_none());
    }
}
