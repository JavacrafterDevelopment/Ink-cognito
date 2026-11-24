use crate::{
    AppState, Overlay,
    arena::{Colliders, FIELD_HALF, setup_arena},
    combat::{Combat, LocalState},
    net::{NetEvent, NetworkState},
    player_model::{PlayerPose, spawn_player_model},
    protocol::{PlayerId, PlayerSnapshot, PlayerState, Team, spawn_position},
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

const WALK_SPEED: f32 = 6.5;
const SPRINT_SPEED: f32 = 10.0;
const JUMP_SPEED: f32 = 7.2;
const GRAVITY: f32 = 22.0;
const MOUSE_SENSITIVITY: f32 = 0.0022;
const MAX_PITCH: f32 = 1.5;
/// Vertical field of view, radians.
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
            .add_systems(Startup, (setup_world, setup_arena))
            .add_systems(Update, poll_network_events)
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
    Move,
}

#[derive(Component)]
pub struct MainCamera;

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
    pub yaw: f32,
    /// Horizontal speed this frame (for footsteps).
    pub speed: f32,
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
    /// Their character model (child entity).
    pub model: Entity,
    /// Smoothed horizontal speed, for the walk animation.
    pub speed: f32,
    pub last_pos: Vec3,
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
    ));

    // Main 3D camera (orbits the field on the menu, first person in game).
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

fn animate_menu_camera(time: Res<Time>, mut camera_q: Query<&mut Transform, With<MainCamera>>) {
    let Ok(mut transform) = camera_q.single_mut() else {
        return;
    };
    let angle = time.elapsed_secs() * 0.08;
    let radius = 30.0;
    let pos = Vec3::new(angle.cos() * radius, 12.0, angle.sin() * radius);
    *transform = Transform::from_translation(pos).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y);
}

fn reset_camera_fov(mut camera_q: Query<&mut Projection, With<MainCamera>>) {
    if let Ok(mut projection) = camera_q.single_mut()
        && let Projection::Perspective(p) = projection.as_mut()
    {
        p.fov = BASE_FOV;
    }
}

/// Faces a fresh spawn toward the middle of the field.
pub fn spawn_yaw(id: PlayerId) -> f32 {
    match Team::of(id) {
        Team::Blue => 0.0,
        Team::Orange => std::f32::consts::PI,
    }
}

pub fn spawn_point(id: PlayerId) -> Vec3 {
    let p = spawn_position(id);
    Vec3::new(p[0], PLAYER_GROUND_Y, p[2])
}

fn spawn_remote(
    commands: &mut Commands,
    asset_server: &AssetServer,
    snapshot: &PlayerSnapshot,
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
            speed: 0.0,
            loadout: snapshot.state.loadout,
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
        model,
        speed: 0.0,
        last_pos: pos,
    });
    root
}

fn spawn_in_game_players(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    net: Res<NetworkState>,
    mut camera_q: Query<&mut CameraRig, With<MainCamera>>,
    mut local_state: ResMut<LocalState>,
    mut combat: ResMut<Combat>,
    loadout: Res<crate::loadout::Loadout>,
) {
    let local_id = net.local_player_id.unwrap_or(0);
    let spawn_pos = spawn_point(local_id);
    let yaw = spawn_yaw(local_id);

    // First person: our own body isn't drawn, the camera is the player.
    commands.spawn((
        Transform::from_translation(spawn_pos).with_rotation(Quat::from_rotation_y(yaw)),
        LocalPlayer {
            id: local_id,
            velocity_y: 0.0,
            grounded: true,
            yaw,
            speed: 0.0,
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
        spawn_remote(&mut commands, &asset_server, snapshot);
    }

    if let Ok(mut rig) = camera_q.single_mut() {
        *rig = CameraRig { yaw, pitch: 0.0 };
    }
}

fn poll_network_events(
    mut net: ResMut<NetworkState>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
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
                net.players.insert(local_player.id, local_player);
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
            NetEvent::Connected { your_id, players } => {
                net.is_connecting = false;
                net.error_text = None;
                net.local_player_id = Some(your_id);
                net.status_text = "Connected".to_string();
                net.players.clear();
                for player in players {
                    net.players.insert(player.id, player);
                }
                next_state.set(AppState::InGame);
            }
            NetEvent::PlayerJoined(player) => {
                net.players.insert(player.id, player);
            }
            NetEvent::PlayerLeft { id } => {
                net.players.remove(&id);
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

#[allow(clippy::too_many_arguments)]
fn move_local_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    overlay: Res<Overlay>,
    colliders: Res<Colliders>,
    combat: Res<Combat>,
    local_state: Res<LocalState>,
    camera_q: Query<&CameraRig, With<MainCamera>>,
    mut player_q: Query<(&mut Transform, &mut LocalPlayer)>,
) {
    let Ok((mut transform, mut player)) = player_q.single_mut() else {
        return;
    };
    let rig_yaw = camera_q.single().map(|r| r.yaw).unwrap_or(0.0);
    let alive = local_state.0.health > 0.0;

    let mut input_dir = Vec2::ZERO;
    if alive && !overlay.blocks_input() {
        if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
            input_dir.y += 1.0;
        }
        if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
            input_dir.y -= 1.0;
        }
        if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
            input_dir.x += 1.0;
        }
        if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
            input_dir.x -= 1.0;
        }
    }

    let dt = time.delta_secs().min(0.05);
    let before = transform.translation;

    if input_dir.length_squared() > 0.0 {
        let input_dir = input_dir.normalize();
        // Forward direction on the XZ plane aligned with camera yaw.
        let forward = Vec3::new(-rig_yaw.sin(), 0.0, -rig_yaw.cos());
        let right = Vec3::new(rig_yaw.cos(), 0.0, -rig_yaw.sin());
        let move_vec = (forward * input_dir.y + right * input_dir.x).normalize_or_zero();

        let sprinting = (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
            && !combat.aiming;
        let mut speed = if sprinting { SPRINT_SPEED } else { WALK_SPEED };
        speed *= combat.stats.mobility;
        if combat.aiming {
            speed *= 0.6;
        }
        transform.translation += move_vec * speed * dt;
    }

    // First person: the body faces where we look, so others see where that is.
    player.yaw = rig_yaw;
    transform.rotation = Quat::from_rotation_y(player.yaw);

    // Jump & gravity.
    if alive && !overlay.blocks_input() && player.grounded && keys.just_pressed(KeyCode::Space) {
        player.velocity_y = JUMP_SPEED;
        player.grounded = false;
    }

    player.velocity_y -= GRAVITY * dt;
    transform.translation.y += player.velocity_y * dt;

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

    // Keep the player inside the nets.
    let limit = FIELD_HALF - PLAYER_RADIUS - 0.15;
    transform.translation.x = transform.translation.x.clamp(-limit, limit);
    transform.translation.z = transform.translation.z.clamp(-limit, limit);

    let moved = (transform.translation - before).with_y(0.0).length();
    player.speed = if dt > 0.0 { moved / dt } else { 0.0 };
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
    mut mouse_motion: MessageReader<MouseMotion>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    player_q: Query<&Transform, (With<LocalPlayer>, Without<MainCamera>)>,
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
    for motion in mouse_motion.read() {
        if captured && !overlay.blocks_input() {
            rig.yaw -= motion.delta.x * MOUSE_SENSITIVITY / zoom;
            rig.pitch -= motion.delta.y * MOUSE_SENSITIVITY / zoom;
        }
    }
    rig.yaw %= std::f32::consts::TAU;
    rig.pitch = rig.pitch.clamp(-MAX_PITCH, MAX_PITCH);

    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = BASE_FOV / zoom;
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

    let Ok(player_transform) = player_q.single() else {
        return;
    };
    cam_transform.translation = player_transform.translation + Vec3::Y * EYE_OFFSET;
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
    if overlay.blocks_input() {
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
    asset_server: Res<AssetServer>,
    net: Res<NetworkState>,
    mut remote_q: Query<(Entity, &mut RemotePlayer)>,
) {
    let local_id = net.local_player_id;
    let mut existing: HashMap<PlayerId, Entity> = HashMap::new();

    for (entity, mut remote) in &mut remote_q {
        if Some(remote.id) == local_id || !net.players.contains_key(&remote.id) {
            commands.entity(entity).despawn();
            continue;
        }
        if let Some(snapshot) = net.players.get(&remote.id) {
            remote.target_pos = Vec3::from_array(snapshot.position);
            remote.target_yaw = snapshot.yaw;
            remote.state = snapshot.state;
        }
        existing.insert(remote.id, entity);
    }

    for (&id, snapshot) in &net.players {
        if Some(id) == local_id || existing.contains_key(&id) {
            continue;
        }
        spawn_remote(&mut commands, &asset_server, snapshot);
    }
}

fn interpolate_remote_players(
    time: Res<Time>,
    mut remote_q: Query<(&mut Transform, &mut RemotePlayer)>,
    mut pose_q: Query<&mut PlayerPose>,
) {
    let dt = time.delta_secs().min(0.05);
    let blend = 1.0 - (-16.0 * dt).exp();

    for (mut transform, mut remote) in &mut remote_q {
        // A respawn teleports across the field: don't slide there.
        if transform.translation.distance(remote.target_pos) > 6.0 {
            transform.translation = remote.target_pos;
        } else {
            transform.translation = transform.translation.lerp(remote.target_pos, blend);
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
        }
    }
}
