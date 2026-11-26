use crate::{
    AppState, Overlay,
    arena::Colliders,
    combat::{Combat, LocalState},
    game_mode::{MatchSetup, pick_spawn},
    maps::{CurrentMap, MapInfo},
    net::{NetEvent, NetworkState},
    player_model::{PlayerPose, spawn_player_model},
    protocol::{
        FLAG_AIMING, FLAG_AIRBORNE, FLAG_CROUCH, FLAG_RELOADING, MatchState, MovementSpeed,
        PlayerId, PlayerSnapshot, PlayerState, Team,
    },
};
use bevy::{
    camera::{PerspectiveProjection, Projection},
    input::mouse::MouseMotion,
    picking::hover::HoverMap,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use std::collections::{HashMap, HashSet};

pub const PLAYER_RADIUS: f32 = 0.38;
/// Center Y of the player's body when standing on the ground (the model is 1.8 m tall).
pub const PLAYER_GROUND_Y: f32 = 0.9;
/// Eye height above the body's center.
pub const EYE_OFFSET: f32 = 0.72;
/// How far the eyes (and the body's hit center) drop when crouching.
pub const CROUCH_DROP: f32 = 0.75;
const CROUCH_SPEED: f32 = 0.5;

/// Run speeds and jump physics for each host-picked `MovementSpeed`.
pub(crate) struct MovementTuning {
    pub walk: f32,
    pub sprint: f32,
    pub jump: f32,
    pub gravity: f32,
    /// Whether WASD can steer while airborne.
    pub air_control: bool,
}

impl MovementTuning {
    pub(crate) fn of(movement: MovementSpeed) -> Self {
        match movement {
            // The default: slower, and a jump keeps the direction you took off in.
            MovementSpeed::Normal => MovementTuning {
                walk: 4.6,
                sprint: 7.0,
                jump: 6.8,
                gravity: 20.0,
                air_control: false,
            },
            // The original movement.
            MovementSpeed::SuperDuper => MovementTuning {
                walk: 6.5,
                sprint: 10.0,
                jump: 7.2,
                gravity: 22.0,
                air_control: true,
            },
        }
    }
}
/// Radians of look per mouse count at a 1.0x sensitivity setting.
const BASE_MOUSE_SENSITIVITY: f32 = 0.0022;
const MAX_PITCH: f32 = 1.5;
/// Default vertical field of view, radians (the player's FOV setting
/// overrides this in game; see `settings::ViewSettings`).
pub const BASE_FOV: f32 = 1.13;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.55, 0.76, 0.96)))
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.88, 0.93, 1.0),
                brightness: 380.0,
                ..default()
            })
            .add_systems(Startup, setup_world)
            .add_systems(Update, poll_network_events.in_set(WorldSystems::Net))
            .add_systems(
                Update,
                animate_menu_camera.run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(
                OnEnter(AppState::InGame),
                (spawn_in_game_players, grab_cursor),
            )
            .add_systems(OnExit(AppState::InGame), (release_cursor, reset_camera_fov))
            .add_systems(
                Update,
                (
                    keep_cursor_in_sync,
                    move_local_player,
                    update_game_camera,
                    sync_remote_players,
                    interpolate_remote_players,
                )
                    .chain()
                    .in_set(WorldSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                send_local_state
                    .after(WorldSystems::Move)
                    .after(crate::combat::CombatSystems)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorldSystems {
    /// Network events are applied (state changes, the match, the map).
    Net,
    Move,
}

#[derive(Component)]
pub struct MainCamera;

/// The sun (or moon) light; `maps` sets it for each map's time of day.
#[derive(Component)]
pub struct Sun;

/// Where the first-person camera looks: `yaw` turns (0 looks down -Z),
/// `pitch` looks up (+) and down (-).
#[derive(Component, Default)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Component)]
pub struct LocalPlayer {
    pub id: PlayerId,
    pub velocity_y: f32,
    pub grounded: bool,
    /// Horizontal velocity while on the ground; kept as-is through a jump or
    /// fall when the match has no air control.
    pub ground_velocity: Vec3,
    pub yaw: f32,
    /// Horizontal speed this frame (for footsteps).
    pub speed: f32,
    /// 0 standing to 1 fully crouched, eased.
    pub crouch: f32,
}

#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
    pub target_pos: Vec3,
    pub target_yaw: f32,
    pub state: PlayerState,
    /// The shots / deaths counters already acted on.
    pub seen_shots: u32,
    pub seen_deaths: u32,
    /// The footstep counter already heard.
    pub seen_steps: u32,
    /// Their character model (child entity).
    pub model: Entity,
    /// Smoothed horizontal speed, for the walk animation.
    pub speed: f32,
    pub last_pos: Vec3,
    /// Estimated velocity between network snapshots for low-latency interpolation.
    pub net_velocity: Vec3,
    pub last_snapshot_time: f32,
}

impl RemotePlayer {
    pub fn alive(&self) -> bool {
        self.state.health > 0.0
    }
}

fn setup_world(mut commands: Commands) {
    // Sunlight with soft shadows.
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

    // Main 3D camera (orbits the field on the menu, first person in game).
    // 3D audio spatialization reads this camera's GlobalTransform directly in
    // `crate::audio` (bypassing rodio's inverted-ear SpatialListener).
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: BASE_FOV,
            near: 0.05,
            ..default()
        }),
        Transform::from_xyz(0.0, 14.0, 24.0).looking_at(Vec3::ZERO, Vec3::Y),
        MainCamera,
        CameraRig::default(),
    ));
}

fn animate_menu_camera(
    time: Res<Time>,
    map: Res<MapInfo>,
    mut camera_q: Query<&mut Transform, With<MainCamera>>,
) {
    let Ok(mut transform) = camera_q.single_mut() else {
        return;
    };
    let angle = time.elapsed_secs() * 0.08;
    // Orbit just outside the field, whatever its size, framing the arena
    // cleanly in the open center of the Play lobby.
    let radius = map.half.max_element().clamp(14.0, 34.0) * 1.12;
    let dir = Vec3::new(angle.cos(), 0.0, angle.sin());
    let pos = dir * radius + Vec3::Y * radius * 0.56;
    let target = dir * radius * 0.18 + Vec3::Y * 1.2;
    *transform = Transform::from_translation(pos).looking_at(target, Vec3::Y);
}

fn reset_camera_fov(mut camera_q: Query<&mut Projection, With<MainCamera>>) {
    if let Ok(mut projection) = camera_q.single_mut()
        && let Projection::Perspective(p) = projection.as_mut()
    {
        p.fov = BASE_FOV;
    }
}

fn spawn_remote(
    commands: &mut Commands,
    asset_server: &AssetServer,
    snapshot: &PlayerSnapshot,
    now: f32,
) -> Entity {
    let pos = Vec3::from_array(snapshot.position);
    let root = commands
        .spawn((
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(snapshot.yaw)),
            Visibility::default(),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    let model = spawn_player_model(commands, asset_server, Team::of(snapshot.id));
    commands.entity(model).insert((
        Transform::from_xyz(0.0, -PLAYER_GROUND_Y, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        PlayerPose {
            pitch: snapshot.state.pitch,
            loadout: snapshot.state.loadout,
            health: snapshot.state.health,
            ..default()
        },
    ));
    commands.entity(root).add_child(model);
    commands.entity(root).insert(RemotePlayer {
        id: snapshot.id,
        target_pos: pos,
        target_yaw: snapshot.yaw,
        state: snapshot.state,
        seen_shots: snapshot.state.shots,
        seen_deaths: snapshot.state.deaths,
        seen_steps: snapshot.state.steps,
        model,
        speed: 0.0,
        last_pos: pos,
        net_velocity: Vec3::ZERO,
        last_snapshot_time: now,
    });
    root
}

#[allow(clippy::too_many_arguments)]
fn spawn_in_game_players(
    mut commands: Commands,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    net: Res<NetworkState>,
    map: Res<MapInfo>,
    mut camera_q: Query<&mut CameraRig, With<MainCamera>>,
    mut local_state: ResMut<LocalState>,
    mut combat: ResMut<Combat>,
    loadout: Res<crate::loadout::Loadout>,
) {
    let local_id = net.local_player_id.unwrap_or(0);
    let (spawn_pos, yaw) = pick_spawn(&map, &net, local_id, 0);
    let now = time.elapsed_secs();

    // First person: our own body isn't drawn, the camera is the player.
    commands.spawn((
        Transform::from_translation(spawn_pos).with_rotation(Quat::from_rotation_y(yaw)),
        LocalPlayer {
            id: local_id,
            velocity_y: 0.0,
            grounded: true,
            ground_velocity: Vec3::ZERO,
            yaw,
            speed: 0.0,
            crouch: 0.0,
        },
        DespawnOnExit(AppState::InGame),
    ));

    *local_state = LocalState(PlayerState {
        loadout: loadout.pack(),
        ..default()
    });
    combat.reset(&loadout.stats());

    for (&id, snapshot) in &net.players {
        if id == local_id {
            continue;
        }
        spawn_remote(&mut commands, &asset_server, snapshot, now);
    }

    if let Ok(mut rig) = camera_q.single_mut() {
        *rig = CameraRig { yaw, pitch: 0.0 };
    }
}

pub fn poll_network_events(
    mut net: ResMut<NetworkState>,
    setup: Res<MatchSetup>,
    mut current_map: ResMut<CurrentMap>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    // Builds the match's map right away, so it's there when the game starts.
    let mut follow = |ms: &MatchState| {
        if current_map.0 != ms.settings.map {
            current_map.0 = ms.settings.map;
        }
    };
    let events = net.poll_events();
    for event in events {
        match event {
            NetEvent::HostingStarted {
                udp_port,
                local_player,
            } => {
                net.is_connecting = false;
                net.error_text = None;
                net.udp_port = Some(udp_port);
                net.local_player_id = Some(local_player.id);
                net.status_text = "Hosting".to_string();
                net.players.clear();
                net.pending_hits.clear();
                net.pending_ragdolls.clear();
                net.pending_votes.clear();
                net.players.insert(local_player.id, local_player);
                net.match_state = MatchState {
                    settings: setup.0,
                    ..default()
                };
                follow(&net.match_state);
                next_state.set(AppState::InGame);
            }
            NetEvent::TailcatReady(addr) => {
                net.tailcat_addr = Some(addr);
                net.tailcat_error = None;
            }
            NetEvent::TailcatFailed(err) => {
                warn!("{err}");
                net.tailcat_error = Some(err);
            }
            NetEvent::Connected {
                your_id,
                players,
                match_state,
            } => {
                net.is_connecting = false;
                net.error_text = None;
                net.local_player_id = Some(your_id);
                net.status_text = "Connected".to_string();
                net.players.clear();
                net.pending_hits.clear();
                net.pending_ragdolls.clear();
                net.pending_votes.clear();
                for player in players {
                    net.players.insert(player.id, player);
                }
                net.match_state = match_state.unwrap_or_default();
                follow(&net.match_state);
                next_state.set(AppState::InGame);
            }
            NetEvent::PlayerJoined(player) => {
                net.players.insert(player.id, player);
            }
            NetEvent::PlayerLeft { id } => {
                net.players.remove(&id);
                net.forget_name(id);
            }
            NetEvent::WorldState { players } => {
                let local_id = net.local_player_id;
                let incoming_ids: HashSet<PlayerId> = players.iter().map(|p| p.id).collect();
                net.players
                    .retain(|id, _| Some(*id) == local_id || incoming_ids.contains(id));
                for player in players {
                    if Some(player.id) == local_id {
                        continue;
                    }
                    net.players.insert(player.id, player);
                }
            }
            NetEvent::PlayerUpdates(players) => {
                let local_id = net.local_player_id;
                for player in players {
                    if Some(player.id) != local_id {
                        net.players.insert(player.id, player);
                    }
                }
            }
            NetEvent::Roster(ids) => {
                let local_id = net.local_player_id;
                net.players
                    .retain(|id, _| Some(*id) == local_id || ids.contains(id));
            }
            NetEvent::Match(ms) => {
                // The host's word, except we keep our own clock ticking smoothly.
                follow(&ms);
                net.match_state = ms;
            }
            NetEvent::HitReceived(hit) => {
                net.pending_hits.push(hit);
            }
            NetEvent::BotHit(hit) => {
                net.pending_bot_hits.push(hit);
            }
            NetEvent::Ragdolls(frame) => {
                net.pending_ragdolls.push(frame);
            }
            NetEvent::Names(names) => {
                net.learn_names(names);
            }
            NetEvent::Chat { from, text } => {
                net.pending_chat.push((from, text));
            }
            NetEvent::MapVote {
                from,
                match_id,
                choice,
            } => {
                net.pending_votes.push((from, match_id, choice));
            }
            NetEvent::Error(err) => {
                net.disconnect();
                net.error_text = Some(err);
                if *state.get() == AppState::InGame {
                    next_state.set(AppState::MainMenu);
                }
            }
            NetEvent::Disconnected(reason) => {
                net.disconnect();
                net.error_text = Some(reason);
                if *state.get() == AppState::InGame {
                    next_state.set(AppState::MainMenu);
                }
            }
        }
    }
}

/// Longest frame movement will catch up on at once (longer ones, like a
/// loading hitch, are dropped rather than teleporting the player).
const MAX_FRAME_DT: f32 = 0.25;
/// Longest single physics step for the local player.
const MAX_PHYSICS_STEP: f32 = 1.0 / 120.0;

#[allow(clippy::too_many_arguments)]
fn move_local_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<crate::settings::KeybindSettings>,
    overlay: Res<Overlay>,
    colliders: Res<Colliders>,
    map: Res<MapInfo>,
    net: Res<NetworkState>,
    combat: Res<Combat>,
    mut local_state: ResMut<LocalState>,
    camera_q: Query<&CameraRig, With<MainCamera>>,
    mut player_q: Query<(&mut Transform, &mut LocalPlayer)>,
) {
    use crate::settings::KeyAction;
    let Ok((mut transform, mut player)) = player_q.single_mut() else {
        return;
    };
    let rig_yaw = camera_q.single().map(|r| r.yaw).unwrap_or(0.0);
    let alive = local_state.0.health > 0.0;
    let can_move = alive && !overlay.blocks_input() && !net.match_state.frozen();
    // A long frame (a hitch, or a low frame rate) is split into short physics
    // steps, so movement, jumps and collisions play out the same at any FPS.
    let dt = time.delta_secs().min(MAX_FRAME_DT);

    let crouch_held =
        alive && !overlay.blocks_input() && keybinds.pressed(KeyAction::Crouch, &keys, &mouse);
    let crouch_target = if crouch_held { 1.0 } else { 0.0 };
    player.crouch += (crouch_target - player.crouch) * (1.0 - (-14.0 * dt).exp());

    let mut input_dir = Vec2::ZERO;
    if can_move {
        if keybinds.pressed(KeyAction::MoveForward, &keys, &mouse) {
            input_dir.y += 1.0;
        }
        if keybinds.pressed(KeyAction::MoveBackward, &keys, &mouse) {
            input_dir.y -= 1.0;
        }
        if keybinds.pressed(KeyAction::MoveRight, &keys, &mouse) {
            input_dir.x += 1.0;
        }
        if keybinds.pressed(KeyAction::MoveLeft, &keys, &mouse) {
            input_dir.x -= 1.0;
        }
    }

    let before = transform.translation;
    let tuning = MovementTuning::of(net.match_state.settings.movement);

    let mut wish_velocity = Vec3::ZERO;
    if input_dir.length_squared() > 0.0 {
        let input_dir = input_dir.normalize();
        // Forward direction on the XZ plane aligned with camera yaw.
        let forward = Vec3::new(-rig_yaw.sin(), 0.0, -rig_yaw.cos());
        let right = Vec3::new(rig_yaw.cos(), 0.0, -rig_yaw.sin());
        let move_vec = (forward * input_dir.y + right * input_dir.x).normalize_or_zero();

        let sprinting =
            keybinds.pressed(KeyAction::Sprint, &keys, &mouse) && !combat.aiming && !crouch_held;
        let mut speed = if sprinting {
            tuning.sprint
        } else {
            tuning.walk
        };
        speed *= combat.stats.mobility;
        if combat.aiming {
            speed *= 0.6;
        }
        if crouch_held {
            speed *= CROUCH_SPEED;
        }
        wish_velocity = move_vec * speed;
    }
    if player.grounded || tuning.air_control {
        player.ground_velocity = wish_velocity;
    } else if !can_move {
        // Splatted or frozen mid-air: don't keep drifting.
        player.ground_velocity = Vec3::ZERO;
    }

    // First person: the body faces where we look, so others see where that is.
    player.yaw = rig_yaw;
    transform.rotation = Quat::from_rotation_y(player.yaw);

    // Jump & gravity.
    if can_move && player.grounded && keybinds.just_pressed(KeyAction::Jump, &keys, &mouse) {
        player.velocity_y = tuning.jump;
        player.grounded = false;
    }

    let steps = (dt / MAX_PHYSICS_STEP).ceil().max(1.0);
    let step = dt / steps;
    let limit = map.half - Vec2::splat(PLAYER_RADIUS + 0.15);
    for _ in 0..steps as u32 {
        transform.translation += player.ground_velocity * step;

        // Exact under constant gravity, so jump height doesn't vary with FPS.
        transform.translation.y += player.velocity_y * step - 0.5 * tuning.gravity * step * step;
        player.velocity_y -= tuning.gravity * step;

        // Bunkers push us out; their tops can be stood on.
        let mut pos = transform.translation;
        let feet = pos.y - PLAYER_GROUND_Y;
        let top = pos.y + PLAYER_GROUND_Y;
        let floor = colliders.resolve_player(&mut pos, PLAYER_RADIUS, feet, top);
        transform.translation = pos;

        let ground = floor + PLAYER_GROUND_Y;
        if transform.translation.y <= ground {
            transform.translation.y = ground;
            player.velocity_y = 0.0;
            player.grounded = true;
        } else if transform.translation.y > ground + 0.05 {
            player.grounded = false;
        }

        // Keep the player inside the field.
        transform.translation.x = transform.translation.x.clamp(-limit.x, limit.x);
        transform.translation.z = transform.translation.z.clamp(-limit.y, limit.y);
    }

    let moved = (transform.translation - before).with_y(0.0).length();
    player.speed = if dt > 0.0 { moved / dt } else { 0.0 };

    // What others need to animate us.
    let mut flags = 0;
    if player.crouch > 0.5 {
        flags |= FLAG_CROUCH;
    }
    if !player.grounded {
        flags |= FLAG_AIRBORNE;
    }
    if combat.aiming {
        flags |= FLAG_AIMING;
    }
    if combat.reloading.is_some() {
        flags |= FLAG_RELOADING;
    }
    if local_state.0.flags != flags {
        local_state.0.flags = flags;
    }
}

/// Sends where we are and our state (health, shots, ...) once everything this
/// frame has had its say.
fn send_local_state(
    rig_q: Query<&CameraRig, With<MainCamera>>,
    player_q: Query<(&Transform, &LocalPlayer)>,
    mut local_state: ResMut<LocalState>,
    mut net: ResMut<NetworkState>,
) {
    let Ok((transform, player)) = player_q.single() else {
        return;
    };
    if let Ok(rig) = rig_q.single() {
        local_state.0.pitch = rig.pitch;
    }
    let pos_arr = transform.translation.to_array();
    let state = local_state.0;
    if let Some(entry) = net.players.get_mut(&player.id) {
        entry.position = pos_arr;
        entry.yaw = player.yaw;
        entry.state = state;
    }
    net.send_local_transform(pos_arr, player.yaw, state);
}

#[allow(clippy::too_many_arguments)]
fn update_game_camera(
    time: Res<Time>,
    overlay: Res<Overlay>,
    combat: Res<Combat>,
    view: Res<crate::settings::ViewSettings>,
    mut mouse_motion: MessageReader<MouseMotion>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    player_q: Query<(&Transform, &LocalPlayer), Without<MainCamera>>,
    mut camera_q: Query<(&mut Transform, &mut CameraRig, &mut Projection), With<MainCamera>>,
) {
    let Ok((mut cam_transform, mut rig, mut projection)) = camera_q.single_mut() else {
        return;
    };

    // Mouse look while the mouse is captured and no menu is open.
    let captured = cursor_q
        .single()
        .is_ok_and(|c| c.grab_mode != CursorGrabMode::None);
    let zoom = 1.0 + (combat.stats.zoom - 1.0) * combat.aim_blend;
    let sensitivity = BASE_MOUSE_SENSITIVITY * view.look_multiplier(combat.aim_blend) / zoom;
    for motion in mouse_motion.read() {
        // Looking around still works while typing in chat.
        if captured && !overlay.menu_open() {
            rig.yaw -= motion.delta.x * sensitivity;
            rig.pitch -= motion.delta.y * sensitivity * view.y_sign();
        }
    }
    rig.yaw %= std::f32::consts::TAU;
    rig.pitch = rig.pitch.clamp(-MAX_PITCH, MAX_PITCH);

    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = view.fov_radians() / zoom;
    }

    // Eliminated: watch our ragdoll from above while waiting to respawn.
    if let Some(death) = combat.death_cam {
        let t = combat.dead_for;
        let angle = rig.yaw + t * 0.35;
        let dist = 4.0 + (t * 1.5).min(2.5);
        let eye = death + Vec3::new(angle.sin() * dist, 2.6 + t.min(2.0), angle.cos() * dist);
        let target = death + Vec3::Y * 0.4;
        let desired = Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let blend = 1.0 - (-6.0 * time.delta_secs()).exp();
        cam_transform.translation = cam_transform.translation.lerp(desired.translation, blend);
        cam_transform.rotation = cam_transform.rotation.slerp(desired.rotation, blend);
        return;
    }

    let Ok((player_transform, player)) = player_q.single() else {
        return;
    };
    cam_transform.translation =
        player_transform.translation + Vec3::Y * (EYE_OFFSET - CROUCH_DROP * player.crouch);
    cam_transform.rotation = Quat::from_rotation_y(rig.yaw) * Quat::from_rotation_x(rig.pitch);
}

pub fn set_cursor_grab(cursor_q: &mut Query<&mut CursorOptions, With<PrimaryWindow>>, grab: bool) {
    if let Ok(mut cursor) = cursor_q.single_mut() {
        cursor.grab_mode = if grab {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        cursor.visible = !grab;
    }
}

fn grab_cursor(mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    set_cursor_grab(&mut cursor_q, true);
}

fn release_cursor(mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    set_cursor_grab(&mut cursor_q, false);
}

/// The mouse is free while a menu is open, and captured otherwise. If the
/// window lost the mouse (alt-tab), clicking the world takes it back.
fn keep_cursor_in_sync(
    overlay: Res<Overlay>,
    mouse: Res<ButtonInput<MouseButton>>,
    hover: Option<Res<HoverMap>>,
    ui_nodes: Query<(), With<Node>>,
    mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(cursor) = cursor_q.single() else {
        return;
    };
    let captured = cursor.grab_mode != CursorGrabMode::None;
    // Chat keeps the mouse captured: only menus free it.
    if overlay.menu_open() {
        if captured {
            set_cursor_grab(&mut cursor_q, false);
        }
    } else if overlay.is_changed() && !captured {
        set_cursor_grab(&mut cursor_q, true);
    } else if !captured && mouse.just_pressed(MouseButton::Left) {
        let over_ui = hover.is_some_and(|h| {
            h.values()
                .any(|hits| hits.keys().any(|e| ui_nodes.contains(*e)))
        });
        if !over_ui {
            set_cursor_grab(&mut cursor_q, true);
        }
    }
}

fn sync_remote_players(
    mut commands: Commands,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    net: Res<NetworkState>,
    mut remote_q: Query<(Entity, &mut RemotePlayer)>,
) {
    let now = time.elapsed_secs();
    let local_id = net.local_player_id;
    let mut existing: HashMap<PlayerId, Entity> = HashMap::new();

    for (entity, mut remote) in &mut remote_q {
        if Some(remote.id) == local_id || !net.players.contains_key(&remote.id) {
            commands.entity(entity).despawn();
            continue;
        }
        if let Some(snapshot) = net.players.get(&remote.id) {
            let new_pos = Vec3::from_array(snapshot.position);
            let delta = new_pos - remote.target_pos;
            let dist = delta.length();
            if dist > 6.0 {
                // Respawn teleport: don't extrapolate across the field.
                remote.net_velocity = Vec3::ZERO;
                remote.last_snapshot_time = now;
            } else if dist > 1e-4 {
                let dt_snap = (now - remote.last_snapshot_time).clamp(0.012, 0.25);
                let raw_vel = (delta / dt_snap).clamp_length_max(16.0);
                remote.net_velocity = remote.net_velocity.lerp(raw_vel, 0.55);
                remote.last_snapshot_time = now;
            }
            remote.target_pos = new_pos;
            remote.target_yaw = snapshot.yaw;
            remote.state = snapshot.state;
        }
        existing.insert(remote.id, entity);
    }

    for (&id, snapshot) in &net.players {
        if Some(id) == local_id || existing.contains_key(&id) {
            continue;
        }
        spawn_remote(&mut commands, &asset_server, snapshot, now);
    }
}

fn interpolate_remote_players(
    time: Res<Time>,
    net: Res<NetworkState>,
    mut remote_q: Query<(&mut Transform, &mut RemotePlayer)>,
    mut pose_q: Query<&mut PlayerPose>,
) {
    let dt = time.delta_secs().min(0.05);
    let now = time.elapsed_secs();
    let blend = 1.0 - (-22.0 * dt).exp();

    for (mut transform, mut remote) in &mut remote_q {
        let since = (now - remote.last_snapshot_time).max(0.0);
        // If no new position arrived recently, damp velocity so stopped players don't drift.
        if since > 0.09 {
            remote.net_velocity *= (-18.0 * dt).exp();
        }
        // Light dead-reckoning (capped at 65 ms) compensates for smoothing + tick lag.
        let predicted = remote.target_pos + remote.net_velocity * since.min(0.065);

        // A respawn teleports across the field: don't slide there.
        if transform.translation.distance(remote.target_pos) > 6.0 {
            transform.translation = remote.target_pos;
            remote.net_velocity = Vec3::ZERO;
        } else {
            transform.translation = transform.translation.lerp(predicted, blend);
        }
        let target_rot = Quat::from_rotation_y(remote.target_yaw);
        transform.rotation = transform.rotation.slerp(target_rot, blend);

        if dt > 0.0 {
            let moved = (transform.translation - remote.last_pos)
                .with_y(0.0)
                .length()
                / dt;
            let k = 1.0 - (-8.0 * dt).exp();
            remote.speed += (moved - remote.speed) * k;
        }
        remote.last_pos = transform.translation;

        if let Ok(mut pose) = pose_q.get_mut(remote.model) {
            pose.pitch = remote.state.pitch;
            pose.speed = remote.speed;
            pose.loadout = remote.state.loadout;
            pose.flags = remote.state.flags;
            pose.health = remote.state.health;
            pose.victory = net.match_state.is_winner(remote.id);
        }
    }
}
