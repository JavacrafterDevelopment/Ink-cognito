use crate::{
    AppState, Overlay,
    hub::{self, HubPlay},
    net::{NetRole, NetworkState, Transport},
    portside::{Portside, Session, Status},
};
use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    picking::prelude::*,
    prelude::*,
};

pub const BG_CARD: Color = Color::srgba(0.07, 0.10, 0.15, 0.88);
pub const BORDER_SUBTLE: Color = Color::srgba(0.32, 0.42, 0.56, 0.55);

pub const BTN_PRIMARY: Color = Color::srgb(0.18, 0.52, 0.90);
pub const BTN_PRIMARY_HOVER: Color = Color::srgb(0.24, 0.60, 0.98);
pub const BTN_PRIMARY_PRESS: Color = Color::srgb(0.13, 0.42, 0.76);

pub const BTN_SECONDARY: Color = Color::srgb(0.17, 0.22, 0.30);
pub const BTN_SECONDARY_HOVER: Color = Color::srgb(0.23, 0.29, 0.39);
pub const BTN_SECONDARY_PRESS: Color = Color::srgb(0.13, 0.17, 0.24);

pub const BTN_DANGER: Color = Color::srgb(0.68, 0.20, 0.24);
pub const BTN_DANGER_HOVER: Color = Color::srgb(0.80, 0.25, 0.30);
pub const BTN_DANGER_PRESS: Color = Color::srgb(0.54, 0.15, 0.18);

pub const TEXT_PRIMARY: Color = Color::srgb(0.95, 0.97, 1.0);
pub const TEXT_MUTED: Color = Color::srgb(0.68, 0.75, 0.84);
pub const TEXT_STATUS: Color = Color::srgb(0.45, 0.82, 1.0);
pub const TEXT_ERROR: Color = Color::srgb(1.0, 0.45, 0.45);

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuState>()
            .init_resource::<HudState>()
            .add_systems(Startup, load_ui_sounds)
            .add_observer(on_button_over)
            .add_observer(on_button_out)
            .add_observer(on_button_press)
            .add_observer(on_button_release)
            .add_observer(on_menu_click)
            .add_observer(on_hud_click)
            .add_observer(on_hub_join_click)
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(
                Update,
                (handle_join_text_input, update_menu_ui, update_hub_menu)
                    .run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(OnEnter(AppState::InGame), spawn_in_game_hud)
            .add_systems(
                Update,
                (update_in_game_hud, update_hub_hud, join_toast_with_key)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum MenuScreen {
    #[default]
    Home,
    Join,
}

#[derive(Resource, Default)]
struct MenuState {
    screen: MenuScreen,
    join_address: String,
}

#[derive(Resource, Default)]
struct HudState {
    copied_timer: f32,
}

#[derive(Component, Clone, Copy)]
enum MenuAction {
    CreateGame,
    OpenJoin,
    OpenLoadout,
    PasteAddress,
    ClearAddress,
    ConnectJoin,
    BackToHome,
    CancelConnect,
    RetryPortside,
}

#[derive(Component, Clone, Copy)]
pub enum HudAction {
    CopyAddress,
    LeaveGame,
}

#[derive(Component, Clone, Copy)]
pub struct ButtonStyle {
    pub normal: Color,
    pub hover: Color,
    pub press: Color,
}

impl ButtonStyle {
    pub const PRIMARY: ButtonStyle = ButtonStyle {
        normal: BTN_PRIMARY,
        hover: BTN_PRIMARY_HOVER,
        press: BTN_PRIMARY_PRESS,
    };
    pub const SECONDARY: ButtonStyle = ButtonStyle {
        normal: BTN_SECONDARY,
        hover: BTN_SECONDARY_HOVER,
        press: BTN_SECONDARY_PRESS,
    };
    pub const DANGER: ButtonStyle = ButtonStyle {
        normal: BTN_DANGER,
        hover: BTN_DANGER_HOVER,
        press: BTN_DANGER_PRESS,
    };
}

/// The pack's UI sounds, for every button.
#[derive(Resource)]
pub struct UiSounds {
    pub hover: Handle<AudioSource>,
    pub click: Handle<AudioSource>,
    pub confirm: Handle<AudioSource>,
}

fn load_ui_sounds(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(UiSounds {
        hover: assets.load("audio/ui/ui_hover.ogg"),
        click: assets.load("audio/ui/ui_click.ogg"),
        confirm: assets.load("audio/ui/ui_confirm.ogg"),
    });
}

fn ui_sound(commands: &mut Commands, sound: &Handle<AudioSource>, volume: f32) {
    commands.spawn((
        AudioPlayer::new(sound.clone()),
        PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(volume)),
    ));
}

#[derive(Component)]
struct HomeSection;

#[derive(Component)]
struct JoinSection;

#[derive(Component)]
struct CancelButtonNode;

#[derive(Component)]
struct JoinAddressText;

#[derive(Component)]
struct MenuStatusText;

#[derive(Component)]
struct HudInfoText;

#[derive(Component)]
struct HudAddrText;

#[derive(Component)]
pub struct HudCopyBtnText;

/// A Join button for a game in the Portside hub.
#[derive(Component, Clone)]
struct HubJoin(Session);

/// The pop-up's Join button: joins whatever the pop-up is showing.
#[derive(Component)]
struct HubToastJoin;

#[derive(Component)]
struct HubSection;

#[derive(Component)]
struct HubStatusText;

#[derive(Component)]
struct HubSessionList;

#[derive(Component)]
struct HubRetryNode;

#[derive(Component)]
struct HudHubText;

#[derive(Component)]
struct HubToast;

#[derive(Component)]
struct HubToastText;

fn on_button_over(
    over: On<PointerOver>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    mut q: Query<(&ButtonStyle, &mut BackgroundColor)>,
) {
    if let Ok((style, mut bg)) = q.get_mut(over.entity) {
        *bg = BackgroundColor(style.hover);
        if let Some(sounds) = sounds {
            ui_sound(&mut commands, &sounds.hover, 0.35);
        }
    }
}

fn on_button_out(out: On<PointerOut>, mut q: Query<(&ButtonStyle, &mut BackgroundColor)>) {
    if let Ok((style, mut bg)) = q.get_mut(out.entity) {
        *bg = BackgroundColor(style.normal);
    }
}

fn on_button_press(
    press: On<PointerPress>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    mut q: Query<(&ButtonStyle, &mut BackgroundColor)>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if let Ok((style, mut bg)) = q.get_mut(press.entity) {
        *bg = BackgroundColor(style.press);
        if let Some(sounds) = sounds {
            ui_sound(&mut commands, &sounds.click, 0.6);
        }
    }
}

fn on_button_release(
    release: On<PointerRelease>,
    mut q: Query<(&ButtonStyle, &mut BackgroundColor)>,
) {
    if release.button != PointerButton::Primary {
        return;
    }
    if let Ok((style, mut bg)) = q.get_mut(release.entity) {
        *bg = BackgroundColor(style.hover);
    }
}

#[allow(clippy::too_many_arguments)]
fn on_menu_click(
    click: On<PointerClick>,
    actions: Query<&MenuAction>,
    mut menu: ResMut<MenuState>,
    mut net: ResMut<NetworkState>,
    mut overlay: ResMut<Overlay>,
    portside: Res<Portside>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };

    match *action {
        MenuAction::CreateGame => {
            if !net.is_connecting {
                if let Some(sounds) = sounds {
                    ui_sound(&mut commands, &sounds.confirm, 0.7);
                }
                net.start_host();
            }
        }
        MenuAction::OpenLoadout => {
            if !net.is_connecting {
                overlay.loadout = true;
            }
        }
        MenuAction::OpenJoin => {
            if !net.is_connecting {
                net.error_text = None;
                menu.screen = MenuScreen::Join;
            }
        }
        MenuAction::PasteAddress => {
            if let Some(clip) = read_clipboard() {
                let cleaned: String = clip.chars().filter(|c| !c.is_whitespace()).collect();
                if !cleaned.is_empty() {
                    menu.join_address = cleaned;
                    net.error_text = None;
                }
            } else {
                net.error_text = Some("Clipboard unavailable; type the address instead.".into());
            }
        }
        MenuAction::ClearAddress => {
            menu.join_address.clear();
            net.error_text = None;
        }
        MenuAction::ConnectJoin => {
            if !net.is_connecting {
                let addr = menu.join_address.clone();
                net.join_game(addr);
            }
        }
        MenuAction::BackToHome => {
            if !net.is_connecting {
                net.error_text = None;
                menu.screen = MenuScreen::Home;
            }
        }
        MenuAction::CancelConnect => {
            net.disconnect();
            net.error_text = None;
        }
        MenuAction::RetryPortside => portside.retry(),
    }
}

fn on_hub_join_click(
    click: On<PointerClick>,
    buttons: Query<(Option<&HubJoin>, Has<HubToastJoin>)>,
    mut hub: ResMut<HubPlay>,
    mut net: ResMut<NetworkState>,
    portside: Res<Portside>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok((join, is_toast)) = buttons.get(click.entity) else {
        return;
    };
    let session = match (join, is_toast) {
        (Some(HubJoin(session)), _) => session.clone(),
        (None, true) => match &hub.toast {
            Some((session, _)) => session.clone(),
            None => return,
        },
        _ => return,
    };
    if net.is_connecting {
        return;
    }
    // Joining from inside a game leaves it first.
    if *state.get() == AppState::InGame {
        net.disconnect();
        next_state.set(AppState::MainMenu);
    }
    hub.start_join(&session, &mut net, &portside);
}

fn on_hud_click(
    click: On<PointerClick>,
    actions: Query<&HudAction>,
    mut net: ResMut<NetworkState>,
    mut hud_state: ResMut<HudState>,
    mut overlay: ResMut<Overlay>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };

    match *action {
        HudAction::CopyAddress => {
            if let Some(addr) = &net.tailcat_addr {
                if !write_clipboard(addr) {
                    info!("Share this Tailcat address:\n{addr}");
                }
                hud_state.copied_timer = 2.0;
            }
        }
        HudAction::LeaveGame => {
            *overlay = Overlay::default();
            net.disconnect();
            net.error_text = None;
            next_state.set(AppState::MainMenu);
        }
    }
}

fn spawn_main_menu(
    mut commands: Commands,
    mut menu: ResMut<MenuState>,
    mut overlay: ResMut<Overlay>,
    assets: Res<AssetServer>,
) {
    menu.screen = MenuScreen::Home;
    *overlay = Overlay::default();

    // Full-screen key art behind the menu, darkened on the left for the card.
    commands.spawn((
        ImageNode::new(assets.load("branding/menu_background.png")),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        Pickable::IGNORE,
        GlobalZIndex(-10),
        DespawnOnExit(AppState::MainMenu),
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
        Pickable::IGNORE,
        GlobalZIndex(-9),
        DespawnOnExit(AppState::MainMenu),
    ));
    commands.spawn((
        Text::new(format!("v{}", env!("CARGO_PKG_VERSION"))),
        TextFont::from_font_size(13.0),
        TextColor(TEXT_MUTED),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(14.0),
            bottom: Val::Px(10.0),
            ..default()
        },
        Pickable::IGNORE,
        DespawnOnExit(AppState::MainMenu),
    ));

    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::FlexStart,
                align_items: AlignItems::Center,
                padding: UiRect::left(Val::Percent(4.0)),
                ..default()
            },
            Pickable::IGNORE,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(440.0),
                    max_height: Val::Percent(96.0),
                    overflow: Overflow::clip_y(),
                    padding: UiRect::all(Val::Px(32.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    row_gap: Val::Px(16.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(16.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(BORDER_SUBTLE),
            ))
            .with_children(|card| {
                // Header
                card.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(6.0),
                    margin: UiRect::bottom(Val::Px(8.0)),
                    ..default()
                })
                .with_children(|header| {
                    header.spawn((
                        ImageNode::new(assets.load("branding/logo.png")),
                        Node {
                            width: Val::Px(376.0),
                            height: Val::Px(205.0),
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                    ));
                    header.spawn((
                        Text::new("Paintball  |  Portside  |  Tailcat"),
                        TextFont::from_font_size(15.0),
                        TextColor(TEXT_MUTED),
                    ));
                });

                // Home screen buttons (Create Game / Join Game)
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    HomeSection,
                ))
                .with_children(|home| {
                    spawn_menu_button(
                        home,
                        "Create Game",
                        MenuAction::CreateGame,
                        ButtonStyle {
                            normal: BTN_PRIMARY,
                            hover: BTN_PRIMARY_HOVER,
                            press: BTN_PRIMARY_PRESS,
                        },
                        52.0,
                        20.0,
                        false,
                    );
                    spawn_menu_button(
                        home,
                        "Join by Address",
                        MenuAction::OpenJoin,
                        ButtonStyle {
                            normal: BTN_SECONDARY,
                            hover: BTN_SECONDARY_HOVER,
                            press: BTN_SECONDARY_PRESS,
                        },
                        52.0,
                        20.0,
                        false,
                    );
                    spawn_menu_button(
                        home,
                        "Loadout",
                        MenuAction::OpenLoadout,
                        ButtonStyle::SECONDARY,
                        46.0,
                        18.0,
                        false,
                    );
                });

                // Games in your Portside hub
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(8.0),
                        padding: UiRect::top(Val::Px(10.0)),
                        border: UiRect::top(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(BORDER_SUBTLE),
                    HubSection,
                ))
                .with_children(|hub| {
                    hub.spawn((
                        Text::new("YOUR PORTSIDE HUB"),
                        TextFont::from_font_size(12.0),
                        TextColor(TEXT_MUTED),
                    ));
                    hub.spawn((
                        Text::new(""),
                        TextFont::from_font_size(14.0),
                        TextColor(TEXT_STATUS),
                        HubStatusText,
                    ));
                    hub.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Stretch,
                            row_gap: Val::Px(6.0),
                            ..default()
                        },
                        HubSessionList,
                    ));
                    hub.spawn((
                        Node {
                            display: Display::None,
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Stretch,
                            ..default()
                        },
                        HubRetryNode,
                    ))
                    .with_children(|retry| {
                        spawn_menu_button(
                            retry,
                            "Retry Portside",
                            MenuAction::RetryPortside,
                            ButtonStyle {
                                normal: BTN_SECONDARY,
                                hover: BTN_SECONDARY_HOVER,
                                press: BTN_SECONDARY_PRESS,
                            },
                            34.0,
                            14.0,
                            false,
                        );
                    });
                });

                // Join screen (Tailcat address input + Connect / Paste / Back)
                card.spawn((
                    Node {
                        display: Display::None,
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    JoinSection,
                ))
                .with_children(|join| {
                    join.spawn((
                        Text::new("Host Tailcat Address (type or Ctrl+V):"),
                        TextFont::from_font_size(14.0),
                        TextColor(TEXT_MUTED),
                    ));

                    // Address text display box
                    join.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            min_height: Val::Px(54.0),
                            padding: UiRect::axes(Val::Px(12.0), Val::Px(10.0)),
                            align_items: AlignItems::Center,
                            border: UiRect::all(Val::Px(1.5)),
                            border_radius: BorderRadius::all(Val::Px(8.0)),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.95)),
                        BorderColor::all(Color::srgba(0.32, 0.56, 0.86, 0.8)),
                    ))
                    .with_children(|input_box| {
                        input_box.spawn((
                            Text::new("Paste tc... address here"),
                            TextFont::from_font_size(14.0),
                            TextColor(TEXT_MUTED),
                            JoinAddressText,
                        ));
                    });

                    // Utility row: Paste & Clear
                    join.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(10.0),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_menu_button(
                            row,
                            "Paste Clipboard",
                            MenuAction::PasteAddress,
                            ButtonStyle {
                                normal: BTN_SECONDARY,
                                hover: BTN_SECONDARY_HOVER,
                                press: BTN_SECONDARY_PRESS,
                            },
                            38.0,
                            14.0,
                            true,
                        );
                        spawn_menu_button(
                            row,
                            "Clear",
                            MenuAction::ClearAddress,
                            ButtonStyle {
                                normal: BTN_SECONDARY,
                                hover: BTN_SECONDARY_HOVER,
                                press: BTN_SECONDARY_PRESS,
                            },
                            38.0,
                            14.0,
                            true,
                        );
                    });

                    // Action row: Back & Connect
                    join.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(10.0),
                        margin: UiRect::top(Val::Px(4.0)),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_menu_button(
                            row,
                            "Back",
                            MenuAction::BackToHome,
                            ButtonStyle {
                                normal: BTN_SECONDARY,
                                hover: BTN_SECONDARY_HOVER,
                                press: BTN_SECONDARY_PRESS,
                            },
                            46.0,
                            17.0,
                            true,
                        );
                        spawn_menu_button(
                            row,
                            "Connect",
                            MenuAction::ConnectJoin,
                            ButtonStyle {
                                normal: BTN_PRIMARY,
                                hover: BTN_PRIMARY_HOVER,
                                press: BTN_PRIMARY_PRESS,
                            },
                            46.0,
                            17.0,
                            true,
                        );
                    });
                });

                // Cancel button shown while connecting / starting host
                card.spawn((
                    Node {
                        display: Display::None,
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        ..default()
                    },
                    CancelButtonNode,
                ))
                .with_children(|cancel_wrap| {
                    spawn_menu_button(
                        cancel_wrap,
                        "Cancel",
                        MenuAction::CancelConnect,
                        ButtonStyle {
                            normal: BTN_DANGER,
                            hover: BTN_DANGER_HOVER,
                            press: BTN_DANGER_PRESS,
                        },
                        42.0,
                        16.0,
                        false,
                    );
                });

                // Status / error line
                card.spawn((
                    Text::new(""),
                    TextFont::from_font_size(14.0),
                    TextColor(TEXT_STATUS),
                    MenuStatusText,
                ));
            });
        });
}

fn spawn_menu_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    action: MenuAction,
    style: ButtonStyle,
    height: f32,
    font_size: f32,
    flex_grow: bool,
) {
    spawn_button(parent, label, action, style, height, font_size, flex_grow);
}

/// A rounded button with hover / press colours and the UI sounds.
pub fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    marker: impl Bundle,
    style: ButtonStyle,
    height: f32,
    font_size: f32,
    flex_grow: bool,
) {
    parent
        .spawn((
            Node {
                flex_grow: if flex_grow { 1.0 } else { 0.0 },
                height: Val::Px(height),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(style.normal),
            marker,
            style,
        ))
        .with_children(|btn| {
            btn.spawn((
                Text::new(label),
                TextFont::from_font_size(font_size),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
        });
}

fn handle_join_text_input(
    mut key_events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuState>,
    mut net: ResMut<NetworkState>,
) {
    if menu.screen != MenuScreen::Join || net.is_connecting {
        key_events.clear();
        return;
    }

    let modifier = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight);

    for event in key_events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }

        if modifier && event.key_code == KeyCode::KeyV {
            if let Some(clip) = read_clipboard() {
                let cleaned: String = clip.chars().filter(|c| !c.is_whitespace()).collect();
                menu.join_address.push_str(&cleaned);
                net.error_text = None;
            }
            continue;
        }

        match &event.logical_key {
            Key::Backspace => {
                if modifier {
                    menu.join_address.clear();
                } else {
                    menu.join_address.pop();
                }
                net.error_text = None;
            }
            Key::Enter => {
                let addr = menu.join_address.clone();
                net.join_game(addr);
            }
            Key::Escape => {
                menu.screen = MenuScreen::Home;
                net.error_text = None;
            }
            _ => {
                if !modifier && let Some(text) = &event.text {
                    for ch in text.chars() {
                        if !ch.is_control() && !ch.is_whitespace() {
                            menu.join_address.push(ch);
                        }
                    }
                    net.error_text = None;
                }
            }
        }
    }
}

fn update_menu_ui(
    menu: Res<MenuState>,
    net: Res<NetworkState>,
    mut home_q: Query<
        &mut Node,
        (
            With<HomeSection>,
            Without<JoinSection>,
            Without<CancelButtonNode>,
        ),
    >,
    mut join_q: Query<
        &mut Node,
        (
            With<JoinSection>,
            Without<HomeSection>,
            Without<CancelButtonNode>,
        ),
    >,
    mut cancel_q: Query<
        &mut Node,
        (
            With<CancelButtonNode>,
            Without<HomeSection>,
            Without<JoinSection>,
        ),
    >,
    mut addr_text_q: Query<
        (&mut Text, &mut TextColor),
        (With<JoinAddressText>, Without<MenuStatusText>),
    >,
    mut status_q: Query<
        (&mut Text, &mut TextColor),
        (With<MenuStatusText>, Without<JoinAddressText>),
    >,
) {
    if let Ok(mut node) = home_q.single_mut() {
        node.display = if !net.is_connecting && menu.screen == MenuScreen::Home {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let Ok(mut node) = join_q.single_mut() {
        node.display = if !net.is_connecting && menu.screen == MenuScreen::Join {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let Ok(mut node) = cancel_q.single_mut() {
        node.display = if net.is_connecting {
            Display::Flex
        } else {
            Display::None
        };
    }

    if let Ok((mut text, mut color)) = addr_text_q.single_mut() {
        if menu.join_address.is_empty() {
            text.0 = "Paste tc... address here".to_string();
            color.0 = TEXT_MUTED;
        } else {
            text.0 = format!("{}|", shorten_middle(&menu.join_address, 44));
            color.0 = TEXT_PRIMARY;
        }
    }

    if let Ok((mut text, mut color)) = status_q.single_mut() {
        if let Some(err) = &net.error_text {
            text.0 = err.clone();
            color.0 = TEXT_ERROR;
        } else if net.is_connecting {
            text.0 = net.status_text.clone();
            color.0 = TEXT_STATUS;
        } else {
            text.0.clear();
        }
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_hub_menu(
    mut commands: Commands,
    hub: Res<HubPlay>,
    menu: Res<MenuState>,
    net: Res<NetworkState>,
    mut section_q: Query<&mut Node, (With<HubSection>, Without<HubRetryNode>)>,
    mut retry_q: Query<&mut Node, (With<HubRetryNode>, Without<HubSection>)>,
    mut status_q: Query<&mut Text, With<HubStatusText>>,
    list_q: Query<(Entity, Ref<HubSessionList>)>,
    mut shown: Local<u64>,
) {
    if let Ok(mut node) = section_q.single_mut() {
        node.display = if !net.is_connecting && menu.screen == MenuScreen::Home {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let Ok(mut node) = retry_q.single_mut() {
        node.display = match hub.snapshot.status {
            Status::Denied(_) | Status::NotRunning => Display::Flex,
            _ => Display::None,
        };
    }
    if let Ok(mut text) = status_q.single_mut() {
        let line = hub::status_line(&hub);
        if text.0 != line {
            text.0 = line;
        }
    }

    let Ok((list, list_ref)) = list_q.single() else {
        return;
    };
    if !list_ref.is_added() && *shown == hub.snapshot.version {
        return;
    }
    *shown = hub.snapshot.version;
    commands.entity(list).despawn_related::<Children>();
    let sessions: Vec<Session> = hub.others_sessions().take(5).cloned().collect();
    commands.entity(list).with_children(|list| {
        for session in sessions {
            list.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(10.0),
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(10.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.9)),
                BorderColor::all(BORDER_SUBTLE),
            ))
            .with_children(|row| {
                let players = match session.players {
                    Some(1) => "  |  1 player".to_string(),
                    Some(n) => format!("  |  {n} players"),
                    None => String::new(),
                };
                row.spawn((
                    Node {
                        flex_grow: 1.0,
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(2.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|col| {
                    col.spawn((
                        Text::new(shorten_middle(&session.title, 30)),
                        TextFont::from_font_size(15.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    col.spawn((
                        Text::new(format!("{}{players}", session.host_name)),
                        TextFont::from_font_size(12.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                    ));
                });
                spawn_hub_join_button(row, HubJoin(session.clone()));
            });
        }
    });
}

/// J joins the game the pop-up shows, without freeing the mouse first.
fn join_toast_with_key(
    keys: Res<ButtonInput<KeyCode>>,
    mut hub: ResMut<HubPlay>,
    mut net: ResMut<NetworkState>,
    portside: Res<Portside>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !keys.just_pressed(KeyCode::KeyJ) || net.is_connecting {
        return;
    }
    let Some((session, _)) = hub.toast.clone() else {
        return;
    };
    net.disconnect();
    next_state.set(AppState::MainMenu);
    hub.start_join(&session, &mut net, &portside);
}

fn spawn_hub_join_button(parent: &mut ChildSpawnerCommands, marker: impl Bundle) {
    let style = ButtonStyle {
        normal: BTN_PRIMARY,
        hover: BTN_PRIMARY_HOVER,
        press: BTN_PRIMARY_PRESS,
    };
    parent
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(style.normal),
            style,
            marker,
        ))
        .with_children(|btn| {
            btn.spawn((
                Text::new("Join"),
                TextFont::from_font_size(14.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
        });
}

#[allow(clippy::type_complexity)]
fn update_hub_hud(
    hub: Res<HubPlay>,
    net: Res<NetworkState>,
    mut line_q: Query<(&mut Text, &mut Node), (With<HudHubText>, Without<HubToastText>)>,
    mut toast_q: Query<&mut Node, (With<HubToast>, Without<HudHubText>)>,
    mut toast_text_q: Query<&mut Text, (With<HubToastText>, Without<HudHubText>)>,
) {
    if let Ok((mut text, mut node)) = line_q.single_mut() {
        match hub::hud_line(&hub, &net) {
            Some(line) => {
                if text.0 != line {
                    text.0 = line;
                }
                node.display = Display::Flex;
            }
            None => node.display = Display::None,
        }
    }
    if let Ok(mut node) = toast_q.single_mut() {
        node.display = if hub.toast.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let (Ok(mut text), Some((session, _))) = (toast_text_q.single_mut(), &hub.toast) {
        let line = format!(
            "{} started a game in your hub (J to join)\n{}",
            session.host_name,
            shorten_middle(&session.title, 34)
        );
        if text.0 != line {
            text.0 = line;
        }
    }
}

fn spawn_in_game_hud(
    mut commands: Commands,
    net: Res<NetworkState>,
    mut hud_state: ResMut<HudState>,
) {
    hud_state.copied_timer = 0.0;
    let is_host = net.active_role() == Some(NetRole::Host);

    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(16.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::FlexStart,
                ..default()
            },
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|root| {
            // Top-left multiplayer info & actions bar
            root.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(12.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(BORDER_SUBTLE),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Connected"),
                    TextFont::from_font_size(16.0),
                    TextColor(TEXT_PRIMARY),
                    HudInfoText,
                ));

                if is_host {
                    panel.spawn((
                        Text::new("Tailcat address: starting..."),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_STATUS),
                        HudAddrText,
                    ));
                }

                panel.spawn((
                    Node::default(),
                    Text::new(""),
                    TextFont::from_font_size(13.0),
                    TextColor(TEXT_MUTED),
                    HudHubText,
                ));

            });

            // Controls hint, bottom center (above the health and ammo panels).
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    bottom: Val::Px(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|wrap| {
                wrap.spawn((
                    Node {
                        padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.07, 0.10, 0.15, 0.55)),
                    Pickable::IGNORE,
                ))
                .with_children(|hint| {
                    hint.spawn((
                        Text::new(
                            "WASD: Move  |  Shift: Sprint  |  Space: Jump  |  LMB: Shoot  |  RMB: Aim  |  R: Reload  |  Esc: Menu",
                        ),
                        TextFont::from_font_size(12.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                    ));
                });
            });

            // Pop-up: someone in the hub started a game
            root.spawn((
                Node {
                    display: Display::None,
                    position_type: PositionType::Absolute,
                    top: Val::Px(16.0),
                    right: Val::Px(16.0),
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(14.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(12.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(Color::srgba(0.32, 0.56, 0.86, 0.9)),
                HubToast,
            ))
            .with_children(|toast| {
                toast.spawn((
                    Text::new(""),
                    TextFont::from_font_size(14.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                    HubToastText,
                ));
                spawn_hub_join_button(toast, HubToastJoin);
            });
        });
}

fn update_in_game_hud(
    time: Res<Time>,
    net: Res<NetworkState>,
    mut hud_state: ResMut<HudState>,
    mut info_q: Query<
        &mut Text,
        (
            With<HudInfoText>,
            Without<HudAddrText>,
            Without<HudCopyBtnText>,
        ),
    >,
    mut addr_q: Query<
        &mut Text,
        (
            With<HudAddrText>,
            Without<HudInfoText>,
            Without<HudCopyBtnText>,
        ),
    >,
    mut copy_q: Query<
        &mut Text,
        (
            With<HudCopyBtnText>,
            Without<HudInfoText>,
            Without<HudAddrText>,
        ),
    >,
) {
    if hud_state.copied_timer > 0.0 {
        hud_state.copied_timer = (hud_state.copied_timer - time.delta_secs()).max(0.0);
    }

    let role_label = match net.active_role() {
        Some(NetRole::Host) => "HOST",
        Some(NetRole::Client) => "CLIENT",
        None => "OFFLINE",
    };
    let count = net.players.len().max(1);
    let plural = if count == 1 { "player" } else { "players" };
    let via = match (net.active_role(), net.transport) {
        (_, Some(Transport::Portside)) => "  |  Portside",
        (Some(NetRole::Client), _) => "  |  Tailcat",
        _ => "",
    };

    if let Ok(mut text) = info_q.single_mut() {
        text.0 = format!("{role_label}  |  {count} {plural}{via}");
    }

    if let Ok(mut text) = addr_q.single_mut() {
        if let Some(addr) = &net.tailcat_addr {
            text.0 = format!("Address: {}", shorten_middle(addr, 36));
        } else if let Some(err) = &net.tailcat_error {
            text.0 = err.clone();
        }
    }

    if let Ok(mut text) = copy_q.single_mut() {
        text.0 = if hud_state.copied_timer > 0.0 {
            "Copied!".to_string()
        } else {
            "Copy Tailcat Address".to_string()
        };
    }
}

fn shorten_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars || max_chars < 10 {
        return value.to_string();
    }
    let keep = (max_chars - 3) / 2;
    let start: String = chars[..keep].iter().collect();
    let end: String = chars[chars.len() - keep..].iter().collect();
    format!("{start}...{end}")
}

fn read_clipboard() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.get_text().ok()
}

fn write_clipboard(text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text.to_owned()))
        .is_ok()
}
