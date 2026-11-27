//! File operations: New, Open, Import, Save, Save As, Texture Load/Export,
//! native OS file dialogs (on main thread via `NonSendMarker`), built-in
//! cross-platform file browser, and the first-save overwrite confirmation flow.

use crate::{
    io::{self, ModelFormat},
    model::ModelDoc,
    state::{
        BrowserEntry, BrowserPurpose, Editor, FileBrowserState, Modal, NativeDialogRequest,
        PendingAfterDiscard, TexMapKind,
    },
};
use bevy::{ecs::system::NonSendMarker, prelude::*};
use std::path::{Path, PathBuf};

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, process_native_dialogs);
    }
}

/// Finds a helpful starting directory: `assets/models` next to the executable
/// or repository root if present, otherwise the current working directory.
pub fn default_models_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for cand in [
            dir.join("assets/models"),
            dir.join("../Resources/assets/models"),
        ] {
            if cand.is_dir() {
                return cand;
            }
        }
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
    if repo.is_dir() {
        return repo;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|_| cfg!(windows))
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Starts a fresh starter cube document (prompting first if there are unsaved changes).
pub fn request_new(ed: &mut Editor) {
    if ed.dirty {
        ed.modal = Some(Modal::ConfirmDiscard(PendingAfterDiscard::NewModel));
        ed.bump_ui();
        return;
    }
    ed.replace_document(
        ModelDoc::starter_cube(),
        None,
        "Started a new model (1m cube).".into(),
    );
}

/// Opens the native Open dialog (prompting first if there are unsaved changes).
pub fn request_open_native(ed: &mut Editor) {
    if ed.dirty {
        ed.modal = Some(Modal::ConfirmDiscard(PendingAfterDiscard::OpenNativeDialog));
        ed.bump_ui();
        return;
    }
    ed.pending_dialog = Some(NativeDialogRequest::OpenModel);
}

/// Opens the built-in in-app file browser to pick a model.
pub fn request_open_browser(ed: &mut Editor) {
    if ed.dirty {
        ed.modal = Some(Modal::ConfirmDiscard(PendingAfterDiscard::OpenBrowser));
        ed.bump_ui();
        return;
    }
    open_file_browser(ed, BrowserPurpose::OpenModel);
}

/// Opens a specific path (e.g. from drag-and-drop), checking unsaved edits first.
pub fn request_open_path(ed: &mut Editor, path: PathBuf) {
    if ed.dirty {
        ed.modal = Some(Modal::ConfirmDiscard(PendingAfterDiscard::OpenPath(path)));
        ed.bump_ui();
        return;
    }
    open_path_now(ed, &path);
}

/// Executes a pending New/Open action after the user clicks "Discard & Continue".
pub fn confirm_discard(ed: &mut Editor) {
    let Some(Modal::ConfirmDiscard(pending)) = ed.modal.take() else {
        return;
    };
    ed.dirty = false;
    match pending {
        PendingAfterDiscard::NewModel => {
            ed.replace_document(ModelDoc::starter_cube(), None, "Started a new model.".into());
        }
        PendingAfterDiscard::OpenNativeDialog => {
            ed.pending_dialog = Some(NativeDialogRequest::OpenModel);
        }
        PendingAfterDiscard::OpenBrowser => {
            open_file_browser(ed, BrowserPurpose::OpenModel);
        }
        PendingAfterDiscard::OpenPath(path) => {
            open_path_now(ed, &path);
        }
    }
    ed.bump_ui();
}

/// Loads a model file directly from disk, resetting `overwrite_confirmed = false`
/// so the first Save will warn before overwriting the original file.
pub fn open_path_now(ed: &mut Editor, path: &Path) {
    match io::load_model(path) {
        Ok(doc) => {
            let parts = doc.parts.len();
            let tris = doc.triangle_count();
            let rig_note = if let Some(rig) = &doc.preserved_rig {
                format!(
                    " (preserving {} skin{}, {} anim{})",
                    rig.skins.len(),
                    if rig.skins.len() == 1 { "" } else { "s" },
                    rig.animations.len(),
                    if rig.animations.len() == 1 { "" } else { "s" },
                )
            } else {
                String::new()
            };
            let status = format!(
                "Opened {} ({parts} part{}, {tris} tris){rig_note}.",
                path.display(),
                if parts == 1 { "" } else { "s" }
            );
            ed.replace_document(doc, Some(path.to_path_buf()), status);
        }
        Err(err) => {
            ed.status = err;
            ed.bump_ui();
        }
    }
}

/// Imports another 3D model and appends its parts into the currently open model.
pub fn import_part_now(ed: &mut Editor, path: &Path) {
    match io::load_model(path) {
        Ok(other) => {
            let added = other.parts.len();
            ed.snapshot();
            if let Some(first) = ed.doc.append_from(other) {
                ed.select_part(Some(first));
                ed.status = format!(
                    "Imported {added} part{} from {}.",
                    if added == 1 { "" } else { "s" },
                    path.display()
                );
                ed.bump_scene();
            } else {
                ed.status = format!("No geometry found in {}.", path.display());
            }
        }
        Err(err) => {
            ed.status = err;
            ed.bump_ui();
        }
    }
}

/// Loads an image file into a material's texture slot.
pub fn load_texture_now(ed: &mut Editor, mat_idx: usize, slot: TexMapKind, path: &Path) {
    if mat_idx >= ed.doc.materials.len() {
        return;
    }
    match io::load_texture(path) {
        Ok(tex) => {
            let (w, h) = (tex.width, tex.height);
            ed.snapshot();
            let ti = ed.doc.textures.len();
            ed.doc.textures.push(tex);
            let mat = &mut ed.doc.materials[mat_idx];
            match slot {
                TexMapKind::BaseColor => {
                    mat.base_color_texture = Some(ti);
                    // Reset dark/tinted base color to white so the texture displays accurately
                    mat.base_color = [1.0, 1.0, 1.0, mat.base_color[3]];
                }
                TexMapKind::Normal => mat.normal_texture = Some(ti),
                TexMapKind::MetallicRoughness => mat.metallic_roughness_texture = Some(ti),
                TexMapKind::Emissive => {
                    mat.emissive_texture = Some(ti);
                    if mat.emissive == [0.0, 0.0, 0.0] {
                        mat.emissive = [1.0, 1.0, 1.0];
                    }
                }
            }
            // Ensure any part using this material has UVs so the texture immediately appears!
            let mut gen_uvs = false;
            for p in &mut ed.doc.parts {
                if p.material == mat_idx && p.ensure_uvs() {
                    gen_uvs = true;
                }
            }
            if let Some(parent) = path.parent()
                && parent.is_dir()
            {
                ed.last_dir = parent.to_path_buf();
            }
            ed.status = if gen_uvs {
                format!("Loaded {} ({w}×{h}) into {} (auto-generated UVs).", path.display(), slot.name())
            } else {
                format!("Loaded {} ({w}×{h}) into {}.", path.display(), slot.name())
            };
            ed.bump_scene();
        }
        Err(err) => {
            ed.status = err;
            ed.bump_ui();
        }
    }
}

// --- Save / Save As / First-Save Overwrite Confirmation ---------------------

/// Triggered by the **Save** button or `Ctrl+S`.
///
/// - If no file is associated yet (`source_path == None`), opens **Save As**.
/// - If an existing file was opened and the user has NOT yet confirmed
///   overwriting it (`!overwrite_confirmed`), opens the Overwrite Warning modal.
/// - If the user has already confirmed overwriting this file (or created it via
///   Save As), writes directly.
pub fn request_save(ed: &mut Editor) {
    let Some(target) = ed.source_path.clone() else {
        request_save_as_native(ed);
        return;
    };
    if !ed.overwrite_confirmed {
        ed.modal = Some(Modal::ConfirmOverwrite {
            target: target.clone(),
        });
        ed.status = format!(
            "Warning: saving will overwrite {}. Confirm to overwrite, Cancel to keep editing, or Save As Copy.",
            target.file_name().and_then(|s| s.to_str()).unwrap_or("file")
        );
        ed.bump_ui();
        return;
    }
    write_model_to_path(ed, target, true);
}

/// Triggered by **Save As** or `Ctrl+Shift+S`. Always writes a copy to a new
/// chosen path (defaulting to `.glb`).
pub fn request_save_as_native(ed: &mut Editor) {
    ed.modal = None;
    ed.pending_dialog = Some(NativeDialogRequest::SaveModelAs);
    ed.bump_ui();
}

/// Triggered when the user clicks **"Yes, Overwrite"** in the Overwrite Warning
/// modal. Saves the file and marks `overwrite_confirmed = true` for subsequent
/// saves of this file.
pub fn confirm_overwrite(ed: &mut Editor) {
    let Some(Modal::ConfirmOverwrite { target }) = ed.modal.take() else {
        return;
    };
    write_model_to_path(ed, target, true);
}

/// Triggered when the user clicks **"No, Cancel"** (or presses Esc) on the
/// Overwrite Warning modal.
///
/// IMPORTANT: Preserves ALL edits in `ed.doc`, keeps `ed.dirty` untouched, and
/// keeps `ed.overwrite_confirmed == false` so clicking Save again will display
/// the overwrite warning prompt again!
pub fn cancel_overwrite(ed: &mut Editor) {
    if matches!(ed.modal, Some(Modal::ConfirmOverwrite { .. })) {
        ed.modal = None;
        ed.status = "Overwrite cancelled — all your edits are still here. Click Save again or use Save As to make a copy.".into();
        ed.bump_ui();
    }
}

/// Writes `ed.doc` to `path` (appending `.glb` if no supported extension is
/// present). Updates `source_path`, `display_name`, `overwrite_confirmed`, and
/// clears `dirty` on success.
pub fn write_model_to_path(ed: &mut Editor, mut path: PathBuf, mark_confirmed: bool) {
    if ModelFormat::from_path(&path).is_none() {
        let mut os = path.into_os_string();
        os.push(".glb");
        path = PathBuf::from(os);
    }
    match io::save_model(&ed.doc, &path) {
        Ok(()) => {
            if let Some(parent) = path.parent()
                && parent.is_dir()
            {
                ed.last_dir = parent.to_path_buf();
            }
            ed.display_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("model.glb")
                .to_string();
            ed.source_path = Some(path.clone());
            if mark_confirmed {
                ed.overwrite_confirmed = true;
            }
            ed.dirty = false;
            ed.modal = None;
            ed.status = format!("Saved {}.", path.display());
            ed.bump_ui();
        }
        Err(err) => {
            ed.status = err;
            ed.bump_ui();
        }
    }
}

/// Suggests a safe copy filename for Save As (e.g. `brick_copy.glb` when
/// copying an opened file, or `model.glb` for an unsaved model).
pub fn suggested_save_as_name(ed: &Editor) -> String {
    match &ed.source_path {
        Some(p) => {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
            format!("{stem}_copy.glb")
        }
        None => {
            if ed.display_name.ends_with(".glb") {
                ed.display_name.clone()
            } else {
                "model.glb".into()
            }
        }
    }
}

// --- Built-in In-App File Browser -------------------------------------------

pub fn open_file_browser(ed: &mut Editor, purpose: BrowserPurpose) {
    let dir = if ed.last_dir.is_dir() {
        ed.last_dir.clone()
    } else {
        default_models_dir()
    };
    let (file_name, save_format) = match purpose {
        BrowserPurpose::SaveAs => (suggested_save_as_name(ed), ModelFormat::Glb),
        _ => (String::new(), ModelFormat::Glb),
    };
    let entries = scan_directory(&dir, purpose);
    ed.modal = Some(Modal::FileBrowser(FileBrowserState {
        purpose,
        dir,
        entries,
        file_name,
        save_format,
    }));
    ed.bump_ui();
}

pub fn browser_navigate(ed: &mut Editor, new_dir: PathBuf) {
    if !new_dir.is_dir() {
        return;
    }
    let Some(Modal::FileBrowser(fb)) = &mut ed.modal else {
        return;
    };
    fb.entries = scan_directory(&new_dir, fb.purpose);
    fb.dir = new_dir.clone();
    ed.last_dir = new_dir;
    ed.bump_ui();
}

pub fn browser_submit_save(ed: &mut Editor) {
    let target = {
        let Some(Modal::FileBrowser(fb)) = &ed.modal else {
            return;
        };
        if fb.purpose != BrowserPurpose::SaveAs {
            return;
        }
        let raw = fb.file_name.trim();
        if raw.is_empty() {
            ed.status = "Enter a file name first.".into();
            ed.bump_ui();
            return;
        }
        let mut path = fb.dir.join(raw);
        if ModelFormat::from_path(&path).is_none() {
            let ext = fb.save_format.extension();
            let mut os = path.into_os_string();
            os.push(".");
            os.push(ext);
            path = PathBuf::from(os);
        }
        path
    };

    if target.exists() {
        ed.modal = Some(Modal::ConfirmOverwrite { target });
        ed.bump_ui();
    } else {
        write_model_to_path(ed, target, true);
    }
}

pub fn scan_directory(dir: &Path, purpose: BrowserPurpose) -> Vec<BrowserEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push(BrowserEntry {
                name: format!("[DIR] {name}"),
                path,
                is_dir: true,
            });
        } else {
            let keep = match purpose {
                BrowserPurpose::OpenModel | BrowserPurpose::ImportPart | BrowserPurpose::SaveAs => {
                    io::is_model_path(&path)
                }
                BrowserPurpose::LoadTexture(..) => io::is_image_path(&path),
            };
            if keep {
                files.push(BrowserEntry {
                    name,
                    path,
                    is_dir: false,
                });
            }
        }
    }
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);
    dirs
}

// --- Native OS File Dialogs (Main Thread) -----------------------------------

/// Runs native OS Open / Save dialogs on the main thread (required on macOS).
fn process_native_dialogs(mut ed: ResMut<Editor>, _main_thread: NonSendMarker) {
    let Some(req) = ed.pending_dialog.take() else {
        return;
    };
    let start_dir = if ed.last_dir.is_dir() {
        ed.last_dir.clone()
    } else {
        default_models_dir()
    };

    match req {
        NativeDialogRequest::OpenModel => {
            let picked = rfd::FileDialog::new()
                .set_title("Open 3D Model")
                .set_directory(&start_dir)
                .add_filter("3D Models", &["glb", "gltf", "obj", "stl", "ply"])
                .add_filter("GLB / glTF", &["glb", "gltf"])
                .add_filter("Wavefront OBJ", &["obj"])
                .add_filter("STL / PLY", &["stl", "ply"])
                .pick_file();
            if let Some(path) = picked {
                open_path_now(&mut ed, &path);
            }
        }
        NativeDialogRequest::ImportPart => {
            let picked = rfd::FileDialog::new()
                .set_title("Import 3D Model Part")
                .set_directory(&start_dir)
                .add_filter("3D Models", &["glb", "gltf", "obj", "stl", "ply"])
                .pick_file();
            if let Some(path) = picked {
                import_part_now(&mut ed, &path);
            }
        }
        NativeDialogRequest::SaveModelAs => {
            let suggested = suggested_save_as_name(&ed);
            let picked = rfd::FileDialog::new()
                .set_title("Save 3D Model As")
                .set_directory(&start_dir)
                .set_file_name(&suggested)
                .add_filter("GLB Binary glTF (*.glb)", &["glb"])
                .add_filter("glTF 2.0 (*.gltf)", &["gltf"])
                .add_filter("Wavefront OBJ (*.obj)", &["obj"])
                .add_filter("STL Mesh (*.stl)", &["stl"])
                .add_filter("PLY Mesh (*.ply)", &["ply"])
                .save_file();
            if let Some(path) = picked {
                write_model_to_path(&mut ed, path, true);
            }
        }
        NativeDialogRequest::LoadTexture { mat_idx, slot } => {
            let picked = rfd::FileDialog::new()
                .set_title(format!("Load {} Texture", slot.name()))
                .set_directory(&start_dir)
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "tga"])
                .pick_file();
            if let Some(path) = picked {
                load_texture_now(&mut ed, mat_idx, slot, &path);
            }
        }
        NativeDialogRequest::ExportTexture { tex_idx } => {
            let name = ed
                .doc
                .textures
                .get(tex_idx)
                .map(|t| format!("{}.png", t.name))
                .unwrap_or_else(|| "texture.png".into());
            let picked = rfd::FileDialog::new()
                .set_title("Export Texture as PNG")
                .set_directory(&start_dir)
                .set_file_name(&name)
                .add_filter("PNG Image", &["png"])
                .save_file();
            if let Some(mut path) = picked {
                if path.extension().is_none() {
                    let mut os = path.into_os_string();
                    os.push(".png");
                    path = PathBuf::from(os);
                }
                if let Some(tex) = ed.doc.textures.get(tex_idx) {
                    match io::save_texture_png(tex, &path) {
                        Ok(()) => ed.status = format!("Exported texture to {}.", path.display()),
                        Err(e) => ed.status = e,
                    }
                    ed.bump_ui();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PrimitiveKind;

    #[test]
    fn first_save_on_opened_glb_prompts_overwrite_and_cancel_preserves_work() {
        // Create an existing GLB on disk.
        let orig_path = std::env::temp_dir().join("ink_model_editor_overwrite_test.glb");
        let starter = ModelDoc::starter_cube();
        io::save_model(&starter, &orig_path).expect("seed glb");
        let orig_bytes = std::fs::read(&orig_path).unwrap();

        // Open it in the editor.
        let mut ed = Editor::default();
        open_path_now(&mut ed, &orig_path);
        assert_eq!(ed.source_path.as_deref(), Some(orig_path.as_path()));
        assert!(!ed.overwrite_confirmed);
        assert!(!ed.dirty);

        // Make changes in the editor (add a sphere!).
        ed.add_primitive(PrimitiveKind::Sphere);
        assert_eq!(ed.doc.parts.len(), 2);
        assert!(ed.dirty);

        // First press of Save MUST prompt overwrite warning and NOT overwrite disk yet.
        request_save(&mut ed);
        assert!(matches!(ed.modal, Some(Modal::ConfirmOverwrite { .. })));
        assert_eq!(std::fs::read(&orig_path).unwrap(), orig_bytes);

        // Clicking "No" cancels the prompt WITHOUT deleting any work!
        cancel_overwrite(&mut ed);
        assert!(ed.modal.is_none());
        assert_eq!(ed.doc.parts.len(), 2, "edits must still be present!");
        assert!(ed.dirty, "dirty flag must remain true!");
        assert!(!ed.overwrite_confirmed, "overwrite_confirmed must stay false!");

        // Pressing Save a second time MUST show the overwrite prompt again!
        request_save(&mut ed);
        assert!(matches!(ed.modal, Some(Modal::ConfirmOverwrite { .. })));

        // Clicking "Yes, Overwrite" actually overwrites the file and marks confirmed.
        confirm_overwrite(&mut ed);
        assert!(ed.modal.is_none());
        assert!(!ed.dirty);
        assert!(ed.overwrite_confirmed);
        let reloaded = io::load_model(&orig_path).expect("reload overwritten glb");
        assert_eq!(reloaded.parts.len(), 2, "overwritten file should have both parts");

        // Subsequent Save after another edit writes directly without re-prompting.
        ed.add_primitive(PrimitiveKind::Cone);
        request_save(&mut ed);
        assert!(ed.modal.is_none(), "already confirmed file should save directly");
        assert!(!ed.dirty);
        let reloaded2 = io::load_model(&orig_path).expect("reload again");
        assert_eq!(reloaded2.parts.len(), 3);

        let _ = std::fs::remove_file(&orig_path);
    }

    #[test]
    fn save_as_writes_copy_without_touching_original() {
        let orig_path = std::env::temp_dir().join("ink_model_editor_orig_copy_test.glb");
        let copy_path = std::env::temp_dir().join("ink_model_editor_copy_out.glb");
        let _ = std::fs::remove_file(&copy_path);

        io::save_model(&ModelDoc::starter_cube(), &orig_path).expect("seed");
        let mut ed = Editor::default();
        open_path_now(&mut ed, &orig_path);
        ed.add_primitive(PrimitiveKind::Cylinder);

        write_model_to_path(&mut ed, copy_path.clone(), true);
        assert_eq!(ed.source_path.as_deref(), Some(copy_path.as_path()));
        assert!(!ed.dirty);
        assert_eq!(io::load_model(&orig_path).unwrap().parts.len(), 1, "original untouched");
        assert_eq!(io::load_model(&copy_path).unwrap().parts.len(), 2, "copy has edits");

        let _ = std::fs::remove_file(&orig_path);
        let _ = std::fs::remove_file(&copy_path);
    }
}
