//! In-game HUD built from the pack's UI art: health bar (bottom left), ammo
//! (bottom right), crosshair and hit marker, the elimination screen and a
//! killfeed.

use crate::{
    AppState, Overlay,
    combat::{Combat, Killfeed, LocalState},
    loadout::{CROSSHAIRS, GunKind, Loadout},
    net::NetworkState,
    protocol::{MAX_HEALTH, PlayerId, Team},
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
                    update_eliminated,
                    update_killfeed,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

pub const TEXT_PRIMARY: Color = Color::srgb(0.95, 0.97, 1.0);
pub const TEXT_MUTED: Color = Color::srgb(0.68, 0.75, 0.84);
const PANEL: Color = Color::srgba(0.05, 0.07, 0.11, 0.72);
const WATER: Color = Color::srgb(0.62, 0.89, 1.0);

pub fn team_color(team: Team) -> Color {
    let [r, g, b] = team.srgb();
    Color::srgb(r, g, b)
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
struct Crosshair;
#[derive(Component)]
struct HitMarker;
#[derive(Component)]
struct DamageTint;
#[derive(Component)]
struct EliminatedPanel;
#[derive(Component)]
struct EliminatedBy;
#[derive(Component)]
struct RespawnText;
#[derive(Component)]
struct KillfeedList;

/// Green at full health, through yellow, to red when nearly out.
pub fn health_color(frac: f32) -> Color {
    Color::hsl(120.0 * frac.clamp(0.0, 1.0), 0.78, 0.47)
}

pub fn player_name(id: PlayerId, local: Option<PlayerId>) -> String {
    if Some(id) == local {
        "You".to_string()
    } else {
        format!("Player {}", id + 1)
    }
}

fn spawn_hud(
    mut commands: Commands,
    assets: Res<AssetServer>,
    net: Res<NetworkState>,
    loadout: Res<Loadout>,
) {
    let local = net.local_player_id.unwrap_or(0);
    let team = Team::of(local);
    let shield = match team {
        Team::Blue => "textures/ui/team_blue.png",
        Team::Orange => "textures/ui/team_orange.png",
    };

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
    ));

    // Health, bottom left.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(20.0),
                bottom: Val::Px(20.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                column_gap: Val::Px(12.0),
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|row| {
            row.spawn((
                ImageNode::new(assets.load(shield)),
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
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|col| {
                col.spawn((
                    Text::new(format!("HEALTH  |  {} TEAM", team.name().to_uppercase())),
                    TextFont::from_font_size(11.0),
                    TextColor(TEXT_MUTED),
                    Pickable::IGNORE,
                ));
                col.spawn((
                    Node {
                        width: Val::Px(260.0),
                        height: Val::Px(30.0),
                        padding: UiRect::all(Val::Px(6.0)),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|bar| {
                    // Track.
                    bar.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            border_radius: BorderRadius::all(Val::Px(7.0)),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                        Pickable::IGNORE,
                    ))
                    .with_children(|track| {
                        track.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                border_radius: BorderRadius::all(Val::Px(7.0)),
                                ..default()
                            },
                            BackgroundColor(health_color(1.0)),
                            Pickable::IGNORE,
                            HealthFill,
                        ));
                    });
                    // The pack's meter frame drawn over the bar.
                    bar.spawn((
                        ImageNode {
                            image: assets.load("textures/ui/meter_frame.png"),
                            // The pill inside the 64x64 texture, ends kept round.
                            rect: Some(Rect::new(0.0, 17.0, 64.0, 48.0)),
                            image_mode: NodeImageMode::Sliced(TextureSlicer {
                                border: BorderRect::all(15.0),
                                center_scale_mode: SliceScaleMode::Stretch,
                                sides_scale_mode: SliceScaleMode::Stretch,
                                max_corner_scale: 1.0,
                            }),
                            ..default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            right: Val::Px(0.0),
                            top: Val::Px(0.0),
                            bottom: Val::Px(0.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                });
            });
            row.spawn((
                Text::new("100"),
                TextFont::from_font_size(26.0),
                TextColor(TEXT_PRIMARY),
                Node {
                    min_width: Val::Px(48.0),
                    ..default()
                },
                Pickable::IGNORE,
                HealthText,
            ));
        });

    // Ammo, bottom right.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(20.0),
                bottom: Val::Px(20.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                column_gap: Val::Px(12.0),
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|row| {
            row.spawn((
                ImageNode::new(assets.load("textures/ui/ammo_ball.png")),
                Node {
                    width: Val::Px(34.0),
                    height: Val::Px(34.0),
                    ..default()
                },
                Pickable::IGNORE,
                AmmoIcon,
            ));
            row.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexEnd,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|col| {
                col.spawn((
                    Text::new(""),
                    TextFont::from_font_size(26.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                    AmmoText,
                ));
                col.spawn((
                    Text::new(loadout.display_name()),
                    TextFont::from_font_size(12.0),
                    TextColor(TEXT_MUTED),
                    Pickable::IGNORE,
                    AmmoSub,
                ));
            });
        });

    // Crosshair and hit marker, dead center.
    let crosshair = CROSSHAIRS[loadout.crosshair as usize].1;
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
        ));
        if marker {
            e.insert(Crosshair);
        } else {
            e.insert(HitMarker);
        }
    }

    // Elimination screen.
    commands
        .spawn((
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(24.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(10.0),
                ..default()
            },
            Pickable::IGNORE,
            EliminatedPanel,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|col| {
            col.spawn((
                ImageNode::new(assets.load("textures/ui/eliminated.png"))
                    .with_color(Color::srgb(1.0, 0.35, 0.35)),
                Node {
                    width: Val::Px(84.0),
                    height: Val::Px(84.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            col.spawn((
                Text::new("ELIMINATED"),
                TextFont::from_font_size(46.0),
                TextColor(Color::srgb(1.0, 0.42, 0.42)),
                Pickable::IGNORE,
            ));
            col.spawn((
                Text::new(""),
                TextFont::from_font_size(18.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
                EliminatedBy,
            ));
            col.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                    column_gap: Val::Px(10.0),
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(Val::Px(10.0)),
                    ..default()
                },
                BackgroundColor(PANEL),
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    ImageNode::new(assets.load("textures/ui/timer.png")),
                    Node {
                        width: Val::Px(24.0),
                        height: Val::Px(24.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Text::new(""),
                    TextFont::from_font_size(17.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                    RespawnText,
                ));
            });
        });

    // Killfeed, top right (under the hub pop-up's spot).
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(16.0),
            top: Val::Px(96.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            row_gap: Val::Px(6.0),
            ..default()
        },
        Pickable::IGNORE,
        KillfeedList,
        DespawnOnExit(AppState::InGame),
    ));
}

fn update_health(
    local_state: Res<LocalState>,
    combat: Res<Combat>,
    mut fill_q: Query<(&mut Node, &mut BackgroundColor), With<HealthFill>>,
    mut text_q: Query<(&mut Text, &mut TextColor), With<HealthText>>,
    mut tint_q: Query<&mut BackgroundColor, (With<DamageTint>, Without<HealthFill>)>,
) {
    let hp = local_state.0.health;
    let frac = hp / MAX_HEALTH;
    if let Ok((mut node, mut bg)) = fill_q.single_mut() {
        node.width = Val::Percent(frac * 100.0);
        bg.0 = health_color(frac);
    }
    if let Ok((mut text, mut color)) = text_q.single_mut() {
        let s = format!("{}", hp.ceil() as i32);
        if text.0 != s {
            text.0 = s;
        }
        color.0 = health_color(frac).lighter(0.15);
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

#[allow(clippy::too_many_arguments)]
fn update_ammo(
    assets: Res<AssetServer>,
    combat: Res<Combat>,
    loadout: Res<Loadout>,
    net: Res<NetworkState>,
    mut icon_q: Query<&mut ImageNode, With<AmmoIcon>>,
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
            None if combat.ammo == 0 => "EMPTY  |  R to reload".to_string(),
            None => loadout.display_name(),
        };
        if text.0 != s {
            text.0 = s;
        }
    }
}

fn update_center(
    combat: Res<Combat>,
    overlay: Res<Overlay>,
    local_state: Res<LocalState>,
    loadout: Res<Loadout>,
    assets: Res<AssetServer>,
    mut cross_q: Query<&mut ImageNode, (With<Crosshair>, Without<HitMarker>)>,
    mut hit_q: Query<&mut ImageNode, (With<HitMarker>, Without<Crosshair>)>,
) {
    let alive = local_state.0.health > 0.0;
    if let Ok(mut img) = cross_q.single_mut() {
        if loadout.is_changed() {
            img.image = assets.load(CROSSHAIRS[loadout.crosshair as usize].1);
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
        img.color = Color::srgb(1.0, 0.9, 0.35).with_alpha((combat.hitmarker / 0.25).min(1.0));
    }
}

fn update_eliminated(
    combat: Res<Combat>,
    local_state: Res<LocalState>,
    net: Res<NetworkState>,
    mut panel_q: Query<&mut Node, With<EliminatedPanel>>,
    mut by_q: Query<&mut Text, (With<EliminatedBy>, Without<RespawnText>)>,
    mut respawn_q: Query<&mut Text, (With<RespawnText>, Without<EliminatedBy>)>,
) {
    let respawn = combat.respawn_in();
    if let Ok(mut node) = panel_q.single_mut() {
        let want = if respawn.is_some() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
    let Some(left) = respawn else {
        return;
    };
    if let Ok(mut text) = by_q.single_mut() {
        let killer = local_state.0.killer;
        let s = format!("Splatted by {}", player_name(killer, net.local_player_id));
        if text.0 != s {
            text.0 = s;
        }
    }
    if let Ok(mut text) = respawn_q.single_mut() {
        let s = format!("Respawning in {}", left.ceil() as i32);
        if text.0 != s {
            text.0 = s;
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
) {
    let Ok(list) = list_q.single() else {
        return;
    };
    let now: Vec<_> = feed
        .entries
        .iter()
        .map(|(k, v, t)| (*k, *v, t.to_bits()))
        .collect();
    if *shown == now {
        return;
    }
    *shown = now;
    commands.entity(list).despawn_related::<Children>();
    let local = net.local_player_id;
    let entries: Vec<_> = feed.entries.iter().rev().take(5).copied().collect();
    commands.entity(list).with_children(|list| {
        for (killer, victim, _) in entries {
            list.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                    column_gap: Val::Px(8.0),
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(PANEL),
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new(player_name(killer, local)),
                    TextFont::from_font_size(14.0),
                    TextColor(team_color(Team::of(killer)).lighter(0.15)),
                    Pickable::IGNORE,
                ));
                row.spawn((
                    ImageNode::new(assets.load("textures/ui/splat_icon.png"))
                        .with_color(team_color(Team::of(killer))),
                    Node {
                        width: Val::Px(22.0),
                        height: Val::Px(22.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Text::new(player_name(victim, local)),
                    TextFont::from_font_size(14.0),
                    TextColor(team_color(Team::of(victim)).lighter(0.15)),
                    Pickable::IGNORE,
                ));
            });
        }
    });
}
