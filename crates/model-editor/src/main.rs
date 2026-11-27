//! Standalone 3D Model Editor (`ink-model-editor`).
//!
//! Completely decoupled from the game code — ships alongside the game and map
//! editor so artists and developers can open `.glb`, `.gltf`, `.obj`, `.stl`,
//! and `.ply` files, edit geometry, attach PBR materials and textures, draw
//! directly onto models in 3D (or on the 2D UV canvas), and save as `.glb`
//! (or overwrite an opened file with a first-time overwrite confirmation).

// Release builds on Windows open no console window beside the editor.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod files;
mod gizmo;
mod io;
mod mesh_ops;
mod model;
mod paint;
mod scene;
mod state;
mod ui;
mod view;

use bevy::prelude::*;
use state::Editor;
use std::path::PathBuf;

fn main() {
    // Allow launching with a model file path argument: `ink-model-editor foo.glb`
    let startup_file: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);
    let mut editor = Editor::default();
    if let Some(path) = startup_file
        && path.is_file()
    {
        files::open_path_now(&mut editor, &path);
    }

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Model Editor".to_string(),
                resolution: (1480, 880).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(editor)
        .add_plugins((
            files::FilesPlugin,
            scene::ScenePlugin,
            gizmo::GizmoPlugin,
            view::ViewPlugin,
            ui::UiPlugin,
        ))
        .add_systems(Update, update_title)
        .run();
}

/// `"filename.glb* - Model Editor"` (star marks unsaved edits).
fn update_title(editor: Res<Editor>, mut windows: Query<&mut Window>) {
    if !editor.is_changed() {
        return;
    }
    let title = format!(
        "{}{} - Model Editor",
        editor.display_name,
        if editor.dirty { "*" } else { "" }
    );
    for mut w in &mut windows {
        if w.title != title {
            w.title = title.clone();
        }
    }
}
