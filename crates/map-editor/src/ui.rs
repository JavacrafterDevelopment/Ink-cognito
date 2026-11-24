//! The panels: the file bar on top, the piece palette on the left, map and
//! selection settings on the right, and a line of help at the bottom. They're
//! rebuilt whenever what they show changes.

use crate::{
    files,
    state::{Editor, Field, Page, SIZES, SKIES, SNAPS, Sel, SpawnKind, TINTS, Tool, nearest},
    view,
};
use bevy::{picking::prelude::*, prelude::*};
use ink_map::{Category, PIECES};

pub const TOP_H: f32 = 46.0;
pub const LEFT_W: f32 = 236.0;
pub const RIGHT_W: f32 = 290.0;
pub const BOTTOM_H: f32 = 26.0;

const PANEL: Color = Color::srgba(0.07, 0.10, 0.15, 0.94);
const BTN: Color = Color::srgb(0.17, 0.22, 0.30);
const BTN_HOVER: Color = Color::srgb(0.25, 0.32, 0.43);
const BTN_ON: Color = Color::srgb(0.18, 0.52, 0.90);
const BTN_ON_HOVER: Color = Color::srgb(0.24, 0.60, 0.98);
const BTN_OFF: Color = Color::srgb(0.12, 0.15, 0.2);
const TEXT: Color = Color::srgb(0.93, 0.95, 0.98);
const MUTED: Color = Color::srgb(0.62, 0.68, 0.76);
const ACCENT: Color = Color::srgb(1.0, 0.84, 0.2);
const FIELD_BG: Color = Color::srgb(0.04, 0.06, 0.09);
const FIELD_FOCUS: Color = Color::srgb(0.10, 0.16, 0.26);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, rebuild_ui)
            .add_observer(on_click)
            .add_observer(on_over)
            .add_observer(on_out);
    }
}

/// What a button does.
#[derive(Component, Clone, Copy, Debug)]
enum Action {
    New,
    Open,
    Save,
    SaveCopy,
    Undo,
    Redo,
    OpenFile(usize),
    CloseOpen,
    Page(Page),
    SelectTool,
    PickPiece(usize),
    PickSpawn(SpawnKind),
    Snap,
    TurnStep,
    Scenery,
    Focus(Field),
    SizeLabel,
    Resize(f32, f32),
    Perimeter,
    Sky,
    GroundTexture,
    GroundTint,
    MirrorSpawns,
    Turn(f32),
    Raise(f32),
    Stack,
    Solid,
    Scale(f32),
    Duplicate,
    Mirror,
    Delete,
}

/// A button's colors.
#[derive(Component)]
struct Btn {
    normal: Color,
    hover: Color,
}

/// The root of all the panels.
#[derive(Component)]
struct UiRoot;

fn on_over(over: On<PointerOver>, mut q: Query<(&Btn, &mut BackgroundColor)>) {
    if let Ok((b, mut bg)) = q.get_mut(over.entity) {
        bg.0 = b.hover;
    }
}

fn on_out(out: On<PointerOut>, mut q: Query<(&Btn, &mut BackgroundColor)>) {
    if let Ok((b, mut bg)) = q.get_mut(out.entity) {
        bg.0 = b.normal;
    }
}

fn on_click(click: On<PointerClick>, actions: Query<&Action>, mut editor: ResMut<Editor>) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(&action) = actions.get(click.entity) else {
        return;
    };
    let ed = &mut *editor;
    if !matches!(action, Action::Focus(_)) {
        ed.focus = None;
    }
    if !matches!(action, Action::New | Action::Open) {
        ed.discard_armed = false;
    }
    match action {
        Action::New | Action::Open => {
            if ed.dirty && !ed.discard_armed {
                ed.discard_armed = true;
                ed.status = "You have unsaved changes. Click again to discard them, or Save first.".into();
                return;
            }
            ed.discard_armed = false;
            if matches!(action, Action::New) {
                files::new_map(ed);
            } else {
                ed.open_list = Some(files::list_maps());
            }
        }
        Action::Save => files::save(ed),
        Action::SaveCopy => files::save_copy(ed),
        Action::Undo => ed.undo(),
        Action::Redo => ed.redo(),
        Action::OpenFile(i) => {
            if let Some((_, path)) = ed.open_list.take().and_then(|l| l.get(i).cloned()) {
                files::open(ed, &path);
            }
        }
        Action::CloseOpen => ed.open_list = None,
        Action::Page(p) => {
            ed.page = p;
        }
        Action::SelectTool => ed.tool = Tool::Select,
        Action::PickPiece(i) => {
            ed.tool = Tool::Place(&PIECES[i]);
            ed.sel = None;
            ed.status = format!(
                "Placing {}: click in the field. R turns it, Shift+click stacks it, Esc stops.",
                PIECES[i].label
            );
        }
        Action::PickSpawn(kind) => {
            ed.tool = Tool::Spawn(kind);
            ed.sel = None;
            ed.status = format!("Placing {}s: click in the field. Esc stops.", kind.name().to_lowercase());
        }
        Action::Snap => ed.snap = (ed.snap + 1) % SNAPS.len(),
        Action::TurnStep => ed.turn_step = (ed.turn_step + 1) % crate::state::TURN_STEPS.len(),
        Action::Scenery => ed.place_scenery = !ed.place_scenery,
        Action::Focus(f) => {
            if f != Field::FileName && ed.focus != Some(f) {
                ed.snapshot();
            }
            ed.focus = Some(f);
        }
        Action::SizeLabel => ed.cycle_size_label(),
        Action::Resize(dx, dz) => ed.resize(dx, dz),
        Action::Perimeter => ed.cycle_perimeter(),
        Action::Sky => ed.cycle_sky(),
        Action::GroundTexture => ed.cycle_ground_texture(),
        Action::GroundTint => ed.cycle_ground_tint(),
        Action::MirrorSpawns => ed.mirror_team_spawns(),
        Action::Turn(d) => ed.turn(d),
        Action::Raise(d) => ed.raise(d),
        Action::Stack => {
            if let Some(Sel::Object(i)) = ed.sel
                && let Some(o) = ed.map.objects.get(i)
            {
                let at = Vec2::new(o.pos[0], o.pos[2]);
                let y = view::top_at(ed, at, Some(i));
                ed.set_height(y);
            }
        }
        Action::Solid => ed.toggle_solid(),
        Action::Scale(f) => ed.scale_by(f),
        Action::Duplicate => ed.duplicate(),
        Action::Mirror => ed.mirror(),
        Action::Delete => ed.delete(),
    }
}

/// Everything the panels show, as text: they're rebuilt when it changes.
fn signature(ed: &Editor) -> String {
    let sel = match ed.sel {
        Some(Sel::Object(i)) => format!("{:?}", ed.map.objects.get(i)),
        Some(Sel::Spawn(k, i)) => format!("{k:?}{:?}", ed.spawns(k).get(i)),
        None => String::new(),
    };
    format!(
        "{:?}|{sel}|{:?}|{:?}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{:?}",
        ed.sel,
        ed.tool.clone_label(),
        ed.page,
        ed.file_name,
        ed.map.name,
        ed.map.style,
        ed.map.blurb,
        ed.map.size,
        ed.focus,
        ed.status,
        ed.dirty,
        ed.snap,
        ed.turn_step,
        ed.place_scenery,
        ed.map.half,
        ed.map.perimeter,
        ed.map.sky,
        ed.map.ground.first(),
        ed.open_list.as_ref().map(|l| l.len()),
        ed.map.objects.len(),
        ed.map.spawns.blue.len(),
        ed.map.spawns.orange.len(),
        ed.map.spawns.ffa.len(),
        ed.can_undo() as u8 * 2 + ed.can_redo() as u8,
        ed.place_yaw,
    )
}

impl Tool {
    fn clone_label(&self) -> String {
        match self {
            Tool::Select => "select".into(),
            Tool::Place(p) => p.id.into(),
            Tool::Spawn(k) => format!("{k:?}"),
        }
    }
}

fn rebuild_ui(
    mut commands: Commands,
    editor: Res<Editor>,
    roots: Query<Entity, With<UiRoot>>,
    time: Res<Time>,
    mut last: Local<String>,
) {
    // The text cursor blinks.
    let blink = editor.focus.is_some() && (time.elapsed_secs() * 2.0) as u32 % 2 == 0;
    let sig = format!("{}{blink}", signature(&editor));
    if *last == sig {
        return;
    }
    *last = sig;
    for e in &roots {
        commands.entity(e).despawn();
    }
    let ed = &*editor;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            Pickable::IGNORE,
            UiRoot,
        ))
        .with_children(|root| {
            top_bar(root, ed, blink);
            root.spawn((
                Node {
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|mid| {
                palette(mid, ed);
                settings(mid, ed, blink);
            });
            help_bar(root, ed);
        });
    if let Some(list) = &ed.open_list {
        open_dialog(&mut commands, list);
    }
}

// --- Widgets ----------------------------------------------------------------

fn label(p: &mut ChildSpawnerCommands, text: impl Into<String>, size: f32, color: Color) {
    p.spawn((
        Text::new(text.into()),
        TextFont::from_font_size(size),
        TextColor(color),
        Pickable::IGNORE,
    ));
}

fn button_sized(p: &mut ChildSpawnerCommands, text: impl Into<String>, action: Action, on: bool, enabled: bool, grow: bool) {
    let (normal, hover) = if !enabled {
        (BTN_OFF, BTN_OFF)
    } else if on {
        (BTN_ON, BTN_ON_HOVER)
    } else {
        (BTN, BTN_HOVER)
    };
    let mut e = p.spawn((
        Node {
            padding: UiRect::axes(Val::Px(9.0), Val::Px(5.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_grow: if grow { 1.0 } else { 0.0 },
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(normal),
        Btn { normal, hover },
    ));
    if enabled {
        e.insert(action);
    }
    e.with_children(|b| label(b, text, 13.0, if enabled { TEXT } else { MUTED }));
}

fn button(p: &mut ChildSpawnerCommands, text: impl Into<String>, action: Action, on: bool) {
    button_sized(p, text, action, on, true, false);
}

fn row(p: &mut ChildSpawnerCommands, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(5.0),
            row_gap: Val::Px(5.0),
            align_items: AlignItems::Center,
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(f);
}

fn heading(p: &mut ChildSpawnerCommands, text: &str) {
    p.spawn((
        Node {
            margin: UiRect::top(Val::Px(8.0)),
            ..default()
        },
        Pickable::IGNORE,
    ))
    .with_children(|h| label(h, text.to_uppercase(), 12.0, MUTED));
}

/// A text box: click to type, Enter to finish.
fn text_field(p: &mut ChildSpawnerCommands, ed: &Editor, field: Field, value: &str, blink: bool, width: Val) {
    let focused = ed.focus == Some(field);
    let shown = if focused {
        format!("{value}{}", if blink { "|" } else { " " })
    } else if value.is_empty() {
        "(click to type)".to_string()
    } else {
        value.to_string()
    };
    let bg = if focused { FIELD_FOCUS } else { FIELD_BG };
    p.spawn((
        Node {
            width,
            padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(bg),
        BorderColor::all(if focused { BTN_ON } else { Color::srgb(0.25, 0.3, 0.38) }),
        Btn { normal: bg, hover: FIELD_FOCUS },
        Action::Focus(field),
    ))
    .with_children(|b| label(b, shown, 13.0, if value.is_empty() && !focused { MUTED } else { TEXT }));
}

fn panel_node(width: f32) -> Node {
    Node {
        width: Val::Px(width),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        padding: UiRect::all(Val::Px(10.0)),
        row_gap: Val::Px(5.0),
        overflow: Overflow::clip(),
        ..default()
    }
}

// --- Panels -------------------------------------------------------------------

fn top_bar(root: &mut ChildSpawnerCommands, ed: &Editor, blink: bool) {
    root.spawn((
        Node {
            height: Val::Px(TOP_H),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            padding: UiRect::horizontal(Val::Px(10.0)),
            ..default()
        },
        BackgroundColor(PANEL),
    ))
    .with_children(|bar| {
        label(bar, "MAP EDITOR", 15.0, ACCENT);
        bar.spawn((Node { width: Val::Px(8.0), ..default() }, Pickable::IGNORE));
        button(bar, "New", Action::New, false);
        button(bar, "Open", Action::Open, false);
        button(bar, "Save", Action::Save, false);
        button(bar, "Save copy", Action::SaveCopy, false);
        button_sized(bar, "Undo", Action::Undo, false, ed.can_undo(), false);
        button_sized(bar, "Redo", Action::Redo, false, ed.can_redo(), false);
        bar.spawn((Node { width: Val::Px(10.0), ..default() }, Pickable::IGNORE));
        label(bar, "File", 13.0, MUTED);
        text_field(bar, ed, Field::FileName, &ed.file_name, blink, Val::Px(170.0));
        label(bar, if ed.dirty { ".json  (unsaved)" } else { ".json" }, 13.0, if ed.dirty { ACCENT } else { MUTED });
        bar.spawn((
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                margin: UiRect::left(Val::Px(12.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|s| label(s, ed.status.clone(), 12.0, TEXT));
    });
}

fn palette(mid: &mut ChildSpawnerCommands, ed: &Editor) {
    mid.spawn((panel_node(LEFT_W), BackgroundColor(PANEL))).with_children(|p| {
        row(p, |r| {
            button_sized(r, "Select / move", Action::SelectTool, ed.tool == Tool::Select, true, true);
        });
        heading(p, "Pieces");
        row(p, |r| {
            for c in Category::ALL {
                button(r, c.name(), Action::Page(Page::Pieces(c)), ed.page == Page::Pieces(c));
            }
            button(r, "Spawns", Action::Page(Page::Spawns), ed.page == Page::Spawns);
        });
        p.spawn((Node { height: Val::Px(4.0), ..default() }, Pickable::IGNORE));
        match ed.page {
            Page::Pieces(cat) => {
                for (i, piece) in PIECES.iter().enumerate().filter(|(_, p)| p.category == cat) {
                    let on = matches!(ed.tool, Tool::Place(p) if p.id == piece.id);
                    let tag = if piece.parts.is_empty() { "  (scenery)" } else { "" };
                    row(p, |r| button_sized(r, format!("{}{tag}", piece.label), Action::PickPiece(i), on, true, true));
                }
            }
            Page::Spawns => {
                for kind in SpawnKind::ALL {
                    let on = ed.tool == Tool::Spawn(kind);
                    row(p, |r| {
                        button_sized(r, format!("{}  ({})", kind.name(), ed.spawns(kind).len()), Action::PickSpawn(kind), on, true, true)
                    });
                }
                label(p, "Blue starts at the +Z end, orange at -Z. Each team needs a spawn per player (8+).", 12.0, MUTED);
                row(p, |r| button_sized(r, "Mirror blue spawns to orange", Action::MirrorSpawns, false, true, true));
            }
        }
        heading(p, "Placing");
        row(p, |r| {
            let snap = SNAPS[ed.snap];
            let snap = if snap == 0.0 { "off".to_string() } else { format!("{snap} m") };
            button(r, format!("Snap: {snap}"), Action::Snap, false);
            button(r, format!("Turn: {}°", crate::state::TURN_STEPS[ed.turn_step]), Action::TurnStep, false);
        });
        row(p, |r| {
            button_sized(
                r,
                if ed.place_scenery { "New pieces: scenery only" } else { "New pieces: solid" },
                Action::Scenery,
                ed.place_scenery,
                true,
                true,
            )
        });
        label(p, format!("Facing {}°", ed.place_yaw), 12.0, MUTED);
    });
}

fn settings(mid: &mut ChildSpawnerCommands, ed: &Editor, blink: bool) {
    mid.spawn((panel_node(RIGHT_W), BackgroundColor(PANEL))).with_children(|p| {
        heading(p, "Map");
        label(p, "Name", 12.0, MUTED);
        text_field(p, ed, Field::Name, &ed.map.name, blink, Val::Percent(100.0));
        label(p, "Style", 12.0, MUTED);
        text_field(p, ed, Field::Style, &ed.map.style, blink, Val::Percent(100.0));
        label(p, "Blurb", 12.0, MUTED);
        text_field(p, ed, Field::Blurb, &ed.map.blurb, blink, Val::Percent(100.0));
        row(p, |r| {
            let size = if SIZES.contains(&ed.map.size.as_str()) { ed.map.size.clone() } else { "?".into() };
            button(r, format!("Size: {size}"), Action::SizeLabel, false);
            button(r, format!("Edge: {}", ed.map.perimeter.name()), Action::Perimeter, false);
        });
        row(p, |r| {
            button(r, format!("Sky: {}", SKIES[nearest(&SKIES, ed.map.sky)].0), Action::Sky, false);
        });
        if let Some(base) = ed.map.ground.first() {
            row(p, |r| {
                let tex = match base.texture {
                    ink_map::GroundTex::Turf => "Turf",
                    ink_map::GroundTex::Dirt => "Dirt",
                };
                button(r, format!("Ground: {tex}"), Action::GroundTexture, false);
                button(r, format!("Tint: {}", TINTS[nearest(&TINTS, base.tint)].0), Action::GroundTint, false);
            });
        }
        let [hx, hz] = ed.map.half;
        row(p, |r| {
            label(r, format!("Width {} m", hx * 2.0), 13.0, TEXT);
            button(r, "-", Action::Resize(-1.0, 0.0), false);
            button(r, "+", Action::Resize(1.0, 0.0), false);
        });
        row(p, |r| {
            label(r, format!("Length {} m", hz * 2.0), 13.0, TEXT);
            button(r, "-", Action::Resize(0.0, -1.0), false);
            button(r, "+", Action::Resize(0.0, 1.0), false);
        });
        label(
            p,
            format!(
                "{} pieces, spawns: {} blue, {} orange, {} free-for-all",
                ed.map.objects.len(),
                ed.map.spawns.blue.len(),
                ed.map.spawns.orange.len(),
                ed.map.spawns.ffa.len()
            ),
            12.0,
            MUTED,
        );
        let problems = ed.map.problems();
        for problem in problems.iter().take(3) {
            label(p, format!("! {problem}"), 12.0, ACCENT);
        }

        heading(p, "Selected");
        match ed.sel {
            Some(Sel::Object(i)) if i < ed.map.objects.len() => {
                let o = &ed.map.objects[i];
                let name = ink_map::piece(&o.piece).map_or(o.piece.as_str(), |p| p.label);
                label(p, name, 15.0, ACCENT);
                label(
                    p,
                    format!("x {:.2}  y {:.2}  z {:.2}   facing {}°", o.pos[0], o.pos[1], o.pos[2], o.yaw),
                    12.0,
                    TEXT,
                );
                if o.scale != [1.0; 3] {
                    label(p, format!("scale {:.2}", o.scale[0]), 12.0, TEXT);
                }
                row(p, |r| {
                    button(r, "Turn left", Action::Turn(1.0), false);
                    button(r, "Turn right", Action::Turn(-1.0), false);
                });
                row(p, |r| {
                    button(r, "Up", Action::Raise(0.25), false);
                    button(r, "Down", Action::Raise(-0.25), false);
                    button(r, "Stack", Action::Stack, false);
                });
                row(p, |r| {
                    button(r, "Smaller", Action::Scale(1.0 / 1.25), false);
                    button(r, "Bigger", Action::Scale(1.25), false);
                });
                row(p, |r| {
                    button_sized(r, if o.solid { "Solid" } else { "Scenery only" }, Action::Solid, !o.solid, true, true);
                });
                row(p, |r| {
                    button(r, "Duplicate", Action::Duplicate, false);
                    button(r, "Mirror", Action::Mirror, false);
                    button(r, "Delete", Action::Delete, false);
                });
                label(p, "Mirror: a copy turned round the middle, for the other team's half.", 11.0, MUTED);
            }
            Some(Sel::Spawn(kind, i)) if i < ed.spawns(kind).len() => {
                let [x, z] = ed.spawns(kind)[i];
                label(p, kind.name(), 15.0, ACCENT);
                label(p, format!("x {x:.2}  z {z:.2}"), 12.0, TEXT);
                row(p, |r| {
                    button(r, "Duplicate", Action::Duplicate, false);
                    button(r, "Mirror", Action::Mirror, false);
                    button(r, "Delete", Action::Delete, false);
                });
            }
            _ => label(p, "Nothing. Click a piece or spawn in the field.", 12.0, MUTED),
        }
    });
}

fn help_bar(root: &mut ChildSpawnerCommands, ed: &Editor) {
    let tool = match ed.tool {
        Tool::Select => "Click to select, drag to move.",
        Tool::Place(_) => "Click to place, Shift+click to stack, R / Shift+R turn.",
        Tool::Spawn(_) => "Click to place a spawn.",
    };
    root.spawn((
        Node {
            height: Val::Px(BOTTOM_H),
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(10.0)),
            ..default()
        },
        BackgroundColor(PANEL),
        Pickable::IGNORE,
    ))
    .with_children(|b| {
        label(
            b,
            format!(
                "{tool}   Hold right mouse + WASD to fly (Q/E down/up, Shift fast), wheel to zoom.   R turn  M mirror  Del delete  PgUp/PgDn raise  G snap  Ctrl+Z/Y undo/redo  Ctrl+D duplicate  Ctrl+S save  Esc stop"
            ),
            12.0,
            MUTED,
        )
    });
}

fn open_dialog(commands: &mut Commands, list: &[(String, std::path::PathBuf)]) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(10),
            UiRoot,
        ))
        .with_children(|bg| {
            bg.spawn((
                Node {
                    width: Val::Px(460.0),
                    max_height: Val::Percent(80.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(14.0)),
                    row_gap: Val::Px(5.0),
                    overflow: Overflow::clip(),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    ..default()
                },
                BackgroundColor(PANEL),
            ))
            .with_children(|p| {
                label(p, "OPEN A MAP", 15.0, ACCENT);
                let dir = ink_map::custom_maps_dir().map(|d| d.display().to_string()).unwrap_or_default();
                label(p, format!("Custom maps are in {dir}"), 11.0, MUTED);
                if list.is_empty() {
                    label(p, "No map files found.", 13.0, TEXT);
                }
                for (i, (name, _)) in list.iter().enumerate() {
                    row(p, |r| button_sized(r, name.clone(), Action::OpenFile(i), false, true, true));
                }
                p.spawn((Node { height: Val::Px(6.0), ..default() }, Pickable::IGNORE));
                row(p, |r| button(r, "Cancel", Action::CloseOpen, false));
            });
        });
}
