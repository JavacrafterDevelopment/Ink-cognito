//! The end-of-match podium and 3-map voting screen: the top three splatters
//! celebrate on a stage while everyone votes among 3 random next maps (ties
//! broken randomly among the tied options), then the winning map loads
//! automatically for the next match.

use crate::{
    AppState, Overlay,
    combat::LocalState,
    hud::{HudPart, player_name},
    maps::MapLibrary,
    menu::{ButtonStyle, HudAction, TEXT_MUTED, TEXT_PRIMARY, UiSounds, spawn_button},
    net::NetworkState,
    player_model::{PlayerPose, spawn_player_model},
    protocol::{GameMode, MatchState, Phase, PlayerId, Team, WIN_BLUE, WIN_DRAW, WIN_ORANGE},
    ranked::{self, RankResult},
    settings::AudioSettings,
    ui_kit::{KIT_YELLOW, UiKit, kit, sliced},
    world::{MainCamera, WorldSystems, set_cursor_grab},
};
use bevy::{picking::prelude::*, prelude::*, window::PrimaryWindow};

/// The stage floats high above the map, against the sky.
const STAGE: Vec3 = Vec3::new(0.0, 160.0, 0.0);
/// Block x offset and height for 1st, 2nd and 3rd.
const PLACES: [(f32, f32); 3] = [(0.0, 1.3), (-2.3, 0.85), (2.3, 0.5)];
const PODIUM_FOV: f32 = 0.85;

const VOTE_CARD_BG: Color = Color::srgba(0.06, 0.08, 0.12, 0.94);
const VOTE_CARD_SELECTED_BG: Color = Color::srgba(0.11, 0.14, 0.20, 0.96);
const VOTE_CARD_WINNER_BG: Color = Color::srgba(0.13, 0.16, 0.10, 0.97);
const VOTE_BORDER_IDLE: Color = Color::srgba(0.45, 0.55, 0.68, 0.35);
const VOTE_WINNER_GREEN: Color = Color::srgb(0.32, 0.88, 0.48);

pub struct PodiumPlugin;

impl Plugin for PodiumPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Podium>()
            .add_observer(on_vote_card_click)
            .add_observer(on_vote_card_over)
            .add_observer(on_vote_card_out)
            .add_systems(OnEnter(AppState::InGame), |mut p: ResMut<Podium>| {
                *p = Podium::default()
            })
            .add_systems(
                Update,
                (
                    build_podium,
                    vote_with_keys,
                    hide_hud,
                    update_podium_ui,
                )
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
    /// Which vote option (`0..3`) the local player selected for `shown_for`.
    my_vote: Option<u8>,
    /// Whether we already played the lock-in confirm sound for this match.
    announced_lock: bool,
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

#[derive(Component)]
struct VoteHeaderText;

#[derive(Component)]
struct VoteTieBadge;

#[derive(Component, Clone, Copy)]
struct VoteCard {
    index: u8,
    hovered: bool,
}

#[derive(Component, Clone, Copy)]
struct VoteCardTitle(u8);

#[derive(Component, Clone, Copy)]
struct VoteCardCount(u8);

#[derive(Component, Clone, Copy)]
struct VoteCardStatus(u8);

#[derive(Component, Clone, Copy)]
struct VoteCardBar(u8);

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

fn play_ui_sfx(
    commands: &mut Commands,
    sound: &Handle<AudioSource>,
    volume: f32,
    settings: Option<&AudioSettings>,
) {
    let final_vol = volume * settings.map(|s| s.effective_ui()).unwrap_or(1.0);
    if final_vol <= 0.005 {
        return;
    }
    commands.spawn((
        AudioPlayer::new(sound.clone()),
        PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(final_vol)),
    ));
}

fn cast_vote(
    choice: u8,
    podium: &mut Podium,
    net: &NetworkState,
    commands: &mut Commands,
    sounds: Option<&UiSounds>,
    settings: Option<&AudioSettings>,
) {
    let ms = &net.match_state;
    if ms.phase != Phase::MatchOver || ms.vote.winner.is_some() {
        return;
    }
    if (choice as usize) >= ms.vote.options.len() {
        return;
    }
    podium.my_vote = Some(choice);
    net.send_map_vote(ms.match_id, choice);
    if let Some(sounds) = sounds {
        play_ui_sfx(commands, &sounds.click, 0.65, settings);
    }
}

fn on_vote_card_click(
    click: On<PointerClick>,
    cards: Query<&VoteCard>,
    mut podium: ResMut<Podium>,
    net: Res<NetworkState>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(card) = cards.get(click.entity) else {
        return;
    };
    cast_vote(
        card.index,
        &mut podium,
        &net,
        &mut commands,
        sounds.as_deref(),
        settings.as_deref(),
    );
}

fn on_vote_card_over(
    over: On<PointerOver>,
    mut cards: Query<&mut VoteCard>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
) {
    if let Ok(mut card) = cards.get_mut(over.entity) {
        card.hovered = true;
        if let Some(sounds) = sounds {
            play_ui_sfx(&mut commands, &sounds.hover, 0.35, settings.as_deref());
        }
    }
}

fn on_vote_card_out(out: On<PointerOut>, mut cards: Query<&mut VoteCard>) {
    if let Ok(mut card) = cards.get_mut(out.entity) {
        card.hovered = false;
    }
}

fn vote_with_keys(
    keys: Res<ButtonInput<KeyCode>>,
    overlay: Res<Overlay>,
    mut podium: ResMut<Podium>,
    net: Res<NetworkState>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
) {
    if !podium.active() || overlay.blocks_input() {
        return;
    }
    let picked = if keys.just_pressed(KeyCode::Digit1) {
        Some(0)
    } else if keys.just_pressed(KeyCode::Digit2) {
        Some(1)
    } else if keys.just_pressed(KeyCode::Digit3) {
        Some(2)
    } else {
        None
    };
    if let Some(idx) = picked {
        cast_vote(
            idx,
            &mut podium,
            &net,
            &mut commands,
            sounds.as_deref(),
            settings.as_deref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn build_podium(
    mut commands: Commands,
    mut podium: ResMut<Podium>,
    net: Res<NetworkState>,
    local: Res<LocalState>,
    overlay: Res<Overlay>,
    library: Res<MapLibrary>,
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
        let was_active = podium.active();
        clear(&mut podium, &mut commands);
        podium.shown_for = None;
        podium.my_vote = None;
        podium.announced_lock = false;
        // Leaving podium into the next match: recapture mouse look.
        if was_active && !overlay.menu_open() {
            set_cursor_grab(&mut cursor_q, true);
        }
        return;
    }
    // Wait until the host's vote options have arrived before building the cards.
    if ms.vote.options.is_empty() {
        return;
    }
    if podium.shown_for == Some(ms.match_id) {
        return;
    }
    podium.shown_for = Some(ms.match_id);
    podium.my_vote = None;
    podium.announced_lock = false;
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

    // --- The results + 3-map voting panel over it --------------------------
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
        // Top match result banner.
        ui.spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(14.0),
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
                    width: Val::Px(280.0),
                    height: Val::Px(70.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            if ms.winner != WIN_DRAW {
                top.spawn((
                    Text::new(headline(&net)),
                    fonts.heading(36.0),
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
                TextFont::from_font_size(14.0),
                TextColor(KIT_YELLOW),
                Pickable::IGNORE,
            ));
        });

        // Podium player nameplates.
        let medals = [
            "badges/medal_gold.png",
            "badges/medal_silver.png",
            "badges/medal_bronze.png",
        ];
        for (i, &(id, splats)) in top.iter().enumerate() {
            ui.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(170.0),
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    display: Display::None,
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.04, 0.06, 0.84)),
                Pickable::IGNORE,
                PodiumLabel(i),
            ))
            .with_children(|l| {
                l.spawn((
                    ImageNode::new(kit(&assets, medals[i])),
                    Node {
                        width: Val::Px(34.0),
                        height: Val::Px(34.0),
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
                    fonts.heading(24.0),
                    TextColor(color),
                    Pickable::IGNORE,
                ));
                l.spawn((
                    Text::new(format!(
                        "{splats} SPLAT{}",
                        if splats == 1 { "" } else { "S" }
                    )),
                    TextFont::from_font_size(13.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                ));
            });
        }

        // Bottom: 3 Random Maps Voting Dock + Clock + Main Menu button.
        ui.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(28.0),
                right: Val::Px(28.0),
                bottom: Val::Px(18.0),
                padding: UiRect::axes(Val::Px(20.0), Val::Px(14.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                border: UiRect::all(Val::Px(1.5)),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.03, 0.045, 0.07, 0.90)),
            BorderColor::all(VOTE_BORDER_IDLE),
        ))
        .with_children(|dock| {
            // Header row: title + tiebreaker badge + clock + Main Menu button.
            dock.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    column_gap: Val::Px(14.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|hdr| {
                hdr.spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(12.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|left| {
                    left.spawn((
                        Text::new("VOTE FOR NEXT MAP"),
                        fonts.heading(26.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                        VoteHeaderText,
                    ));
                    left.spawn((
                        Node {
                            display: Display::None,
                            padding: UiRect::axes(Val::Px(10.0), Val::Px(3.0)),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(KIT_YELLOW),
                        Pickable::IGNORE,
                        VoteTieBadge,
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Text::new("RANDOM TIEBREAKER!"),
                            TextFont::from_font_size(11.0),
                            TextColor(Color::srgb(0.05, 0.06, 0.08)),
                            Pickable::IGNORE,
                        ));
                    });
                });

                hdr.spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(16.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|right| {
                    right.spawn((
                        Text::new(""),
                        TextFont::from_font_size(14.0),
                        TextColor(KIT_YELLOW),
                        Pickable::IGNORE,
                        PodiumClock,
                    ));
                    right
                        .spawn(Node {
                            width: Val::Px(170.0),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        })
                        .with_children(|w| {
                            spawn_button(
                                w,
                                "MAIN MENU",
                                HudAction::LeaveGame,
                                ButtonStyle::SECONDARY,
                                38.0,
                                14.0,
                                false,
                            );
                        });
                });
            });

            // The 3 random map cards.
            dock.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(14.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                for (i, &map_id) in ms.vote.options.iter().enumerate() {
                    let idx = i as u8;
                    let meta = library.meta(map_id);
                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(6.0),
                            border: UiRect::all(Val::Px(2.0)),
                            border_radius: BorderRadius::all(Val::Px(8.0)),
                            ..default()
                        },
                        BackgroundColor(VOTE_CARD_BG),
                        BorderColor::all(VOTE_BORDER_IDLE),
                        VoteCard {
                            index: idx,
                            hovered: false,
                        },
                    ))
                    .with_children(|card| {
                        // Top line: [1] MAP NAME + vote count badge.
                        card.spawn((
                            Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                justify_content: JustifyContent::SpaceBetween,
                                column_gap: Val::Px(8.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|top| {
                            top.spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(8.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|name_row| {
                                name_row
                                    .spawn((
                                        Node {
                                            width: Val::Px(24.0),
                                            height: Val::Px(24.0),
                                            justify_content: JustifyContent::Center,
                                            align_items: AlignItems::Center,
                                            border_radius: BorderRadius::all(Val::Px(4.0)),
                                            ..default()
                                        },
                                        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.12)),
                                        Pickable::IGNORE,
                                    ))
                                    .with_children(|k| {
                                        k.spawn((
                                            Text::new(format!("{}", i + 1)),
                                            TextFont::from_font_size(12.0),
                                            TextColor(KIT_YELLOW),
                                            Pickable::IGNORE,
                                        ));
                                    });
                                name_row.spawn((
                                    Text::new(meta.name.to_uppercase()),
                                    fonts.heading(24.0),
                                    TextColor(TEXT_PRIMARY),
                                    Pickable::IGNORE,
                                    VoteCardTitle(idx),
                                ));
                            });

                            top.spawn((
                                Text::new("0 VOTES"),
                                TextFont::from_font_size(13.0),
                                TextColor(KIT_YELLOW),
                                Pickable::IGNORE,
                                VoteCardCount(idx),
                            ));
                        });

                        // Style | Size & status tag.
                        card.spawn((
                            Node {
                                flex_direction: FlexDirection::Row,
                                justify_content: JustifyContent::SpaceBetween,
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|sub| {
                            sub.spawn((
                                Text::new(format!(
                                    "{}  |  {}",
                                    meta.style.to_uppercase(),
                                    meta.size.to_uppercase()
                                )),
                                TextFont::from_font_size(11.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                            ));
                            sub.spawn((
                                Text::new("CLICK TO VOTE"),
                                TextFont::from_font_size(11.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                                VoteCardStatus(idx),
                            ));
                        });

                        // Short map blurb.
                        card.spawn((
                            Text::new(meta.blurb),
                            TextFont::from_font_size(11.0),
                            TextColor(TEXT_MUTED),
                            Pickable::IGNORE,
                        ));

                        // Proportional vote bar.
                        card.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Px(10.0),
                                padding: UiRect::all(Val::Px(2.0)),
                                margin: UiRect::top(Val::Px(2.0)),
                                ..default()
                            },
                            sliced(kit(&assets, "controls/progress_frame.png"), 4.0),
                            Pickable::IGNORE,
                        ))
                        .with_children(|bar| {
                            bar.spawn((
                                Node {
                                    width: Val::Percent(0.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                sliced(kit(&assets, "controls/progress_fill_yellow.png"), 4.0),
                                Pickable::IGNORE,
                                VoteCardBar(idx),
                            ));
                        });
                    });
                }
            });
        });
    });

    // Free the mouse so map vote cards and Main Menu can be clicked.
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
    // Slightly higher look-down framing so the top-3 podium sits cleanly above
    // the bottom map-voting bar.
    Transform::from_translation(STAGE + Vec3::new(0.0, 2.05, 8.6))
        .looking_at(STAGE + Vec3::new(0.0, 1.55, 0.0), Vec3::Y)
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

/// Keeps the names under their blocks, vote counts/bars live, and clock running.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_podium_ui(
    mut commands: Commands,
    mut podium: ResMut<Podium>,
    net: Res<NetworkState>,
    library: Res<MapLibrary>,
    assets: Res<AssetServer>,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
    cam_q: Query<&Camera, With<MainCamera>>,
    mut cards: Query<(&VoteCard, &mut BackgroundColor, &mut BorderColor)>,
    mut nodes: Query<(
        &mut Node,
        Option<&PodiumLabel>,
        Has<VoteTieBadge>,
        Option<&VoteCardBar>,
        Option<&mut ImageNode>,
    )>,
    mut texts: Query<(
        &mut Text,
        &mut TextColor,
        Has<PodiumClock>,
        Has<VoteHeaderText>,
        Option<&VoteCardTitle>,
        Option<&VoteCardCount>,
        Option<&VoteCardStatus>,
    )>,
) {
    if !podium.active() {
        return;
    }

    let vote = &net.match_state.vote;
    let total_votes: u32 = vote.counts.iter().map(|&c| c as u32).sum();
    let winner = vote.winner;

    if winner.is_some() && !podium.announced_lock {
        podium.announced_lock = true;
        if let Some(sounds) = sounds {
            play_ui_sfx(&mut commands, &sounds.confirm, 0.7, settings.as_deref());
        }
    }

    let cam_opt = cam_q.single().ok();
    // Where `podium_camera` puts the camera (its global transform may lag a frame).
    let cam_tf = GlobalTransform::from(camera_spot());

    for (mut node, label, is_tie_badge, bar, img) in &mut nodes {
        if let Some(label) = label {
            if let Some(camera) = cam_opt {
                let (x, h) = PLACES[label.0];
                // On the front of the block.
                let anchor = STAGE + Vec3::new(x, h * 0.5, 0.95);
                if let Ok(p) = camera.world_to_viewport(&cam_tf, anchor) {
                    node.left = Val::Px(p.x - 85.0);
                    node.top = Val::Px(p.y - 46.0);
                    node.display = Display::Flex;
                }
            }
        } else if is_tie_badge {
            let want = if winner.is_some() && vote.tiebreak {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != want {
                node.display = want;
            }
        } else if let Some(bar) = bar {
            let n = vote.counts.get(bar.0 as usize).copied().unwrap_or(0) as f32;
            let is_winner = winner == Some(bar.0);
            let pct = if is_winner && total_votes == 0 {
                100.0
            } else if total_votes > 0 {
                (n / total_votes as f32) * 100.0
            } else {
                0.0
            };
            node.width = Val::Percent(pct);
            if let Some(mut img) = img {
                let fill_art = kit(
                    &assets,
                    if is_winner {
                        "controls/progress_fill_green.png"
                    } else {
                        "controls/progress_fill_yellow.png"
                    },
                );
                if img.image != fill_art {
                    img.image = fill_art;
                }
            }
        }
    }

    for (card, mut bg, mut border) in &mut cards {
        let is_winner = winner == Some(card.index);
        let is_mine = podium.my_vote == Some(card.index);
        let locked_loser = winner.is_some() && !is_winner;

        *bg = BackgroundColor(if is_winner {
            VOTE_CARD_WINNER_BG
        } else if is_mine {
            VOTE_CARD_SELECTED_BG
        } else {
            VOTE_CARD_BG
        });
        *border = BorderColor::all(if is_winner {
            VOTE_WINNER_GREEN
        } else if is_mine || (card.hovered && winner.is_none()) {
            KIT_YELLOW
        } else if locked_loser {
            Color::srgba(0.3, 0.35, 0.42, 0.22)
        } else {
            VOTE_BORDER_IDLE
        });
    }

    let left = net.match_state.time_left.ceil() as u32;
    let clock_line = if winner.is_some() {
        format!("Loading next map in {left}s...")
    } else {
        format!("Voting closes in {}:{:02}", left / 60, left % 60)
    };
    let header_line = if let Some(w_map) = vote.winning_map() {
        format!("NEXT MAP: {}", library.meta(w_map).name.to_uppercase())
    } else {
        "VOTE FOR NEXT MAP  (CLICK OR PRESS 1-3)".to_string()
    };

    for (mut text, mut color, is_clock, is_header, title, count, status) in &mut texts {
        if is_clock {
            if text.0 != clock_line {
                text.0 = clock_line.clone();
            }
        } else if is_header {
            if text.0 != header_line {
                text.0 = header_line.clone();
            }
        } else if let Some(title) = title {
            let is_winner = winner == Some(title.0);
            let is_mine = podium.my_vote == Some(title.0);
            color.0 = if is_winner {
                VOTE_WINNER_GREEN
            } else if is_mine {
                KIT_YELLOW
            } else if winner.is_some() {
                TEXT_MUTED
            } else {
                TEXT_PRIMARY
            };
        } else if let Some(count) = count {
            let n = vote.counts.get(count.0 as usize).copied().unwrap_or(0);
            let want = format!("{n} VOTE{}", if n == 1 { "" } else { "S" });
            if text.0 != want {
                text.0 = want;
            }
        } else if let Some(status) = status {
            let is_winner = winner == Some(status.0);
            let is_mine = podium.my_vote == Some(status.0);
            let (want, col) = if is_winner {
                (
                    if vote.tiebreak {
                        "SELECTED (TIEBREAKER)"
                    } else {
                        "NEXT MAP"
                    },
                    VOTE_WINNER_GREEN,
                )
            } else if winner.is_some() {
                ("", TEXT_MUTED)
            } else if is_mine {
                ("YOUR VOTE", KIT_YELLOW)
            } else {
                ("CLICK TO VOTE", TEXT_MUTED)
            };
            if text.0 != want {
                text.0 = want.to_string();
            }
            color.0 = col;
        }
    }
}
