//! The Ink-cognito map editor: a separate program from the game that builds
//! the map files the game plays.
//!
//! Fly around with the right mouse button held and WASD, pick a piece on the
//! left and click in the field to place it, click a piece to select and drag
//! it. Maps save to the game's custom maps folder, where the game finds them.
//!
//! Live edit (top bar) edits a map together with friends: one hosts, others
//! join through Portside or a TailCat address (see `live.rs`).

// Release builds on Windows open no console window beside the editor.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod files;
mod gizmo;
mod live;
mod live_net;
mod live_sync;
mod scene;
mod state;
mod ui;
mod view;

use bevy::{asset::AssetPlugin, prelude::*};
use state::Editor;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Ink-cognito Map Editor".to_string(),
                        resolution: (1440, 860).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: files::assets_dir().to_string_lossy().into_owned(),
                    ..default()
                }),
        )
        .init_resource::<Editor>()
        .add_plugins((scene::ScenePlugin, view::ViewPlugin, ui::UiPlugin, gizmo::GizmoPlugin, live::LivePlugin))
        .add_systems(Update, update_title)
        .run();
}

/// "name.json* - Ink-cognito Map Editor" (the star: unsaved changes).
fn update_title(editor: Res<Editor>, mut windows: Query<&mut Window>) {
    if !editor.is_changed() {
        return;
    }
    let title = format!(
        "{}.json{} - Ink-cognito Map Editor",
        editor.file_name,
        if editor.dirty { "*" } else { "" }
    );
    for mut w in &mut windows {
        if w.title != title {
            w.title = title.clone();
        }
    }
}
