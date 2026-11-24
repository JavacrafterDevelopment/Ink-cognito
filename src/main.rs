// Release builds on Windows open no console window beside the game.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod hub;
mod menu;
mod net;
mod portside;
mod protocol;
mod world;

use bevy::prelude::*;
use hub::HubPlugin;
use menu::MenuPlugin;
use net::NetworkState;
use world::WorldPlugin;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    MainMenu,
    InGame,
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
        .add_plugins((WorldPlugin, MenuPlugin, HubPlugin))
        .run();
}
