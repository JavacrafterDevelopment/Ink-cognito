use crate::{
    AppState, Overlay,
    game_mode::MatchSetup,
    hub::{self, HubPlay},
    maps::{self, CurrentMap},
    net::{NetRole, NetworkState, Transport},
    portside::{Portside, Session, Status},
    protocol::{GameMode, MapId, MatchSettings},
    ranked::{self, Rank, Tier},
    settings::AudioSettings,
    ui_kit::{KIT_YELLOW, KitButton, UiKit, kit, sliced, sliced_rect},
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
                (
                    handle_join_text_input,
                    menu_escape,
                    show_menu_screens,
                    update_menu_ui,
                    update_hub_menu,
                    update_play_screen,
                    update_rank_card,
                )
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
    Play,
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
    OpenPlay,
    PickMode(usize),
    PickMap(MapId),
    Quit,
    CreateGame,
    OpenJoin,
    OpenLoadout,
    OpenSettings,
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

fn ui_sound(
    commands: &mut Commands,
    sound: &Handle<AudioSource>,
    volume: f32,
    settings: Option<&AudioSettings>,
) {
    let scale = settings.map(|s| s.effective_ui()).unwrap_or(1.0);
    let final_vol = volume * scale;
    if final_vol <= 0.005 {
        return;
    }
    commands.spawn((
        AudioPlayer::new(sound.clone()),
        PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(final_vol)),
    ));
}

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

// Kit buttons swap art (see `ui_kit`); plain coloured ones change colour.
type PlainButtons<'w, 's> =
    Query<'w, 's, (&'static ButtonStyle, &'static mut BackgroundColor), Without<KitButton>>;

fn on_button_over(
    over: On<PointerOver>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
    styled: Query<(), With<ButtonStyle>>,
    mut q: PlainButtons,
) {
    if let Ok((style, mut bg)) = q.get_mut(over.entity) {
        *bg = BackgroundColor(style.hover);
    }
    if styled.contains(over.entity)
        && let Some(sounds) = sounds
    {
        ui_sound(&mut commands, &sounds.hover, 0.35, settings.as_deref());
    }
}

fn on_button_out(out: On<PointerOut>, mut q: PlainButtons) {
    if let Ok((style, mut bg)) = q.get_mut(out.entity) {
        *bg = BackgroundColor(style.normal);
    }
}

fn on_button_press(
    press: On<PointerPress>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
    styled: Query<(), With<ButtonStyle>>,
    mut q: PlainButtons,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if let Ok((style, mut bg)) = q.get_mut(press.entity) {
        *bg = BackgroundColor(style.press);
    }
    if styled.contains(press.entity)
        && let Some(sounds) = sounds
    {
        ui_sound(&mut commands, &sounds.click, 0.6, settings.as_deref());
    }
}

fn on_button_release(release: On<PointerRelease>, mut q: PlainButtons) {
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
    mut setup: ResMut<MatchSetup>,
    mut current_map: ResMut<CurrentMap>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };

    match *action {
        MenuAction::OpenPlay => {
            if !net.is_connecting {
                net.error_text = None;
                menu.screen = MenuScreen::Play;
                current_map.0 = setup.0.map;
            }
        }
        MenuAction::PickMode(i) => {
            let choice = &MODE_CHOICES[i.min(MODE_CHOICES.len() - 1)];
            setup.0.mode = choice.mode;
            setup.0.ranked = choice.ranked;
            if let Some(sounds) = &sounds {
                ui_sound(&mut commands, &sounds.click, 0.6, settings.as_deref());
            }
        }
        MenuAction::PickMap(id) => {
            setup.0.map = id;
            current_map.0 = id;
            if let Some(sounds) = &sounds {
                ui_sound(&mut commands, &sounds.click, 0.6, settings.as_deref());
            }
        }
        MenuAction::Quit => {
            exit.write(AppExit::Success);
        }
        MenuAction::CreateGame => {
            if !net.is_connecting {
                if let Some(sounds) = sounds {
                    ui_sound(&mut commands, &sounds.confirm, 0.7, settings.as_deref());
                }
                net.start_host();
            }
        }
        MenuAction::OpenLoadout => {
            if !net.is_connecting {
                overlay.loadout = true;
            }
        }
        MenuAction::OpenSettings => {
            if !net.is_connecting {
                overlay.settings = true;
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

/// The four ways to play, as shown on the play screen.
pub struct ModeChoice {
    pub mode: GameMode,
    pub ranked: bool,
    pub title: &'static str,
    pub blurb: &'static str,
    pub tag: &'static str,
    pub icon: &'static str,
}

pub const MODE_CHOICES: [ModeChoice; 4] = [
    ModeChoice {
        mode: GameMode::TeamSplat,
        ranked: false,
        title: "TEAM SPLAT",
        blurb: "Blue vs orange. The team with the most splats after 10 minutes wins.",
        tag: "10 MIN  |  TEAMS",
        icon: "icons/team.png",
    },
    ModeChoice {
        mode: GameMode::FreeForAll,
        ranked: false,
        title: "FREE-FOR-ALL",
        blurb: "Everyone for themselves. Top splatter after 10 minutes wins.",
        tag: "10 MIN  |  SOLO",
        icon: "icons/target.png",
    },
    ModeChoice {
        mode: GameMode::Elimination,
        ranked: false,
        title: "ELIMINATION",
        blurb: "One life per round. The first team to win 5 rounds takes the match.",
        tag: "FIRST TO 5 ROUNDS",
        icon: "icons/shield.png",
    },
    ModeChoice {
        mode: GameMode::Elimination,
        ranked: true,
        title: "RANKED",
        blurb: "Elimination for rank points. Win to climb; quitting counts as a loss.",
        tag: "RANKED  |  FIRST TO 5",
        icon: "icons/trophy.png",
    },
];

fn mode_index(settings: &MatchSettings) -> usize {
    MODE_CHOICES
        .iter()
        .position(|c| c.mode == settings.mode && c.ranked == settings.ranked)
        .unwrap_or(0)
}

/// Which screen(s) a part of the main menu shows on.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ShowOn {
    Home,
    Join,
    Play,
    /// Home or join (the left card).
    Card,
    /// Home while not connecting (the player card).
    HomeIdle,
    Connecting,
    /// The key art behind the menu (not on the play screen: the map shows).
    Backdrop,
}

impl ShowOn {
    fn visible(self, screen: MenuScreen, connecting: bool) -> bool {
        match self {
            ShowOn::Home => !connecting && screen == MenuScreen::Home,
            ShowOn::Join => !connecting && screen == MenuScreen::Join,
            ShowOn::Play => !connecting && screen == MenuScreen::Play,
            ShowOn::Card => connecting || screen != MenuScreen::Play,
            ShowOn::HomeIdle => screen == MenuScreen::Home,
            ShowOn::Connecting => connecting,
            ShowOn::Backdrop => screen != MenuScreen::Play,
        }
    }
}

#[derive(Component)]
struct ModeCard(usize);

#[derive(Component)]
struct ModeCardTitle(usize);

#[derive(Component)]
struct MapTile(MapId);

#[derive(Component)]
struct MapTileText(MapId);

#[derive(Component)]
enum MapInfoText {
    Name,
    Style,
    Blurb,
}

#[derive(Component)]
struct ModeSummaryText;

#[derive(Component)]
enum RankCardPart {
    Badge,
    Name,
    Points,
    Fill,
    Next,
    Record,
}

fn spawn_main_menu(
    mut commands: Commands,
    mut menu: ResMut<MenuState>,
    mut overlay: ResMut<Overlay>,
    assets: Res<AssetServer>,
    kit_fonts: Res<UiKit>,
) {
    menu.screen = MenuScreen::Home;
    *overlay = Overlay::default();
    let k = |p: &str| kit(&assets, p);

    // Full-screen key art behind the menu.
    commands.spawn((
        ImageNode::new(k("backgrounds/menu_background_1920x1080.png")),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        Pickable::IGNORE,
        GlobalZIndex(-10),
        ShowOn::Backdrop,
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
        ShowOn::Backdrop,
        DespawnOnExit(AppState::MainMenu),
    ));

    // Left: logo, buttons, hub, join, status.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(4.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            ShowOn::Card,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(430.0),
                    max_height: Val::Percent(96.0),
                    overflow: Overflow::clip_y(),
                    padding: UiRect::all(Val::Px(28.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    row_gap: Val::Px(12.0),
                    ..default()
                },
                sliced(k("panels/panel.png"), 28.0),
            ))
            .with_children(|card| {
                card.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    margin: UiRect::bottom(Val::Px(4.0)),
                    ..default()
                })
                .with_children(|header| {
                    header.spawn((
                        ImageNode::new(assets.load("branding/logo.png")),
                        Node {
                            width: Val::Px(330.0),
                            height: Val::Px(180.0),
                            ..default()
                        },
                    ));
                });

                // Home: Play / Join / Loadout / Options / Quit.
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(10.0),
                        ..default()
                    },
                    ShowOn::Home,
                ))
                .with_children(|home| {
                    spawn_menu_button(
                        home,
                        "PLAY",
                        MenuAction::OpenPlay,
                        ButtonStyle::PRIMARY,
                        58.0,
                        24.0,
                        false,
                    );
                    spawn_menu_button(
                        home,
                        "JOIN BY ADDRESS",
                        MenuAction::OpenJoin,
                        ButtonStyle::SECONDARY,
                        46.0,
                        17.0,
                        false,
                    );
                    home.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(10.0),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_menu_button(
                            row,
                            "LOADOUT",
                            MenuAction::OpenLoadout,
                            ButtonStyle::SECONDARY,
                            46.0,
                            17.0,
                            true,
                        );
                        spawn_menu_button(
                            row,
                            "OPTIONS",
                            MenuAction::OpenSettings,
                            ButtonStyle::SECONDARY,
                            46.0,
                            17.0,
                            true,
                        );
                    });
                    spawn_menu_button(
                        home,
                        "QUIT",
                        MenuAction::Quit,
                        ButtonStyle::DANGER,
                        40.0,
                        15.0,
                        false,
                    );
                });

                // Games in your Portside hub.
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(8.0),
                        padding: UiRect::all(Val::Px(14.0)),
                        ..default()
                    },
                    sliced(k("panels/panel_inset.png"), 28.0),
                    ShowOn::Home,
                    HubSection,
                ))
                .with_children(|hub| {
                    hub.spawn((
                        Text::new("YOUR PORTSIDE HUB"),
                        TextFont::from_font_size(12.0),
                        TextColor(KIT_YELLOW),
                    ));
                    hub.spawn((
                        Text::new(""),
                        TextFont::from_font_size(13.0),
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
                            "RETRY PORTSIDE",
                            MenuAction::RetryPortside,
                            ButtonStyle::SECONDARY,
                            36.0,
                            14.0,
                            false,
                        );
                    });
                });

                // Join screen (Tailcat address input + Connect / Paste / Back).
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    ShowOn::Join,
                ))
                .with_children(|join| {
                    join.spawn((
                        Text::new("HOST TAILCAT ADDRESS (TYPE OR CTRL+V)"),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_MUTED),
                    ));
                    join.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            min_height: Val::Px(54.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                            align_items: AlignItems::Center,
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        sliced(k("panels/panel_inset.png"), 28.0),
                    ))
                    .with_children(|input_box| {
                        input_box.spawn((
                            Text::new("Paste tc... address here"),
                            TextFont::from_font_size(14.0),
                            TextColor(TEXT_MUTED),
                            JoinAddressText,
                        ));
                    });
                    join.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(10.0),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_menu_button(
                            row,
                            "PASTE",
                            MenuAction::PasteAddress,
                            ButtonStyle::SECONDARY,
                            40.0,
                            14.0,
                            true,
                        );
                        spawn_menu_button(
                            row,
                            "CLEAR",
                            MenuAction::ClearAddress,
                            ButtonStyle::SECONDARY,
                            40.0,
                            14.0,
                            true,
                        );
                    });
                    join.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(10.0),
                        margin: UiRect::top(Val::Px(4.0)),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_menu_button(
                            row,
                            "BACK",
                            MenuAction::BackToHome,
                            ButtonStyle::SECONDARY,
                            48.0,
                            17.0,
                            true,
                        );
                        spawn_menu_button(
                            row,
                            "CONNECT",
                            MenuAction::ConnectJoin,
                            ButtonStyle::PRIMARY,
                            48.0,
                            17.0,
                            true,
                        );
                    });
                });

                // Cancel while connecting / starting a game.
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        ..default()
                    },
                    ShowOn::Connecting,
                ))
                .with_children(|cancel_wrap| {
                    spawn_menu_button(
                        cancel_wrap,
                        "CANCEL",
                        MenuAction::CancelConnect,
                        ButtonStyle::DANGER,
                        44.0,
                        16.0,
                        false,
                    );
                });

                card.spawn((
                    Text::new(""),
                    TextFont::from_font_size(14.0),
                    TextColor(TEXT_STATUS),
                    MenuStatusText,
                ));
            });
        });

    // Right: the player card with rank.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Percent(4.0),
                top: Val::Px(28.0),
                width: Val::Px(300.0),
                padding: UiRect::all(Val::Px(22.0)),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(8.0),
                ..default()
            },
            sliced(k("panels/card_neutral.png"), 28.0),
            ShowOn::HomeIdle,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|card| {
            card.spawn((
                Text::new("RANKED ELIMINATION"),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_MUTED),
            ));
            card.spawn((
                ImageNode::new(k("badges/rank_bronze.png")),
                Node {
                    width: Val::Px(96.0),
                    height: Val::Px(96.0),
                    ..default()
                },
                RankCardPart::Badge,
            ));
            card.spawn((
                Text::new(""),
                kit_fonts.heading(32.0),
                TextColor(TEXT_PRIMARY),
                RankCardPart::Name,
            ));
            card.spawn((
                Text::new(""),
                TextFont::from_font_size(15.0),
                TextColor(KIT_YELLOW),
                RankCardPart::Points,
            ));
            card.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(20.0),
                    padding: UiRect::all(Val::Px(3.0)),
                    ..default()
                },
                sliced(k("controls/progress_frame.png"), 10.0),
            ))
            .with_children(|bar| {
                bar.spawn((
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    sliced(k("controls/progress_fill_yellow.png"), 10.0),
                    RankCardPart::Fill,
                ));
            });
            card.spawn((
                Text::new(""),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_MUTED),
                RankCardPart::Next,
            ));
            card.spawn((
                Text::new(""),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_PRIMARY),
                RankCardPart::Record,
            ));
        });

    spawn_play_screen(&mut commands, &assets, &kit_fonts);
}

/// The play screen: mode cards and the map picker over a live view of the map.
fn spawn_play_screen(commands: &mut Commands, assets: &AssetServer, fonts: &UiKit) {
    let k = |p: &str| kit(assets, p);

    // The picked map's name over the 3D preview.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(36.0),
                top: Val::Px(28.0),
                max_width: Val::Px(520.0),
                padding: UiRect::axes(Val::Px(24.0), Val::Px(16.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            sliced(k("panels/panel_glass.png"), 28.0),
            ShowOn::Play,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(""),
                TextFont::from_font_size(14.0),
                TextColor(KIT_YELLOW),
                MapInfoText::Style,
            ));
            p.spawn((
                Text::new(""),
                fonts.heading(52.0),
                TextColor(TEXT_PRIMARY),
                MapInfoText::Name,
            ));
            p.spawn((
                Text::new(""),
                TextFont::from_font_size(14.0),
                TextColor(TEXT_MUTED),
                MapInfoText::Blurb,
            ));
        });

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(24.0),
                right: Val::Px(24.0),
                bottom: Val::Px(20.0),
                padding: UiRect {
                    left: Val::Px(28.0),
                    right: Val::Px(28.0),
                    top: Val::Px(14.0),
                    bottom: Val::Px(22.0),
                },
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                ..default()
            },
            sliced_rect(
                k("panels/dialog.png"),
                BorderRect {
                    min_inset: Vec2::new(28.0, 72.0),
                    max_inset: Vec2::new(28.0, 28.0),
                },
            ),
            ShowOn::Play,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|panel| {
            // Header strip.
            panel
                .spawn(Node {
                    height: Val::Px(44.0),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                })
                .with_children(|h| {
                    h.spawn((
                        Text::new("CHOOSE A MODE"),
                        fonts.heading(28.0),
                        TextColor(TEXT_PRIMARY),
                    ));
                    h.spawn((
                        Text::new(""),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_MUTED),
                        ModeSummaryText,
                    ));
                });

            // Mode cards.
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(12.0),
                    margin: UiRect::top(Val::Px(6.0)),
                    ..default()
                })
                .with_children(|row| {
                    for (i, choice) in MODE_CHOICES.iter().enumerate() {
                        row.spawn((
                            Node {
                                flex_grow: 1.0,
                                flex_basis: Val::Px(0.0),
                                min_height: Val::Px(138.0),
                                padding: UiRect::axes(Val::Px(16.0), Val::Px(14.0)),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(6.0),
                                ..default()
                            },
                            sliced(k("panels/panel.png"), 28.0),
                            MenuAction::PickMode(i),
                            ModeCard(i),
                        ))
                        .with_children(|card| {
                            card.spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(10.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|top| {
                                top.spawn((
                                    ImageNode::new(k(choice.icon)),
                                    Node {
                                        width: Val::Px(34.0),
                                        height: Val::Px(34.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                                top.spawn((
                                    Text::new(choice.title),
                                    fonts.heading(26.0),
                                    TextColor(TEXT_PRIMARY),
                                    Pickable::IGNORE,
                                    ModeCardTitle(i),
                                ));
                            });
                            card.spawn((
                                Text::new(choice.tag),
                                TextFont::from_font_size(12.0),
                                TextColor(KIT_YELLOW),
                                Pickable::IGNORE,
                            ));
                            card.spawn((
                                Text::new(choice.blurb),
                                TextFont::from_font_size(12.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });

            panel.spawn((
                Text::new("MAP"),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_MUTED),
            ));
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(10.0),
                    ..default()
                })
                .with_children(|row| {
                    for id in MapId::ALL {
                        let m = maps::meta(id);
                        row.spawn((
                            Node {
                                flex_grow: 1.0,
                                flex_basis: Val::Px(0.0),
                                height: Val::Px(52.0),
                                padding: UiRect::axes(Val::Px(16.0), Val::Px(6.0)),
                                flex_direction: FlexDirection::Column,
                                justify_content: JustifyContent::Center,
                                ..default()
                            },
                            sliced(k("rows/list_row.png"), 14.0),
                            MenuAction::PickMap(id),
                            MapTile(id),
                        ))
                        .with_children(|t| {
                            t.spawn((
                                Text::new(m.name.to_uppercase()),
                                TextFont::from_font_size(15.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                                MapTileText(id),
                            ));
                            t.spawn((
                                Text::new(format!("{}  |  {}", m.style, m.size)),
                                TextFont::from_font_size(11.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });

            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(12.0),
                    justify_content: JustifyContent::SpaceBetween,
                    margin: UiRect::top(Val::Px(6.0)),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn(Node {
                        width: Val::Px(180.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    })
                    .with_children(|w| {
                        spawn_menu_button(
                            w,
                            "BACK",
                            MenuAction::BackToHome,
                            ButtonStyle::SECONDARY,
                            50.0,
                            18.0,
                            false,
                        );
                    });
                    row.spawn(Node {
                        width: Val::Px(280.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    })
                    .with_children(|w| {
                        spawn_menu_button(
                            w,
                            "CREATE MATCH",
                            MenuAction::CreateGame,
                            ButtonStyle::PRIMARY,
                            50.0,
                            20.0,
                            false,
                        );
                    });
                });
        });
}

/// Shows the parts of the menu for the current screen.
fn show_menu_screens(
    menu: Res<MenuState>,
    net: Res<NetworkState>,
    mut q: Query<(&ShowOn, &mut Node)>,
) {
    for (show, mut node) in &mut q {
        let want = if show.visible(menu.screen, net.is_connecting) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
}

/// Highlights the picked mode and map, and shows the map's details.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_play_screen(
    assets: Res<AssetServer>,
    setup: Res<MatchSetup>,
    rank: Res<Rank>,
    mut cards: Query<(&ModeCard, &mut ImageNode), Without<MapTile>>,
    mut tiles: Query<(&MapTile, &mut ImageNode), Without<ModeCard>>,
    mut titles: Query<(&ModeCardTitle, &mut TextColor), Without<MapTileText>>,
    mut tile_texts: Query<(&MapTileText, &mut TextColor), Without<ModeCardTitle>>,
    mut info: Query<(&MapInfoText, &mut Text), Without<ModeSummaryText>>,
    mut summary: Query<&mut Text, With<ModeSummaryText>>,
) {
    if !setup.is_changed() && !rank.is_changed() {
        return;
    }
    let picked = mode_index(&setup.0);
    for (card, mut img) in &mut cards {
        let sel = card.0 == picked;
        img.image = kit(
            &assets,
            if sel {
                "panels/card_neutral.png"
            } else {
                "panels/panel.png"
            },
        );
        img.color = if sel {
            Color::WHITE
        } else {
            Color::srgb(0.8, 0.8, 0.85)
        };
    }
    for (t, mut c) in &mut titles {
        c.0 = if t.0 == picked {
            KIT_YELLOW
        } else {
            TEXT_PRIMARY
        };
    }
    for (tile, mut img) in &mut tiles {
        let sel = tile.0 == setup.0.map;
        img.image = kit(
            &assets,
            if sel {
                "rows/list_row_selected.png"
            } else {
                "rows/list_row.png"
            },
        );
    }
    for (t, mut c) in &mut tile_texts {
        c.0 = if t.0 == setup.0.map {
            KIT_YELLOW
        } else {
            TEXT_PRIMARY
        };
    }
    let m = maps::meta(setup.0.map);
    for (part, mut text) in &mut info {
        text.0 = match part {
            MapInfoText::Name => m.name.to_uppercase(),
            MapInfoText::Style => format!(
                "{}  |  {} MAP",
                m.style.to_uppercase(),
                m.size.to_uppercase()
            ),
            MapInfoText::Blurb => m.blurb.to_string(),
        };
    }
    if let Ok(mut text) = summary.single_mut() {
        let choice = &MODE_CHOICES[picked];
        text.0 = if choice.ranked {
            format!(
                "Your rank: {}  ({} RP)",
                ranked::rank_name(rank.points),
                rank.points
            )
        } else {
            "Your friends join from the hub or with your Tailcat address".to_string()
        };
    }
}

/// The player card: rank badge, name, points and progress.
#[allow(clippy::type_complexity)]
fn update_rank_card(
    assets: Res<AssetServer>,
    rank: Res<Rank>,
    mut parts: Query<(
        &RankCardPart,
        Option<&mut Text>,
        Option<&mut TextColor>,
        Option<&mut ImageNode>,
        &mut Node,
    )>,
    mut done: Local<bool>,
) {
    if *done && !rank.is_changed() {
        return;
    }
    *done = true;
    let tier = Tier::of(rank.points);
    let (progress, next) = ranked::division_progress(rank.points);
    for (part, text, color, image, mut node) in &mut parts {
        match part {
            RankCardPart::Badge => {
                if let Some(mut img) = image {
                    img.image = assets.load(tier.badge());
                }
            }
            RankCardPart::Name => {
                if let (Some(mut t), Some(mut c)) = (text, color) {
                    t.0 = ranked::rank_name(rank.points).to_uppercase();
                    c.0 = tier.color();
                }
            }
            RankCardPart::Points => {
                if let Some(mut t) = text {
                    t.0 = format!("{} RP", rank.points);
                }
            }
            RankCardPart::Fill => node.width = Val::Percent(progress * 100.0),
            RankCardPart::Next => {
                if let Some(mut t) = text {
                    t.0 = match next {
                        Some(n) => format!("{n} RP to the next division"),
                        None => "Top tier".to_string(),
                    };
                }
            }
            RankCardPart::Record => {
                if let Some(mut t) = text {
                    t.0 = format!("WINS {}   |   LOSSES {}", rank.wins, rank.losses);
                }
            }
        }
    }
}

/// Escape steps back out of the play and join screens.
fn menu_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuState>,
    overlay: Res<Overlay>,
) {
    if keys.just_pressed(KeyCode::Escape)
        && menu.screen == MenuScreen::Play
        && !overlay.blocks_input()
    {
        menu.screen = MenuScreen::Home;
    }
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
    mut addr_text_q: Query<
        (&mut Text, &mut TextColor),
        (With<JoinAddressText>, Without<MenuStatusText>),
    >,
    mut status_q: Query<
        (&mut Text, &mut TextColor),
        (With<MenuStatusText>, Without<JoinAddressText>),
    >,
) {
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
    _menu: Res<MenuState>,
    _net: Res<NetworkState>,
    mut retry_q: Query<&mut Node, With<HubRetryNode>>,
    mut status_q: Query<&mut Text, With<HubStatusText>>,
    list_q: Query<(Entity, Ref<HubSessionList>)>,
    mut shown: Local<u64>,
) {
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
                    max_width: Val::Px(360.0),
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
                    TextFont::from_font_size(14.0),
                    TextColor(TEXT_PRIMARY),
                    HudInfoText,
                ));

                if is_host {
                    panel.spawn((
                        Text::new("Tailcat address: starting..."),
                        TextFont::from_font_size(11.0),
                        TextColor(TEXT_STATUS),
                        HudAddrText,
                    ));
                }

                panel.spawn((
                    Node::default(),
                    Text::new(""),
                    TextFont::from_font_size(11.0),
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
                            "WASD: Move  |  Shift: Sprint  |  C: Crouch  |  Space: Jump  |  LMB: Shoot  |  RMB: Aim  |  R: Reload  |  Tab: Scores  |  Esc: Menu",
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
