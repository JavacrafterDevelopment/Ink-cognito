// Release builds on Windows open no console window beside the game.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod arena;
mod armory_v2_fx;
mod chat;
mod combat;
mod frame_rate;
mod game_mode;
mod grenades;
mod hub;
mod hud;
mod loadout;
mod loadout_ui;
mod maps;
mod menu;
mod music;
mod net;
mod pause;
mod player_model;
mod podium;
mod portside;
mod progression;
mod protocol;
mod ragdoll;
mod ranked;
mod settings;
mod ui_kit;
mod world;

use bevy::prelude::*;
use combat::CombatPlugin;
use game_mode::GameModePlugin;
use hub::HubPlugin;
use hud::HudPlugin;
use loadout_ui::LoadoutUiPlugin;
use maps::MapsPlugin;
use menu::MenuPlugin;
use music::MusicPlugin;
use net::NetworkState;
use pause::PausePlugin;
use player_model::PlayerModelPlugin;
use ragdoll::RagdollPlugin;
use ranked::RankedPlugin;
use settings::SettingsPlugin;
use ui_kit::UiKitPlugin;
use world::WorldPlugin;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    MainMenu,
    InGame,
}

/// Menus open over the game. While one is, the mouse is free and the player
/// doesn't move or shoot (the game keeps running: it's multiplayer).
#[derive(Resource, Default)]
pub struct Overlay {
    /// The Escape menu.
    pub pause: bool,
    /// The loadout screen (from the main menu or the Escape menu).
    pub loadout: bool,
    /// The options / audio settings screen (from the main menu or the Escape menu).
    pub settings: bool,
    /// Typing a chat line (Enter by default). The mouse stays captured.
    pub chat: bool,
}

impl Overlay {
    pub fn blocks_input(&self) -> bool {
        self.menu_open() || self.chat
    }

    /// A menu is open (the mouse is free then). Chat isn't a menu.
    pub fn menu_open(&self) -> bool {
        self.pause || self.loadout || self.settings
    }
}

/// Where the assets folder is, relative to the executable's folder (what
/// Bevy's `AssetPlugin::file_path` is relative to). In the macOS app bundle
/// they're in `Contents/Resources/assets`, since code signing wants only code
/// in `Contents/MacOS`; everywhere else they're next to the executable.
pub fn asset_folder() -> &'static str {
    const BUNDLED: &str = "../Resources/assets";
    if cfg!(target_os = "macos")
        && let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join(BUNDLED).is_dir()
    {
        return BUNDLED;
    }
    "assets"
}

fn main() {
    let saved_settings = settings::load_saved_settings();
    let saved_book = loadout::load_saved_loadout_book();
    let active_loadout = saved_book.active_loadout();

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Ink-cognito".to_string(),
                        resolution: (1280, 720).into(),
                        mode: saved_settings.display.window_mode.to_window_mode(),
                        present_mode: if saved_settings.display.vsync {
                            bevy::window::PresentMode::AutoVsync
                        } else {
                            bevy::window::PresentMode::AutoNoVsync
                        },
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: asset_folder().to_string(),
                    ..default()
                }),
        )
        .init_state::<AppState>()
        .init_resource::<NetworkState>()
        .init_resource::<Overlay>()
        .insert_resource(saved_book)
        .insert_resource(active_loadout)
        .insert_resource(saved_settings.audio)
        .insert_resource(saved_settings.display)
        .insert_resource(saved_settings.crosshair)
        .insert_resource(saved_settings.keybinds)
        .add_systems(Startup, loadout::load_gun_assets)
        .add_observer(loadout::apply_gun_skin)
        .add_plugins((
            UiKitPlugin,
            WorldPlugin,
            MapsPlugin,
            GameModePlugin,
            RankedPlugin,
            MenuPlugin,
            HubPlugin,
            podium::PodiumPlugin,
            progression::ProgressionPlugin,
        ))
        .add_plugins((
            PlayerModelPlugin,
            RagdollPlugin,
            CombatPlugin,
            grenades::GrenadesPlugin,
            HudPlugin,
            PausePlugin,
            LoadoutUiPlugin,
            SettingsPlugin,
            MusicPlugin,
            frame_rate::FrameRatePlugin,
            chat::ChatPlugin,
            armory_v2_fx::ArmoryV2FxPlugin,
        ))
        .run();
}
