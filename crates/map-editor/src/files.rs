//! New, open and save.

use crate::state::Editor;
use ink_map::{BUILT_IN_IDS, EXTENSION, MapFile};
use std::path::{Path, PathBuf};

/// The game's `assets` folder: next to the editor's exe in a release (the
/// editor ships in the game's folder), or the repository's when run with cargo.
pub fn assets_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let assets = dir.join("assets");
        if assets.is_dir() {
            return assets;
        }
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if repo.is_dir() {
        return repo;
    }
    PathBuf::from("assets")
}

/// The built-in maps' files (the editor opens them as starting points).
pub fn built_in_dir() -> PathBuf {
    assets_dir().join("maps")
}

/// Everything there is to open: custom maps first, then the built-in ones.
pub fn list_maps() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut add = |dir: &Path, tag: &str| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
            .map(|p| {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                (format!("{stem}{tag}"), p)
            })
            .collect();
        found.sort();
        out.extend(found);
    };
    if let Some(dir) = ink_map::custom_maps_dir() {
        add(&dir, "");
    }
    add(&built_in_dir(), "  (built-in)");
    out
}

/// Whether `path` is one of the game's built-in map files.
fn is_built_in(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| BUILT_IN_IDS.contains(&s))
        && path.parent().is_some_and(|p| p.ends_with("maps") && p.parent().is_some_and(|a| a.ends_with("assets")))
}

pub fn new_map(editor: &mut Editor) {
    editor.replace(MapFile::blank("New map"), None, "new-map".into());
    editor.status = "New map. Add spawns for both teams and free-for-all before playing it.".into();
}

pub fn open(editor: &mut Editor, path: &Path) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            editor.status = format!("Couldn't open {}: {e}", path.display());
            return;
        }
    };
    let map = match MapFile::from_json(&text) {
        Ok(m) => m,
        Err(e) => {
            editor.status = format!("Couldn't read {}: {e}", path.display());
            return;
        }
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("map").to_string();
    let built_in = is_built_in(path);
    let unknown = map.objects.iter().filter(|o| ink_map::piece(&o.piece).is_none()).count();
    editor.replace(map, Some(path.to_path_buf()), stem);
    editor.status = if built_in {
        "Opened a built-in map. Save writes it back to assets/maps (for the game's source code); Save copy makes a custom map you can play.".into()
    } else {
        format!("Opened {}.", path.display())
    };
    if unknown > 0 {
        editor.status += &format!(" {unknown} pieces this editor doesn't know are kept but not shown.");
    }
}

/// Saves where the map came from (or as a new custom map).
pub fn save(editor: &mut Editor) {
    match editor.path.clone() {
        Some(path) => write(editor, path),
        None => save_copy(editor),
    }
}

/// Saves to the custom maps folder under the file name box's name.
pub fn save_copy(editor: &mut Editor) {
    let Some(dir) = ink_map::custom_maps_dir() else {
        editor.status = "Couldn't find the settings folder to save maps in.".into();
        return;
    };
    let mut stem = ink_map::id_from_stem(editor.file_name.trim());
    if stem.is_empty() {
        stem = "custom-map".into();
    }
    if BUILT_IN_IDS.contains(&stem.as_str()) {
        stem = ink_map::id_from_stem(&format!("{stem}-custom"));
    }
    editor.file_name = stem.clone();
    let path = dir.join(format!("{stem}.{EXTENSION}"));
    write(editor, path);
}

fn write(editor: &mut Editor, path: PathBuf) {
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        editor.status = format!("Couldn't make {}: {e}", dir.display());
        return;
    }
    editor.map.format = ink_map::FORMAT;
    match std::fs::write(&path, editor.map.to_json()) {
        Ok(()) => {
            let problems = editor.map.problems();
            editor.status = if problems.is_empty() {
                format!("Saved {}. Restart the game to see it in the map picker.", path.display())
            } else {
                format!("Saved {}, but: {}.", path.display(), problems.join("; "))
            };
            editor.dirty = false;
            editor.discard_armed = false;
            editor.path = Some(path);
        }
        Err(e) => editor.status = format!("Couldn't save {}: {e}", path.display()),
    }
}
