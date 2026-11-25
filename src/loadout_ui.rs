//! The loadout screen: Call of Duty-style custom named loadouts where each
//! loadout has its own paintball marker (body and attachments) or water gun,
//! with a 3D preview and stat comparison.
//!
//! Opens as a top-bar tab in the Main Menu (keeping the top bar clickable) and
//! as a full-screen overlay from the in-game Escape pause menu.

use crate::{
    AppState, Overlay,
    loadout::{
        GunAssets, GunKind, GunModel, Loadout, LoadoutBook, MAX_LOADOUT_NAME_LEN, MAX_LOADOUTS,
        Slot, WeaponStats, save_loadout, save_loadout_book, spawn_gun_parts,
    },
    menu::{
        BG_CARD, BORDER_SUBTLE, ButtonStyle, MenuScreen, MenuState, TEXT_MUTED, TEXT_PRIMARY,
        TOP_BAR_HEIGHT, spawn_button,
    },
    progression::{Progress, lock_loadout},
    ui_kit::{KIT_YELLOW, kit},
};
use bevy::{
    camera::RenderTarget,
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    picking::prelude::*,
    prelude::*,
    render::render_resource::TextureFormat,
};

pub struct LoadoutUiPlugin;

impl Plugin for LoadoutUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_preview)
            .add_observer(on_loadout_click)
            .add_systems(
                Update,
                (
                    handle_rename_input,
                    show_loadout_screen,
                    spin_preview,
                ),
            );
    }
}

/// The preview stage sits far outside the field, beyond the main camera's reach.
const STAGE: Vec3 = Vec3::new(4000.0, 4000.0, 4000.0);
const PREVIEW_SIZE: UVec2 = UVec2::new(800, 450);

#[derive(Resource)]
pub struct LoadoutScreen {
    slot: Slot,
    image: Handle<Image>,
    renaming: bool,
    rename_draft: String,
}

impl LoadoutScreen {
    pub fn is_renaming(&self) -> bool {
        self.renaming
    }
}

#[derive(Component)]
struct LoadoutRoot;

#[derive(Component)]
struct PreviewCamera;

#[derive(Component)]
struct PreviewGun;

#[derive(Component, Clone, Copy)]
enum LoadoutAction {
    SelectPreset(usize),
    AddPreset,
    DeletePreset,
    ToggleRename,
    CancelRename,
    Kind(GunKind),
    Slot(Slot),
    Pick(Slot, u8),
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
        renaming: false,
        rename_draft: String::new(),
    });
}

fn is_loadout_open(state: &State<AppState>, menu: &MenuState, overlay: &Overlay) -> (bool, bool) {
    let embedded = *state.get() == AppState::MainMenu && menu.screen == MenuScreen::Loadout;
    (embedded || overlay.loadout, embedded)
}

#[allow(clippy::too_many_arguments)]
fn spin_preview(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<State<AppState>>,
    menu: Res<MenuState>,
    overlay: Res<Overlay>,
    loadout: Res<Loadout>,
    guns: Option<Res<GunAssets>>,
    mut gun_q: Query<(Entity, &mut Transform, &mut GunModel), With<PreviewGun>>,
) {
    let (open, _) = is_loadout_open(&state, &menu, &overlay);
    if !open {
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

fn handle_rename_input(
    mut key_events: MessageReader<KeyboardInput>,
    state: Res<State<AppState>>,
    menu: Res<MenuState>,
    overlay: Res<Overlay>,
    mut screen: ResMut<LoadoutScreen>,
    mut book: ResMut<LoadoutBook>,
) {
    let (open, _) = is_loadout_open(&state, &menu, &overlay);
    if !open || !screen.renaming {
        key_events.clear();
        return;
    }

    for event in key_events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        match &event.logical_key {
            Key::Backspace => {
                screen.rename_draft.pop();
            }
            Key::Enter => {
                let draft = screen.rename_draft.clone();
                book.rename_active(&draft);
                save_loadout_book(&book);
                screen.renaming = false;
            }
            Key::Escape => {
                screen.renaming = false;
                screen.rename_draft.clear();
            }
            _ => {
                if let Some(text) = &event.text {
                    for ch in text.chars() {
                        if !ch.is_control()
                            && screen.rename_draft.chars().count() < MAX_LOADOUT_NAME_LEN
                        {
                            screen.rename_draft.push(ch);
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn on_loadout_click(
    click: On<PointerClick>,
    actions: Query<&LoadoutAction>,
    mut loadout: ResMut<Loadout>,
    mut book: ResMut<LoadoutBook>,
    mut screen: ResMut<LoadoutScreen>,
    mut overlay: ResMut<Overlay>,
    progress: Res<Progress>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };
    match *action {
        LoadoutAction::SelectPreset(idx) => {
            screen.renaming = false;
            book.select(idx);
            let mut active = book.active_loadout();
            lock_loadout(&mut active, &progress);
            book.active_preset_mut().loadout = active;
            *loadout = active;
            screen.slot = match loadout.kind {
                GunKind::Water => Slot::Water,
                GunKind::Paintball if screen.slot == Slot::Water => Slot::Body,
                GunKind::Paintball => screen.slot,
            };
            save_loadout_book(&book);
            save_loadout(&loadout);
        }
        LoadoutAction::AddPreset => {
            screen.renaming = false;
            if book.add_preset().is_some() {
                let mut active = book.active_loadout();
                lock_loadout(&mut active, &progress);
                book.active_preset_mut().loadout = active;
                *loadout = active;
                screen.slot = Slot::Body;
                save_loadout_book(&book);
                save_loadout(&loadout);
            }
        }
        LoadoutAction::DeletePreset => {
            screen.renaming = false;
            if book.delete_active() {
                let mut active = book.active_loadout();
                lock_loadout(&mut active, &progress);
                book.active_preset_mut().loadout = active;
                *loadout = active;
                screen.slot = match loadout.kind {
                    GunKind::Water => Slot::Water,
                    GunKind::Paintball if screen.slot == Slot::Water => Slot::Body,
                    GunKind::Paintball => screen.slot,
                };
                save_loadout_book(&book);
                save_loadout(&loadout);
            }
        }
        LoadoutAction::ToggleRename => {
            if screen.renaming {
                let draft = screen.rename_draft.clone();
                book.rename_active(&draft);
                save_loadout_book(&book);
                screen.renaming = false;
            } else {
                screen.rename_draft = book.active_preset().name.clone();
                screen.renaming = true;
            }
        }
        LoadoutAction::CancelRename => {
            screen.renaming = false;
            screen.rename_draft.clear();
        }
        LoadoutAction::Kind(kind) => {
            loadout.kind = kind;
            book.active_preset_mut().loadout = *loadout;
            screen.slot = match kind {
                GunKind::Water => Slot::Water,
                GunKind::Paintball => Slot::Body,
            };
            save_loadout_book(&book);
            save_loadout(&loadout);
        }
        LoadoutAction::Slot(slot) => screen.slot = slot,
        LoadoutAction::Pick(slot, i) => {
            if !progress.unlocked(loadout.body, slot, i) {
                return;
            }
            loadout.set(slot, i);
            // A new body starts with only what it has unlocked.
            lock_loadout(&mut loadout, &progress);
            if slot == Slot::Water {
                loadout.kind = GunKind::Water;
            } else {
                loadout.kind = GunKind::Paintball;
            }
            book.active_preset_mut().loadout = *loadout;
            save_loadout_book(&book);
            save_loadout(&loadout);
        }
        LoadoutAction::Done => {
            screen.renaming = false;
            save_loadout_book(&book);
            save_loadout(&loadout);
            overlay.loadout = false;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn show_loadout_screen(
    mut commands: Commands,
    state: Res<State<AppState>>,
    menu: Res<MenuState>,
    overlay: Res<Overlay>,
    loadout: Res<Loadout>,
    book: Res<LoadoutBook>,
    mut screen: ResMut<LoadoutScreen>,
    assets: Res<AssetServer>,
    progress: Res<Progress>,
    root_q: Query<Entity, With<LoadoutRoot>>,
    mut camera_q: Query<&mut Camera, With<PreviewCamera>>,
    mut was_open: Local<bool>,
    mut was_embedded: Local<bool>,
) {
    let (open, embedded) = is_loadout_open(&state, &menu, &overlay);
    if let Ok(mut camera) = camera_q.single_mut()
        && camera.is_active != open
    {
        camera.is_active = open;
    }
    if !open {
        if *was_open {
            *was_open = false;
            *was_embedded = false;
            screen.renaming = false;
            save_loadout_book(&book);
            save_loadout(&loadout);
            for root in &root_q {
                commands.entity(root).despawn();
            }
        }
        return;
    }
    let changed = loadout.is_changed()
        || book.is_changed()
        || screen.is_changed()
        || *was_embedded != embedded;
    if *was_open && !changed && !root_q.is_empty() {
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
    *was_embedded = embedded;
    for root in &root_q {
        commands.entity(root).despawn();
    }
    spawn_screen(&mut commands, embedded, &loadout, &book, &screen, &assets, &progress);
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

#[allow(clippy::too_many_arguments)]
fn spawn_screen(
    commands: &mut Commands,
    embedded: bool,
    loadout: &Loadout,
    book: &LoadoutBook,
    screen: &LoadoutScreen,
    assets: &AssetServer,
    progress: &Progress,
) {
    let slot = match (loadout.kind, screen.slot) {
        (GunKind::Water, _) => Slot::Water,
        (GunKind::Paintball, Slot::Water) => Slot::Body,
        (_, s) => s,
    };

    let root_node = if embedded {
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(TOP_BAR_HEIGHT),
            bottom: Val::Px(0.0),
            padding: UiRect::axes(Val::Px(32.0), Val::Px(18.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(12.0),
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(24.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(12.0),
            ..default()
        }
    };

    let root_bg = if embedded {
        Color::NONE
    } else {
        Color::srgba(0.02, 0.03, 0.06, 0.985)
    };
    let z = if embedded { 1 } else { 30 };

    commands
        .spawn((root_node, BackgroundColor(root_bg), GlobalZIndex(z), LoadoutRoot))
        .with_children(|root| {
            // CoD-style custom loadout class tabs strip.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    row_gap: Val::Px(8.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|strip| {
                for (idx, preset) in book.presets.iter().enumerate() {
                    let selected = idx == book.active;
                    let style = style_for(selected);
                    strip
                        .spawn((
                            Node {
                                min_width: Val::Px(124.0),
                                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(2.0),
                                border_radius: BorderRadius::all(Val::Px(10.0)),
                                ..default()
                            },
                            BackgroundColor(style.normal),
                            style,
                            LoadoutAction::SelectPreset(idx),
                        ))
                        .with_children(|tab| {
                            tab.spawn((
                                Text::new(preset.name.clone()),
                                TextFont::from_font_size(14.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                            ));
                            tab.spawn((
                                Text::new(preset.loadout.display_name()),
                                TextFont::from_font_size(11.0),
                                TextColor(if selected {
                                    Color::srgb(0.15, 0.17, 0.20)
                                } else {
                                    TEXT_MUTED
                                }),
                                Pickable::IGNORE,
                            ));
                        });
                }
                if book.presets.len() < MAX_LOADOUTS {
                    strip
                        .spawn((
                            Node {
                                width: Val::Px(110.0),
                                flex_direction: FlexDirection::Column,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|wrap| {
                            spawn_button(
                                wrap,
                                "+ New",
                                LoadoutAction::AddPreset,
                                ButtonStyle::SECONDARY,
                                44.0,
                                14.0,
                                false,
                            );
                        });
                }
            });

            // Active loadout header: name / rename controls / delete / (Done if overlay).
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(12.0),
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(12.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(BORDER_SUBTLE),
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    ImageNode::new(assets.load("textures/ui/team_shield_white.png")),
                    Node {
                        width: Val::Px(30.0),
                        height: Val::Px(30.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));

                if screen.renaming {
                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            height: Val::Px(40.0),
                            padding: UiRect::axes(Val::Px(14.0), Val::Px(6.0)),
                            align_items: AlignItems::Center,
                            border: UiRect::all(Val::Px(1.5)),
                            border_radius: BorderRadius::all(Val::Px(8.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.92)),
                        BorderColor::all(KIT_YELLOW),
                        Pickable::IGNORE,
                    ))
                    .with_children(|input| {
                        let shown = if screen.rename_draft.is_empty() {
                            "Type a loadout name...".to_string()
                        } else {
                            format!("{}|", screen.rename_draft)
                        };
                        input.spawn((
                            Text::new(shown),
                            TextFont::from_font_size(18.0),
                            TextColor(if screen.rename_draft.is_empty() {
                                TEXT_MUTED
                            } else {
                                TEXT_PRIMARY
                            }),
                            Pickable::IGNORE,
                        ));
                    });

                    row.spawn((
                        Node {
                            width: Val::Px(110.0),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|w| {
                        spawn_button(
                            w,
                            "Save",
                            LoadoutAction::ToggleRename,
                            ButtonStyle::PRIMARY,
                            38.0,
                            14.0,
                            false,
                        );
                    });

                    row.spawn((
                        Node {
                            width: Val::Px(100.0),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|w| {
                        spawn_button(
                            w,
                            "Cancel",
                            LoadoutAction::CancelRename,
                            ButtonStyle::SECONDARY,
                            38.0,
                            14.0,
                            false,
                        );
                    });
                } else {
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
                            Text::new(book.active_preset().name.to_uppercase()),
                            TextFont::from_font_size(24.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                        ));
                        col.spawn((
                            Text::new("Splat with a marker to unlock its parts and camos. Each body levels up on its own."),
                            TextFont::from_font_size(12.0),
                            TextColor(TEXT_MUTED),
                            Pickable::IGNORE,
                        ));
                    });

                    row.spawn((
                        Node {
                            width: Val::Px(110.0),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|w| {
                        spawn_button(
                            w,
                            "Rename",
                            LoadoutAction::ToggleRename,
                            ButtonStyle::SECONDARY,
                            38.0,
                            14.0,
                            false,
                        );
                    });

                    if book.presets.len() > 1 {
                        row.spawn((
                            Node {
                                width: Val::Px(100.0),
                                flex_direction: FlexDirection::Column,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|w| {
                            spawn_button(
                                w,
                                "Delete",
                                LoadoutAction::DeletePreset,
                                ButtonStyle::DANGER,
                                38.0,
                                14.0,
                                false,
                            );
                        });
                    }
                }

                if !embedded {
                    row.spawn((
                        Node {
                            width: Val::Px(130.0),
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|wrap| {
                        spawn_button(
                            wrap,
                            "Done",
                            LoadoutAction::Done,
                            ButtonStyle::PRIMARY,
                            38.0,
                            16.0,
                            false,
                        );
                    });
                }
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
                // Slots (no Crosshair here — moved to Options).
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
                });

                // Options for the chosen slot.
                body.spawn(column(300.0)).with_children(|col| {
                    label(col, &slot.label().to_uppercase());
                    let chosen = loadout.get(slot);
                    let body_name = crate::loadout::BODIES
                        .get(loadout.body as usize)
                        .map(|b| b.name)
                        .unwrap_or("");
                    for (i, part) in slot.parts().iter().enumerate() {
                        let i = i as u8;
                        let status = progress.status(loadout.body, slot, i);
                        let locked = status.is_some_and(|(r, have)| have < r.need);
                        let style = if locked {
                            ButtonStyle::LOCKED
                        } else {
                            style_for(i == chosen)
                        };
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
                            LoadoutAction::Pick(slot, i),
                        ))
                        .with_children(|b| {
                            b.spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(6.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|row| {
                                if locked {
                                    row.spawn((
                                        ImageNode::new(kit(assets, "icons/lock.png")),
                                        Node {
                                            width: Val::Px(16.0),
                                            height: Val::Px(16.0),
                                            ..default()
                                        },
                                        Pickable::IGNORE,
                                    ));
                                }
                                row.spawn((
                                    Text::new(part.name),
                                    TextFont::from_font_size(16.0),
                                    TextColor(if locked { TEXT_MUTED } else { TEXT_PRIMARY }),
                                    Pickable::IGNORE,
                                ));
                            });
                            match status {
                                Some((r, have)) if locked => {
                                    b.spawn((
                                        Text::new(format!(
                                            "{} with the {body_name}  ({have}/{})",
                                            r.describe(),
                                            r.need
                                        )),
                                        TextFont::from_font_size(12.0),
                                        TextColor(KIT_YELLOW),
                                        Pickable::IGNORE,
                                    ));
                                    // How far along.
                                    b.spawn((
                                        Node {
                                            height: Val::Px(4.0),
                                            margin: UiRect::top(Val::Px(3.0)),
                                            border_radius: BorderRadius::all(Val::Px(2.0)),
                                            overflow: Overflow::clip(),
                                            ..default()
                                        },
                                        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.08)),
                                        Pickable::IGNORE,
                                    ))
                                    .with_children(|t| {
                                        t.spawn((
                                            Node {
                                                width: Val::Percent(
                                                    have as f32 / r.need.max(1) as f32 * 100.0,
                                                ),
                                                height: Val::Percent(100.0),
                                                ..default()
                                            },
                                            BackgroundColor(KIT_YELLOW),
                                            Pickable::IGNORE,
                                        ));
                                    });
                                }
                                _ => {
                                    b.spawn((
                                        Text::new(part.blurb),
                                        TextFont::from_font_size(12.0),
                                        TextColor(TEXT_MUTED),
                                        Pickable::IGNORE,
                                    ));
                                }
                            }
                            if slot == Slot::Body {
                                let rec = progress.record(i);
                                let (have, all) = progress.unlocked_count(i);
                                b.spawn((
                                    Text::new(format!(
                                        "{} splat{}  |  {have}/{all} unlocked",
                                        rec.splats,
                                        if rec.splats == 1 { "" } else { "s" }
                                    )),
                                    TextFont::from_font_size(11.0),
                                    TextColor(KIT_YELLOW),
                                    Pickable::IGNORE,
                                ));
                            }
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
        (
            "Damage",
            (s.damage * s.pellets as f32 / 75.0).min(1.0),
            if s.pellets > 1 {
                // "6 x 13": balls per shot, then damage per ball.
                format!("{} x {:.0}", s.pellets, s.damage)
            } else {
                format!("{:.0}", s.damage)
            },
        ),
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
            if s.pellets > 1 {
                format!("{} shells", s.capacity)
            } else {
                format!("{}", s.capacity)
            },
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
                    width: Val::Px(84.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}
