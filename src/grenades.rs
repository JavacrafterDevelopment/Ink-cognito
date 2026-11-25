//! Throwables: the paint bomb and the flashbang.
//!
//! Both recharge on their own. Throwing one bumps a counter in our
//! `PlayerState` (`bombs` / `flashes`), so every screen throws the same
//! grenade from where we stand and look, like shots.
//!
//! - **Paint bomb** (G): arcs, bounces, and after a short fuse bursts into a
//!   ball of paintballs flying out every way. The balls are ordinary
//!   projectiles owned by the thrower, so the thrower's screen confirms their
//!   hits the same way it does gun hits.
//! - **Flashbang** (Q): bursts into a cloud of paint. The thrower's screen
//!   checks who is close with a clear line to it and sends each of them a
//!   `HitEvent` with `flash` set (no damage); they get paint over most of
//!   their screen for 10 seconds. The paint is the thrower's team colour,
//!   so flashing yourself paints your screen your own colour. Each flash
//!   adds its own layer of paint, so a second flash doesn't wipe the first,
//!   and dying clears it all. A ringing loops while any paint is up and
//!   fades out with the last layer.
//!
//! The loadout picks which kind each key throws (`Loadout::grenade` /
//! `Loadout::flash`, sent to others in `PlayerState::throwables`):
//!
//! - **Ultra Bang** (G): lands like a paint bomb, then bursts like one every
//!   0.5 s for 7 s. 90 s recharge.
//! - **9-Banger** (Q): a flashbang that goes off 9 times in a row. Three
//!   times the flashbang's recharge.

use crate::{
    AppState, Overlay,
    arena::Colliders,
    combat::{
        CombatSystems, Fx, LocalState, paint_burst_fx, play_sfx, play_sfx_at, spawn_paintball,
    },
    hud::HudPart,
    loadout::Loadout,
    net::NetworkState,
    protocol::{FLAG_CROUCH, HitEvent, PlayerId, Team, enemies},
    settings::{AudioSettings, KeyAction, KeybindSettings},
    world::{CROUCH_DROP, EYE_OFFSET, LocalPlayer, MainCamera, RemotePlayer},
};
use bevy::{
    audio::{AudioSink, AudioSinkPlayback, Volume},
    prelude::*,
};
use std::collections::{HashMap, HashSet, VecDeque};

/// Seconds for a used paint bomb to come back.
pub const BOMB_RECHARGE: f32 = 18.0;
/// Seconds for a used flashbang to come back.
pub const FLASH_RECHARGE: f32 = 22.0;
/// Seconds for a used Ultra Bang to come back.
pub const ULTRA_RECHARGE: f32 = 90.0;
/// Seconds for a used 9-Banger to come back: three flashbangs' worth.
pub const NINE_BANG_RECHARGE: f32 = FLASH_RECHARGE * 3.0;
/// The Ultra Bang bursts this often...
const ULTRA_INTERVAL: f32 = 0.5;
/// ...this many times: every 0.5 s over 7 s.
const ULTRA_BURSTS: u32 = 14;
/// The 9-Banger's bursts, and the time between them.
const NINE_BANG_BURSTS: u32 = 9;
const NINE_BANG_INTERVAL: f32 = 0.3;
/// How many paintballs a paint bomb bursts into. Nothing per ball goes over
/// the network: every screen bursts the same bomb itself from the thrower's
/// throw counter, and the thrower's hits on a player are summed into one
/// `HitEvent` per frame (see `combat::move_projectiles`).
const BOMB_BALLS: u32 = 850;
/// Same as a normal paintball out of the stock marker (the Vanta body).
const BOMB_BALL_SPEED: f32 = 80.0;
const BOMB_BALL_GRAVITY: f32 = 6.0;
/// Per ball.
const BOMB_BALL_DAMAGE: f32 = 40.0;
/// Bomb balls only fly this long (they're done well before; it just bounds how
/// many can be alive at once).
const BOMB_BALL_LIFE: f32 = 1.6;
const BOMB_FUSE: f32 = 1.7;
const FLASH_FUSE: f32 = 1.4;
/// Players within this many meters (with nothing in between) get painted.
const FLASH_RADIUS: f32 = 9.0;
/// How long a flashbang's paint stays on screen.
pub const FLASH_SECONDS: f32 = 10.0;
/// The last part of it, when the paint fades away.
const FLASH_FADE: f32 = 2.5;
/// Volume of the ringing while flashed (before the SFX volume setting).
const RING_VOLUME: f32 = 0.6;
const THROW_SPEED: f32 = 19.0;
const THROW_LIFT: f32 = 4.5;
const GRAVITY: f32 = 9.81;
const GRENADE_RADIUS: f32 = 0.09;

pub struct GrenadesPlugin;

impl Plugin for GrenadesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Throwables>()
            .init_resource::<FlashOverlay>()
            .init_resource::<FlashBookkeeping>()
            .add_systems(Startup, load_grenade_assets)
            .add_systems(
                OnEnter(AppState::InGame),
                (reset_throwables, spawn_charge_hud),
            )
            .add_systems(OnExit(AppState::InGame), reset_throwables)
            .add_systems(
                Update,
                (
                    throw_local,
                    throw_remote,
                    move_grenades,
                    receive_flashes,
                    update_flash_overlay,
                    update_charge_hud,
                )
                    .chain()
                    .after(CombatSystems)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bomb,
    Flash,
    UltraBang,
    NineBang,
}

impl Kind {
    /// What the paint bomb key throws for a `Loadout::grenade` index.
    fn for_bomb_slot(i: u8) -> Self {
        match i {
            1 => Kind::UltraBang,
            _ => Kind::Bomb,
        }
    }

    /// What the flashbang key throws for a `Loadout::flash` index.
    fn for_flash_slot(i: u8) -> Self {
        match i {
            1 => Kind::NineBang,
            _ => Kind::Flash,
        }
    }

    /// Both keys' kinds from `PlayerState::throwables`.
    fn from_packed(packed: u8) -> (Self, Self) {
        (
            Self::for_bomb_slot(packed & 0xF),
            Self::for_flash_slot(packed >> 4),
        )
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Bomb => "Paint Bomb",
            Kind::Flash => "Flashbang",
            Kind::UltraBang => "Ultra Bang",
            Kind::NineBang => "9-Banger",
        }
    }

    fn recharge(self) -> f32 {
        match self {
            Kind::Bomb => BOMB_RECHARGE,
            Kind::Flash => FLASH_RECHARGE,
            Kind::UltraBang => ULTRA_RECHARGE,
            Kind::NineBang => NINE_BANG_RECHARGE,
        }
    }

    fn fuse(self) -> f32 {
        match self {
            Kind::Bomb | Kind::UltraBang => BOMB_FUSE,
            Kind::Flash | Kind::NineBang => FLASH_FUSE,
        }
    }

    /// How many times it goes off, and the seconds between.
    fn bursts(self) -> (u32, f32) {
        match self {
            Kind::Bomb | Kind::Flash => (1, 0.0),
            Kind::UltraBang => (ULTRA_BURSTS, ULTRA_INTERVAL),
            Kind::NineBang => (NINE_BANG_BURSTS, NINE_BANG_INTERVAL),
        }
    }
}

/// Seconds until each throwable is ready again (0 = ready).
#[derive(Resource, Default)]
pub struct Throwables {
    pub bomb: f32,
    pub flash: f32,
}

/// The paint over our screen from flashbangs: one layer per flash, each
/// counting down and fading on its own.
#[derive(Resource, Default)]
struct FlashOverlay {
    layers: Vec<FlashLayer>,
    /// The looping ringing sound, while any layer is up.
    ring: Option<Entity>,
}

struct FlashLayer {
    /// Seconds left.
    left: f32,
    /// Whose paint it is (the thrower's team).
    paint: Team,
    /// Its UI nodes, once spawned.
    root: Option<Entity>,
}

impl FlashOverlay {
    /// How opaque the layer that lasts longest is (1 until it starts to
    /// fade): the ringing follows it.
    fn last_fade(&self) -> f32 {
        let left = self.layers.iter().map(|l| l.left).fold(0.0, f32::max);
        (left / FLASH_FADE).min(1.0)
    }

    fn clear(&mut self, commands: &mut Commands) {
        for layer in self.layers.drain(..) {
            if let Some(root) = layer.root {
                commands.entity(root).despawn();
            }
        }
        if let Some(ring) = self.ring.take() {
            commands.entity(ring).despawn();
        }
    }
}

/// Flash ids we sent, and ones we already took (UDP resends).
#[derive(Resource, Default)]
struct FlashBookkeeping {
    next_id: u32,
    seen: HashSet<(PlayerId, u32)>,
    order: VecDeque<(PlayerId, u32)>,
}

impl FlashBookkeeping {
    fn alloc_id(&mut self) -> u32 {
        // A separate range from gun hits so the host's dedup never mixes them.
        self.next_id = self.next_id.wrapping_add(1) & 0x7FFF_FFFF;
        self.next_id.max(1) | 0x8000_0000
    }

    fn record(&mut self, attacker: PlayerId, id: u32) -> bool {
        if !self.seen.insert((attacker, id)) {
            return false;
        }
        self.order.push_back((attacker, id));
        while self.order.len() > 128 {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }
}

#[derive(Resource)]
struct GrenadeAssets {
    mesh: Handle<Mesh>,
    bomb: HashMap<Team, Handle<StandardMaterial>>,
    flash: HashMap<Team, Handle<StandardMaterial>>,
    throw: Handle<AudioSource>,
    bounce: Handle<AudioSource>,
    /// The paint bomb going off.
    nade: Handle<AudioSource>,
    /// The flashbang going off.
    flash_bang: Handle<AudioSource>,
    ringing: Handle<AudioSource>,
    /// Plays through the Ultra Bang's bursts.
    ultra: Handle<AudioSource>,
}

#[derive(Component)]
struct Grenade {
    kind: Kind,
    owner: PlayerId,
    team: Team,
    velocity: Vec3,
    fuse: f32,
    resting: bool,
    /// Bursts still to go (more than one for the Ultra Bang and 9-Banger).
    bursts_left: u32,
}

#[derive(Component)]
struct FlashSplat {
    alpha: f32,
    /// The layer's root node, to find how far it has faded.
    layer: Entity,
}

/// The looping ringing while flashed.
#[derive(Component)]
struct FlashRing;

#[derive(Component)]
struct ChargeText;

fn team_color(team: Team, light: f32) -> Color {
    let [r, g, b] = team.srgb();
    Color::srgb(
        r + (1.0 - r) * light,
        g + (1.0 - g) * light,
        b + (1.0 - b) * light,
    )
}

fn load_grenade_assets(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut bomb = HashMap::new();
    let mut flash = HashMap::new();
    for team in [Team::Blue, Team::Orange] {
        bomb.insert(
            team,
            materials.add(StandardMaterial {
                base_color: team_color(team, 0.0),
                perceptual_roughness: 0.35,
                ..default()
            }),
        );
        flash.insert(
            team,
            materials.add(StandardMaterial {
                base_color: team_color(team, 0.7),
                perceptual_roughness: 0.25,
                metallic: 0.4,
                ..default()
            }),
        );
    }
    commands.insert_resource(GrenadeAssets {
        mesh: meshes.add(Sphere::new(GRENADE_RADIUS)),
        bomb,
        flash,
        throw: assets.load("audio/weapons/pump_cycle.ogg"),
        bounce: assets.load("audio/impacts/ball_bounce.ogg"),
        nade: assets.load("audio/weapons/nade.flac"),
        flash_bang: assets.load("audio/weapons/flashexplosion.flac"),
        ringing: assets.load("audio/game/ringing.flac"),
        ultra: assets.load("audio/weapons/ultra_bang.flac"),
    });
}

fn reset_throwables(
    mut throwables: ResMut<Throwables>,
    mut overlay: ResMut<FlashOverlay>,
    mut book: ResMut<FlashBookkeeping>,
) {
    *throwables = Throwables::default();
    // The overlay's nodes and the ringing go with the game (`DespawnOnExit`).
    *overlay = FlashOverlay::default();
    book.seen.clear();
    book.order.clear();
}

fn look_rotation(yaw: f32, pitch: f32) -> Quat {
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
}

fn throw_velocity(forward: Vec3) -> Vec3 {
    forward * THROW_SPEED + Vec3::Y * THROW_LIFT
}

fn spawn_grenade(
    commands: &mut Commands,
    assets: &GrenadeAssets,
    kind: Kind,
    owner: PlayerId,
    origin: Vec3,
    velocity: Vec3,
) {
    let team = Team::of(owner);
    let mat = match kind {
        Kind::Bomb | Kind::UltraBang => assets.bomb[&team].clone(),
        Kind::Flash | Kind::NineBang => assets.flash[&team].clone(),
    };
    commands.spawn((
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(origin),
        Grenade {
            kind,
            owner,
            team,
            velocity,
            fuse: kind.fuse(),
            resting: false,
            bursts_left: kind.bursts().0,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

/// G / Q: throw one if it's charged.
#[allow(clippy::too_many_arguments)]
fn throw_local(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<KeybindSettings>,
    overlay: Res<Overlay>,
    net: Res<NetworkState>,
    settings: Res<AudioSettings>,
    loadout: Res<Loadout>,
    assets: Option<Res<GrenadeAssets>>,
    mut throwables: ResMut<Throwables>,
    mut local_state: ResMut<LocalState>,
    player_q: Query<&LocalPlayer>,
    camera_q: Query<&Transform, With<MainCamera>>,
) {
    let dt = time.delta_secs();
    throwables.bomb = (throwables.bomb - dt).max(0.0);
    throwables.flash = (throwables.flash - dt).max(0.0);
    // Others throw whatever this says when our counters go up.
    let packed = loadout.pack_throwables();
    if local_state.0.throwables != packed {
        local_state.0.throwables = packed;
    }
    let (bomb_kind, flash_kind) = Kind::from_packed(packed);
    let Some(assets) = assets else {
        return;
    };
    let (Ok(player), Ok(cam)) = (player_q.single(), camera_q.single()) else {
        return;
    };
    if local_state.0.health <= 0.0 || overlay.blocks_input() || net.match_state.frozen() {
        return;
    }
    let kind = if throwables.bomb <= 0.0
        && keybinds.just_pressed(KeyAction::PaintBomb, &keys, &mouse)
    {
        throwables.bomb = bomb_kind.recharge();
        local_state.0.bombs = local_state.0.bombs.wrapping_add(1);
        bomb_kind
    } else if throwables.flash <= 0.0 && keybinds.just_pressed(KeyAction::Flashbang, &keys, &mouse)
    {
        throwables.flash = flash_kind.recharge();
        local_state.0.flashes = local_state.0.flashes.wrapping_add(1);
        flash_kind
    } else {
        return;
    };
    let forward = cam.forward().as_vec3();
    let origin = cam.translation + forward * 0.5 - cam.right() * 0.15 - cam.up() * 0.1;
    let id = net.local_player_id.unwrap_or(player.id);
    spawn_grenade(
        &mut commands,
        &assets,
        kind,
        id,
        origin,
        throw_velocity(forward),
    );
    play_sfx(&mut commands, &assets.throw, 0.5, &settings);
}

/// Others' throws: their counter went up, so throw one from where they look.
fn throw_remote(
    mut commands: Commands,
    settings: Res<AudioSettings>,
    assets: Option<Res<GrenadeAssets>>,
    remote_q: Query<&RemotePlayer>,
    mut seen: Local<HashMap<PlayerId, (u8, u8)>>,
) {
    let Some(assets) = assets else {
        return;
    };
    for remote in &remote_q {
        let now = (remote.state.bombs, remote.state.flashes);
        let Some(before) = seen.insert(remote.id, now) else {
            // First time we hear of them: nothing to throw yet.
            continue;
        };
        if !remote.alive() {
            continue;
        }
        let rot = look_rotation(remote.target_yaw, remote.state.pitch);
        let forward = rot * Vec3::NEG_Z;
        let crouched = remote.state.flags & FLAG_CROUCH != 0;
        let eye =
            remote.target_pos + Vec3::Y * (EYE_OFFSET - if crouched { CROUCH_DROP } else { 0.0 });
        let origin = eye + forward * 0.5 + rot * Vec3::new(-0.15, -0.1, 0.0);
        let (bomb_kind, flash_kind) = Kind::from_packed(remote.state.throwables);
        for (kind, new) in [
            (bomb_kind, now.0.wrapping_sub(before.0)),
            (flash_kind, now.1.wrapping_sub(before.1)),
        ] {
            // A handful at most (more means a rejoin or a wrapped counter).
            if new == 0 || new > 3 {
                continue;
            }
            spawn_grenade(
                &mut commands,
                &assets,
                kind,
                remote.id,
                origin,
                throw_velocity(forward),
            );
            play_sfx_at(&mut commands, &assets.throw, 0.5, origin, &settings);
        }
    }
}

/// Directions spread evenly over a sphere (a Fibonacci spiral), turned a
/// little per bomb so bursts don't all look the same.
fn burst_directions(count: u32, twist: f32) -> impl Iterator<Item = Vec3> {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..count).map(move |i| {
        let y = 1.0 - (i as f32 + 0.5) / count as f32 * 2.0;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let a = golden * i as f32 + twist;
        Vec3::new(a.cos() * r, y, a.sin() * r)
    })
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn move_grenades(
    mut commands: Commands,
    time: Res<Time>,
    colliders: Res<Colliders>,
    net: Res<NetworkState>,
    settings: Res<AudioSettings>,
    assets: Option<Res<GrenadeAssets>>,
    mut fx: ResMut<Fx>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut book: ResMut<FlashBookkeeping>,
    mut flash_overlay: ResMut<FlashOverlay>,
    local_state: Res<LocalState>,
    local_q: Query<(&Transform, &LocalPlayer), Without<Grenade>>,
    remote_q: Query<(&Transform, &RemotePlayer), Without<Grenade>>,
    mut grenades: Query<(Entity, &mut Transform, &mut Grenade)>,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = time.delta_secs().min(0.05);
    let local = local_q.single().ok();
    let local_id = net.local_player_id.or(local.map(|(_, p)| p.id));

    for (entity, mut tf, mut g) in &mut grenades {
        g.fuse -= dt;
        if g.fuse > 0.0 {
            if g.resting {
                continue;
            }
            g.velocity.y -= GRAVITY * dt;
            let from = tf.translation;
            let to = from + g.velocity * dt;
            if let Some(hit) = colliders.segment_hit(from, to) {
                let n = hit.normal.normalize_or(Vec3::Y);
                let v = g.velocity;
                let speed = v.length();
                g.velocity = (v - 2.0 * v.dot(n) * n) * 0.42;
                tf.translation = hit.point + n * (GRENADE_RADIUS + 0.01);
                if speed > 2.5 {
                    play_sfx_at(&mut commands, &assets.bounce, 0.4, hit.point, &settings);
                }
                if n.y > 0.7 && g.velocity.length() < 1.2 {
                    g.resting = true;
                    g.velocity = Vec3::ZERO;
                }
            } else {
                tf.translation = to;
                let spin = Quat::from_rotation_x(-g.velocity.length() * dt * 2.0);
                tf.rotation = spin * tf.rotation;
            }
            continue;
        }

        // Boom. The Ultra Bang and 9-Banger go again after a beat, from
        // wherever they've rolled to.
        let point = tf.translation + Vec3::Y * 0.15;
        let (total, interval) = g.kind.bursts();
        if g.kind == Kind::UltraBang && g.bursts_left == total {
            play_sfx_at(&mut commands, &assets.ultra, 1.0, point, &settings);
        }
        g.bursts_left = g.bursts_left.saturating_sub(1);
        if g.bursts_left > 0 {
            g.fuse = interval;
        } else {
            commands.entity(entity).despawn();
        }
        match g.kind {
            Kind::Bomb | Kind::UltraBang => {
                paint_burst_fx(&mut commands, &mut fx, &mut materials, point, g.team, 1.0);
                play_sfx_at(&mut commands, &assets.nade, 1.0, point, &settings);
                let twist = (g.owner as f32) * 0.37;
                for dir in burst_directions(BOMB_BALLS, twist) {
                    spawn_paintball(
                        &mut commands,
                        &mut fx,
                        &mut materials,
                        g.owner,
                        g.team,
                        point + dir * 0.12,
                        dir * BOMB_BALL_SPEED,
                        BOMB_BALL_GRAVITY,
                        BOMB_BALL_DAMAGE,
                        BOMB_BALL_LIFE,
                    );
                }
            }
            Kind::Flash | Kind::NineBang => {
                paint_burst_fx(&mut commands, &mut fx, &mut materials, point, g.team, 2.0);
                play_sfx_at(&mut commands, &assets.flash_bang, 1.0, point, &settings);
                // Only the thrower's screen decides who got painted.
                if Some(g.owner) != local_id {
                    continue;
                }
                let sees = |eye: Vec3| {
                    eye.distance(point) <= FLASH_RADIUS
                        && colliders.segment_hit(point, eye).is_none()
                };
                // Our own flash gets us too if we're standing in it.
                if let Some((lt, lp)) = local
                    && local_state.0.health > 0.0
                    && sees(lt.translation + Vec3::Y * (EYE_OFFSET - lp.crouch * CROUCH_DROP))
                {
                    paint_screen(&mut flash_overlay, g.team);
                }
                for (rt, remote) in &remote_q {
                    if !enemies(g.owner, remote.id) || !remote.alive() {
                        continue;
                    }
                    let crouched = remote.state.flags & FLAG_CROUCH != 0;
                    let eye = rt.translation
                        + Vec3::Y * (EYE_OFFSET - if crouched { CROUCH_DROP } else { 0.0 });
                    if !sees(eye) {
                        continue;
                    }
                    net.send_hit(HitEvent {
                        hit_id: book.alloc_id(),
                        attacker: g.owner,
                        victim: remote.id,
                        damage: 0.0,
                        velocity: [0.0; 3],
                        point: point.to_array(),
                        water: false,
                        headshot: false,
                        flash: true,
                    });
                }
            }
        }
    }
}

/// Adds a layer of paint on top of whatever is already there.
fn paint_screen(overlay: &mut FlashOverlay, paint: Team) {
    overlay.layers.push(FlashLayer {
        left: FLASH_SECONDS,
        paint,
        root: None,
    });
}

/// Flashbangs others say went off next to us.
fn receive_flashes(
    mut net: ResMut<NetworkState>,
    mut book: ResMut<FlashBookkeeping>,
    mut overlay: ResMut<FlashOverlay>,
    local_state: Res<LocalState>,
    local_q: Query<&LocalPlayer>,
) {
    if !net.pending_hits.iter().any(|h| h.flash) {
        return;
    }
    let mut flashes = Vec::new();
    net.pending_hits.retain(|h| {
        if h.flash {
            flashes.push(*h);
            false
        } else {
            true
        }
    });
    let Some(me) = net.local_player_id.or(local_q.single().ok().map(|p| p.id)) else {
        return;
    };
    for hit in flashes {
        if hit.victim != me || !enemies(hit.attacker, me) || local_state.0.health <= 0.0 {
            continue;
        }
        if book.record(hit.attacker, hit.hit_id) {
            paint_screen(&mut overlay, Team::of(hit.attacker));
        }
    }
}

/// Paint over ~90% of the screen while a flash lasts, fading at the end.
/// Each flash is its own layer; the ringing loops while any is up and
/// fades with the last one. Dying (or a round restart) wipes it all.
#[allow(clippy::too_many_arguments)]
fn update_flash_overlay(
    mut commands: Commands,
    time: Res<Time>,
    fx: Res<Fx>,
    net: Res<NetworkState>,
    settings: Res<AudioSettings>,
    assets: Option<Res<GrenadeAssets>>,
    local_state: Res<LocalState>,
    mut overlay: ResMut<FlashOverlay>,
    mut splats: Query<(&FlashSplat, &mut ImageNode)>,
    mut ring_q: Query<&mut AudioSink, With<FlashRing>>,
    mut last_resets: Local<Option<u32>>,
) {
    let resets = net.match_state.resets;
    let restarted = last_resets.is_some_and(|r| r != resets);
    *last_resets = Some(resets);
    if local_state.0.health <= 0.0 || restarted {
        overlay.clear(&mut commands);
        return;
    }

    let dt = time.delta_secs();
    let overlay = &mut *overlay;
    overlay.layers.retain_mut(|layer| {
        layer.left = (layer.left - dt).max(0.0);
        if layer.left > 0.0 {
            return true;
        }
        if let Some(root) = layer.root.take() {
            commands.entity(root).despawn();
        }
        false
    });
    for layer in &mut overlay.layers {
        if layer.root.is_none() {
            layer.root = Some(spawn_flash_overlay(&mut commands, &fx, layer.paint));
        }
    }

    for (splat, mut image) in &mut splats {
        let Some(layer) = overlay
            .layers
            .iter()
            .find(|l| l.root == Some(splat.layer))
        else {
            continue;
        };
        let fade = (layer.left / FLASH_FADE).min(1.0);
        image.color = image.color.with_alpha(splat.alpha * fade);
    }

    // Ringing: on while any paint is up, fading out with the last layer.
    if overlay.layers.is_empty() {
        if let Some(ring) = overlay.ring.take() {
            commands.entity(ring).despawn();
        }
        return;
    }
    let volume = RING_VOLUME * settings.effective_sfx() * overlay.last_fade();
    match overlay.ring {
        Some(ring) => {
            if let Ok(mut sink) = ring_q.get_mut(ring) {
                sink.set_volume(Volume::Linear(volume));
            }
        }
        None => {
            if let Some(assets) = assets {
                overlay.ring = Some(
                    commands
                        .spawn((
                            AudioPlayer::new(assets.ringing.clone()),
                            PlaybackSettings::LOOP.with_volume(Volume::Linear(volume)),
                            FlashRing,
                            DespawnOnExit(AppState::InGame),
                        ))
                        .id(),
                );
            }
        }
    }
}

fn spawn_flash_overlay(commands: &mut Commands, fx: &Fx, paint: Team) -> Entity {
    let images = fx.splat_images();
    let [r, g, b] = paint.srgb();
    // A grid of big overlapping splats, a few cells left clear so ~90% of the
    // screen is covered. Seeded per flash so the gaps move around.
    let mut seed = (fx_seed() | 1) ^ 0xA511_E9B3;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    const COLS: usize = 6;
    const ROWS: usize = 5;
    let gap_a = (rand() * (COLS * ROWS) as f32) as usize;
    let gap_b = (rand() * (COLS * ROWS) as f32) as usize;
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                overflow: Overflow::clip(),
                ..default()
            },
            Pickable::IGNORE,
            GlobalZIndex(4),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    if images.is_empty() {
        return root;
    }
    commands.entity(root).with_children(|parent| {
        for row in 0..ROWS {
            for col in 0..COLS {
                let cell = row * COLS + col;
                if cell == gap_a || cell == gap_b {
                    continue;
                }
                let cw = 100.0 / COLS as f32;
                let ch = 100.0 / ROWS as f32;
                let size = 2.1 + rand() * 0.6;
                let w = cw * size;
                let h = ch * size;
                let cx = (col as f32 + 0.5 + (rand() - 0.5) * 0.5) * cw;
                let cy = (row as f32 + 0.5 + (rand() - 0.5) * 0.5) * ch;
                let shade = 0.85 + rand() * 0.15;
                let alpha = 0.9 + rand() * 0.1;
                let i = ((rand() * images.len() as f32) as usize).min(images.len() - 1);
                parent.spawn((
                    ImageNode {
                        flip_x: rand() < 0.5,
                        flip_y: rand() < 0.5,
                        ..ImageNode::new(images[i].clone()).with_color(Color::srgba(
                            r * shade,
                            g * shade,
                            b * shade,
                            alpha,
                        ))
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(cx - w * 0.5),
                        top: Val::Percent(cy - h * 0.5),
                        width: Val::Percent(w),
                        height: Val::Percent(h),
                        ..default()
                    },
                    FlashSplat { alpha, layer: root },
                    Pickable::IGNORE,
                ));
            }
        }
    });
    root
}

/// A bit of randomness for where the paint lands.
fn fx_seed() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0x1234_5678)
}

fn spawn_charge_hud(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont::from_font_size(14.0),
        TextColor(Color::srgb(0.95, 0.97, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(24.0),
            bottom: Val::Px(128.0),
            ..default()
        },
        ChargeText,
        Pickable::IGNORE,
        DespawnOnExit(AppState::InGame),
        HudPart,
    ));
}

fn charge_line(key: &str, name: &str, left: f32) -> String {
    if left <= 0.0 {
        format!("[{key}] {name}  READY")
    } else {
        format!("[{key}] {name}  {}s", left.ceil() as i32)
    }
}

fn update_charge_hud(
    throwables: Res<Throwables>,
    keybinds: Res<KeybindSettings>,
    loadout: Res<Loadout>,
    mut text_q: Query<&mut Text, With<ChargeText>>,
) {
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };
    let (bomb_kind, flash_kind) = Kind::from_packed(loadout.pack_throwables());
    let s = format!(
        "{}\n{}",
        charge_line(
            keybinds.get(KeyAction::PaintBomb).label(),
            bomb_kind.name(),
            throwables.bomb
        ),
        charge_line(
            keybinds.get(KeyAction::Flashbang).label(),
            flash_kind.name(),
            throwables.flash
        ),
    );
    if text.0 != s {
        text.0 = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_goes_every_way() {
        let dirs: Vec<Vec3> = burst_directions(BOMB_BALLS, 0.3).collect();
        assert_eq!(dirs.len(), BOMB_BALLS as usize);
        for d in &dirs {
            assert!((d.length() - 1.0).abs() < 1e-3);
        }
        let sum: Vec3 = dirs.iter().copied().sum();
        assert!(sum.length() < 1.0, "burst is lopsided: {sum:?}");
        assert!(dirs.iter().any(|d| d.y > 0.9));
        assert!(dirs.iter().any(|d| d.y < -0.9));
    }

    #[test]
    fn throwable_kinds_round_trip() {
        let mut l = Loadout::default();
        assert_eq!(
            Kind::from_packed(l.pack_throwables()),
            (Kind::Bomb, Kind::Flash)
        );
        l.grenade = 1;
        l.flash = 1;
        assert_eq!(
            Kind::from_packed(l.pack_throwables()),
            (Kind::UltraBang, Kind::NineBang)
        );
        assert_eq!(NINE_BANG_RECHARGE, 66.0);
        assert_eq!(ULTRA_BURSTS as f32 * ULTRA_INTERVAL, 7.0);
    }

    #[test]
    fn flash_ids_never_clash_with_gun_hits() {
        let mut book = FlashBookkeeping::default();
        for _ in 0..5 {
            assert!(book.alloc_id() & 0x8000_0000 != 0);
        }
    }
}
