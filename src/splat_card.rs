//! The "splatted by" card: while we're out, who got us and what they carry,
//! with their gun turning in a little 3D preview (the same V2 parts the
//! loadout screen shows), its parts listed, and how much health they had
//! left. Bots have loadouts like anyone else, so it works for them too.
//!
//! Everything comes from the shooter's `PlayerState` (the packed loadout rides
//! along in it), so nothing extra goes over the network.

use crate::{
    AppState,
    combat::{Combat, LocalState},
    hud::{HudPart, TEXT_MUTED, TEXT_PRIMARY, player_color},
    loadout::{GunAssets, GunKind, GunModel, Loadout, SKINS, Slot, WATER_GUNS, spawn_gun_parts},
    net::NetworkState,
    protocol::{Phase, PlayerId},
    ui_kit::{KIT_YELLOW, UiKit, kit, sliced},
};
use bevy::{camera::RenderTarget, prelude::*, render::render_resource::TextureFormat};

pub struct SplatCardPlugin;

impl Plugin for SplatCardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CardState>()
            .add_systems(Startup, setup_stage)
            .add_systems(OnEnter(AppState::InGame), spawn_card)
            .add_systems(
                Update,
                update_card
                    .after(crate::world::WorldSystems::Net)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), stop_card);
    }
}

/// The card's preview stage, far from the field and from the loadout
/// screen's stage (at +x).
const STAGE: Vec3 = Vec3::new(-4000.0, 4000.0, 4000.0);
const PREVIEW_SIZE: UVec2 = UVec2::new(480, 270);

#[derive(Resource)]
struct CardStage {
    image: Handle<Image>,
}

#[derive(Component)]
struct CardCamera;

#[derive(Component)]
struct CardGun;

#[derive(Component)]
struct CardRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum CardText {
    Name,
    Gun,
    Parts,
    Footer,
}

/// Who splatted us last, as they were at that moment.
#[derive(Debug, Clone, PartialEq)]
struct Shooter {
    id: PlayerId,
    name: String,
    loadout: Loadout,
    health: f32,
    distance: Option<f32>,
}

#[derive(Resource, Default)]
struct CardState {
    /// Our deaths counter last frame (None until the first look).
    seen_deaths: Option<u32>,
    shooter: Option<Shooter>,
}

fn setup_stage(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = images.add(Image::new_target_texture(
        PREVIEW_SIZE.x,
        PREVIEW_SIZE.y,
        TextureFormat::Bgra8UnormSrgb,
        None,
    ));
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: false,
            order: -2,
            clear_color: ClearColorConfig::Custom(Color::srgba(0.05, 0.07, 0.11, 1.0)),
            ..default()
        },
        RenderTarget::Image(image.clone().into()),
        Transform::from_translation(STAGE + Vec3::new(0.0, 0.14, 0.82))
            .looking_at(STAGE + Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        AmbientLight {
            color: Color::srgb(0.88, 0.93, 1.0),
            brightness: 380.0,
            ..default()
        },
        CardCamera,
    ));
    commands.spawn((
        PointLight {
            intensity: 400_000.0,
            range: 12.0,
            ..default()
        },
        Transform::from_translation(STAGE + Vec3::new(1.2, 1.8, 1.6)),
    ));
    commands.spawn((
        PointLight {
            intensity: 150_000.0,
            range: 12.0,
            color: Color::srgb(1.0, 0.75, 0.55),
            ..default()
        },
        Transform::from_translation(STAGE + Vec3::new(-1.5, 0.4, -1.2)),
    ));
    commands.spawn((
        Transform::from_translation(STAGE),
        Visibility::default(),
        GunModel { packed: u32::MAX },
        CardGun,
    ));
    commands.insert_resource(CardStage { image });
}

/// The parts worth naming on the card: (slot, part). Empty slots ("None")
/// and the factory paint are left out; a water gun has none.
fn loadout_parts(loadout: &Loadout) -> Vec<(&'static str, &'static str)> {
    if loadout.kind == GunKind::Water {
        return Vec::new();
    }
    Slot::PAINTBALL
        .into_iter()
        .filter(|&s| s != Slot::Body)
        .filter_map(|slot| {
            let part = slot.parts().get(loadout.get(slot) as usize)?;
            let empty = part.name == "None" || (slot == Slot::Skin && part.name == SKINS[0].name);
            (!empty).then_some((slot.label(), part.name))
        })
        .collect()
}

/// The card's parts text: one "Slot  Part" per line, or a water gun's blurb.
fn parts_text(loadout: &Loadout) -> String {
    if loadout.kind == GunKind::Water {
        return WATER_GUNS
            .get(loadout.water as usize)
            .map_or("", |w| w.blurb)
            .to_string();
    }
    let parts = loadout_parts(loadout);
    if parts.is_empty() {
        return "Bare body, no attachments".to_string();
    }
    parts
        .iter()
        .map(|(slot, part)| format!("{slot}: {part}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn footer_text(shooter: &Shooter) -> String {
    let hp = shooter.health.clamp(0.0, 100.0).ceil() as u32;
    let left = if hp == 0 {
        "They went down too".to_string()
    } else {
        format!("{hp} HP left")
    };
    match shooter.distance {
        Some(d) => format!("{left}  |  {d:.0} m away"),
        None => left,
    }
}

fn spawn_card(
    mut commands: Commands,
    assets: Res<AssetServer>,
    fonts: Res<UiKit>,
    stage: Res<CardStage>,
    mut card: ResMut<CardState>,
) {
    *card = CardState::default();
    commands
        .spawn((
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                bottom: Val::Px(112.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Pickable::IGNORE,
            CardRoot,
            HudPart,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    padding: UiRect::all(Val::Px(14.0)),
                    column_gap: Val::Px(16.0),
                    ..default()
                },
                sliced(kit(&assets, "panels/panel_glass.png"), 28.0),
                Pickable::IGNORE,
            ))
            .with_children(|card| {
                card.spawn((
                    ImageNode::new(stage.image.clone()),
                    Node {
                        width: Val::Px(288.0),
                        height: Val::Px(162.0),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(3.0),
                        min_width: Val::Px(240.0),
                        max_width: Val::Px(320.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|col| {
                    col.spawn((
                        Text::new("SPLATTED BY"),
                        TextFont::from_font_size(12.0),
                        TextColor(KIT_YELLOW),
                        Pickable::IGNORE,
                    ));
                    col.spawn((
                        Text::new(""),
                        fonts.heading(32.0),
                        TextColor(TEXT_PRIMARY),
                        TextShadow::default(),
                        Pickable::IGNORE,
                        CardText::Name,
                    ));
                    col.spawn((
                        Text::new(""),
                        TextFont::from_font_size(16.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                        CardText::Gun,
                    ));
                    col.spawn((
                        Text::new(""),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_MUTED),
                        Node {
                            margin: UiRect::vertical(Val::Px(4.0)),
                            ..default()
                        },
                        Pickable::IGNORE,
                        CardText::Parts,
                    ));
                    col.spawn((
                        Text::new(""),
                        TextFont::from_font_size(13.0),
                        TextColor(KIT_YELLOW),
                        Pickable::IGNORE,
                        CardText::Footer,
                    ));
                });
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn update_card(
    mut commands: Commands,
    time: Res<Time>,
    net: Res<NetworkState>,
    local: Res<LocalState>,
    combat: Res<Combat>,
    guns: Option<Res<GunAssets>>,
    mut card: ResMut<CardState>,
    mut root_q: Query<&mut Node, With<CardRoot>>,
    mut texts: Query<(&mut Text, &mut TextColor, &CardText)>,
    mut camera_q: Query<&mut Camera, With<CardCamera>>,
    mut gun_q: Query<(Entity, &mut Transform, &mut GunModel), With<CardGun>>,
) {
    let me = net.local_player_id;

    // A fresh elimination: remember who did it and what they carried.
    let deaths = local.0.deaths;
    let fresh = card
        .seen_deaths
        .is_some_and(|seen| deaths != seen && deaths.wrapping_sub(seen) < 10);
    card.seen_deaths = Some(deaths);
    if fresh {
        let killer = local.0.killer;
        card.shooter = (Some(killer) != me)
            .then(|| net.players.get(&killer))
            .flatten()
            .map(|p| Shooter {
                id: killer,
                name: net.display_name(killer),
                loadout: Loadout::unpack(p.state.loadout),
                health: p.state.health,
                distance: combat
                    .death_cam
                    .map(|feet| feet.distance(Vec3::from_array(p.position))),
            });
        if let Some(shooter) = &card.shooter {
            for (mut text, mut color, part) in &mut texts {
                let (value, tint) = match part {
                    CardText::Name => (
                        shooter.name.clone(),
                        Some(player_color(&net.match_state, shooter.id)),
                    ),
                    CardText::Gun => (shooter.loadout.display_name(), None),
                    CardText::Parts => (parts_text(&shooter.loadout), None),
                    CardText::Footer => (footer_text(shooter), None),
                };
                text.0 = value;
                if let Some(tint) = tint {
                    color.0 = tint;
                }
            }
            if let (Ok((entity, _, mut gun)), Some(guns)) = (gun_q.single_mut(), guns.as_deref()) {
                gun.packed = shooter.loadout.pack();
                spawn_gun_parts(&mut commands, guns, entity, &shooter.loadout);
            }
        }
    }

    let ms = &net.match_state;
    let show = card.shooter.is_some()
        && local.0.health <= 0.0
        && combat.death_cam.is_some()
        && !matches!(ms.phase, Phase::RoundOver | Phase::MatchOver);
    if let Ok(mut node) = root_q.single_mut() {
        let want = if show { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if let Ok(mut camera) = camera_q.single_mut()
        && camera.is_active != show
    {
        camera.is_active = show;
    }
    if show && let Ok((_, mut tf, _)) = gun_q.single_mut() {
        tf.rotation = Quat::from_rotation_y(time.elapsed_secs() * 0.7);
    }
}

fn stop_card(mut card: ResMut<CardState>, mut camera_q: Query<&mut Camera, With<CardCamera>>) {
    card.shooter = None;
    if let Ok(mut camera) = camera_q.single_mut() {
        camera.is_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_marker_lists_its_parts() {
        let parts = loadout_parts(&Loadout::default());
        // Vanta with a Stub barrel, Gravity hopper, iron sight, fixed stock,
        // small tank and no foregrip or camo.
        assert!(parts.contains(&("Barrel", "Stub")));
        assert!(parts.contains(&("Hopper", "Gravity")));
        assert!(parts.iter().all(|(_, p)| *p != "None"));
        assert!(parts.iter().all(|(s, _)| *s != "Skin" && *s != "Body"));
    }

    #[test]
    fn camo_shows_and_survives_the_wire() {
        let mut l = Loadout {
            skin: 2,
            barrel: 1,
            ..default()
        };
        l = Loadout::unpack(l.pack());
        let parts = loadout_parts(&l);
        assert!(parts.contains(&("Skin", SKINS[2].name)));
        assert!(parts.contains(&("Barrel", "Long")));
        assert!(parts_text(&l).contains("Barrel: Long"));
    }

    #[test]
    fn water_guns_show_their_blurb() {
        let l = Loadout {
            kind: GunKind::Water,
            water: 0,
            ..default()
        };
        assert!(loadout_parts(&l).is_empty());
        assert_eq!(parts_text(&l), WATER_GUNS[0].blurb);
    }

    #[test]
    fn footer_says_health_and_range() {
        let mut s = Shooter {
            id: 101,
            name: "BOT Viper".into(),
            loadout: Loadout::default(),
            health: 41.2,
            distance: Some(22.6),
        };
        assert_eq!(footer_text(&s), "42 HP left  |  23 m away");
        s.health = 0.0;
        s.distance = None;
        assert_eq!(footer_text(&s), "They went down too");
    }
}
