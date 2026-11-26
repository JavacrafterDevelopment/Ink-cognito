use crate::{
    AppState, Overlay,
    game_mode::{MatchSetup, pick_random_map, random_seed},
    hub::{self, HubPlay},
    maps::{CurrentMap, MapLibrary},
    net::{NetRole, NetworkState, Transport},
    portside::{Portside, Session, Status},
    protocol::{BotLevel, GameMode, MapId, MatchSettings, MovementSpeed},
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

/// Something that can't be picked yet (a locked part).
pub const BTN_LOCKED: Color = Color::srgb(0.08, 0.09, 0.12);

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
            .add_observer(on_hover_over)
            .add_observer(on_hover_out)
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(
                Update,
                (
                    handle_join_text_input,
                    menu_escape,
                    show_menu_screens,
                    update_menu_ui,
                    update_hub_menu,
                    update_play_lobby,
                    update_match_options_ui,
                    update_rank_card,
                    style_tiles_and_tabs,
                    fit_cover_images,
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
pub enum MenuScreen {
    #[default]
    Home,
    Join,
    Play,
    Loadout,
    Stats,
    Options,
}

/// Sub-screens inside `MenuScreen::Play`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaySubScreen {
    /// Step 1: pick a game mode (Team Splat, Free-for-All, Elimination, Ranked).
    #[default]
    ModeSelect,
    /// Step 2: Call-of-Duty-style Private Match lobby (or Ranked lobby with random map).
    Lobby,
    /// Step 3 (Private Match): Match Options panel for Map, Mode & Game Rules.
    MatchOptions,
}

/// Tabs inside the Match Options panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchOptionsTab {
    #[default]
    Map,
    Rules,
}

#[derive(Resource, Default)]
pub struct MenuState {
    pub screen: MenuScreen,
    pub play_sub: PlaySubScreen,
    pub options_tab: MatchOptionsTab,
    /// If true, Loadout was opened from the Play lobby so Back/PLAY returns there.
    pub loadout_from_play: bool,
    /// If true in a private match, rolls a random map when the match starts.
    pub random_map: bool,
    join_address: String,
}

#[derive(Resource, Default)]
struct HudState {
    copied_timer: f32,
}

#[derive(Component, Clone, Copy)]
enum MenuAction {
    OpenPlay,
    PlayRanked,
    /// Mode select card: picks mode and enters the Play Lobby (Ranked rolls random map).
    ChooseMode(usize),
    /// Inside Match Options: switches non-ranked mode without leaving options.
    SetPrivateMode(GameMode),
    /// Returns from Lobby to Mode Select.
    BackToModeSelect,
    /// Opens Match Options drawer on a specific tab (`Map` or `Rules`).
    OpenMatchOptions(MatchOptionsTab),
    /// Switches tab inside Match Options.
    SwitchOptionsTab(MatchOptionsTab),
    /// Closes Match Options and returns to Play Lobby.
    CloseMatchOptions,
    /// Opens Loadout from inside the Play Lobby.
    OpenLoadoutFromPlay,
    PickMap(MapId),
    PickRandomMap,
    PickMovement(MovementSpeed),
    PickBots(BotLevel),
    PickMatchMinutes(u16),
    PickScoreLimit(u32),
    PickRoundsToWin(u32),
    PickRoundSeconds(u16),
    PickRespawnSeconds(u8),
    ResetMatchRules,
    Quit,
    CreateGame,
    OpenJoin,
    OpenLoadout,
    OpenSettings,
    OpenStats,
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
    pub const LOCKED: ButtonStyle = ButtonStyle {
        normal: BTN_LOCKED,
        hover: BTN_LOCKED,
        press: BTN_LOCKED,
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
    portside: Res<Portside>,
    library: Res<MapLibrary>,
    mut setup: ResMut<MatchSetup>,
    mut current_map: ResMut<CurrentMap>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
    settings_ui: Res<crate::settings::SettingsUiState>,
) {
    if click.button != PointerButton::Primary || settings_ui.is_listening() {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };

    let click_sfx = |commands: &mut Commands| {
        if let Some(sounds) = &sounds {
            ui_sound(commands, &sounds.click, 0.6, settings.as_deref());
        }
    };

    match *action {
        MenuAction::OpenPlay => {
            if !net.is_connecting {
                net.error_text = None;
                let return_to_lobby = menu.screen == MenuScreen::Loadout && menu.loadout_from_play;
                menu.loadout_from_play = false;
                menu.screen = MenuScreen::Play;
                if !return_to_lobby {
                    menu.play_sub = PlaySubScreen::ModeSelect;
                }
                current_map.0 = setup.0.map;
            }
        }
        MenuAction::PlayRanked => {
            if !net.is_connecting {
                net.error_text = None;
                setup.0.mode = GameMode::Elimination;
                setup.0.ranked = true;
                setup.0.reset_rules();
                // Ranked always selects a random official map.
                let rolled = pick_random_map(&MapId::BUILT_IN, Some(setup.0.map), random_seed());
                setup.0.map = rolled;
                current_map.0 = rolled;
                menu.random_map = true;
                menu.loadout_from_play = false;
                menu.screen = MenuScreen::Play;
                menu.play_sub = PlaySubScreen::Lobby;
                click_sfx(&mut commands);
            }
        }
        MenuAction::ChooseMode(i) => {
            let choice = &MODE_CHOICES[i.min(MODE_CHOICES.len() - 1)];
            setup.0.mode = choice.mode;
            setup.0.ranked = choice.ranked;
            if choice.ranked {
                setup.0.reset_rules();
                let rolled = pick_random_map(&MapId::BUILT_IN, Some(setup.0.map), random_seed());
                setup.0.map = rolled;
                current_map.0 = rolled;
                menu.random_map = true;
            } else {
                menu.random_map = false;
                current_map.0 = setup.0.map;
            }
            menu.play_sub = PlaySubScreen::Lobby;
            click_sfx(&mut commands);
        }
        MenuAction::SetPrivateMode(mode) => {
            if !setup.0.ranked {
                setup.0.mode = mode;
                click_sfx(&mut commands);
            }
        }
        MenuAction::BackToModeSelect => {
            if !net.is_connecting {
                menu.play_sub = PlaySubScreen::ModeSelect;
                click_sfx(&mut commands);
            }
        }
        MenuAction::OpenMatchOptions(tab) => {
            if !setup.0.ranked && !net.is_connecting {
                menu.options_tab = tab;
                menu.play_sub = PlaySubScreen::MatchOptions;
                click_sfx(&mut commands);
            }
        }
        MenuAction::SwitchOptionsTab(tab) => {
            menu.options_tab = tab;
            click_sfx(&mut commands);
        }
        MenuAction::CloseMatchOptions => {
            menu.play_sub = PlaySubScreen::Lobby;
            click_sfx(&mut commands);
        }
        MenuAction::OpenLoadoutFromPlay => {
            if !net.is_connecting {
                net.error_text = None;
                menu.loadout_from_play = true;
                menu.screen = MenuScreen::Loadout;
            }
        }
        MenuAction::PickMap(id) => {
            if !setup.0.ranked {
                setup.0.map = id;
                current_map.0 = id;
                menu.random_map = false;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickRandomMap => {
            if !setup.0.ranked {
                let pool: Vec<MapId> = library.ids().collect();
                let rolled = pick_random_map(&pool, Some(setup.0.map), random_seed());
                setup.0.map = rolled;
                current_map.0 = rolled;
                menu.random_map = true;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickMovement(movement) => {
            if !setup.0.ranked {
                setup.0.movement = movement;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickBots(level) => {
            if !setup.0.ranked {
                setup.0.bots = level;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickMatchMinutes(mins) => {
            if !setup.0.ranked {
                setup.0.match_minutes = mins;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickScoreLimit(limit) => {
            if !setup.0.ranked {
                setup.0.score_limit = limit;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickRoundsToWin(rounds) => {
            if !setup.0.ranked {
                setup.0.rounds_to_win = rounds;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickRoundSeconds(secs) => {
            if !setup.0.ranked {
                setup.0.round_seconds = secs;
                click_sfx(&mut commands);
            }
        }
        MenuAction::PickRespawnSeconds(secs) => {
            if !setup.0.ranked {
                setup.0.respawn_seconds = secs;
                click_sfx(&mut commands);
            }
        }
        MenuAction::ResetMatchRules => {
            if !setup.0.ranked {
                setup.0.reset_rules();
                click_sfx(&mut commands);
            }
        }
        MenuAction::Quit => {
            exit.write(AppExit::Success);
        }
        MenuAction::CreateGame => {
            if !net.is_connecting {
                if setup.0.ranked {
                    setup.0.reset_rules();
                    let rolled = pick_random_map(&MapId::BUILT_IN, None, random_seed());
                    setup.0.map = rolled;
                    current_map.0 = rolled;
                } else if menu.random_map {
                    let pool: Vec<MapId> = library.ids().collect();
                    let rolled = pick_random_map(&pool, None, random_seed());
                    setup.0.map = rolled;
                    current_map.0 = rolled;
                }
                if let Some(sounds) = sounds {
                    ui_sound(&mut commands, &sounds.confirm, 0.7, settings.as_deref());
                }
                net.start_host();
            }
        }
        MenuAction::OpenLoadout => {
            if !net.is_connecting {
                net.error_text = None;
                menu.loadout_from_play = false;
                menu.screen = MenuScreen::Loadout;
            }
        }
        MenuAction::OpenSettings => {
            if !net.is_connecting {
                net.error_text = None;
                menu.loadout_from_play = false;
                menu.screen = MenuScreen::Options;
            }
        }
        MenuAction::OpenStats => {
            if !net.is_connecting {
                net.error_text = None;
                menu.loadout_from_play = false;
                menu.screen = MenuScreen::Stats;
            }
        }
        MenuAction::OpenJoin => {
            if !net.is_connecting {
                net.error_text = None;
                menu.loadout_from_play = false;
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
                menu.loadout_from_play = false;
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
        blurb: "Competitive Elimination on a random official map. Win to climb; quitting counts as a loss.",
        tag: "FIRST TO 5  |  RANDOM MAP",
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
    PlayModeSelect,
    PlayLobby,
    PlayMatchOptions,
    /// The solid backdrop (not on the play screen: the map shows there).
    Backdrop,
}

impl ShowOn {
    fn visible(self, menu: &MenuState) -> bool {
        match self {
            ShowOn::Home => menu.screen == MenuScreen::Home,
            ShowOn::Join => menu.screen == MenuScreen::Join,
            ShowOn::PlayModeSelect => {
                menu.screen == MenuScreen::Play && menu.play_sub == PlaySubScreen::ModeSelect
            }
            ShowOn::PlayLobby => {
                menu.screen == MenuScreen::Play && menu.play_sub == PlaySubScreen::Lobby
            }
            ShowOn::PlayMatchOptions => {
                menu.screen == MenuScreen::Play && menu.play_sub == PlaySubScreen::MatchOptions
            }
            ShowOn::Backdrop => menu.screen != MenuScreen::Play,
        }
    }
}

/// Elements in the Play Lobby visible only for Private Match (hidden in Ranked).
#[derive(Component)]
struct PrivateOnly;

/// Elements in the Play Lobby visible only for Ranked (hidden in Private Match).
#[derive(Component)]
struct RankedOnly;

#[derive(Component)]
enum LobbyPart {
    Eyebrow,
    ModeIcon,
    ModeTitle,
    ModeTag,
    StartBtnText,
    MapEyebrow,
    MapCue,
    MapName,
    MapStyle,
    MapBlurb,
    SummaryHeading,
    SummaryMode,
    SummaryMap,
    SummaryMovement,
    SummaryBots,
    SummaryRuleALabel,
    SummaryRuleAVal,
    SummaryRuleBLabel,
    SummaryRuleBVal,
    SummaryRuleCLabel,
    SummaryRuleCVal,
}

#[derive(Component)]
struct OptionsTabBtn(MatchOptionsTab);

#[derive(Component)]
struct OptionsTabText(MatchOptionsTab);

#[derive(Component)]
struct OptionsTabPane(MatchOptionsTab);

/// Shows only when the selected private mode is Team Splat or Free-for-All.
#[derive(Component)]
struct TimedModeRuleRow;

/// Shows only when the selected private mode is Elimination.
#[derive(Component)]
struct ElimModeRuleRow;

#[derive(Component)]
struct MapTile(MapId);

#[derive(Component)]
struct MapTileText(MapId);

#[derive(Component)]
struct RandomMapTile;

#[derive(Component)]
struct RandomMapTileText;

#[derive(Component)]
enum OptionsMapPreview {
    Name,
    Style,
    Blurb,
}

#[derive(Component)]
struct OptionsRuleMapValue;

/// Selectable pill button inside Match Options -> Game Rules.
#[derive(Component, Clone, Copy)]
enum RuleOption {
    Mode(GameMode),
    Movement(MovementSpeed),
    Bots(BotLevel),
    MatchMinutes(u16),
    ScoreLimit(u32),
    RoundsToWin(u32),
    RoundSeconds(u16),
    RespawnSeconds(u8),
}

impl RuleOption {
    fn selected(self, s: &MatchSettings) -> bool {
        match self {
            RuleOption::Mode(m) => s.mode == m,
            RuleOption::Movement(mv) => s.movement == mv,
            RuleOption::Bots(level) => s.bots == level,
            RuleOption::MatchMinutes(m) => s.effective_match_minutes() == m,
            RuleOption::ScoreLimit(l) => s.score_limit == l,
            RuleOption::RoundsToWin(r) => s.effective_rounds_to_win() == r,
            RuleOption::RoundSeconds(sec) => s.effective_round_seconds().round() as u16 == sec,
            RuleOption::RespawnSeconds(sec) => s.effective_respawn_seconds().round() as u8 == sec,
        }
    }
}

#[derive(Component)]
struct RuleOptionText(RuleOption);

#[derive(Component)]
enum RankCardPart {
    Badge,
    Name,
    Points,
    Fill,
    Next,
    Record,
}

/// The menu's solid backdrop colours (top and bottom).
const MENU_BG_TOP: Color = Color::srgb(0.075, 0.094, 0.125);
const MENU_BG_BOTTOM: Color = Color::srgb(0.035, 0.043, 0.059);
/// The strip behind the tabs.
const TOP_BAR_BG: Color = Color::srgba(0.02, 0.025, 0.035, 0.96);
pub const TOP_BAR_HEIGHT: f32 = 76.0;
/// A tile's border at rest and under the mouse.
const TILE_BORDER: Color = Color::srgba(0.55, 0.62, 0.72, 0.25);
const TILE_BORDER_HOVER: Color = KIT_YELLOW;
const TILE_BG: Color = Color::srgb(0.085, 0.105, 0.14);

/// A tab in the top bar.
#[derive(Component)]
struct NavTab(MenuScreen);

/// The yellow line under the active tab.
#[derive(Component)]
struct NavUnderline(MenuScreen);

/// Something that lights up under the mouse (tiles and tabs).
#[derive(Component, Default)]
struct Hover(bool);

/// A clickable tile on the home screen.
#[derive(Component)]
struct Tile;

/// An image cropped to fill its box without stretching (like CSS `cover`).
#[derive(Component)]
struct CoverImage;

/// The bar at the bottom with connection status, errors and Cancel.
#[derive(Component)]
struct StatusBar;

/// The Cancel button, only while connecting.
#[derive(Component)]
struct CancelWrap;

fn spawn_main_menu(
    mut commands: Commands,
    mut menu: ResMut<MenuState>,
    mut overlay: ResMut<Overlay>,
    assets: Res<AssetServer>,
    kit_fonts: Res<UiKit>,
    library: Res<MapLibrary>,
) {
    menu.screen = MenuScreen::Home;
    menu.play_sub = PlaySubScreen::ModeSelect;
    menu.loadout_from_play = false;
    *overlay = Overlay::default();
    let k = |p: &str| kit(&assets, p);
    let fonts = &*kit_fonts;
    let map_count = library.ids().count();

    // A solid backdrop: nothing of the 3D world shows through, at any size.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            ..default()
        },
        BackgroundColor(MENU_BG_BOTTOM),
        BackgroundGradient::from(LinearGradient::to_bottom(vec![
            ColorStop::auto(MENU_BG_TOP),
            ColorStop::auto(MENU_BG_BOTTOM),
        ])),
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

    spawn_top_bar(&mut commands, &assets, fonts);

    // Home: a big PLAY tile, ranked, join, loadout and the hub.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(48.0),
                right: Val::Px(48.0),
                top: Val::Px(TOP_BAR_HEIGHT + 28.0),
                bottom: Val::Px(44.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(20.0),
                ..default()
            },
            Pickable::IGNORE,
            ShowOn::Home,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|row| {
            // The hero tile.
            row.spawn((
                tile_node(Node {
                    flex_grow: 1.8,
                    flex_basis: Val::Px(0.0),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::FlexEnd,
                    overflow: Overflow::clip(),
                    ..default()
                }),
                BackgroundColor(TILE_BG),
                BorderColor::all(TILE_BORDER),
                Tile,
                Hover::default(),
                MenuAction::OpenPlay,
            ))
            .with_children(|hero| {
                hero.spawn((
                    ImageNode::new(assets.load("branding/menu_background.png"))
                        .with_mode(NodeImageMode::Stretch),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        top: Val::Px(0.0),
                        bottom: Val::Px(0.0),
                        ..default()
                    },
                    CoverImage,
                    Pickable::IGNORE,
                ));
                hero.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        padding: UiRect {
                            left: Val::Px(32.0),
                            right: Val::Px(32.0),
                            top: Val::Px(60.0),
                            bottom: Val::Px(26.0),
                        },
                        ..default()
                    },
                    BackgroundGradient::from(LinearGradient::to_bottom(vec![
                        ColorStop::auto(Color::NONE),
                        ColorStop::percent(Color::srgba(0.02, 0.03, 0.04, 0.85), 55.0),
                        ColorStop::auto(Color::srgba(0.02, 0.03, 0.04, 0.95)),
                    ])),
                    Pickable::IGNORE,
                ))
                .with_children(|text| {
                    text.spawn((
                        Text::new("PLAY"),
                        fonts.heading(72.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    text.spawn((
                        Text::new(format!(
                            "TEAM SPLAT  |  FREE-FOR-ALL  |  ELIMINATION  |  {map_count} MAPS"
                        )),
                        TextFont::from_font_size(15.0),
                        TextColor(KIT_YELLOW),
                        Pickable::IGNORE,
                    ));
                    text.spawn((
                        Text::new("Pick a mode, configure match options & map, or queue Ranked on a random map."),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                    ));
                });
            });

            // The column on the right.
            row.spawn((
                Node {
                    flex_grow: 1.0,
                    flex_basis: Val::Px(0.0),
                    min_width: Val::Px(340.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(20.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|col| {
                spawn_ranked_tile(col, &assets, fonts);

                col.spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(20.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|small| {
                    spawn_small_tile(
                        small,
                        &assets,
                        fonts,
                        "icons/forward.png",
                        "JOIN",
                        "By Tailcat address",
                        MenuAction::OpenJoin,
                    );
                    spawn_small_tile(
                        small,
                        &assets,
                        fonts,
                        "icons/paintball_gun.png",
                        "LOADOUT",
                        "Guns, parts and camos",
                        MenuAction::OpenLoadout,
                    );
                });

                // Games in your Portside hub.
                col.spawn((
                    tile_node(Node {
                        flex_grow: 1.0,
                        min_height: Val::Px(120.0),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        row_gap: Val::Px(8.0),
                        padding: UiRect::all(Val::Px(18.0)),
                        overflow: Overflow::clip_y(),
                        ..default()
                    }),
                    BackgroundColor(TILE_BG),
                    BorderColor::all(TILE_BORDER),
                    HubSection,
                ))
                .with_children(|hub| {
                    hub.spawn((
                        Text::new("YOUR PORTSIDE HUB"),
                        fonts.heading(22.0),
                        TextColor(TEXT_PRIMARY),
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
            });
        });

    // Join screen: the Tailcat address input.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(TOP_BAR_HEIGHT),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            ShowOn::Join,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(580.0),
                    padding: UiRect::all(Val::Px(30.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    row_gap: Val::Px(12.0),
                    ..default()
                },
                sliced(k("panels/panel.png"), 28.0),
            ))
            .with_children(|join| {
                join.spawn((
                    Text::new("JOIN A MATCH"),
                    fonts.heading(40.0),
                    TextColor(TEXT_PRIMARY),
                ));
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
        });

    // Status, errors and Cancel, at the bottom.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(56.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Pickable::IGNORE,
            GlobalZIndex(5),
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    display: Display::None,
                    min_width: Val::Px(420.0),
                    max_width: Val::Px(760.0),
                    padding: UiRect::axes(Val::Px(26.0), Val::Px(18.0)),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(18.0),
                    ..default()
                },
                sliced(k("panels/dialog.png"), 28.0),
                StatusBar,
            ))
            .with_children(|bar| {
                bar.spawn((
                    Text::new(""),
                    TextFont::from_font_size(15.0),
                    TextColor(TEXT_STATUS),
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                    MenuStatusText,
                ));
                bar.spawn((
                    Node {
                        width: Val::Px(150.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    CancelWrap,
                ))
                .with_children(|w| {
                    spawn_menu_button(
                        w,
                        "CANCEL",
                        MenuAction::CancelConnect,
                        ButtonStyle::DANGER,
                        42.0,
                        15.0,
                        false,
                    );
                });
            });
        });

    spawn_play_screen(&mut commands, &assets, &kit_fonts, &library);
}

/// A tile's box: a thin border and square-ish corners.
fn tile_node(node: Node) -> Node {
    Node {
        border: UiRect::all(Val::Px(2.0)),
        border_radius: BorderRadius::all(Val::Px(6.0)),
        ..node
    }
}

/// The top bar: logo, tabs, and the player's rank on the right.
fn spawn_top_bar(commands: &mut Commands, assets: &AssetServer, fonts: &UiKit) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                height: Val::Px(TOP_BAR_HEIGHT),
                padding: UiRect::axes(Val::Px(28.0), Val::Px(0.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(TOP_BAR_BG),
            BorderColor::all(TILE_BORDER),
            GlobalZIndex(2),
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|bar| {
            let tabs: [(&str, MenuAction, MenuScreen); 5] = [
                ("HOME", MenuAction::BackToHome, MenuScreen::Home),
                ("PLAY", MenuAction::OpenPlay, MenuScreen::Play),
                ("LOADOUT", MenuAction::OpenLoadout, MenuScreen::Loadout),
                ("STATS", MenuAction::OpenStats, MenuScreen::Stats),
                ("OPTIONS", MenuAction::OpenSettings, MenuScreen::Options),
            ];
            for (label, action, screen) in tabs {
                bar.spawn((
                    Node {
                        height: Val::Percent(100.0),
                        padding: UiRect::axes(Val::Px(18.0), Val::Px(0.0)),
                        flex_direction: FlexDirection::Column,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    action,
                    NavTab(screen),
                    Hover::default(),
                ))
                .with_children(|tab| {
                    tab.spawn((
                        Text::new(label),
                        fonts.heading(28.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                    ));
                    tab.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(12.0),
                            right: Val::Px(12.0),
                            bottom: Val::Px(0.0),
                            height: Val::Px(4.0),
                            ..default()
                        },
                        BackgroundColor(KIT_YELLOW),
                        Visibility::Hidden,
                        Pickable::IGNORE,
                        NavUnderline(screen),
                    ));
                });
            }

            // Push the profile to the right.
            bar.spawn((
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
                Pickable::IGNORE,
            ));

            bar.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(12.0),
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(6.0)),
                    margin: UiRect::right(Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    ..default()
                },
                BackgroundColor(TILE_BG),
                BorderColor::all(TILE_BORDER),
                Pickable::IGNORE,
            ))
            .with_children(|profile| {
                profile.spawn((
                    ImageNode::new(kit(assets, "badges/rank_bronze.png")),
                    Node {
                        width: Val::Px(44.0),
                        height: Val::Px(44.0),
                        ..default()
                    },
                    RankCardPart::Badge,
                ));
                profile
                    .spawn(Node {
                        flex_direction: FlexDirection::Column,
                        ..default()
                    })
                    .with_children(|c| {
                        c.spawn((
                            Text::new(""),
                            fonts.heading(20.0),
                            TextColor(TEXT_PRIMARY),
                            RankCardPart::Name,
                        ));
                        c.spawn((
                            Text::new(""),
                            TextFont::from_font_size(12.0),
                            TextColor(KIT_YELLOW),
                            RankCardPart::Points,
                        ));
                    });
            });

            // Quit.
            bar.spawn((
                Node {
                    width: Val::Px(48.0),
                    height: Val::Px(48.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    ..default()
                },
                BackgroundColor(TILE_BG),
                BorderColor::all(TILE_BORDER),
                Tile,
                Hover::default(),
                MenuAction::Quit,
            ))
            .with_children(|q| {
                q.spawn((
                    ImageNode::new(kit(assets, "icons/exit.png")),
                    Node {
                        width: Val::Px(26.0),
                        height: Val::Px(26.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        });
}

/// The ranked tile: badge, division, progress and record.
fn spawn_ranked_tile(col: &mut ChildSpawnerCommands, assets: &AssetServer, fonts: &UiKit) {
    col.spawn((
        tile_node(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(20.0),
            padding: UiRect::all(Val::Px(22.0)),
            ..default()
        }),
        BackgroundColor(TILE_BG),
        BackgroundGradient::from(LinearGradient::to_right(vec![
            ColorStop::auto(Color::srgba(0.96, 0.77, 0.17, 0.10)),
            ColorStop::auto(Color::NONE),
        ])),
        BorderColor::all(TILE_BORDER),
        Tile,
        Hover::default(),
        MenuAction::PlayRanked,
    ))
    .with_children(|tile| {
        tile.spawn((
            ImageNode::new(kit(assets, "badges/rank_bronze.png")),
            Node {
                width: Val::Px(104.0),
                height: Val::Px(104.0),
                flex_shrink: 0.0,
                ..default()
            },
            Pickable::IGNORE,
            RankCardPart::Badge,
        ));
        tile.spawn((
            Node {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((
                Text::new("RANKED"),
                fonts.heading(34.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
            c.spawn((
                Text::new(""),
                TextFont::from_font_size(16.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
                RankCardPart::Name,
            ));
            c.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(16.0),
                    padding: UiRect::all(Val::Px(3.0)),
                    margin: UiRect::vertical(Val::Px(4.0)),
                    ..default()
                },
                sliced(kit(assets, "controls/progress_frame.png"), 8.0),
                Pickable::IGNORE,
            ))
            .with_children(|bar| {
                bar.spawn((
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    sliced(kit(assets, "controls/progress_fill_yellow.png"), 6.0),
                    Pickable::IGNORE,
                    RankCardPart::Fill,
                ));
            });
            c.spawn((
                Text::new(""),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
                RankCardPart::Next,
            ));
            c.spawn((
                Text::new(""),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
                RankCardPart::Record,
            ));
        });
    });
}

/// A small square-ish tile with an icon and two lines.
fn spawn_small_tile(
    row: &mut ChildSpawnerCommands,
    assets: &AssetServer,
    fonts: &UiKit,
    icon: &str,
    title: &str,
    line: &str,
    action: MenuAction,
) {
    row.spawn((
        tile_node(Node {
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            height: Val::Px(112.0),
            padding: UiRect::all(Val::Px(18.0)),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        }),
        BackgroundColor(TILE_BG),
        BorderColor::all(TILE_BORDER),
        Tile,
        Hover::default(),
        action,
    ))
    .with_children(|t| {
        t.spawn((
            ImageNode::new(kit(assets, icon)),
            Node {
                width: Val::Px(34.0),
                height: Val::Px(34.0),
                ..default()
            },
            Pickable::IGNORE,
        ));
        t.spawn((
            Node {
                flex_direction: FlexDirection::Column,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((
                Text::new(title.to_string()),
                fonts.heading(26.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
            c.spawn((
                Text::new(line.to_string()),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
            ));
        });
    });
}

fn on_hover_over(
    over: On<PointerOver>,
    mut q: Query<&mut Hover>,
    mut commands: Commands,
    sounds: Option<Res<UiSounds>>,
    settings: Option<Res<AudioSettings>>,
) {
    if let Ok(mut h) = q.get_mut(over.entity) {
        h.0 = true;
        if let Some(sounds) = sounds {
            ui_sound(&mut commands, &sounds.hover, 0.35, settings.as_deref());
        }
    }
}

fn on_hover_out(out: On<PointerOut>, mut q: Query<&mut Hover>) {
    if let Ok(mut h) = q.get_mut(out.entity) {
        h.0 = false;
    }
}

/// Lights up tiles and tabs under the mouse, and the tab you're on.
#[allow(clippy::type_complexity)]
fn style_tiles_and_tabs(
    menu: Res<MenuState>,
    mut tiles: Query<(&Hover, &mut BorderColor), (With<Tile>, Changed<Hover>)>,
    tabs: Query<(&NavTab, &Hover, &Children)>,
    mut texts: Query<&mut TextColor>,
    mut lines: Query<(&NavUnderline, &mut Visibility)>,
) {
    for (h, mut border) in &mut tiles {
        *border = BorderColor::all(if h.0 { TILE_BORDER_HOVER } else { TILE_BORDER });
    }
    for (tab, h, children) in &tabs {
        let color = if tab.0 == menu.screen {
            TEXT_PRIMARY
        } else if h.0 {
            KIT_YELLOW
        } else {
            TEXT_MUTED
        };
        for &c in children {
            if let Ok(mut t) = texts.get_mut(c)
                && t.0 != color
            {
                t.0 = color;
            }
        }
    }
    for (line, mut vis) in &mut lines {
        let want = if line.0 == menu.screen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

/// Crops [`CoverImage`]s to their box so the art fills it without stretching.
fn fit_cover_images(
    images: Res<Assets<Image>>,
    mut q: Query<(&mut ImageNode, &ComputedNode), With<CoverImage>>,
) {
    for (mut img, node) in &mut q {
        let Some(image) = images.get(&img.image) else {
            continue;
        };
        let size = image.size_f32();
        let boxed = node.size();
        if size.x <= 0.0 || size.y <= 0.0 || boxed.x <= 0.0 || boxed.y <= 0.0 {
            continue;
        }
        let want = boxed.x / boxed.y;
        let have = size.x / size.y;
        let crop = if have > want {
            let w = size.y * want;
            Rect::new((size.x - w) * 0.5, 0.0, (size.x + w) * 0.5, size.y)
        } else {
            let h = size.x / want;
            Rect::new(0.0, (size.y - h) * 0.5, size.x, (size.y + h) * 0.5)
        };
        if img.rect != Some(crop) {
            img.rect = Some(crop);
        }
    }
}

/// The play screen: 1) Mode Select, 2) COD-style Private Match / Ranked Lobby,
/// 3) Match Options drawer (Map + Game Rules), all over a live 3D view of the map.
fn spawn_play_screen(
    commands: &mut Commands,
    assets: &AssetServer,
    fonts: &UiKit,
    library: &MapLibrary,
) {
    spawn_play_mode_select(commands, assets, fonts);
    spawn_play_lobby(commands, assets, fonts);
    spawn_play_match_options(commands, assets, fonts, library);
}

/// Step 1 of Play: clean 4-card game mode selector over the live 3D map.
fn spawn_play_mode_select(commands: &mut Commands, assets: &AssetServer, fonts: &UiKit) {
    let k = |p: &str| kit(assets, p);

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(TOP_BAR_HEIGHT),
                bottom: Val::Px(0.0),
                padding: UiRect {
                    left: Val::Px(48.0),
                    right: Val::Px(48.0),
                    top: Val::Px(28.0),
                    bottom: Val::Px(32.0),
                },
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
            BackgroundGradient::from(LinearGradient::to_bottom(vec![
                ColorStop::auto(Color::srgba(0.02, 0.03, 0.05, 0.62)),
                ColorStop::percent(Color::srgba(0.02, 0.03, 0.05, 0.25), 35.0),
                ColorStop::auto(Color::srgba(0.02, 0.03, 0.05, 0.78)),
            ])),
            Pickable::IGNORE,
            ShowOn::PlayModeSelect,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            // Header block.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|hdr| {
                hdr.spawn((
                    Text::new("PLAY  /  SELECT MODE"),
                    TextFont::from_font_size(13.0),
                    TextColor(KIT_YELLOW),
                    Pickable::IGNORE,
                ));
                hdr.spawn((
                    Text::new("CHOOSE YOUR MODE"),
                    fonts.heading(52.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                ));
                hdr.spawn((
                    Text::new("Pick a mode to enter the match lobby. Ranked plays Elimination on a random official map."),
                    TextFont::from_font_size(14.0),
                    TextColor(TEXT_MUTED),
                    Pickable::IGNORE,
                ));
            });

            // 4 vertical mode cards.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(18.0),
                    align_items: AlignItems::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                for (i, choice) in MODE_CHOICES.iter().enumerate() {
                    let accent_grad = if choice.ranked {
                        LinearGradient::to_bottom(vec![
                            ColorStop::auto(Color::srgba(0.96, 0.77, 0.17, 0.16)),
                            ColorStop::auto(Color::srgba(0.05, 0.07, 0.10, 0.94)),
                        ])
                    } else {
                        LinearGradient::to_bottom(vec![
                            ColorStop::auto(Color::srgba(0.09, 0.12, 0.18, 0.90)),
                            ColorStop::auto(Color::srgba(0.04, 0.06, 0.09, 0.94)),
                        ])
                    };

                    row.spawn((
                        tile_node(Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            min_height: Val::Px(270.0),
                            padding: UiRect::all(Val::Px(22.0)),
                            flex_direction: FlexDirection::Column,
                            justify_content: JustifyContent::SpaceBetween,
                            row_gap: Val::Px(14.0),
                            ..default()
                        }),
                        BackgroundColor(Color::srgba(0.05, 0.07, 0.11, 0.92)),
                        BackgroundGradient::from(accent_grad),
                        BorderColor::all(TILE_BORDER),
                        Tile,
                        Hover::default(),
                        MenuAction::ChooseMode(i),
                    ))
                    .with_children(|card| {
                        card.spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(10.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|top| {
                            top.spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::SpaceBetween,
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|icon_row| {
                                icon_row.spawn((
                                    ImageNode::new(k(choice.icon)),
                                    Node {
                                        width: Val::Px(46.0),
                                        height: Val::Px(46.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                                icon_row
                                    .spawn((
                                        Node {
                                            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                                            border_radius: BorderRadius::all(Val::Px(4.0)),
                                            ..default()
                                        },
                                        BackgroundColor(if choice.ranked {
                                            Color::srgba(0.96, 0.77, 0.17, 0.22)
                                        } else {
                                            Color::srgba(0.24, 0.60, 0.98, 0.18)
                                        }),
                                        Pickable::IGNORE,
                                    ))
                                    .with_children(|pill| {
                                        pill.spawn((
                                            Text::new(if choice.ranked {
                                                "RANDOM MAP"
                                            } else {
                                                "PRIVATE MATCH"
                                            }),
                                            TextFont::from_font_size(10.0),
                                            TextColor(if choice.ranked {
                                                KIT_YELLOW
                                            } else {
                                                TEXT_STATUS
                                            }),
                                            Pickable::IGNORE,
                                        ));
                                    });
                            });

                            top.spawn((
                                Text::new(choice.title),
                                fonts.heading(36.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                            ));
                            top.spawn((
                                Text::new(choice.tag),
                                TextFont::from_font_size(12.0),
                                TextColor(KIT_YELLOW),
                                Pickable::IGNORE,
                            ));
                            top.spawn((
                                Text::new(choice.blurb),
                                TextFont::from_font_size(13.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                            ));
                        });

                        // Bottom footer on each mode card.
                        if choice.ranked {
                            card.spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(10.0),
                                    padding: UiRect::top(Val::Px(10.0)),
                                    border: UiRect::top(Val::Px(1.0)),
                                    ..default()
                                },
                                BorderColor::all(TILE_BORDER),
                                Pickable::IGNORE,
                            ))
                            .with_children(|r| {
                                r.spawn((
                                    ImageNode::new(k("badges/rank_bronze.png")),
                                    Node {
                                        width: Val::Px(34.0),
                                        height: Val::Px(34.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                    RankCardPart::Badge,
                                ));
                                r.spawn((
                                    Node {
                                        flex_direction: FlexDirection::Column,
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ))
                                .with_children(|rc| {
                                    rc.spawn((
                                        Text::new(""),
                                        fonts.heading(18.0),
                                        TextColor(TEXT_PRIMARY),
                                        Pickable::IGNORE,
                                        RankCardPart::Name,
                                    ));
                                    rc.spawn((
                                        Text::new(""),
                                        TextFont::from_font_size(11.0),
                                        TextColor(KIT_YELLOW),
                                        Pickable::IGNORE,
                                        RankCardPart::Points,
                                    ));
                                });
                            });
                        } else {
                            card.spawn((
                                Node {
                                    padding: UiRect::top(Val::Px(10.0)),
                                    border: UiRect::top(Val::Px(1.0)),
                                    flex_direction: FlexDirection::Row,
                                    justify_content: JustifyContent::SpaceBetween,
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                BorderColor::all(TILE_BORDER),
                                Pickable::IGNORE,
                            ))
                            .with_children(|ft| {
                                ft.spawn((
                                    Text::new("CONFIGURE & PLAY"),
                                    TextFont::from_font_size(12.0),
                                    TextColor(KIT_YELLOW),
                                    Pickable::IGNORE,
                                ));
                                ft.spawn((
                                    ImageNode::new(k("icons/forward.png")),
                                    Node {
                                        width: Val::Px(18.0),
                                        height: Val::Px(18.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                            });
                        }
                    });
                }
            });

            // Bottom back button.
            root.spawn((
                Node {
                    width: Val::Px(190.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|w| {
                spawn_menu_button(
                    w,
                    "BACK TO HOME",
                    MenuAction::BackToHome,
                    ButtonStyle::SECONDARY,
                    46.0,
                    15.0,
                    false,
                );
            });
        });
}

/// Step 2 of Play: Call-of-Duty-style Private Match Lobby (or Ranked Lobby).
/// Left column has the stacked match controls + bottom-left map preview card;
/// center stays open for the live orbiting 3D map; right side shows match rules.
fn spawn_play_lobby(commands: &mut Commands, assets: &AssetServer, fonts: &UiKit) {
    let k = |p: &str| kit(assets, p);

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(TOP_BAR_HEIGHT),
                bottom: Val::Px(0.0),
                padding: UiRect {
                    left: Val::Px(36.0),
                    right: Val::Px(36.0),
                    top: Val::Px(24.0),
                    bottom: Val::Px(26.0),
                },
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Stretch,
                ..default()
            },
            // Subtle side vignettes so the left stack and right card read crisply
            // while keeping the live 3D arena clear in the middle.
            BackgroundGradient::from(LinearGradient::to_right(vec![
                ColorStop::auto(Color::srgba(0.02, 0.03, 0.05, 0.72)),
                ColorStop::percent(Color::NONE, 36.0),
                ColorStop::percent(Color::NONE, 68.0),
                ColorStop::auto(Color::srgba(0.02, 0.03, 0.05, 0.65)),
            ])),
            Pickable::IGNORE,
            ShowOn::PlayLobby,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            // === LEFT COLUMN: COD Private Match Action Stack + Map Card ===
            root.spawn((
                Node {
                    width: Val::Px(380.0),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::SpaceBetween,
                    row_gap: Val::Px(16.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|left| {
                // Top: Mode Header + Vertical COD Button Stack.
                left.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(10.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|top| {
                    top.spawn((
                        Text::new("PRIVATE MATCH  /  LOBBY"),
                        TextFont::from_font_size(12.0),
                        TextColor(KIT_YELLOW),
                        Pickable::IGNORE,
                        LobbyPart::Eyebrow,
                    ));
                    top.spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(12.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|title_row| {
                        title_row.spawn((
                            ImageNode::new(k("icons/team.png")),
                            Node {
                                width: Val::Px(42.0),
                                height: Val::Px(42.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                            LobbyPart::ModeIcon,
                        ));
                        title_row.spawn((
                            Text::new("TEAM SPLAT"),
                            fonts.heading(44.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                            LobbyPart::ModeTitle,
                        ));
                    });
                    top.spawn((
                        Text::new(""),
                        TextFont::from_font_size(12.0),
                        TextColor(TEXT_MUTED),
                        Node {
                            margin: UiRect::bottom(Val::Px(8.0)),
                            ..default()
                        },
                        Pickable::IGNORE,
                        LobbyPart::ModeTag,
                    ));

                    // Primary COD START MATCH button.
                    top.spawn((
                        Node {
                            height: Val::Px(58.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                        BackgroundColor(ButtonStyle::PRIMARY.normal),
                        ButtonStyle::PRIMARY,
                        MenuAction::CreateGame,
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new("START MATCH"),
                            fonts.heading(24.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                            LobbyPart::StartBtnText,
                        ));
                    });

                    // Private Match options stack (hidden in Ranked).
                    top.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(10.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                        PrivateOnly,
                    ))
                    .with_children(|priv_btns| {
                        spawn_menu_button(
                            priv_btns,
                            "MATCH OPTIONS",
                            MenuAction::OpenMatchOptions(MatchOptionsTab::Rules),
                            ButtonStyle::SECONDARY,
                            48.0,
                            16.0,
                            false,
                        );
                        spawn_menu_button(
                            priv_btns,
                            "CHANGE MAP",
                            MenuAction::OpenMatchOptions(MatchOptionsTab::Map),
                            ButtonStyle::SECONDARY,
                            48.0,
                            16.0,
                            false,
                        );
                    });

                    spawn_menu_button(
                        top,
                        "EDIT LOADOUT",
                        MenuAction::OpenLoadoutFromPlay,
                        ButtonStyle::SECONDARY,
                        46.0,
                        15.0,
                        false,
                    );
                    spawn_menu_button(
                        top,
                        "CHANGE MODE",
                        MenuAction::BackToModeSelect,
                        ButtonStyle::SECONDARY,
                        44.0,
                        14.0,
                        false,
                    );
                });

                // Bottom-Left: Signature COD Map Preview Card.
                left.spawn((
                    tile_node(Node {
                        padding: UiRect::axes(Val::Px(18.0), Val::Px(14.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        ..default()
                    }),
                    BackgroundColor(Color::srgba(0.05, 0.07, 0.11, 0.92)),
                    BorderColor::all(TILE_BORDER),
                    Tile,
                    Hover::default(),
                    MenuAction::OpenMatchOptions(MatchOptionsTab::Map),
                ))
                .with_children(|card| {
                    card.spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|hdr| {
                        hdr.spawn((
                            Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(8.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|l| {
                            l.spawn((
                                ImageNode::new(k("icons/map.png")),
                                Node {
                                    width: Val::Px(18.0),
                                    height: Val::Px(18.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                            l.spawn((
                                Text::new("SELECTED MAP"),
                                TextFont::from_font_size(11.0),
                                TextColor(KIT_YELLOW),
                                Pickable::IGNORE,
                                LobbyPart::MapEyebrow,
                            ));
                        });
                        hdr.spawn((
                            Text::new("CLICK TO CHANGE >"),
                            TextFont::from_font_size(10.0),
                            TextColor(TEXT_STATUS),
                            Pickable::IGNORE,
                            LobbyPart::MapCue,
                        ));
                    });

                    card.spawn((
                        Text::new("SPLAT PARK"),
                        fonts.heading(34.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                        LobbyPart::MapName,
                    ));
                    card.spawn((
                        Text::new(""),
                        TextFont::from_font_size(12.0),
                        TextColor(KIT_YELLOW),
                        Pickable::IGNORE,
                        LobbyPart::MapStyle,
                    ));
                    card.spawn((
                        Text::new(""),
                        TextFont::from_font_size(12.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                        LobbyPart::MapBlurb,
                    ));
                });
            });

            // === RIGHT COLUMN: Clean Floating Match Summary / Rules Card ===
            root.spawn((
                Node {
                    width: Val::Px(350.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(14.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|right| {
                // Active Rules Summary Panel.
                right
                    .spawn((
                        tile_node(Node {
                            padding: UiRect::all(Val::Px(20.0)),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(10.0),
                            ..default()
                        }),
                        BackgroundColor(Color::srgba(0.05, 0.07, 0.11, 0.90)),
                        BorderColor::all(TILE_BORDER),
                        Pickable::IGNORE,
                    ))
                    .with_children(|panel| {
                        panel.spawn((
                            Text::new("MATCH RULES"),
                            fonts.heading(24.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                            LobbyPart::SummaryHeading,
                        ));

                        spawn_summary_row(panel, "MODE", LobbyPart::SummaryMode);
                        spawn_summary_row(panel, "MAP", LobbyPart::SummaryMap);
                        spawn_summary_row(panel, "MOVEMENT", LobbyPart::SummaryMovement);
                        spawn_summary_row(panel, "BOTS", LobbyPart::SummaryBots);
                        spawn_dynamic_summary_row(
                            panel,
                            LobbyPart::SummaryRuleALabel,
                            LobbyPart::SummaryRuleAVal,
                        );
                        spawn_dynamic_summary_row(
                            panel,
                            LobbyPart::SummaryRuleBLabel,
                            LobbyPart::SummaryRuleBVal,
                        );
                        spawn_dynamic_summary_row(
                            panel,
                            LobbyPart::SummaryRuleCLabel,
                            LobbyPart::SummaryRuleCVal,
                        );
                        spawn_static_summary_row(panel, "END OF MATCH", "3-Map Vote");
                    });

                // Ranked standing card (shown only in Ranked Lobby).
                right
                    .spawn((
                        tile_node(Node {
                            padding: UiRect::all(Val::Px(18.0)),
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(14.0),
                            ..default()
                        }),
                        BackgroundColor(Color::srgba(0.06, 0.08, 0.12, 0.92)),
                        BorderColor::all(Color::srgba(0.96, 0.77, 0.17, 0.45)),
                        Pickable::IGNORE,
                        RankedOnly,
                    ))
                    .with_children(|rc| {
                        rc.spawn((
                            ImageNode::new(k("badges/rank_bronze.png")),
                            Node {
                                width: Val::Px(64.0),
                                height: Val::Px(64.0),
                                flex_shrink: 0.0,
                                ..default()
                            },
                            Pickable::IGNORE,
                            RankCardPart::Badge,
                        ));
                        rc.spawn((
                            Node {
                                flex_grow: 1.0,
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(3.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|c| {
                            c.spawn((
                                Text::new(""),
                                fonts.heading(22.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                                RankCardPart::Name,
                            ));
                            c.spawn((
                                Text::new(""),
                                TextFont::from_font_size(12.0),
                                TextColor(KIT_YELLOW),
                                Pickable::IGNORE,
                                RankCardPart::Points,
                            ));
                            c.spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Px(12.0),
                                    padding: UiRect::all(Val::Px(2.0)),
                                    margin: UiRect::vertical(Val::Px(2.0)),
                                    ..default()
                                },
                                sliced(k("controls/progress_frame.png"), 6.0),
                                Pickable::IGNORE,
                            ))
                            .with_children(|bar| {
                                bar.spawn((
                                    Node {
                                        width: Val::Percent(0.0),
                                        height: Val::Percent(100.0),
                                        ..default()
                                    },
                                    sliced(k("controls/progress_fill_yellow.png"), 4.0),
                                    Pickable::IGNORE,
                                    RankCardPart::Fill,
                                ));
                            });
                            c.spawn((
                                Text::new(""),
                                TextFont::from_font_size(11.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                                RankCardPart::Record,
                            ));
                        });
                    });

                // Invite / Lobby info tip.
                right
                    .spawn((
                        tile_node(Node {
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            ..default()
                        }),
                        BackgroundColor(Color::srgba(0.04, 0.06, 0.09, 0.85)),
                        BorderColor::all(TILE_BORDER),
                        Pickable::IGNORE,
                    ))
                    .with_children(|tip| {
                        tip.spawn((
                            Text::new("MULTIPLAYER HOSTING"),
                            TextFont::from_font_size(11.0),
                            TextColor(TEXT_STATUS),
                            Pickable::IGNORE,
                        ));
                        tip.spawn((
                            Text::new("Press Start Match to open the arena. Friends join directly from your Portside Hub or with your Tailcat address."),
                            TextFont::from_font_size(12.0),
                            TextColor(TEXT_MUTED),
                            Pickable::IGNORE,
                        ));
                    });
            });
        });
}

fn spawn_summary_row(parent: &mut ChildSpawnerCommands, label: &str, value_part: LobbyPart) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                padding: UiRect::bottom(Val::Px(6.0)),
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(Color::srgba(0.4, 0.5, 0.65, 0.18)),
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(label),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
            ));
            row.spawn((
                Text::new(""),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
                value_part,
            ));
        });
}

fn spawn_dynamic_summary_row(
    parent: &mut ChildSpawnerCommands,
    label_part: LobbyPart,
    value_part: LobbyPart,
) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                padding: UiRect::bottom(Val::Px(6.0)),
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(Color::srgba(0.4, 0.5, 0.65, 0.18)),
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(""),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
                label_part,
            ));
            row.spawn((
                Text::new(""),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
                value_part,
            ));
        });
}

fn spawn_static_summary_row(parent: &mut ChildSpawnerCommands, label: &str, value: &str) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(label),
                TextFont::from_font_size(12.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
            ));
            row.spawn((
                Text::new(value),
                TextFont::from_font_size(13.0),
                TextColor(KIT_YELLOW),
                Pickable::IGNORE,
            ));
        });
}

/// Step 3 of Play (Private Match): COD-style Match Options drawer on the left
/// with `MAP` and `GAME RULES` tabs, leaving the right side open so you see the
/// live 3D map change as you pick it.
fn spawn_play_match_options(
    commands: &mut Commands,
    assets: &AssetServer,
    fonts: &UiKit,
    library: &MapLibrary,
) {
    let k = |p: &str| kit(assets, p);

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(36.0),
                top: Val::Px(TOP_BAR_HEIGHT + 22.0),
                bottom: Val::Px(24.0),
                width: Val::Px(640.0),
                padding: UiRect {
                    left: Val::Px(26.0),
                    right: Val::Px(26.0),
                    top: Val::Px(14.0),
                    bottom: Val::Px(20.0),
                },
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::SpaceBetween,
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
            ShowOn::PlayMatchOptions,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|panel| {
            // Top section: Title + Tab Bar + Active Tab Body.
            panel
                .spawn((
                    Node {
                        flex_grow: 1.0,
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.0),
                        overflow: Overflow::clip_y(),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|top| {
                    // Header bar.
                    top.spawn((
                        Node {
                            height: Val::Px(42.0),
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|hdr| {
                        hdr.spawn((
                            Text::new("MATCH OPTIONS"),
                            fonts.heading(30.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                        ));
                        hdr.spawn((
                            Text::new("PRIVATE MATCH SETUP"),
                            TextFont::from_font_size(12.0),
                            TextColor(KIT_YELLOW),
                            Pickable::IGNORE,
                        ));
                    });

                    // Tabs: MAP | GAME RULES.
                    top.spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(10.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|tabs| {
                        for (tab, label) in [
                            (MatchOptionsTab::Map, "MAP SELECT"),
                            (MatchOptionsTab::Rules, "GAME RULES"),
                        ] {
                            tabs.spawn((
                                Node {
                                    flex_grow: 1.0,
                                    flex_basis: Val::Px(0.0),
                                    height: Val::Px(42.0),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border: UiRect::all(Val::Px(2.0)),
                                    border_radius: BorderRadius::all(Val::Px(6.0)),
                                    ..default()
                                },
                                BackgroundColor(TILE_BG),
                                BorderColor::all(TILE_BORDER),
                                OptionsTabBtn(tab),
                                MenuAction::SwitchOptionsTab(tab),
                            ))
                            .with_children(|t| {
                                t.spawn((
                                    Text::new(label),
                                    fonts.heading(20.0),
                                    TextColor(TEXT_PRIMARY),
                                    Pickable::IGNORE,
                                    OptionsTabText(tab),
                                ));
                            });
                        }
                    });

                    // === TAB 1: MAP SELECT ===
                    top.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_direction: FlexDirection::Column,
                            justify_content: JustifyContent::SpaceBetween,
                            row_gap: Val::Px(10.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                        OptionsTabPane(MatchOptionsTab::Map),
                    ))
                    .with_children(|pane| {
                        pane.spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(8.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|list| {
                            // Random Map tile.
                            list.spawn((
                                Node {
                                    height: Val::Px(48.0),
                                    padding: UiRect::axes(Val::Px(16.0), Val::Px(6.0)),
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::SpaceBetween,
                                    ..default()
                                },
                                sliced(k("rows/list_row.png"), 14.0),
                                MenuAction::PickRandomMap,
                                RandomMapTile,
                            ))
                            .with_children(|t| {
                                t.spawn((
                                    Text::new("RANDOM MAP"),
                                    TextFont::from_font_size(15.0),
                                    TextColor(TEXT_PRIMARY),
                                    Pickable::IGNORE,
                                    RandomMapTileText,
                                ));
                                t.spawn((
                                    Text::new("ROLLS A RANDOM MAP WHEN THE MATCH STARTS"),
                                    TextFont::from_font_size(11.0),
                                    TextColor(TEXT_MUTED),
                                    Pickable::IGNORE,
                                ));
                            });

                            // 2-column wrapping map grid (built-in + custom).
                            list.spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    flex_wrap: FlexWrap::Wrap,
                                    column_gap: Val::Px(10.0),
                                    row_gap: Val::Px(8.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|grid| {
                                for id in library.ids() {
                                    let m = library.meta(id);
                                    grid.spawn((
                                        Node {
                                            width: Val::Percent(48.5),
                                            height: Val::Px(56.0),
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
                        });

                        // Selected map details card.
                        pane.spawn((
                            tile_node(Node {
                                padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(2.0),
                                ..default()
                            }),
                            BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.85)),
                            BorderColor::all(TILE_BORDER),
                            Pickable::IGNORE,
                        ))
                        .with_children(|info| {
                            info.spawn((
                                Text::new(""),
                                TextFont::from_font_size(11.0),
                                TextColor(KIT_YELLOW),
                                Pickable::IGNORE,
                                OptionsMapPreview::Style,
                            ));
                            info.spawn((
                                Text::new(""),
                                fonts.heading(26.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                                OptionsMapPreview::Name,
                            ));
                            info.spawn((
                                Text::new(""),
                                TextFont::from_font_size(12.0),
                                TextColor(TEXT_MUTED),
                                Pickable::IGNORE,
                                OptionsMapPreview::Blurb,
                            ));
                        });
                    });

                    // === TAB 2: GAME RULES ===
                    top.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(10.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                        OptionsTabPane(MatchOptionsTab::Rules),
                    ))
                    .with_children(|rules| {
                        // Quick Map row inside Game Rules -> jumps right to Map Select tab!
                        rules
                            .spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::SpaceBetween,
                                    padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                                    border: UiRect::all(Val::Px(1.0)),
                                    border_radius: BorderRadius::all(Val::Px(6.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.05, 0.08, 0.12, 0.9)),
                                BorderColor::all(TILE_BORDER),
                                Pickable::IGNORE,
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    Node {
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(2.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ))
                                .with_children(|c| {
                                    c.spawn((
                                        Text::new("MAP"),
                                        TextFont::from_font_size(11.0),
                                        TextColor(TEXT_MUTED),
                                        Pickable::IGNORE,
                                    ));
                                    c.spawn((
                                        Text::new("SPLAT PARK"),
                                        fonts.heading(22.0),
                                        TextColor(KIT_YELLOW),
                                        Pickable::IGNORE,
                                        OptionsRuleMapValue,
                                    ));
                                });
                                row.spawn(Node {
                                    width: Val::Px(170.0),
                                    flex_direction: FlexDirection::Column,
                                    ..default()
                                })
                                .with_children(|w| {
                                    spawn_menu_button(
                                        w,
                                        "CHANGE MAP >",
                                        MenuAction::SwitchOptionsTab(MatchOptionsTab::Map),
                                        ButtonStyle::SECONDARY,
                                        38.0,
                                        13.0,
                                        false,
                                    );
                                });
                            });

                        // Game Mode pills.
                        spawn_rule_row(
                            rules,
                            assets,
                            "GAME MODE",
                            &[
                                (
                                    "TEAM SPLAT",
                                    MenuAction::SetPrivateMode(GameMode::TeamSplat),
                                    RuleOption::Mode(GameMode::TeamSplat),
                                ),
                                (
                                    "FREE-FOR-ALL",
                                    MenuAction::SetPrivateMode(GameMode::FreeForAll),
                                    RuleOption::Mode(GameMode::FreeForAll),
                                ),
                                (
                                    "ELIMINATION",
                                    MenuAction::SetPrivateMode(GameMode::Elimination),
                                    RuleOption::Mode(GameMode::Elimination),
                                ),
                            ],
                        );

                        // Movement speed pills.
                        let move_items: Vec<(String, MenuAction, RuleOption)> = MovementSpeed::ALL
                            .iter()
                            .map(|&mv| {
                                (
                                    mv.name().to_uppercase(),
                                    MenuAction::PickMovement(mv),
                                    RuleOption::Movement(mv),
                                )
                            })
                            .collect();
                        spawn_rule_row_owned(rules, assets, "MOVEMENT PACE", &move_items);

                        // Bots filling the empty places, up to 10 players.
                        let bot_items: Vec<(String, MenuAction, RuleOption)> = BotLevel::ALL
                            .iter()
                            .map(|&level| {
                                (
                                    level.name().to_uppercase(),
                                    MenuAction::PickBots(level),
                                    RuleOption::Bots(level),
                                )
                            })
                            .collect();
                        spawn_rule_row_owned(
                            rules,
                            assets,
                            "BOTS (FILL TO 10 PLAYERS)",
                            &bot_items,
                        );

                        // Timed-mode rules (Team Splat / Free-for-All).
                        rules
                            .spawn((
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(10.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                                TimedModeRuleRow,
                            ))
                            .with_children(|timed| {
                                let time_items: Vec<(String, MenuAction, RuleOption)> =
                                    MatchSettings::MATCH_MINUTE_OPTIONS
                                        .iter()
                                        .map(|&m| {
                                            (
                                                format!("{m} MIN"),
                                                MenuAction::PickMatchMinutes(m),
                                                RuleOption::MatchMinutes(m),
                                            )
                                        })
                                        .collect();
                                spawn_rule_row_owned(timed, assets, "TIME LIMIT", &time_items);

                                let score_items: Vec<(String, MenuAction, RuleOption)> =
                                    MatchSettings::SCORE_LIMIT_OPTIONS
                                        .iter()
                                        .map(|&l| {
                                            (
                                                if l == 0 {
                                                    "NONE".to_string()
                                                } else {
                                                    format!("{l}")
                                                },
                                                MenuAction::PickScoreLimit(l),
                                                RuleOption::ScoreLimit(l),
                                            )
                                        })
                                        .collect();
                                spawn_rule_row_owned(
                                    timed,
                                    assets,
                                    "SCORE LIMIT (SPLATS)",
                                    &score_items,
                                );

                                let respawn_items: Vec<(String, MenuAction, RuleOption)> =
                                    MatchSettings::RESPAWN_SECONDS_OPTIONS
                                        .iter()
                                        .map(|&s| {
                                            (
                                                format!("{s}s"),
                                                MenuAction::PickRespawnSeconds(s),
                                                RuleOption::RespawnSeconds(s),
                                            )
                                        })
                                        .collect();
                                spawn_rule_row_owned(
                                    timed,
                                    assets,
                                    "RESPAWN DELAY",
                                    &respawn_items,
                                );
                            });

                        // Elimination-specific rules.
                        rules
                            .spawn((
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(10.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                                ElimModeRuleRow,
                            ))
                            .with_children(|elim| {
                                let rounds_items: Vec<(String, MenuAction, RuleOption)> =
                                    MatchSettings::ROUNDS_TO_WIN_OPTIONS
                                        .iter()
                                        .map(|&r| {
                                            (
                                                format!("FIRST TO {r}"),
                                                MenuAction::PickRoundsToWin(r),
                                                RuleOption::RoundsToWin(r),
                                            )
                                        })
                                        .collect();
                                spawn_rule_row_owned(elim, assets, "ROUNDS TO WIN", &rounds_items);

                                let round_len_items: Vec<(String, MenuAction, RuleOption)> =
                                    MatchSettings::ROUND_SECONDS_OPTIONS
                                        .iter()
                                        .map(|&s| {
                                            (
                                                format!("{}:{:02}", s / 60, s % 60),
                                                MenuAction::PickRoundSeconds(s),
                                                RuleOption::RoundSeconds(s),
                                            )
                                        })
                                        .collect();
                                spawn_rule_row_owned(
                                    elim,
                                    assets,
                                    "ROUND LENGTH",
                                    &round_len_items,
                                );
                            });
                    });
                });

            // Footer: Reset Defaults + Done (Back to Lobby).
            panel
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        column_gap: Val::Px(12.0),
                        padding: UiRect::top(Val::Px(8.0)),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|ft| {
                    ft.spawn(Node {
                        width: Val::Px(200.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    })
                    .with_children(|w| {
                        spawn_menu_button(
                            w,
                            "RESET DEFAULTS",
                            MenuAction::ResetMatchRules,
                            ButtonStyle::SECONDARY,
                            46.0,
                            14.0,
                            false,
                        );
                    });
                    ft.spawn(Node {
                        width: Val::Px(220.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    })
                    .with_children(|w| {
                        spawn_menu_button(
                            w,
                            "DONE",
                            MenuAction::CloseMatchOptions,
                            ButtonStyle::PRIMARY,
                            46.0,
                            17.0,
                            false,
                        );
                    });
                });
        });
}

fn spawn_rule_row(
    parent: &mut ChildSpawnerCommands,
    assets: &AssetServer,
    label: &str,
    items: &[(&str, MenuAction, RuleOption)],
) {
    let owned: Vec<(String, MenuAction, RuleOption)> = items
        .iter()
        .map(|(l, a, o)| ((*l).to_string(), *a, *o))
        .collect();
    spawn_rule_row_owned(parent, assets, label, &owned);
}

fn spawn_rule_row_owned(
    parent: &mut ChildSpawnerCommands,
    assets: &AssetServer,
    label: &str,
    items: &[(String, MenuAction, RuleOption)],
) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(5.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|col| {
            col.spawn((
                Text::new(label),
                TextFont::from_font_size(11.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
            ));
            col.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                for (text, action, opt) in items {
                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            height: Val::Px(38.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                            ..default()
                        },
                        sliced(kit(assets, "rows/list_row.png"), 12.0),
                        *action,
                        *opt,
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new(text.clone()),
                            TextFont::from_font_size(13.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                            RuleOptionText(*opt),
                        ));
                    });
                }
            });
        });
}

/// Shows the parts of the menu for the current screen & Play sub-screen.
fn show_menu_screens(menu: Res<MenuState>, mut q: Query<(&ShowOn, &mut Node)>) {
    for (show, mut node) in &mut q {
        let want = if show.visible(&menu) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
}

/// Keeps the COD Private Match / Ranked Lobby labels, buttons and summary card synced.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_play_lobby(
    assets: Res<AssetServer>,
    library: Res<MapLibrary>,
    menu: Res<MenuState>,
    setup: Res<MatchSetup>,
    added: Query<(), Added<LobbyPart>>,
    mut parts: Query<(&LobbyPart, Option<&mut Text>, Option<&mut ImageNode>)>,
    mut private_nodes: Query<&mut Node, (With<PrivateOnly>, Without<RankedOnly>)>,
    mut ranked_nodes: Query<&mut Node, (With<RankedOnly>, Without<PrivateOnly>)>,
) {
    if added.is_empty() && !setup.is_changed() && !menu.is_changed() {
        return;
    }
    let s = &setup.0;
    let choice = &MODE_CHOICES[mode_index(s)];
    let ranked = s.ranked;
    let map_meta = library.meta(s.map);

    let show_private = if ranked { Display::None } else { Display::Flex };
    for mut node in &mut private_nodes {
        if node.display != show_private {
            node.display = show_private;
        }
    }
    let show_ranked = if ranked { Display::Flex } else { Display::None };
    for mut node in &mut ranked_nodes {
        if node.display != show_ranked {
            node.display = show_ranked;
        }
    }

    let is_elim = s.mode == GameMode::Elimination;
    let round_sec = s.effective_round_seconds().round() as u32;
    let (rule_a_lbl, rule_a_val, rule_b_lbl, rule_b_val, rule_c_lbl, rule_c_val) = if is_elim {
        (
            "ROUNDS TO WIN",
            format!("First to {}", s.effective_rounds_to_win()),
            "ROUND LENGTH",
            format!("{}:{:02}", round_sec / 60, round_sec % 60),
            "LIVES",
            "1 Life / Round".to_string(),
        )
    } else {
        (
            "TIME LIMIT",
            format!("{} min", s.effective_match_minutes()),
            "SCORE LIMIT",
            match s.effective_score_limit() {
                Some(l) => format!("{l} Splats"),
                None => "Unlimited".to_string(),
            },
            "RESPAWN DELAY",
            format!("{}s", s.effective_respawn_seconds().round() as u32),
        )
    };

    let map_display = if ranked {
        "Random (Official Pool)".to_string()
    } else if menu.random_map {
        "Random Map".to_string()
    } else {
        map_meta.name.clone()
    };

    for (part, text, img) in &mut parts {
        match part {
            LobbyPart::ModeIcon => {
                if let Some(mut img) = img {
                    img.image = kit(&assets, choice.icon);
                }
            }
            LobbyPart::Eyebrow => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "RANKED PLAY  /  COMPETITIVE".to_string()
                    } else {
                        "PRIVATE MATCH  /  CUSTOM LOBBY".to_string()
                    };
                }
            }
            LobbyPart::ModeTitle => {
                if let Some(mut t) = text {
                    t.0 = choice.title.to_string();
                }
            }
            LobbyPart::ModeTag => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "FIRST TO 5 ROUNDS  |  RANDOM MAP  |  RANK POINTS".to_string()
                    } else {
                        choice.blurb.to_string()
                    };
                }
            }
            LobbyPart::StartBtnText => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "START RANKED MATCH".to_string()
                    } else {
                        "START MATCH".to_string()
                    };
                }
            }
            LobbyPart::MapEyebrow => {
                if let Some(mut t) = text {
                    t.0 = if ranked || menu.random_map {
                        "RANDOM MAP".to_string()
                    } else {
                        "SELECTED MAP".to_string()
                    };
                }
            }
            LobbyPart::MapCue => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "LOCKED  |  RANKED".to_string()
                    } else {
                        "CLICK TO CHANGE >".to_string()
                    };
                }
            }
            LobbyPart::MapName => {
                if let Some(mut t) = text {
                    t.0 = if ranked || menu.random_map {
                        "RANDOM MAP".to_string()
                    } else {
                        map_meta.name.to_uppercase()
                    };
                }
            }
            LobbyPart::MapStyle => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        format!("{} OFFICIAL MAPS  |  AUTO-SELECTED", MapId::BUILT_IN.len())
                    } else if menu.random_map {
                        format!(
                            "{} MAPS AVAILABLE  |  ROLLED ON START",
                            library.ids().count()
                        )
                    } else {
                        format!(
                            "{}  |  {} MAP",
                            map_meta.style.to_uppercase(),
                            map_meta.size.to_uppercase()
                        )
                    };
                }
            }
            LobbyPart::MapBlurb => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "A random official map is chosen automatically whenever a Ranked match starts.".to_string()
                    } else if menu.random_map {
                        "A random map from your map library will load when the match starts."
                            .to_string()
                    } else {
                        map_meta.blurb.clone()
                    };
                }
            }
            LobbyPart::SummaryHeading => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "COMPETITIVE RULESET".to_string()
                    } else {
                        "MATCH RULES".to_string()
                    };
                }
            }
            LobbyPart::SummaryMode => {
                if let Some(mut t) = text {
                    t.0 = if ranked {
                        "Ranked Elimination".to_string()
                    } else {
                        s.mode.name().to_string()
                    };
                }
            }
            LobbyPart::SummaryMap => {
                if let Some(mut t) = text {
                    t.0 = map_display.clone();
                }
            }
            LobbyPart::SummaryMovement => {
                if let Some(mut t) = text {
                    t.0 = s.movement.name().to_string();
                }
            }
            LobbyPart::SummaryBots => {
                if let Some(mut t) = text {
                    t.0 = match s.effective_bots() {
                        BotLevel::Off => "Off".to_string(),
                        level => format!("{} (fill to 10)", level.name()),
                    };
                }
            }
            LobbyPart::SummaryRuleALabel => {
                if let Some(mut t) = text {
                    t.0 = rule_a_lbl.to_string();
                }
            }
            LobbyPart::SummaryRuleAVal => {
                if let Some(mut t) = text {
                    t.0 = rule_a_val.clone();
                }
            }
            LobbyPart::SummaryRuleBLabel => {
                if let Some(mut t) = text {
                    t.0 = rule_b_lbl.to_string();
                }
            }
            LobbyPart::SummaryRuleBVal => {
                if let Some(mut t) = text {
                    t.0 = rule_b_val.clone();
                }
            }
            LobbyPart::SummaryRuleCLabel => {
                if let Some(mut t) = text {
                    t.0 = rule_c_lbl.to_string();
                }
            }
            LobbyPart::SummaryRuleCVal => {
                if let Some(mut t) = text {
                    t.0 = rule_c_val.clone();
                }
            }
        }
    }
}

/// Keeps the Match Options tabs, map selection highlights, and rule pills synced.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_match_options_ui(
    assets: Res<AssetServer>,
    library: Res<MapLibrary>,
    menu: Res<MenuState>,
    setup: Res<MatchSetup>,
    added: Query<(), Added<OptionsTabBtn>>,
    mut tab_btns: Query<(&OptionsTabBtn, &mut BackgroundColor, &mut BorderColor)>,
    mut nodes: Query<(
        &mut Node,
        Option<&OptionsTabPane>,
        Has<TimedModeRuleRow>,
        Has<ElimModeRuleRow>,
    )>,
    mut imgs: Query<(
        &mut ImageNode,
        Option<&MapTile>,
        Has<RandomMapTile>,
        Option<&RuleOption>,
    )>,
    mut colors: Query<(
        &mut TextColor,
        Option<&OptionsTabText>,
        Option<&MapTileText>,
        Has<RandomMapTileText>,
        Option<&RuleOptionText>,
    )>,
    mut texts: Query<(
        &mut Text,
        Option<&OptionsMapPreview>,
        Has<OptionsRuleMapValue>,
    )>,
) {
    if added.is_empty() && !setup.is_changed() && !menu.is_changed() {
        return;
    }

    let s = &setup.0;
    let is_elim = s.mode == GameMode::Elimination;

    for (tab, mut bg, mut border) in &mut tab_btns {
        let active = tab.0 == menu.options_tab;
        *bg = BackgroundColor(if active {
            Color::srgba(0.96, 0.77, 0.17, 0.16)
        } else {
            TILE_BG
        });
        *border = BorderColor::all(if active { KIT_YELLOW } else { TILE_BORDER });
    }

    for (mut node, pane, is_timed, is_elim_row) in &mut nodes {
        let want = if let Some(pane) = pane {
            if pane.0 == menu.options_tab {
                Display::Flex
            } else {
                Display::None
            }
        } else if is_timed {
            if !is_elim {
                Display::Flex
            } else {
                Display::None
            }
        } else if is_elim_row {
            if is_elim {
                Display::Flex
            } else {
                Display::None
            }
        } else {
            continue;
        };
        if node.display != want {
            node.display = want;
        }
    }

    let sel_row = kit(&assets, "rows/list_row_selected.png");
    let idle_row = kit(&assets, "rows/list_row.png");

    for (mut img, map_tile, is_rand, rule_opt) in &mut imgs {
        let selected = if let Some(tile) = map_tile {
            !menu.random_map && tile.0 == s.map
        } else if is_rand {
            menu.random_map
        } else if let Some(opt) = rule_opt {
            opt.selected(s)
        } else {
            continue;
        };
        let want = if selected {
            sel_row.clone()
        } else {
            idle_row.clone()
        };
        if img.image != want {
            img.image = want;
        }
    }

    for (mut color, tab_txt, map_txt, is_rand_txt, rule_txt) in &mut colors {
        let active = if let Some(t) = tab_txt {
            t.0 == menu.options_tab
        } else if let Some(m) = map_txt {
            !menu.random_map && m.0 == s.map
        } else if is_rand_txt {
            menu.random_map
        } else if let Some(r) = rule_txt {
            r.0.selected(s)
        } else {
            continue;
        };
        color.0 = if active { KIT_YELLOW } else { TEXT_PRIMARY };
    }

    let m = library.meta(s.map);
    for (mut text, preview, is_rule_map) in &mut texts {
        if let Some(part) = preview {
            text.0 = if menu.random_map {
                match part {
                    OptionsMapPreview::Name => "RANDOM MAP".to_string(),
                    OptionsMapPreview::Style => {
                        format!("PREVIEWING {}  |  RANDOM ON START", m.name.to_uppercase())
                    }
                    OptionsMapPreview::Blurb => {
                        "A random map will be selected automatically when you launch the match."
                            .to_string()
                    }
                }
            } else {
                match part {
                    OptionsMapPreview::Name => m.name.to_uppercase(),
                    OptionsMapPreview::Style => format!(
                        "{}  |  {} MAP",
                        m.style.to_uppercase(),
                        m.size.to_uppercase()
                    ),
                    OptionsMapPreview::Blurb => m.blurb.clone(),
                }
            };
        } else if is_rule_map {
            text.0 = if menu.random_map {
                "RANDOM MAP".to_string()
            } else {
                m.name.to_uppercase()
            };
        }
    }
}

/// The player card: rank badge, name, points and progress.
#[allow(clippy::type_complexity)]
fn update_rank_card(
    assets: Res<AssetServer>,
    rank: Res<Rank>,
    added: Query<(), Added<RankCardPart>>,
    mut parts: Query<(
        &RankCardPart,
        Option<&mut Text>,
        Option<&mut TextColor>,
        Option<&mut ImageNode>,
        &mut Node,
    )>,
) {
    if added.is_empty() && !rank.is_changed() {
        return;
    }
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

/// Escape steps back through Play sub-screens (MatchOptions -> Lobby -> ModeSelect -> Home)
/// and returns from Loadout to Play Lobby if opened from there.
fn menu_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuState>,
    overlay: Res<Overlay>,
    settings_ui: Res<crate::settings::SettingsUiState>,
    loadout_screen: Option<Res<crate::loadout_ui::LoadoutScreen>>,
) {
    if !keys.just_pressed(KeyCode::Escape)
        || overlay.blocks_input()
        || settings_ui.is_listening()
        || loadout_screen.is_some_and(|s| s.is_renaming())
    {
        return;
    }
    match menu.screen {
        MenuScreen::Play => match menu.play_sub {
            PlaySubScreen::MatchOptions => menu.play_sub = PlaySubScreen::Lobby,
            PlaySubScreen::Lobby => menu.play_sub = PlaySubScreen::ModeSelect,
            PlaySubScreen::ModeSelect => menu.screen = MenuScreen::Home,
        },
        MenuScreen::Loadout if menu.loadout_from_play => {
            menu.loadout_from_play = false;
            menu.screen = MenuScreen::Play;
            menu.play_sub = PlaySubScreen::Lobby;
        }
        MenuScreen::Loadout | MenuScreen::Stats | MenuScreen::Options => {
            menu.screen = MenuScreen::Home;
        }
        MenuScreen::Home | MenuScreen::Join => {}
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

#[allow(clippy::type_complexity)]
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
    mut bar_q: Query<&mut Node, (With<StatusBar>, Without<CancelWrap>)>,
    mut cancel_q: Query<&mut Node, (With<CancelWrap>, Without<StatusBar>)>,
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
        let show = |b: bool| if b { Display::Flex } else { Display::None };
        if let Ok(mut bar) = bar_q.single_mut() {
            let want = show(!text.0.is_empty());
            if bar.display != want {
                bar.display = want;
            }
        }
        if let Ok(mut cancel) = cancel_q.single_mut() {
            let want = show(net.is_connecting);
            if cancel.display != want {
                cancel.display = want;
            }
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
            crate::hud::HudPart,
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
