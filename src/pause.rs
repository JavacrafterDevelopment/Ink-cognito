//! The Escape menu: Resume, Loadout, Copy Address (host), Leave Game, Quit.
//! The game keeps running underneath (it's multiplayer); only your input stops.

use crate::{
    AppState, Overlay,
    menu::{
        BG_CARD, BORDER_SUBTLE, ButtonStyle, HudAction, HudCopyBtnText, TEXT_MUTED, spawn_button,
    },
    net::{NetRole, NetworkState},
};
use bevy::{picking::prelude::*, prelude::*};

pub struct PausePlugin;

impl Plugin for PausePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_pause_click)
            .add_systems(Update, escape_key)
            .add_systems(Update, show_pause_menu.run_if(in_state(AppState::InGame)))
            .add_systems(OnExit(AppState::InGame), close_overlays);
    }
}

#[derive(Component, Clone, Copy)]
enum PauseAction {
    Resume,
    Loadout,
    Settings,
    Quit,
}

#[derive(Component)]
struct PauseRoot;

fn close_overlays(mut overlay: ResMut<Overlay>) {
    *overlay = Overlay::default();
}

/// Escape closes the loadout or options screen if open, otherwise opens or closes
/// the Escape menu (in game).
fn escape_key(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<AppState>>,
    mut overlay: ResMut<Overlay>,
    settings_ui: Res<crate::settings::SettingsUiState>,
    loadout_screen: Option<Res<crate::loadout_ui::LoadoutScreen>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if settings_ui.is_listening() || loadout_screen.is_some_and(|s| s.is_renaming()) {
        return;
    }
    if overlay.settings {
        overlay.settings = false;
    } else if overlay.loadout {
        overlay.loadout = false;
    } else if *state.get() == AppState::InGame {
        overlay.pause = !overlay.pause;
    }
}

fn on_pause_click(
    click: On<PointerClick>,
    actions: Query<&PauseAction>,
    mut overlay: ResMut<Overlay>,
    mut net: ResMut<NetworkState>,
    mut exit: MessageWriter<AppExit>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };
    match action {
        PauseAction::Resume => overlay.pause = false,
        PauseAction::Loadout => overlay.loadout = true,
        PauseAction::Settings => overlay.settings = true,
        PauseAction::Quit => {
            net.disconnect();
            exit.write(AppExit::Success);
        }
    }
}

fn show_pause_menu(
    mut commands: Commands,
    overlay: Res<Overlay>,
    net: Res<NetworkState>,
    assets: Res<AssetServer>,
    mut root_q: Query<(Entity, &mut Node), With<PauseRoot>>,
) {
    match (overlay.pause, root_q.single_mut()) {
        (false, Ok((root, _))) => {
            commands.entity(root).despawn();
        }
        (true, Ok((_, mut node))) => {
            // Step aside while Loadout or Options is up.
            let want = if overlay.loadout || overlay.settings {
                Display::None
            } else {
                Display::Flex
            };
            if node.display != want {
                node.display = want;
            }
        }
        (true, Err(_)) => spawn_pause_menu(&mut commands, &net, &assets),
        (false, Err(_)) => {}
    }
}

fn spawn_pause_menu(commands: &mut Commands, net: &NetworkState, assets: &AssetServer) {
    let is_host = net.active_role() == Some(NetRole::Host);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(20),
            PauseRoot,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(380.0),
                    padding: UiRect::all(Val::Px(26.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    row_gap: Val::Px(12.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(16.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(BORDER_SUBTLE),
            ))
            .with_children(|card| {
                card.spawn((
                    Node {
                        justify_content: JustifyContent::Center,
                        margin: UiRect::bottom(Val::Px(4.0)),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|row| {
                    row.spawn((
                        ImageNode::new(assets.load("branding/logo.png")),
                        Node {
                            width: Val::Px(240.0),
                            height: Val::Px(131.0),
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                });
                spawn_button(
                    card,
                    "Resume",
                    PauseAction::Resume,
                    ButtonStyle::PRIMARY,
                    46.0,
                    18.0,
                    false,
                );
                spawn_button(
                    card,
                    "Loadout",
                    PauseAction::Loadout,
                    ButtonStyle::SECONDARY,
                    44.0,
                    17.0,
                    false,
                );
                spawn_button(
                    card,
                    "Options",
                    PauseAction::Settings,
                    ButtonStyle::SECONDARY,
                    44.0,
                    17.0,
                    false,
                );
                if is_host {
                    card.spawn((
                        Node {
                            height: Val::Px(44.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                        BackgroundColor(ButtonStyle::SECONDARY.normal),
                        ButtonStyle::SECONDARY,
                        HudAction::CopyAddress,
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new("Copy Tailcat Address"),
                            TextFont::from_font_size(17.0),
                            TextColor(crate::menu::TEXT_PRIMARY),
                            Pickable::IGNORE,
                            HudCopyBtnText,
                        ));
                    });
                }
                spawn_button(
                    card,
                    "Leave Game",
                    HudAction::LeaveGame,
                    ButtonStyle::DANGER,
                    44.0,
                    17.0,
                    false,
                );
                spawn_button(
                    card,
                    "Quit to Desktop",
                    PauseAction::Quit,
                    ButtonStyle::DANGER,
                    40.0,
                    15.0,
                    false,
                );
                card.spawn((
                    Text::new("The match keeps going while this menu is open."),
                    TextFont::from_font_size(12.0),
                    TextColor(TEXT_MUTED),
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        });
}
