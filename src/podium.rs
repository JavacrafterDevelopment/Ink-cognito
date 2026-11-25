//! The end-of-match podium: the top three splatters celebrate on a stage
//! while the results show, then everyone goes back to the main menu.

use crate::{
    AppState,
    combat::LocalState,
    hud::{HudPart, player_name},
    menu::{ButtonStyle, HudAction, TEXT_MUTED, TEXT_PRIMARY, spawn_button},
    net::{NetRole, NetworkState},
    player_model::{PlayerPose, spawn_player_model},
    protocol::{GameMode, MatchState, Phase, PlayerId, Team, WIN_BLUE, WIN_DRAW, WIN_ORANGE},
    ranked::{self, RankResult},
    ui_kit::{KIT_YELLOW, UiKit, kit},
    world::{MainCamera, WorldSystems, set_cursor_grab},
};
use bevy::{prelude::*, window::PrimaryWindow};

/// The stage floats high above the map, against the sky.
const STAGE: Vec3 = Vec3::new(0.0, 160.0, 0.0);
/// Block x offset and height for 1st, 2nd and 3rd.
const PLACES: [(f32, f32); 3] = [(0.0, 1.3), (-2.3, 0.85), (2.3, 0.5)];
const PODIUM_FOV: f32 = 0.85;
/// Clients leave this much before the host so nobody sees "host left".
const CLIENT_HEAD_START: f32 = 1.5;

pub struct PodiumPlugin;

impl Plugin for PodiumPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Podium>()
            .add_systems(OnEnter(AppState::InGame), |mut p: ResMut<Podium>| {
                *p = Podium::default()
            })
            .add_systems(
                Update,
                (build_podium, hide_hud, update_podium_ui, leave_after_podium)
                    .chain()
                    .after(WorldSystems::Net)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                podium_camera
                    .after(WorldSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// Whether the podium is up, and for which match.
#[derive(Resource, Default)]
pub struct Podium {
    shown_for: Option<u32>,
    root: Option<Entity>,
    ui: Option<Entity>,
    places: Vec<PlayerId>,
}

impl Podium {
    pub fn active(&self) -> bool {
        self.root.is_some()
    }
}

#[derive(Component)]
struct PodiumLabel(usize);

#[derive(Component)]
struct PodiumClock;

/// The top three by splats (fewest deaths breaks ties), as the board has them.
fn top_three(ms: &MatchState) -> Vec<(PlayerId, u32)> {
    ms.board.iter().take(3).map(|l| (l[0], l[1])).collect()
}

/// "BLUE TEAM WINS", "PLAYER 3 WINS", "DRAW".
fn headline(net: &NetworkState) -> String {
    let ms = &net.match_state;
    let local = net.local_player_id;
    match ms.settings.mode {
        GameMode::FreeForAll => match ms.board.first() {
            Some(l) if ms.winner != WIN_DRAW => {
                format!(
                    "{} WIN{}",
                    player_name(net, l[0]).to_uppercase(),
                    if Some(l[0]) == local { "" } else { "S" }
                )
            }
            _ => "DRAW".to_string(),
        },
        _ => match ms.winner {
            WIN_BLUE => "BLUE TEAM WINS".to_string(),
            WIN_ORANGE => "ORANGE TEAM WINS".to_string(),
            _ => "DRAW".to_string(),
        },
    }
}

fn banner(ms: &MatchState, me: Option<PlayerId>) -> &'static str {
    let Some(me) = me else {
        return "banners/banner_draw.png";
    };
    if ms.winner == WIN_DRAW {
        "banners/banner_draw.png"
    } else if ms.is_winner(me) {
        "banners/banner_victory.png"
    } else {
        "banners/banner_defeat.png"
    }
}

#[allow(clippy::too_many_arguments)]
fn build_podium(
    mut commands: Commands,
    mut podium: ResMut<Podium>,
    net: Res<NetworkState>,
    local: Res<LocalState>,
    assets: Res<AssetServer>,
    fonts: Res<UiKit>,
    rank: Res<RankResult>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cursor_q: Query<&mut bevy::window::CursorOptions, With<PrimaryWindow>>,
) {
    let ms = &net.match_state;
    let clear = |podium: &mut Podium, commands: &mut Commands| {
        for e in [podium.root.take(), podium.ui.take()].into_iter().flatten() {
            commands.entity(e).despawn();
        }
    };
    if ms.phase != Phase::MatchOver {
        clear(&mut podium, &mut commands);
        return;
    }
    if podium.shown_for == Some(ms.match_id) {
        return;
    }
    podium.shown_for = Some(ms.match_id);
    clear(&mut podium, &mut commands);
    let me = net.local_player_id;
    let top = top_three(ms);
    podium.places = top.iter().map(|t| t.0).collect();

    // --- The stage ---------------------------------------------------------
    let wall = materials.add(StandardMaterial {
        base_color: Color::srgb(0.42, 0.42, 0.48),
        base_color_texture: Some(assets.load("branding/menu_background.png")),
        unlit: true,
        ..default()
    });
    let mut mat = |c: Color, rough: f32| -> Handle<StandardMaterial> {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: rough,
            ..default()
        })
    };
    let floor = mat(Color::srgb(0.06, 0.07, 0.1), 0.95);
    let block_mats = [
        mat(Color::srgb(0.96, 0.77, 0.17), 0.35),
        mat(Color::srgb(0.78, 0.82, 0.88), 0.35),
        mat(Color::srgb(0.78, 0.52, 0.32), 0.4),
    ];
    let root = commands
        .spawn((
            Transform::from_translation(STAGE),
            Visibility::default(),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    podium.root = Some(root);
    commands.entity(root).with_children(|stage| {
        stage.spawn((
            Mesh3d(meshes.add(Cylinder::new(7.0, 0.3))),
            MeshMaterial3d(floor),
            Transform::from_xyz(0.0, -0.15, 0.0),
        ));
        // The key art as a backdrop.
        stage.spawn((
            Mesh3d(meshes.add(Rectangle::new(25.6, 14.28))),
            MeshMaterial3d(wall),
            Transform::from_xyz(0.0, 3.6, -6.0),
        ));
        for (i, &(x, h)) in PLACES.iter().enumerate() {
            stage.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.9, h, 1.9))),
                MeshMaterial3d(block_mats[i].clone()),
                Transform::from_xyz(x, h * 0.5, 0.0),
            ));
        }
        stage.spawn((
            PointLight {
                intensity: 2_500_000.0,
                range: 40.0,
                ..default()
            },
            Transform::from_xyz(0.0, 7.0, 7.0),
        ));
    });

    // The top three, celebrating, holding their own guns.
    for (i, &(id, _)) in top.iter().enumerate() {
        let (x, h) = PLACES[i];
        let loadout = if Some(id) == me {
            local.0.loadout
        } else {
            net.players.get(&id).map(|p| p.state.loadout).unwrap_or(0)
        };
        let model = spawn_player_model(&mut commands, &assets, Team::of(id));
        commands.entity(model).insert((
            Transform::from_xyz(x, h, 0.1).with_rotation(Quat::from_rotation_y(-x * 0.08)),
            PlayerPose {
                loadout,
                health: 100.0,
                victory: true,
                ..default()
            },
        ));
        commands.entity(root).add_child(model);
    }

    // --- The results over it ----------------------------------------------
    let ui = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            GlobalZIndex(20),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    podium.ui = Some(ui);
    commands.entity(ui).with_children(|ui| {
        ui.spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(18.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(2.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|top| {
            top.spawn((
                ImageNode::new(kit(&assets, banner(ms, me))),
                Node {
                    width: Val::Px(320.0),
                    height: Val::Px(80.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            if ms.winner != WIN_DRAW {
                top.spawn((
                    Text::new(headline(&net)),
                    fonts.heading(40.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                ));
            }
            let mut line = match ms.settings.mode {
                GameMode::FreeForAll => "FREE-FOR-ALL".to_string(),
                GameMode::TeamSplat => format!(
                    "TEAM SPLAT  |  BLUE {}  -  {} ORANGE",
                    ms.score[0], ms.score[1]
                ),
                GameMode::Elimination => format!(
                    "{}ELIMINATION  |  BLUE {}  -  {} ORANGE",
                    if ms.settings.ranked { "RANKED " } else { "" },
                    ms.score[0],
                    ms.score[1]
                ),
            };
            if ms.settings.ranked && rank.match_id == Some(ms.match_id) {
                let after = (rank.before as i32 + rank.delta).max(0) as u32;
                line = format!(
                    "{line}  |  {:+} RP  ({})",
                    rank.delta,
                    ranked::rank_name(after)
                );
            }
            top.spawn((
                Text::new(line),
                TextFont::from_font_size(15.0),
                TextColor(KIT_YELLOW),
                Pickable::IGNORE,
            ));
        });

        let medals = [
            "badges/medal_gold.png",
            "badges/medal_silver.png",
            "badges/medal_bronze.png",
        ];
        for (i, &(id, splats)) in top.iter().enumerate() {
            ui.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(180.0),
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    display: Display::None,
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.04, 0.06, 0.82)),
                Pickable::IGNORE,
                PodiumLabel(i),
            ))
            .with_children(|l| {
                l.spawn((
                    ImageNode::new(kit(&assets, medals[i])),
                    Node {
                        width: Val::Px(40.0),
                        height: Val::Px(40.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                let color = if ms.settings.mode.teams() {
                    crate::hud::team_color(Team::of(id)).lighter(0.2)
                } else {
                    TEXT_PRIMARY
                };
                l.spawn((
                    Text::new(player_name(&net, id).to_uppercase()),
                    fonts.heading(26.0),
                    TextColor(color),
                    Pickable::IGNORE,
                ));
                l.spawn((
                    Text::new(format!(
                        "{splats} SPLAT{}",
                        if splats == 1 { "" } else { "S" }
                    )),
                    TextFont::from_font_size(14.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                ));
            });
        }

        // Bottom: the clock and Leave.
        ui.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(32.0),
                right: Val::Px(32.0),
                bottom: Val::Px(24.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(""),
                TextFont::from_font_size(15.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
                PodiumClock,
            ));
            row.spawn(Node {
                width: Val::Px(240.0),
                flex_direction: FlexDirection::Column,
                ..default()
            })
            .with_children(|w| {
                spawn_button(
                    w,
                    "MAIN MENU",
                    HudAction::LeaveGame,
                    ButtonStyle::PRIMARY,
                    48.0,
                    18.0,
                    false,
                );
            });
        });
    });

    // Free the mouse so Leave can be clicked.
    set_cursor_grab(&mut cursor_q, false);
}

/// The game's HUD hides while the podium is up.
fn hide_hud(podium: Res<Podium>, mut q: Query<&mut Visibility, With<HudPart>>) {
    let want = if podium.active() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut v in &mut q {
        if *v != want {
            *v = want;
        }
    }
}

fn camera_spot() -> Transform {
    Transform::from_translation(STAGE + Vec3::new(0.0, 1.9, 7.8))
        .looking_at(STAGE + Vec3::new(0.0, 2.1, 0.0), Vec3::Y)
}

/// Points the camera at the stage.
fn podium_camera(
    podium: Res<Podium>,
    mut cam_q: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    if !podium.active() {
        return;
    }
    let Ok((mut tf, mut proj)) = cam_q.single_mut() else {
        return;
    };
    *tf = camera_spot();
    if let Projection::Perspective(p) = proj.as_mut()
        && p.fov != PODIUM_FOV
    {
        p.fov = PODIUM_FOV;
    }
}

/// Keeps the names under their blocks, and the clock running.
fn update_podium_ui(
    podium: Res<Podium>,
    net: Res<NetworkState>,
    cam_q: Query<&Camera, With<MainCamera>>,
    mut labels: Query<(&PodiumLabel, &mut Node)>,
    mut clock: Query<&mut Text, With<PodiumClock>>,
) {
    if !podium.active() {
        return;
    }
    if let Ok(camera) = cam_q.single() {
        // Where `podium_camera` puts the camera (its global transform may lag a frame).
        let cam_tf = GlobalTransform::from(camera_spot());
        for (label, mut node) in &mut labels {
            let (x, h) = PLACES[label.0];
            // On the front of the block.
            let anchor = STAGE + Vec3::new(x, h * 0.5, 0.95);
            if let Ok(p) = camera.world_to_viewport(&cam_tf, anchor) {
                node.left = Val::Px(p.x - 90.0);
                node.top = Val::Px(p.y - 52.0);
                node.display = Display::Flex;
            }
        }
    }
    if let Ok(mut t) = clock.single_mut() {
        let left = net.match_state.time_left.ceil() as u32;
        let line = format!("Back to the main menu in {}:{:02}", left / 60, left % 60);
        if t.0 != line {
            t.0 = line;
        }
    }
}

/// When the podium's time is up, everyone goes back to the main menu.
fn leave_after_podium(mut net: ResMut<NetworkState>, mut next_state: ResMut<NextState<AppState>>) {
    let ms = &net.match_state;
    if ms.phase != Phase::MatchOver {
        return;
    }
    let host = net.active_role() == Some(NetRole::Host);
    let limit = if host { 0.0 } else { CLIENT_HEAD_START };
    if ms.time_left > limit {
        return;
    }
    net.disconnect();
    net.error_text = None;
    next_state.set(AppState::MainMenu);
}
