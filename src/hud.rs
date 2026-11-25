//! In-game HUD from the UI kit:
//!
//! - score header (top center): match clock, team scores (you vs the leader
//!   in free-for-all) and round pips in Elimination, with the phase below;
//! - banners (center): get ready, match point, round and match results,
//!   eliminated;
//! - health (bottom left), ammo and hopper meter (bottom right);
//! - crosshair, hit marker and a hit-direction arc;
//! - minimap (top right) and killfeed under it;
//! - scoreboard (hold Tab, and at the end of a match) with rank badges.

use crate::{
    AppState, Overlay,
    combat::{Combat, HEAD_PAINT_SECS, Killfeed, LocalState},
    loadout::{CROSSHAIRS, GunKind, Loadout},
    maps::MapInfo,
    net::NetworkState,
    protocol::{
        GameMode, MAX_HEALTH, MatchState, Phase, PlayerId, ROUNDS_TO_WIN, Team, WIN_BLUE, WIN_DRAW,
        WIN_ORANGE, WIN_PLAYER,
    },
    ranked::{self, RankResult, Tier},
    ui_kit::{KIT_YELLOW, UiKit, kit, sliced, sliced_rect},
    world::{CameraRig, LocalPlayer, MainCamera},
};
use bevy::prelude::*;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), spawn_hud)
            .add_systems(
                Update,
                (
                    update_health,
                    update_ammo,
                    update_center,
                    update_head_paint,
                    update_score_header,
                    update_banner,
                    update_killfeed,
                    update_minimap,
                    update_scoreboard,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

pub const TEXT_PRIMARY: Color = Color::srgb(0.95, 0.97, 1.0);
pub const TEXT_MUTED: Color = Color::srgb(0.68, 0.75, 0.84);
const WATER: Color = Color::srgb(0.62, 0.89, 1.0);
const MINIMAP: f32 = 180.0;

/// Part of the in-game HUD (hidden while the podium is up).
#[derive(Component)]
pub struct HudPart;

pub fn team_color(team: Team) -> Color {
    let [r, g, b] = team.srgb();
    Color::srgb(r, g, b)
}

/// Green at full health, through yellow, to red when nearly out.
pub fn health_color(frac: f32) -> Color {
    Color::hsl(120.0 * frac.clamp(0.0, 1.0), 0.78, 0.47)
}

/// "You" for the local player, otherwise their Portside name (or "Player N").
pub fn player_name(net: &NetworkState, id: PlayerId) -> String {
    if Some(id) == net.local_player_id {
        "You".to_string()
    } else {
        net.display_name(id)
    }
}

/// A player's colour: their team's, or neutral in free-for-all.
fn player_color(ms: &MatchState, id: PlayerId) -> Color {
    if ms.settings.mode.teams() {
        team_color(Team::of(id)).lighter(0.15)
    } else {
        TEXT_PRIMARY
    }
}

#[derive(Component)]
struct HealthFill;
#[derive(Component)]
struct HealthText;
#[derive(Component)]
struct AmmoIcon;
#[derive(Component)]
struct AmmoText;
#[derive(Component)]
struct AmmoSub;
#[derive(Component)]
struct HopperFill;
#[derive(Component)]
struct Crosshair;
#[derive(Component)]
struct HitMarker;
#[derive(Component)]
struct HitArc;
#[derive(Component)]
struct DamageTint;
/// One paint splat covering part of the screen after a headshot.
#[derive(Component)]
struct HeadPaintSplat(usize);

/// How many splats a headshot throws on the screen.
const HEAD_PAINT_SPLATS: usize = 5;
#[derive(Component)]
struct KillfeedList;

#[derive(Component)]
enum HeaderText {
    Clock,
    Left,
    Right,
    LeftLabel,
    RightLabel,
    Phase,
}
#[derive(Component)]
struct RoundPips;
/// Pip `i` (0..5) for team `0` (blue) or `1` (orange).
#[derive(Component)]
struct Pip(usize, usize);

#[derive(Component)]
struct Banner;
#[derive(Component)]
struct BannerImage;
#[derive(Component)]
struct BannerBig;
#[derive(Component)]
struct BannerLine;

#[derive(Component)]
struct MinimapShapes;
#[derive(Component)]
struct MinimapDots;
#[derive(Component)]
struct MinimapSelf;

#[derive(Component)]
struct Scoreboard;
#[derive(Component)]
struct ScoreboardTitle;
#[derive(Component)]
struct ScoreboardRows;

fn abs_fill() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        right: Val::Px(0.0),
        top: Val::Px(0.0),
        bottom: Val::Px(0.0),
        ..default()
    }
}

fn spawn_hud(
    mut commands: Commands,
    assets: Res<AssetServer>,
    fonts: Res<UiKit>,
    net: Res<NetworkState>,
    loadout: Res<Loadout>,
    crosshair_settings: Res<crate::settings::CrosshairSettings>,
) {
    let k = |p: &str| kit(&assets, p);
    let local = net.local_player_id.unwrap_or(0);
    let team = Team::of(local);

    // Red flash when hit (under everything else).
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::NONE),
        Pickable::IGNORE,
        GlobalZIndex(-1),
        DamageTint,
        DespawnOnExit(AppState::InGame),
        HudPart,
    ));

    // Paint on the screen after a headshot (placed by `update_head_paint`).
    for i in 0..HEAD_PAINT_SPLATS {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Vh(40.0),
                height: Val::Vh(40.0),
                ..default()
            },
            ImageNode::new(assets.load("textures/splats/splat_01.png")).with_color(Color::NONE),
            UiTransform::IDENTITY,
            Pickable::IGNORE,
            GlobalZIndex(-1),
            HeadPaintSplat(i),
            DespawnOnExit(AppState::InGame),
            HudPart,
        ));
    }

    // --- Score header, top center -----------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(4.0),
                ..default()
            },
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|col| {
            col.spawn((
                Node {
                    width: Val::Px(480.0),
                    height: Val::Px(66.0),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    ..default()
                },
                ImageNode::new(k("hud/score_header.png")).with_mode(NodeImageMode::Stretch),
                Pickable::IGNORE,
            ))
            .with_children(|h| {
                let side = |h: &mut ChildSpawnerCommands, score: HeaderText, label: HeaderText| {
                    h.spawn((
                        Node {
                            width: Val::Percent(30.0),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|s| {
                        s.spawn((
                            Text::new("0"),
                            fonts.heading(34.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                            score,
                        ));
                        s.spawn((
                            Text::new(""),
                            TextFont::from_font_size(10.0),
                            TextColor(TEXT_PRIMARY.with_alpha(0.85)),
                            Pickable::IGNORE,
                            label,
                        ));
                    });
                };
                side(h, HeaderText::Left, HeaderText::LeftLabel);
                h.spawn((
                    Node {
                        width: Val::Percent(40.0),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|c| {
                    c.spawn((
                        Text::new("--:--"),
                        fonts.heading(34.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                        HeaderText::Clock,
                    ));
                });
                side(h, HeaderText::Right, HeaderText::RightLabel);
            });
            // Elimination: rounds won.
            col.spawn((
                Node {
                    display: Display::None,
                    width: Val::Px(480.0),
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    padding: UiRect::axes(Val::Px(18.0), Val::Px(0.0)),
                    ..default()
                },
                Pickable::IGNORE,
                RoundPips,
            ))
            .with_children(|row| {
                for t in 0..2 {
                    row.spawn((
                        Node {
                            flex_direction: if t == 0 {
                                FlexDirection::Row
                            } else {
                                FlexDirection::RowReverse
                            },
                            column_gap: Val::Px(4.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|pips| {
                        for i in 0..ROUNDS_TO_WIN as usize {
                            pips.spawn((
                                ImageNode::new(k("controls/pip_empty.png")),
                                Node {
                                    width: Val::Px(16.0),
                                    height: Val::Px(16.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                                Pip(t, i),
                            ));
                        }
                    });
                }
            });
            col.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.05, 0.06, 0.08, 0.55)),
                Pickable::IGNORE,
            ))
            .with_children(|p| {
                p.spawn((
                    Text::new(""),
                    TextFont::from_font_size(13.0),
                    TextColor(KIT_YELLOW),
                    Pickable::IGNORE,
                    HeaderText::Phase,
                ));
            });
        });

    // --- Banner, center -----------------------------------------------------
    commands
        .spawn((
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(20.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(8.0),
                ..default()
            },
            Pickable::IGNORE,
            Banner,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|col| {
            col.spawn((
                ImageNode::new(k("banners/banner_get_ready.png")),
                Node {
                    width: Val::Px(520.0),
                    height: Val::Px(112.0),
                    ..default()
                },
                Pickable::IGNORE,
                BannerImage,
            ));
            col.spawn((
                Text::new(""),
                fonts.heading(64.0),
                TextColor(TEXT_PRIMARY),
                TextShadow::default(),
                Pickable::IGNORE,
                BannerBig,
            ));
            col.spawn((
                Node {
                    padding: UiRect {
                        left: Val::Px(34.0),
                        right: Val::Px(18.0),
                        top: Val::Px(8.0),
                        bottom: Val::Px(8.0),
                    },
                    ..default()
                },
                // The toast's yellow tab stays its own width on the left.
                sliced_rect(
                    k("hud/notification_toast.png"),
                    BorderRect {
                        min_inset: Vec2::new(66.0, 14.0),
                        max_inset: Vec2::new(14.0, 14.0),
                    },
                )
                .with_color(Color::WHITE.with_alpha(0.0)),
                Pickable::IGNORE,
            ))
            .with_children(|t| {
                t.spawn((
                    Text::new(""),
                    TextFont::from_font_size(17.0),
                    TextColor(TEXT_PRIMARY),
                    TextShadow::default(),
                    Pickable::IGNORE,
                    BannerLine,
                ));
            });
        });

    // --- Health, bottom left ------------------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(20.0),
                bottom: Val::Px(20.0),
                padding: UiRect::axes(Val::Px(18.0), Val::Px(12.0)),
                column_gap: Val::Px(12.0),
                align_items: AlignItems::Center,
                ..default()
            },
            sliced(k("panels/panel_glass.png"), 28.0),
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|row| {
            row.spawn((
                ImageNode::new(k("icons/shield.png")).with_color(team_color(team).lighter(0.1)),
                Node {
                    width: Val::Px(40.0),
                    height: Val::Px(40.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            row.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(5.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|col| {
                let label = match net.match_state.settings.mode {
                    GameMode::FreeForAll => "HEALTH  |  FREE-FOR-ALL".to_string(),
                    _ => format!("HEALTH  |  {} TEAM", team.name().to_uppercase()),
                };
                col.spawn((
                    Text::new(label),
                    TextFont::from_font_size(11.0),
                    TextColor(TEXT_MUTED),
                    Pickable::IGNORE,
                ));
                col.spawn((
                    Node {
                        width: Val::Px(240.0),
                        height: Val::Px(22.0),
                        padding: UiRect::all(Val::Px(3.0)),
                        ..default()
                    },
                    sliced(k("controls/progress_frame.png"), 10.0),
                    Pickable::IGNORE,
                ))
                .with_children(|bar| {
                    bar.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        sliced(k("controls/progress_fill_green.png"), 10.0),
                        Pickable::IGNORE,
                        HealthFill,
                    ));
                });
            });
            row.spawn((
                Text::new("100"),
                fonts.heading(30.0),
                TextColor(TEXT_PRIMARY),
                Node {
                    min_width: Val::Px(50.0),
                    ..default()
                },
                Pickable::IGNORE,
                HealthText,
            ));
        });

    // --- Ammo, bottom right -------------------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(20.0),
                bottom: Val::Px(20.0),
                column_gap: Val::Px(10.0),
                align_items: AlignItems::FlexEnd,
                ..default()
            },
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|row| {
            // The counter: the kit frame has a round icon slot on the left.
            row.spawn((
                Node {
                    width: Val::Px(270.0),
                    height: Val::Px(90.0),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    ..default()
                },
                ImageNode::new(k("hud/ammo_counter_frame.png")).with_mode(NodeImageMode::Stretch),
                Pickable::IGNORE,
            ))
            .with_children(|frame| {
                frame.spawn((
                    ImageNode::new(assets.load("textures/ui/ammo_ball.png")),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(26.0),
                        width: Val::Px(36.0),
                        height: Val::Px(36.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                    AmmoIcon,
                ));
                frame
                    .spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(100.0),
                            right: Val::Px(16.0),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::FlexEnd,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|col| {
                        col.spawn((
                            Text::new(""),
                            fonts.heading(34.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                            AmmoText,
                        ));
                        col.spawn((
                            Text::new(loadout.display_name()),
                            TextFont::from_font_size(11.0),
                            TextColor(TEXT_MUTED),
                            Pickable::IGNORE,
                            AmmoSub,
                        ));
                    });
            });
            // Hopper meter: fills from the bottom.
            row.spawn((
                Node {
                    width: Val::Px(28.0),
                    height: Val::Px(90.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|m| {
                m.spawn((
                    abs_fill(),
                    ImageNode::new(k("hud/hopper_meter_frame.png"))
                        .with_mode(NodeImageMode::Stretch),
                    Pickable::IGNORE,
                ));
                m.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(7.0),
                        right: Val::Px(7.0),
                        bottom: Val::Px(5.0),
                        height: Val::Percent(100.0),
                        max_height: Val::Px(80.0),
                        ..default()
                    },
                    ImageNode::new(k("hud/hopper_meter_fill.png"))
                        .with_mode(NodeImageMode::Stretch),
                    Pickable::IGNORE,
                    HopperFill,
                ));
            });
        });

    // --- Crosshair, hit marker and hit arc, dead center ---------------------
    let crosshair = CROSSHAIRS[crosshair_settings.style as usize].1;
    for (path, size, marker) in [
        (crosshair, 30.0, true),
        ("textures/ui/hit_marker.png", 34.0, false),
    ] {
        let mut e = commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(size),
                height: Val::Px(size),
                margin: UiRect {
                    left: Val::Px(-size / 2.0),
                    top: Val::Px(-size / 2.0),
                    ..default()
                },
                ..default()
            },
            ImageNode::new(assets.load(path)).with_color(Color::WHITE.with_alpha(0.0)),
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ));
        if marker {
            e.insert(Crosshair);
        } else {
            e.insert(HitMarker);
        }
    }
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(300.0),
                height: Val::Px(300.0),
                margin: UiRect {
                    left: Val::Px(-150.0),
                    top: Val::Px(-150.0),
                    ..default()
                },
                justify_content: JustifyContent::Center,
                ..default()
            },
            UiTransform::IDENTITY,
            Pickable::IGNORE,
            HitArc,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|c| {
            c.spawn((
                ImageNode::new(k("hud/hit_direction_arc.png"))
                    .with_color(Color::srgba(1.0, 0.3, 0.3, 0.0)),
                Node {
                    width: Val::Px(160.0),
                    height: Val::Px(60.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });

    // --- Minimap and killfeed, top right ------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(16.0),
                top: Val::Px(16.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: Val::Px(10.0),
                ..default()
            },
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|col| {
            col.spawn((
                Node {
                    width: Val::Px(MINIMAP),
                    height: Val::Px(MINIMAP),
                    ..default()
                },
                ImageNode::new(k("hud/minimap_mask.png"))
                    .with_mode(NodeImageMode::Stretch)
                    .with_color(Color::srgba(0.08, 0.10, 0.12, 0.8)),
                Pickable::IGNORE,
            ))
            .with_children(|map| {
                map.spawn((abs_fill(), Pickable::IGNORE, MinimapShapes));
                map.spawn((abs_fill(), Pickable::IGNORE, MinimapDots));
                map.spawn((
                    ImageNode::new(k("hud/minimap_player_arrow.png")),
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(16.0),
                        height: Val::Px(16.0),
                        ..default()
                    },
                    UiTransform::IDENTITY,
                    Pickable::IGNORE,
                    MinimapSelf,
                ));
                map.spawn((
                    abs_fill(),
                    ImageNode::new(k("hud/minimap_frame.png")).with_mode(NodeImageMode::Stretch),
                    Pickable::IGNORE,
                ));
            });
            col.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexEnd,
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                Pickable::IGNORE,
                KillfeedList,
            ));
        });

    // --- Scoreboard (Tab) ---------------------------------------------------
    commands
        .spawn((
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(560.0),
                margin: UiRect {
                    left: Val::Px(-280.0),
                    top: Val::Px(-150.0),
                    ..default()
                },
                padding: UiRect {
                    left: Val::Px(24.0),
                    right: Val::Px(24.0),
                    top: Val::Px(18.0),
                    bottom: Val::Px(22.0),
                },
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            sliced_rect(
                k("panels/dialog.png"),
                BorderRect {
                    min_inset: Vec2::new(28.0, 72.0),
                    max_inset: Vec2::new(28.0, 28.0),
                },
            ),
            GlobalZIndex(5),
            Pickable::IGNORE,
            Scoreboard,
            DespawnOnExit(AppState::InGame),
            HudPart,
        ))
        .with_children(|p| {
            p.spawn((
                Node {
                    height: Val::Px(40.0),
                    margin: UiRect::bottom(Val::Px(8.0)),
                    align_items: AlignItems::Center,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|h| {
                h.spawn((
                    Text::new("SCOREBOARD"),
                    fonts.heading(28.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                    ScoreboardTitle,
                ));
            });
            p.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(0.0)),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|h| {
                for (label, w, grow) in [
                    ("PLAYER", 0.0, 1.0),
                    ("SPLATS", 80.0, 0.0),
                    ("OUT", 60.0, 0.0),
                ] {
                    h.spawn((
                        Text::new(label),
                        TextFont::from_font_size(11.0),
                        TextColor(TEXT_MUTED),
                        Node {
                            width: if w > 0.0 { Val::Px(w) } else { Val::Auto },
                            flex_grow: grow,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                }
            });
            p.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                Pickable::IGNORE,
                ScoreboardRows,
            ));
        });
}

fn update_health(
    assets: Res<AssetServer>,
    local_state: Res<LocalState>,
    combat: Res<Combat>,
    mut fill_q: Query<(&mut Node, &mut ImageNode), With<HealthFill>>,
    mut text_q: Query<(&mut Text, &mut TextColor), With<HealthText>>,
    mut tint_q: Query<&mut BackgroundColor, With<DamageTint>>,
    mut shown: Local<u8>,
) {
    let hp = local_state.0.health;
    let frac = hp / MAX_HEALTH;
    if let Ok((mut node, mut img)) = fill_q.single_mut() {
        node.width = Val::Percent(frac * 100.0);
        let band = if frac > 0.6 {
            1
        } else if frac > 0.3 {
            2
        } else {
            3
        };
        if *shown != band {
            *shown = band;
            let file = ["", "green", "yellow", "red"][band as usize];
            img.image = kit(&assets, &format!("controls/progress_fill_{file}.png"));
        }
    }
    if let Ok((mut text, mut color)) = text_q.single_mut() {
        let s = format!("{}", hp.ceil() as i32);
        if text.0 != s {
            text.0 = s;
        }
        color.0 = health_color(frac).lighter(0.2);
    }
    if let Ok(mut bg) = tint_q.single_mut() {
        let low = if hp > 0.0 {
            (1.0 - frac).powi(2) * 0.18
        } else {
            0.0
        };
        let alpha = (combat.damage_flash / 0.35) * 0.32 + low;
        bg.0 = Color::srgba(0.75, 0.0, 0.0, alpha.min(0.5));
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_ammo(
    assets: Res<AssetServer>,
    combat: Res<Combat>,
    loadout: Res<Loadout>,
    keybinds: Res<crate::settings::KeybindSettings>,
    net: Res<NetworkState>,
    mut icon_q: Query<&mut ImageNode, (With<AmmoIcon>, Without<HopperFill>)>,
    mut hopper_q: Query<(&mut Node, &mut ImageNode), (With<HopperFill>, Without<AmmoIcon>)>,
    mut text_q: Query<&mut Text, (With<AmmoText>, Without<AmmoSub>)>,
    mut sub_q: Query<&mut Text, (With<AmmoSub>, Without<AmmoText>)>,
    mut shown: Local<Option<(GunKind, bool)>>,
) {
    let reloading = combat.reloading.is_some();
    let key = (combat.kind, reloading);
    if let Ok(mut icon) = icon_q.single_mut()
        && *shown != Some(key)
    {
        *shown = Some(key);
        let team = Team::of(net.local_player_id.unwrap_or(0));
        let (path, color) = match (combat.kind, reloading) {
            (GunKind::Water, _) => ("textures/particles/droplet.png", WATER),
            (GunKind::Paintball, true) => ("textures/ui/ammo_pod.png", TEXT_PRIMARY),
            (GunKind::Paintball, false) => ("textures/ui/ammo_ball.png", team_color(team)),
        };
        icon.image = assets.load(path);
        icon.color = color;
    }
    let cap = combat.stats.capacity.max(1);
    let frac = combat.ammo as f32 / cap as f32;
    if let Ok((mut node, mut img)) = hopper_q.single_mut() {
        node.height = Val::Percent(frac * 100.0);
        img.color = if combat.kind == GunKind::Water {
            WATER
        } else if frac < 0.25 {
            Color::srgb(1.0, 0.45, 0.4)
        } else {
            Color::WHITE
        };
    }
    if let Ok(mut text) = text_q.single_mut() {
        let s = format!("{} / {}", combat.ammo, combat.stats.capacity);
        if text.0 != s {
            text.0 = s;
        }
    }
    if let Ok(mut text) = sub_q.single_mut() {
        let s = match combat.reloading {
            Some(left) if combat.stats.water => format!("PUMPING...  {left:.1}s"),
            Some(left) => format!("RELOADING...  {left:.1}s"),
            None if combat.ammo == 0 => format!(
                "EMPTY  |  {} TO RELOAD",
                keybinds
                    .get(crate::settings::KeyAction::Reload)
                    .label()
                    .to_uppercase()
            ),
            None => loadout.display_name().to_uppercase(),
        };
        if text.0 != s {
            text.0 = s;
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_center(
    combat: Res<Combat>,
    overlay: Res<Overlay>,
    local_state: Res<LocalState>,
    crosshair_settings: Res<crate::settings::CrosshairSettings>,
    assets: Res<AssetServer>,
    rig_q: Query<&CameraRig, With<MainCamera>>,
    mut cross_q: Query<&mut ImageNode, (With<Crosshair>, Without<HitMarker>)>,
    mut hit_q: Query<&mut ImageNode, (With<HitMarker>, Without<Crosshair>)>,
    mut arc_q: Query<(&mut UiTransform, &Children), With<HitArc>>,
    mut arc_img: Query<&mut ImageNode, (Without<HitMarker>, Without<Crosshair>)>,
) {
    let alive = local_state.0.health > 0.0;
    if let Ok(mut img) = cross_q.single_mut() {
        if crosshair_settings.is_changed() {
            img.image = assets.load(CROSSHAIRS[crosshair_settings.style as usize].1);
        }
        // Fades while aiming down a sight (the sight is the crosshair then).
        let a = if alive && !overlay.blocks_input() {
            0.9 * (1.0 - combat.aim_blend * 0.3)
        } else {
            0.0
        };
        img.color = Color::WHITE.with_alpha(a);
    }
    if let Ok(mut img) = hit_q.single_mut() {
        // White on a hit, gold on a headshot, red on an elimination.
        img.color = combat
            .hitmarker_kind
            .color()
            .with_alpha((combat.hitmarker / 0.25).min(1.0));
    }
    // The arc points toward where the last hit came from.
    if let Ok((mut tf, children)) = arc_q.single_mut() {
        let yaw = rig_q.single().map(|r| r.yaw).unwrap_or(0.0);
        let from = combat.hit_from;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
        // Clockwise from straight ahead, like the UI rotation.
        let rel = from.dot(right).atan2(from.dot(forward));
        tf.rotation = Rot2::radians(rel);
        let a = if alive {
            (combat.hit_arc / 0.6).min(1.0) * 0.85
        } else {
            0.0
        };
        for &c in children {
            if let Ok(mut img) = arc_img.get_mut(c) {
                img.color = Color::srgba(1.0, 0.3, 0.3, a);
            }
        }
    }
}

fn clock(seconds: f32) -> String {
    let s = seconds.ceil().max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

#[allow(clippy::type_complexity)]
fn update_score_header(
    assets: Res<AssetServer>,
    net: Res<NetworkState>,
    mut texts: Query<(&HeaderText, &mut Text)>,
    mut pips_q: Query<&mut Node, With<RoundPips>>,
    mut pips: Query<(&Pip, &mut ImageNode)>,
) {
    let ms = &net.match_state;
    let me = net.local_player_id.unwrap_or(0);
    let ffa = ms.settings.mode == GameMode::FreeForAll;
    let elim = ms.settings.mode == GameMode::Elimination;
    let (left, right, l_label, r_label) = if ffa {
        let mine = ms.kills(me);
        let best = ms
            .board
            .iter()
            .filter(|l| l[0] != me)
            .map(|l| l[1])
            .max()
            .unwrap_or(0);
        (mine, best, "YOU", "BEST RIVAL")
    } else {
        (ms.score[0], ms.score[1], "BLUE", "ORANGE")
    };
    let phase = match ms.phase {
        Phase::Warmup => "WARMUP  |  WAITING FOR PLAYERS".to_string(),
        Phase::Countdown if elim => format!("ROUND {}  |  GET READY", ms.round),
        Phase::Countdown => "GET READY".to_string(),
        Phase::Live if elim => {
            let alive = |team: Team| {
                net.players
                    .values()
                    .filter(|p| Team::of(p.id) == team && p.state.health > 0.0)
                    .count()
            };
            format!(
                "ROUND {}  |  {} VS {} STANDING",
                ms.round,
                alive(Team::Blue),
                alive(Team::Orange)
            )
        }
        Phase::Live => format!(
            "{}  |  MOST SPLATS WINS",
            ms.settings.mode.name().to_uppercase()
        ),
        Phase::RoundOver => format!("ROUND {} OVER", ms.round),
        Phase::MatchOver => "MATCH OVER".to_string(),
    };
    let ranked = if ms.settings.ranked {
        "RANKED  |  "
    } else {
        ""
    };
    for (part, mut text) in &mut texts {
        let s = match part {
            HeaderText::Clock => match ms.phase {
                Phase::Warmup => "--:--".to_string(),
                _ => clock(ms.time_left),
            },
            HeaderText::Left => left.to_string(),
            HeaderText::Right => right.to_string(),
            HeaderText::LeftLabel => l_label.to_string(),
            HeaderText::RightLabel => r_label.to_string(),
            HeaderText::Phase => format!("{ranked}{phase}"),
        };
        if text.0 != s {
            text.0 = s;
        }
    }
    if let Ok(mut node) = pips_q.single_mut() {
        let want = if elim { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if elim && net.is_changed() {
        for (pip, mut img) in &mut pips {
            let won = ms.score[pip.0] as usize > pip.1;
            let file = match (won, pip.0) {
                (false, _) => "pip_empty",
                (true, 0) => "pip_blue",
                (true, _) => "pip_orange",
            };
            let h = kit(&assets, &format!("controls/{file}.png"));
            if img.image != h {
                img.image = h;
            }
        }
    }
}

/// What the banner shows: (image, big text, line).
fn banner_for(
    net: &NetworkState,
    me: PlayerId,
    combat: &Combat,
    local_state: &LocalState,
    rank: &RankResult,
) -> Option<(&'static str, String, String)> {
    let ms = &net.match_state;
    let my_team = Team::of(me);
    let team_win = |w: u8| match w {
        WIN_BLUE => Some(Team::Blue),
        WIN_ORANGE => Some(Team::Orange),
        _ => None,
    };
    match ms.phase {
        Phase::Countdown => {
            let point = ms.settings.mode == GameMode::Elimination
                && ms.score.iter().any(|&s| s + 1 == ROUNDS_TO_WIN);
            let img = if point {
                "banners/banner_match_point.png"
            } else {
                "banners/banner_get_ready.png"
            };
            let line = match ms.settings.mode {
                GameMode::Elimination => {
                    format!("Round {}  |  one life, last team standing", ms.round)
                }
                GameMode::TeamSplat => "Most splats in 10 minutes wins".to_string(),
                GameMode::FreeForAll => "Everyone is an enemy  |  10 minutes".to_string(),
            };
            Some((
                img,
                format!("{}", ms.time_left.ceil().max(1.0) as u32),
                line,
            ))
        }
        Phase::RoundOver => {
            let img = match team_win(ms.round_winner) {
                Some(Team::Blue) => "banners/banner_blue_wins.png",
                Some(Team::Orange) => "banners/banner_orange_wins.png",
                None => "banners/banner_draw.png",
            };
            let line = format!(
                "Blue {}  -  {} Orange  |  first to {}",
                ms.score[0], ms.score[1], ROUNDS_TO_WIN
            );
            Some((img, String::new(), line))
        }
        Phase::MatchOver => {
            let won = if ms.settings.mode == GameMode::FreeForAll {
                (ms.winner != WIN_DRAW)
                    .then(|| ms.winner == WIN_PLAYER.saturating_add(me.min(200) as u8))
            } else {
                team_win(ms.winner).map(|t| t == my_team)
            };
            let img = match won {
                Some(true) => "banners/banner_victory.png",
                Some(false) => "banners/banner_defeat.png",
                None => "banners/banner_draw.png",
            };
            let mut line = format!("Main menu in {}", ms.time_left.ceil() as u32);
            if ms.settings.ranked && rank.match_id == Some(ms.match_id) {
                let after = (rank.before as i32 + rank.delta).max(0) as u32;
                line = format!(
                    "{:+} RP  |  {}  |  {}",
                    rank.delta,
                    ranked::rank_name(after),
                    line
                );
            }
            Some((img, String::new(), line))
        }
        _ => {
            let left = combat.respawn_in()?;
            let killer = local_state.0.killer;
            let by = if killer == me {
                "You're out".to_string()
            } else {
                format!("Splatted by {}", net.display_name(killer))
            };
            let line = if ms.respawns() {
                format!("{by}  |  back in {}", left.ceil() as u32)
            } else {
                format!("{by}  |  out until the next round")
            };
            Some(("banners/banner_eliminated.png", String::new(), line))
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_banner(
    assets: Res<AssetServer>,
    net: Res<NetworkState>,
    combat: Res<Combat>,
    local_state: Res<LocalState>,
    rank: Res<RankResult>,
    mut banner_q: Query<&mut Node, With<Banner>>,
    mut image_q: Query<&mut ImageNode, (With<BannerImage>, Without<BannerLine>)>,
    mut big_q: Query<&mut Text, (With<BannerBig>, Without<BannerLine>)>,
    mut line_q: Query<(&mut Text, &ChildOf), (With<BannerLine>, Without<BannerBig>)>,
    mut toast_q: Query<&mut ImageNode, Without<BannerImage>>,
) {
    let me = net.local_player_id.unwrap_or(0);
    let shown = banner_for(&net, me, &combat, &local_state, &rank);
    if let Ok(mut node) = banner_q.single_mut() {
        let want = if shown.is_some() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
    let Some((img, big, line)) = shown else {
        return;
    };
    if let Ok(mut i) = image_q.single_mut() {
        let h = kit(&assets, img);
        if i.image != h {
            i.image = h;
        }
    }
    if let Ok(mut t) = big_q.single_mut()
        && t.0 != big
    {
        t.0 = big;
    }
    if let Ok((mut t, parent)) = line_q.single_mut() {
        if let Ok(mut toast) = toast_q.get_mut(parent.parent()) {
            toast.color = Color::WHITE.with_alpha(if line.is_empty() { 0.0 } else { 0.92 });
        }
        if t.0 != line {
            t.0 = line;
        }
    }
}

fn update_killfeed(
    mut commands: Commands,
    assets: Res<AssetServer>,
    feed: Res<Killfeed>,
    net: Res<NetworkState>,
    list_q: Query<Entity, With<KillfeedList>>,
    mut shown: Local<Vec<(PlayerId, PlayerId, u32)>>,
    mut names_seen: Local<u32>,
) {
    let Ok(list) = list_q.single() else {
        return;
    };
    let now: Vec<_> = feed
        .entries
        .iter()
        .map(|(k, v, t)| (*k, *v, t.to_bits()))
        .collect();
    if *shown == now && *names_seen == net.names_version {
        return;
    }
    *shown = now;
    *names_seen = net.names_version;
    commands.entity(list).despawn_related::<Children>();
    let local = net.local_player_id;
    let ms = &net.match_state;
    let entries: Vec<_> = feed.entries.iter().rev().take(5).copied().collect();
    commands.entity(list).with_children(|list| {
        for (killer, victim, _) in entries {
            let mine = Some(killer) == local || Some(victim) == local;
            let row = if mine {
                "rows/killfeed_row_self.png"
            } else {
                "rows/killfeed_row.png"
            };
            list.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(6.0)),
                    column_gap: Val::Px(8.0),
                    align_items: AlignItems::Center,
                    ..default()
                },
                sliced(kit(&assets, row), 12.0),
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new(player_name(&net, killer)),
                    TextFont::from_font_size(13.0),
                    TextColor(player_color(ms, killer)),
                    Pickable::IGNORE,
                ));
                row.spawn((
                    ImageNode::new(kit(&assets, "icons/splat.png")).with_color(
                        if ms.settings.mode.teams() {
                            team_color(Team::of(killer))
                        } else {
                            KIT_YELLOW
                        },
                    ),
                    Node {
                        width: Val::Px(20.0),
                        height: Val::Px(20.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Text::new(player_name(&net, victim)),
                    TextFont::from_font_size(13.0),
                    TextColor(player_color(ms, victim)),
                    Pickable::IGNORE,
                ));
            });
        }
    });
}

/// Map position to minimap pixels (x right, z down, like looking from above
/// with -Z up).
fn to_minimap(map: &MapInfo, p: Vec2) -> Vec2 {
    let half = map.half.max_element().max(1.0);
    let s = (MINIMAP * 0.5 - 24.0) / half;
    Vec2::new(MINIMAP * 0.5 + p.x * s, MINIMAP * 0.5 + p.y * s)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_minimap(
    mut commands: Commands,
    assets: Res<AssetServer>,
    map: Res<MapInfo>,
    net: Res<NetworkState>,
    shapes_q: Query<Entity, With<MinimapShapes>>,
    dots_q: Query<Entity, With<MinimapDots>>,
    mut self_q: Query<(&mut Node, &mut UiTransform), With<MinimapSelf>>,
    player_q: Query<(&Transform, &LocalPlayer)>,
    mut built: Local<Option<crate::protocol::MapId>>,
    mut dot_timer: Local<f32>,
    time: Res<Time>,
) {
    // Cover, drawn once per map.
    if let Ok(shapes) = shapes_q.single()
        && *built != Some(map.id)
    {
        *built = Some(map.id);
        commands.entity(shapes).despawn_related::<Children>();
        let half = map.half;
        let a = to_minimap(&map, -half);
        let b = to_minimap(&map, half);
        commands.entity(shapes).with_children(|s| {
            // The field.
            s.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(a.x),
                    top: Val::Px(a.y),
                    width: Val::Px(b.x - a.x),
                    height: Val::Px(b.y - a.y),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.35, 0.45, 0.3, 0.25)),
                Pickable::IGNORE,
            ));
            for (rect, height) in &map.footprints {
                let lo = to_minimap(&map, rect.min);
                let hi = to_minimap(&map, rect.max);
                let size = (hi - lo).max(Vec2::splat(1.5));
                let shade = (0.55 + (height / 3.0).min(1.0) * 0.35).min(0.95);
                s.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(lo.x),
                        top: Val::Px(lo.y),
                        width: Val::Px(size.x),
                        height: Val::Px(size.y),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(shade, shade, shade * 0.95, 0.75)),
                    Pickable::IGNORE,
                ));
            }
        });
    }

    let Ok((tf, player)) = player_q.single() else {
        return;
    };
    if let Ok((mut node, mut ui)) = self_q.single_mut() {
        let p = to_minimap(&map, tf.translation.xz());
        node.left = Val::Px(p.x - 8.0);
        node.top = Val::Px(p.y - 8.0);
        // The arrow art points up (-Z); yaw turns counter-clockwise.
        ui.rotation = Rot2::radians(-player.yaw);
    }

    // Teammates (nobody in free-for-all), refreshed a few times a second.
    *dot_timer -= time.delta_secs();
    if *dot_timer > 0.0 {
        return;
    }
    *dot_timer = 0.15;
    let Ok(dots) = dots_q.single() else {
        return;
    };
    commands.entity(dots).despawn_related::<Children>();
    if !net.match_state.settings.mode.teams() {
        return;
    }
    let my_team = Team::of(player.id);
    let dot = kit(
        &assets,
        match my_team {
            Team::Blue => "hud/minimap_dot_blue.png",
            Team::Orange => "hud/minimap_dot_orange.png",
        },
    );
    let mates: Vec<Vec2> = net
        .players
        .values()
        .filter(|p| p.id != player.id && Team::of(p.id) == my_team && p.state.health > 0.0)
        .map(|p| to_minimap(&map, Vec2::new(p.position[0], p.position[2])))
        .collect();
    commands.entity(dots).with_children(|d| {
        for m in mates {
            d.spawn((
                ImageNode::new(dot.clone()),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(m.x - 6.0),
                    top: Val::Px(m.y - 6.0),
                    width: Val::Px(12.0),
                    height: Val::Px(12.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }
    });
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_scoreboard(
    mut commands: Commands,
    assets: Res<AssetServer>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<crate::settings::KeybindSettings>,
    overlay: Res<Overlay>,
    net: Res<NetworkState>,
    library: Res<crate::maps::MapLibrary>,
    mut board_q: Query<&mut Node, With<Scoreboard>>,
    mut title_q: Query<&mut Text, With<ScoreboardTitle>>,
    rows_q: Query<Entity, With<ScoreboardRows>>,
    mut last: Local<Vec<(PlayerId, u32, u32, u16, bool)>>,
    mut names_seen: Local<u32>,
) {
    let ms = &net.match_state;
    let open = keybinds.pressed(crate::settings::KeyAction::Scoreboard, &keys, &mouse)
        && !overlay.blocks_input();
    if let Ok(mut node) = board_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        return;
    }
    if let Ok(mut t) = title_q.single_mut() {
        let s = format!(
            "{}{}  |  {}",
            if ms.settings.ranked { "RANKED " } else { "" },
            ms.settings.mode.name().to_uppercase(),
            library.meta(ms.settings.map).name.to_uppercase()
        );
        if t.0 != s {
            t.0 = s;
        }
    }
    // Everyone in the game, with their line from the board.
    let mut lines: Vec<(PlayerId, u32, u32, u16, bool)> = net
        .players
        .values()
        .map(|p| {
            (
                p.id,
                ms.kills(p.id),
                ms.deaths(p.id),
                p.state.rank_points,
                p.state.health > 0.0,
            )
        })
        .collect();
    let teams = ms.settings.mode.teams();
    lines.sort_by(|a, b| {
        let ta = if teams { Team::of(a.0).index() } else { 0 };
        let tb = if teams { Team::of(b.0).index() } else { 0 };
        ta.cmp(&tb)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });
    lines.truncate(16);
    if *last == lines && *names_seen == net.names_version {
        return;
    }
    *last = lines.clone();
    *names_seen = net.names_version;
    let Ok(rows) = rows_q.single() else {
        return;
    };
    let me = net.local_player_id;
    let ranked_match = ms.settings.ranked;
    commands.entity(rows).despawn_related::<Children>();
    commands.entity(rows).with_children(|list| {
        for (id, kills, deaths, rp, alive) in lines {
            let row = if Some(id) == me {
                "rows/scoreboard_row_self.png"
            } else if !teams {
                "rows/scoreboard_row.png"
            } else if Team::of(id) == Team::Blue {
                "rows/scoreboard_row_blue.png"
            } else {
                "rows/scoreboard_row_orange.png"
            };
            list.spawn((
                Node {
                    height: Val::Px(38.0),
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(0.0)),
                    column_gap: Val::Px(10.0),
                    align_items: AlignItems::Center,
                    ..default()
                },
                sliced(kit(&assets, row), 14.0),
                Pickable::IGNORE,
            ))
            .with_children(|r| {
                if ranked_match || rp > 0 {
                    r.spawn((
                        ImageNode::new(assets.load(Tier::of(rp as u32).badge())),
                        Node {
                            width: Val::Px(26.0),
                            height: Val::Px(26.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                }
                let name = if Some(id) == me {
                    format!("{} (you)", net.display_name(id))
                } else {
                    net.display_name(id)
                };
                r.spawn((
                    Text::new(name),
                    TextFont::from_font_size(14.0),
                    TextColor(if alive { TEXT_PRIMARY } else { TEXT_MUTED }),
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                if ranked_match {
                    r.spawn((
                        Text::new(ranked::rank_name(rp as u32)),
                        TextFont::from_font_size(11.0),
                        TextColor(Tier::of(rp as u32).color()),
                        Pickable::IGNORE,
                    ));
                }
                r.spawn((
                    Text::new(kills.to_string()),
                    TextFont::from_font_size(16.0),
                    TextColor(KIT_YELLOW),
                    Node {
                        width: Val::Px(80.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                r.spawn((
                    Text::new(deaths.to_string()),
                    TextFont::from_font_size(16.0),
                    TextColor(TEXT_PRIMARY),
                    Node {
                        width: Val::Px(60.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        }
    });
}

/// Headshot paint: a few splats clumped on one part of the screen, in the
/// shooter's colour, that hold for a moment and fade out over
/// `HEAD_PAINT_SECS`.
fn update_head_paint(
    assets: Res<AssetServer>,
    combat: Res<Combat>,
    mut splats: Query<(&HeadPaintSplat, &mut Node, &mut ImageNode, &mut UiTransform)>,
    mut seen: Local<u32>,
) {
    let fresh = combat.head_paint_seed != *seen;
    *seen = combat.head_paint_seed;
    let alpha = if combat.head_paint > 0.0 {
        (combat.head_paint / (HEAD_PAINT_SECS * 0.75)).min(1.0) * 0.94
    } else {
        0.0
    };

    // Same seed, same layout: cheap repeatable randomness.
    let mut rng = combat.head_paint_seed.wrapping_mul(0x9E37_79B9) | 1;
    let mut rand = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng as f32 / u32::MAX as f32
    };
    // Where the paint lands: a spot somewhere off the middle of the screen.
    let (cx, cy) = (15.0 + rand() * 70.0, 15.0 + rand() * 70.0);
    for (splat, mut node, mut img, mut tf) in &mut splats {
        if fresh {
            // The first splat is the big one; the rest spray around it.
            let size = if splat.0 == 0 {
                55.0 + rand() * 15.0
            } else {
                18.0 + rand() * 22.0
            };
            let spread = if splat.0 == 0 { 6.0 } else { 26.0 };
            let x = cx + (rand() - 0.5) * spread * 2.0;
            let y = cy + (rand() - 0.5) * spread * 2.0;
            node.width = Val::Vh(size);
            node.height = Val::Vh(size);
            node.left = Val::Vw(x - size * 0.5 * 9.0 / 16.0);
            node.top = Val::Vh(y - size * 0.5);
            tf.rotation = Rot2::radians(rand() * std::f32::consts::TAU);
            let n = 1 + (rand() * 7.99) as usize;
            img.image = assets.load(format!("textures/splats/splat_{n:02}.png"));
        }
        let a = if splat.0 == 0 { alpha } else { alpha * 0.9 };
        img.color = combat.head_paint_tint.with_alpha(a);
    }
}
