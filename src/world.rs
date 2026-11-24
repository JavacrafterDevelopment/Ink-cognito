use crate::{
    AppState,
    net::{NetEvent, NetworkState},
    protocol::{PlayerId, PlayerSnapshot, player_color, spawn_position},
};
use bevy::{
    asset::RenderAssetUsages,
    input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::{HashMap, HashSet};

pub const BASEPLATE_SIZE: f32 = 60.0;
const BASEPLATE_HALF: f32 = BASEPLATE_SIZE * 0.5;
const PLAYER_RADIUS: f32 = 0.42;
const PLAYER_CYLINDER_LENGTH: f32 = 0.96;
/// Center Y of the player capsule when standing on the y = 0.0 baseplate.
const PLAYER_GROUND_Y: f32 = PLAYER_RADIUS + PLAYER_CYLINDER_LENGTH * 0.5;

const WALK_SPEED: f32 = 7.0;
const SPRINT_SPEED: f32 = 12.0;
const JUMP_SPEED: f32 = 7.8;
const GRAVITY: f32 = 22.0;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.55, 0.76, 0.96)))
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.88, 0.93, 1.0),
                brightness: 340.0,
                ..default()
            })
            .add_systems(Startup, setup_world)
            .add_systems(Update, poll_network_events)
            .add_systems(
                Update,
                animate_menu_camera.run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(OnEnter(AppState::InGame), spawn_in_game_players)
            .add_systems(
                Update,
                (
                    move_local_player,
                    update_game_camera,
                    sync_remote_players,
                    interpolate_remote_players,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.42,
            distance: 9.5,
        }
    }
}

#[derive(Component)]
pub struct LocalPlayer {
    pub id: PlayerId,
    pub velocity_y: f32,
    pub grounded: bool,
    pub yaw: f32,
}

#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
    pub target_pos: Vec3,
    pub target_yaw: f32,
}

fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Sunlight with soft shadows.
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.97, 0.90),
            illuminance: 16_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(24.0, 38.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Main 3D camera (orbits baseplate on menu, follows local player in game).
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 14.0, 24.0).looking_at(Vec3::ZERO, Vec3::Y),
        MainCamera,
        CameraRig::default(),
    ));

    // Procedural grid baseplate texture.
    let grid_texture = images.add(build_baseplate_texture());

    // 3D baseplate slab whose top surface sits cleanly at y = 0.0.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(BASEPLATE_SIZE, 1.0, BASEPLATE_SIZE))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(grid_texture),
            perceptual_roughness: 0.82,
            metallic: 0.04,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    // Subtle raised border trims around the perimeter of the baseplate.
    let border_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.24, 0.29, 0.36),
        perceptual_roughness: 0.65,
        metallic: 0.15,
        ..default()
    });
    let trim_thickness = 0.6;
    let trim_height = 0.22;
    let edge_offset = BASEPLATE_HALF + trim_thickness * 0.5;

    let ns_mesh = meshes.add(Cuboid::new(
        BASEPLATE_SIZE + trim_thickness * 2.0,
        trim_height,
        trim_thickness,
    ));
    let ew_mesh = meshes.add(Cuboid::new(trim_thickness, trim_height, BASEPLATE_SIZE));

    for z in [-edge_offset, edge_offset] {
        commands.spawn((
            Mesh3d(ns_mesh.clone()),
            MeshMaterial3d(border_mat.clone()),
            Transform::from_xyz(0.0, trim_height * 0.5 - 0.05, z),
        ));
    }
    for x in [-edge_offset, edge_offset] {
        commands.spawn((
            Mesh3d(ew_mesh.clone()),
            MeshMaterial3d(border_mat.clone()),
            Transform::from_xyz(x, trim_height * 0.5 - 0.05, 0.0),
        ));
    }

    // Four low corner markers for spatial orientation.
    let pillar_mesh = meshes.add(Cylinder::new(0.45, 1.6));
    let pillar_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.32, 0.38, 0.48),
        perceptual_roughness: 0.5,
        metallic: 0.2,
        ..default()
    });
    let corner = BASEPLATE_HALF - 1.2;
    for (x, z) in [
        (-corner, -corner),
        (corner, -corner),
        (-corner, corner),
        (corner, corner),
    ] {
        commands.spawn((
            Mesh3d(pillar_mesh.clone()),
            MeshMaterial3d(pillar_mat.clone()),
            Transform::from_xyz(x, 0.8, z),
        ));
    }

    // Center spawn pad disk.
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(2.2, 0.06))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.28, 0.52, 0.78),
            perceptual_roughness: 0.45,
            metallic: 0.1,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.03, 0.0),
    ));
}

fn build_baseplate_texture() -> Image {
    const TEX_SIZE: usize = 512;
    const TILES: usize = 30; // 2m x 2m tiles across the 60m baseplate
    let mut data = vec![0u8; TEX_SIZE * TEX_SIZE * 4];

    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            let tx = (x * TILES) / TEX_SIZE;
            let ty = (y * TILES) / TEX_SIZE;
            let fx = (x * TILES) % TEX_SIZE;
            let fy = (y * TILES) % TEX_SIZE;

            let is_line = fx < 16 || fy < 16;
            let is_even = (tx + ty) % 2 == 0;

            let (r, g, b) = if is_line {
                (108u8, 120u8, 134u8)
            } else if is_even {
                (156u8, 168u8, 180u8)
            } else {
                (142u8, 154u8, 167u8)
            };

            let idx = (y * TEX_SIZE + x) * 4;
            data[idx] = r;
            data[idx + 1] = g;
            data[idx + 2] = b;
            data[idx + 3] = 255;
        }
    }

    Image::new(
        Extent3d {
            width: TEX_SIZE as u32,
            height: TEX_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

fn animate_menu_camera(time: Res<Time>, mut camera_q: Query<&mut Transform, With<MainCamera>>) {
    let Ok(mut transform) = camera_q.single_mut() else {
        return;
    };
    let angle = time.elapsed_secs() * 0.14;
    let radius = 26.0;
    let pos = Vec3::new(angle.cos() * radius, 13.5, angle.sin() * radius);
    *transform = Transform::from_translation(pos).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y);
}

fn spawn_player_mesh(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    color: [f32; 3],
    pos: Vec3,
    yaw: f32,
) -> Entity {
    let body_mesh = meshes.add(Capsule3d::new(PLAYER_RADIUS, PLAYER_CYLINDER_LENGTH));
    let body_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(color[0], color[1], color[2]),
        perceptual_roughness: 0.45,
        metallic: 0.05,
        ..default()
    });

    let visor_mesh = meshes.add(Cuboid::new(0.54, 0.22, 0.18));
    let visor_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.09, 0.11, 0.15),
        perceptual_roughness: 0.18,
        metallic: 0.35,
        ..default()
    });

    commands
        .spawn((
            Mesh3d(body_mesh),
            MeshMaterial3d(body_mat),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|parent| {
            // Front-facing visor so players can clearly see which direction everyone is looking.
            parent.spawn((
                Mesh3d(visor_mesh),
                MeshMaterial3d(visor_mat),
                Transform::from_xyz(0.0, 0.38, -0.34),
            ));
        })
        .id()
}

fn spawn_in_game_players(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    net: Res<NetworkState>,
    mut camera_q: Query<&mut CameraRig, With<MainCamera>>,
) {
    let local_id = net.local_player_id.unwrap_or(0);
    let local_snapshot = net
        .players
        .get(&local_id)
        .copied()
        .unwrap_or(PlayerSnapshot {
            id: local_id,
            position: spawn_position(local_id),
            yaw: 0.0,
            color: player_color(local_id),
        });

    let spawn_pos = Vec3::new(
        local_snapshot.position[0],
        PLAYER_GROUND_Y,
        local_snapshot.position[2],
    );
    let local_entity = spawn_player_mesh(
        &mut commands,
        &mut meshes,
        &mut materials,
        local_snapshot.color,
        spawn_pos,
        local_snapshot.yaw,
    );
    commands.entity(local_entity).insert(LocalPlayer {
        id: local_id,
        velocity_y: 0.0,
        grounded: true,
        yaw: local_snapshot.yaw,
    });

    for (&id, snapshot) in &net.players {
        if id == local_id {
            continue;
        }
        let pos = Vec3::from_array(snapshot.position);
        let entity = spawn_player_mesh(
            &mut commands,
            &mut meshes,
            &mut materials,
            snapshot.color,
            pos,
            snapshot.yaw,
        );
        commands.entity(entity).insert(RemotePlayer {
            id,
            target_pos: pos,
            target_yaw: snapshot.yaw,
        });
    }

    if let Ok(mut rig) = camera_q.single_mut() {
        *rig = CameraRig::default();
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

fn move_local_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    camera_q: Query<&CameraRig, With<MainCamera>>,
    mut player_q: Query<(&mut Transform, &mut LocalPlayer)>,
    mut net: ResMut<NetworkState>,
) {
    let Ok((mut transform, mut player)) = player_q.single_mut() else {
        return;
    };
    let rig_yaw = camera_q.single().map(|r| r.yaw).unwrap_or(0.0);

    let mut input_dir = Vec2::ZERO;
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

    let dt = time.delta_secs().min(0.05);

    if input_dir.length_squared() > 0.0 {
        let input_dir = input_dir.normalize();
        // Forward direction on the XZ plane aligned with camera yaw.
        let forward = Vec3::new(-rig_yaw.sin(), 0.0, -rig_yaw.cos());
        let right = Vec3::new(rig_yaw.cos(), 0.0, -rig_yaw.sin());
        let move_vec = (forward * input_dir.y + right * input_dir.x).normalize_or_zero();

        let speed = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };
        transform.translation += move_vec * speed * dt;

        // Smoothly rotate player to face movement direction (-Z is front).
        let target_yaw = (-move_vec.x).atan2(-move_vec.z);
        player.yaw = lerp_angle(player.yaw, target_yaw, (dt * 12.0).min(1.0));
        transform.rotation = Quat::from_rotation_y(player.yaw);
    }

    // Jump & gravity.
    if player.grounded && keys.just_pressed(KeyCode::Space) {
        player.velocity_y = JUMP_SPEED;
        player.grounded = false;
    }

    player.velocity_y -= GRAVITY * dt;
    transform.translation.y += player.velocity_y * dt;

    if transform.translation.y <= PLAYER_GROUND_Y {
        transform.translation.y = PLAYER_GROUND_Y;
        player.velocity_y = 0.0;
        player.grounded = true;
    }

    // Keep player on the baseplate.
    let limit = BASEPLATE_HALF - PLAYER_RADIUS - 0.15;
    transform.translation.x = transform.translation.x.clamp(-limit, limit);
    transform.translation.z = transform.translation.z.clamp(-limit, limit);

    let pos_arr = transform.translation.to_array();
    if let Some(entry) = net.players.get_mut(&player.id) {
        entry.position = pos_arr;
        entry.yaw = player.yaw;
    }
    net.send_local_transform(pos_arr, player.yaw);
}

fn update_game_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut mouse_wheel: MessageReader<MouseWheel>,
    player_q: Query<&Transform, (With<LocalPlayer>, Without<MainCamera>)>,
    mut camera_q: Query<(&mut Transform, &mut CameraRig), With<MainCamera>>,
) {
    let Ok((mut cam_transform, mut rig)) = camera_q.single_mut() else {
        return;
    };
    let dt = time.delta_secs().min(0.05);

    // Keyboard camera rotation for effortless trackpad / keyboard-only play.
    let key_turn_speed = 2.0;
    if keys.pressed(KeyCode::KeyQ) {
        rig.yaw += key_turn_speed * dt;
    }
    if keys.pressed(KeyCode::KeyE) {
        rig.yaw -= key_turn_speed * dt;
    }

    // Right-click or middle-click drag orbits the camera.
    let dragging =
        mouse_buttons.pressed(MouseButton::Right) || mouse_buttons.pressed(MouseButton::Middle);
    for motion in mouse_motion.read() {
        if dragging {
            rig.yaw -= motion.delta.x * 0.006;
            rig.pitch = (rig.pitch + motion.delta.y * 0.005).clamp(0.12, 1.25);
        }
    }

    // Scroll wheel zooms camera distance.
    for wheel in mouse_wheel.read() {
        let delta = match wheel.unit {
            MouseScrollUnit::Line => wheel.y * 0.75,
            MouseScrollUnit::Pixel => wheel.y * 0.015,
        };
        rig.distance = (rig.distance - delta).clamp(4.5, 18.0);
    }

    let Ok(player_transform) = player_q.single() else {
        return;
    };

    let focus = player_transform.translation + Vec3::new(0.0, 0.75, 0.0);
    let horiz = rig.distance * rig.pitch.cos();
    let offset = Vec3::new(
        rig.yaw.sin() * horiz,
        rig.distance * rig.pitch.sin(),
        rig.yaw.cos() * horiz,
    );

    let desired_pos = focus + offset;
    let smooth = 1.0 - (-14.0 * dt).exp();
    cam_transform.translation = cam_transform.translation.lerp(desired_pos, smooth);
    cam_transform.look_at(focus, Vec3::Y);
}

fn sync_remote_players(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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
        }
        existing.insert(remote.id, entity);
    }

    for (&id, snapshot) in &net.players {
        if Some(id) == local_id || existing.contains_key(&id) {
            continue;
        }
        let pos = Vec3::from_array(snapshot.position);
        let entity = spawn_player_mesh(
            &mut commands,
            &mut meshes,
            &mut materials,
            snapshot.color,
            pos,
            snapshot.yaw,
        );
        commands.entity(entity).insert(RemotePlayer {
            id,
            target_pos: pos,
            target_yaw: snapshot.yaw,
        });
    }
}

fn interpolate_remote_players(
    time: Res<Time>,
    mut remote_q: Query<(&mut Transform, &RemotePlayer)>,
) {
    let dt = time.delta_secs().min(0.05);
    let blend = 1.0 - (-16.0 * dt).exp();

    for (mut transform, remote) in &mut remote_q {
        transform.translation = transform.translation.lerp(remote.target_pos, blend);
        let target_rot = Quat::from_rotation_y(remote.target_yaw);
        transform.rotation = transform.rotation.slerp(target_rot, blend);
    }
}

fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    let mut diff = (to - from) % std::f32::consts::TAU;
    if diff > std::f32::consts::PI {
        diff -= std::f32::consts::TAU;
    } else if diff < -std::f32::consts::PI {
        diff += std::f32::consts::TAU;
    }
    from + diff * t
}
