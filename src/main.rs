// Release builds on Windows open no console window beside the game.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod arena;
mod combat;
mod hub;
mod hud;
mod loadout;
mod loadout_ui;
mod menu;
mod music;
mod net;
mod pause;
mod player_model;
mod portside;
mod protocol;
mod ragdoll;
mod world;

use bevy::prelude::*;
use combat::CombatPlugin;
use hub::HubPlugin;
use hud::HudPlugin;
use loadout_ui::LoadoutUiPlugin;
use menu::MenuPlugin;
use music::MusicPlugin;
use net::NetworkState;
use pause::PausePlugin;
use player_model::PlayerModelPlugin;
use ragdoll::RagdollPlugin;
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
}

impl Overlay {
    pub fn blocks_input(&self) -> bool {
        self.pause || self.loadout
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Ink-cognito".to_string(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .init_state::<AppState>()
        .init_resource::<NetworkState>()
        .init_resource::<Overlay>()
        .insert_resource(loadout::load_saved_loadout())
        .add_systems(Startup, loadout::load_gun_assets)
        .add_plugins((WorldPlugin, MenuPlugin, HubPlugin))
        .add_plugins((
            PlayerModelPlugin,
            RagdollPlugin,
            CombatPlugin,
            HudPlugin,
            PausePlugin,
            LoadoutUiPlugin,
            MusicPlugin,
        ))
        .run();
}
