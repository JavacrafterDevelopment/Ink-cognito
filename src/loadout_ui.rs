//! The loadout screen: pick a paintball marker (body and each attachment) or a
//! water gun, see it turn in a 3D preview, compare stats and pick a crosshair.
//! Opens from the main menu and from the Escape menu; saved between runs.

use crate::{
    Overlay,
    loadout::{
        CROSSHAIRS, GunAssets, GunKind, GunModel, Loadout, Slot, WeaponStats, save_loadout,
        spawn_gun_parts,
    },
    menu::{BG_CARD, BORDER_SUBTLE, ButtonStyle, TEXT_MUTED, TEXT_PRIMARY, spawn_button},
};
use bevy::{
    camera::RenderTarget, picking::prelude::*, prelude::*, render::render_resource::TextureFormat,
};

pub struct LoadoutUiPlugin;

impl Plugin for LoadoutUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_preview)
            .add_observer(on_loadout_click)
            .add_systems(Update, (show_loadout_screen, spin_preview));
    }
}

/// The preview stage sits far outside the field, beyond the main camera's reach.
const STAGE: Vec3 = Vec3::new(4000.0, 4000.0, 4000.0);
const PREVIEW_SIZE: UVec2 = UVec2::new(800, 450);

#[derive(Resource)]
struct LoadoutScreen {
    slot: Slot,
    image: Handle<Image>,
}

#[derive(Component)]
struct LoadoutRoot;

#[derive(Component)]
struct PreviewCamera;

#[derive(Component)]
struct PreviewGun;

#[derive(Component, Clone, Copy)]
enum LoadoutAction {
    Kind(GunKind),
    Slot(Slot),
    Pick(Slot, u8),
    Crosshair(u8),
    Done,
}

fn setup_preview(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
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
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.07, 0.09, 0.13)),
            ..default()
        },
        RenderTarget::Image(image.clone().into()),
        Transform::from_translation(STAGE + Vec3::new(0.0, 0.16, 0.9))
            .looking_at(STAGE + Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        PreviewCamera,
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
            color: Color::srgb(0.6, 0.8, 1.0),
            ..default()
        },
        Transform::from_translation(STAGE + Vec3::new(-1.5, 0.4, -1.2)),
    ));
    commands.spawn((
        Transform::from_translation(STAGE),
        Visibility::default(),
        GunModel { packed: u32::MAX },
        PreviewGun,
    ));
    commands.insert_resource(LoadoutScreen {
        slot: Slot::Body,
        image,
    });
}

fn spin_preview(
    mut commands: Commands,
    time: Res<Time>,
    overlay: Res<Overlay>,
    loadout: Res<Loadout>,
    guns: Option<Res<GunAssets>>,
    mut gun_q: Query<(Entity, &mut Transform, &mut GunModel), With<PreviewGun>>,
) {
    if !overlay.loadout {
        return;
    }
    let Ok((entity, mut tf, mut gun)) = gun_q.single_mut() else {
        return;
    };
    tf.rotation = Quat::from_rotation_y(time.elapsed_secs() * 0.6) * Quat::from_rotation_x(0.0);
    let packed = loadout.pack();
    if gun.packed != packed
        && let Some(guns) = guns
    {
        gun.packed = packed;
        spawn_gun_parts(&mut commands, &guns, entity, &loadout);
    }
}

fn on_loadout_click(
    click: On<PointerClick>,
    actions: Query<&LoadoutAction>,
    mut loadout: ResMut<Loadout>,
    mut screen: ResMut<LoadoutScreen>,
    mut overlay: ResMut<Overlay>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };
    match *action {
        LoadoutAction::Kind(kind) => {
            loadout.kind = kind;
            screen.slot = match kind {
                GunKind::Water => Slot::Water,
                GunKind::Paintball => Slot::Body,
            };
        }
        LoadoutAction::Slot(slot) => screen.slot = slot,
        LoadoutAction::Pick(slot, i) => {
            loadout.set(slot, i);
            if slot == Slot::Water {
                loadout.kind = GunKind::Water;
            } else {
                loadout.kind = GunKind::Paintball;
            }
        }
        LoadoutAction::Crosshair(i) => loadout.crosshair = i,
        LoadoutAction::Done => overlay.loadout = false,
    }
}

#[allow(clippy::too_many_arguments)]
fn show_loadout_screen(
    mut commands: Commands,
    overlay: Res<Overlay>,
    loadout: Res<Loadout>,
    mut screen: ResMut<LoadoutScreen>,
    assets: Res<AssetServer>,
    root_q: Query<Entity, With<LoadoutRoot>>,
    mut camera_q: Query<&mut Camera, With<PreviewCamera>>,
    mut was_open: Local<bool>,
) {
    if let Ok(mut camera) = camera_q.single_mut()
        && camera.is_active != overlay.loadout
    {
        camera.is_active = overlay.loadout;
    }
    let open = overlay.loadout;
    if !open {
        if *was_open {
            *was_open = false;
            save_loadout(&loadout);
            for root in &root_q {
                commands.entity(root).despawn();
            }
        }
        return;
    }
    let screen_changed = screen.is_changed();
    if *was_open && !loadout.is_changed() && !screen_changed && !root_q.is_empty() {
        return;
    }
    if !*was_open {
        // Open on the right tab for the current gun.
        screen.slot = match loadout.kind {
            GunKind::Water => Slot::Water,
            GunKind::Paintball if screen.slot == Slot::Water => Slot::Body,
            GunKind::Paintball => screen.slot,
        };
    }
    *was_open = true;
    for root in &root_q {
        commands.entity(root).despawn();
    }
    spawn_screen(&mut commands, &loadout, &screen, &assets);
}

fn style_for(selected: bool) -> ButtonStyle {
    if selected {
        ButtonStyle::PRIMARY
    } else {
        ButtonStyle::SECONDARY
    }
}

fn label(parent: &mut ChildSpawnerCommands, text: &str) {
    parent.spawn((
        Text::new(text),
        TextFont::from_font_size(12.0),
        TextColor(TEXT_MUTED),
        Node {
            margin: UiRect::top(Val::Px(6.0)),
            ..default()
        },
        Pickable::IGNORE,
    ));
}

fn spawn_screen(
    commands: &mut Commands,
    loadout: &Loadout,
    screen: &LoadoutScreen,
    assets: &AssetServer,
) {
    let slot = match (loadout.kind, screen.slot) {
        (GunKind::Water, _) => Slot::Water,
        (GunKind::Paintball, Slot::Water) => Slot::Body,
        (_, s) => s,
    };
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(24.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(14.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.06, 0.94)),
            GlobalZIndex(30),
            LoadoutRoot,
        ))
        .with_children(|root| {
            // Header.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(16.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    ImageNode::new(assets.load("textures/ui/team_shield_white.png")),
                    Node {
                        width: Val::Px(34.0),
                        height: Val::Px(34.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        flex_grow: 1.0,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|col| {
                    col.spawn((
                        Text::new("LOADOUT"),
                        TextFont::from_font_size(30.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    col.spawn((
                        Text::new("Build a paintball marker from parts, or grab a water gun. Saved automatically."),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                    ));
                });
                row.spawn((
                    Node {
                        width: Val::Px(160.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|wrap| {
                    spawn_button(wrap, "Done", LoadoutAction::Done, ButtonStyle::PRIMARY, 44.0, 18.0, false);
                });
            });

            // Body: slots | options | preview + stats.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_grow: 1.0,
                    column_gap: Val::Px(18.0),
                    min_height: Val::Px(0.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|body| {
                // Slots.
                body.spawn(column(220.0)).with_children(|col| {
                    label(col, "GUN TYPE");
                    spawn_button(
                        col,
                        "Paintball Marker",
                        LoadoutAction::Kind(GunKind::Paintball),
                        style_for(loadout.kind == GunKind::Paintball),
                        38.0,
                        15.0,
                        false,
                    );
                    spawn_button(
                        col,
                        "Water Gun",
                        LoadoutAction::Kind(GunKind::Water),
                        style_for(loadout.kind == GunKind::Water),
                        38.0,
                        15.0,
                        false,
                    );
                    label(col, "PARTS");
                    let slots: Vec<Slot> = match loadout.kind {
                        GunKind::Paintball => Slot::PAINTBALL.to_vec(),
                        GunKind::Water => vec![Slot::Water],
                    };
                    for s in slots {
                        let current = s.parts()[loadout.get(s) as usize].name;
                        spawn_button(
                            col,
                            &format!("{}:  {current}", s.label()),
                            LoadoutAction::Slot(s),
                            style_for(s == slot),
                            36.0,
                            14.0,
                            false,
                        );
                    }
                    label(col, "CROSSHAIR");
                    col.spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(10.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|row| {
                        for (i, (_, path)) in CROSSHAIRS.iter().enumerate() {
                            let style = style_for(loadout.crosshair == i as u8);
                            row.spawn((
                                Node {
                                    width: Val::Px(52.0),
                                    height: Val::Px(52.0),
                                    padding: UiRect::all(Val::Px(10.0)),
                                    border_radius: BorderRadius::all(Val::Px(10.0)),
                                    ..default()
                                },
                                BackgroundColor(style.normal),
                                style,
                                LoadoutAction::Crosshair(i as u8),
                            ))
                            .with_children(|b| {
                                b.spawn((
                                    ImageNode::new(assets.load(*path)),
                                    Node {
                                        width: Val::Percent(100.0),
                                        height: Val::Percent(100.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                            });
                        }
                    });
                });

                // Options for the chosen slot.
                body.spawn(column(300.0)).with_children(|col| {
                    label(col, &slot.label().to_uppercase());
                    let chosen = loadout.get(slot);
                    for (i, part) in slot.parts().iter().enumerate() {
                        let style = style_for(i as u8 == chosen);
                        col.spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                                row_gap: Val::Px(2.0),
                                border_radius: BorderRadius::all(Val::Px(10.0)),
                                ..default()
                            },
                            BackgroundColor(style.normal),
                            style,
                            LoadoutAction::Pick(slot, i as u8),
                        ))
                        .with_children(|b| {
                            b.spawn((
                                Text::new(part.name),
                                TextFont::from_font_size(16.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                            ));
                            b.spawn((
                                Text::new(part.blurb),
                                TextFont::from_font_size(12.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });

                // Preview and stats.
                body.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        flex_grow: 1.0,
                        flex_basis: Val::Px(0.0),
                        row_gap: Val::Px(10.0),
                        padding: UiRect::all(Val::Px(14.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(14.0)),
                        ..default()
                    },
                    BackgroundColor(BG_CARD),
                    BorderColor::all(BORDER_SUBTLE),
                    Pickable::IGNORE,
                ))
                .with_children(|col| {
                    col.spawn((
                        ImageNode::new(screen.image.clone()),
                        Node {
                            width: Val::Percent(100.0),
                            max_height: Val::Percent(50.0),
                            aspect_ratio: Some(PREVIEW_SIZE.x as f32 / PREVIEW_SIZE.y as f32),
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                    col.spawn((
                        Text::new(loadout.display_name()),
                        TextFont::from_font_size(20.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    for (name, value, text) in stat_rows(&loadout.stats()) {
                        stat_bar(col, name, value, &text);
                    }
                });
            });
        });
}

fn column(width: f32) -> impl Bundle {
    (
        Node {
            width: Val::Px(width),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// (name, 0..=1 bar, readout).
fn stat_rows(s: &WeaponStats) -> Vec<(&'static str, f32, String)> {
    let rate = 1.0 / s.fire_interval;
    vec![
        ("Damage", s.damage / 45.0, format!("{:.0}", s.damage)),
        ("Fire rate", rate / 20.0, format!("{rate:.1}/s")),
        (
            "Accuracy",
            1.0 - s.spread / 0.05,
            format!("{:.0}%", (1.0 - s.spread / 0.05).clamp(0.0, 1.0) * 100.0),
        ),
        ("Velocity", s.speed / 105.0, format!("{:.0} m/s", s.speed)),
        (
            "Capacity",
            s.capacity as f32 / 150.0,
            format!("{}", s.capacity),
        ),
        (
            "Reload",
            1.0 - (s.reload - 1.0) / 2.6,
            format!("{:.1}s", s.reload),
        ),
        ("Zoom", (s.zoom - 1.0) / 1.6, format!("{:.2}x", s.zoom)),
        (
            "Mobility",
            (s.mobility - 0.8) / 0.3,
            format!("{:.0}%", s.mobility * 100.0),
        ),
    ]
}

fn stat_bar(parent: &mut ChildSpawnerCommands, name: &str, value: f32, text: &str) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(name),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_MUTED),
                Node {
                    width: Val::Px(84.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            row.spawn((
                Node {
                    flex_grow: 1.0,
                    height: Val::Px(8.0),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.08)),
                Pickable::IGNORE,
            ))
            .with_children(|track| {
                track.spawn((
                    Node {
                        width: Val::Percent(value.clamp(0.04, 1.0) * 100.0),
                        height: Val::Percent(100.0),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.95, 0.76, 0.19)),
                    Pickable::IGNORE,
                ));
            });
            row.spawn((
                Text::new(text),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_PRIMARY),
                Node {
                    width: Val::Px(70.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}
