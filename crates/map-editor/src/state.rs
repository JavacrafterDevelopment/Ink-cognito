//! What's being edited and every change you can make to it. All edits go
//! through here so undo works the same for each.

use bevy::prelude::*;
use ink_map::{GroundTex, MapFile, Object, Perimeter, Piece};
use std::path::PathBuf;

/// The three kinds of spawn spot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpawnKind {
    Blue,
    Orange,
    Ffa,
}

impl SpawnKind {
    pub const ALL: [SpawnKind; 3] = [SpawnKind::Blue, SpawnKind::Orange, SpawnKind::Ffa];

    pub fn name(self) -> &'static str {
        match self {
            SpawnKind::Blue => "Blue spawn",
            SpawnKind::Orange => "Orange spawn",
            SpawnKind::Ffa => "Free-for-all spawn",
        }
    }

    pub fn color(self) -> Color {
        match self {
            SpawnKind::Blue => Color::srgb(0.25, 0.55, 1.0),
            SpawnKind::Orange => Color::srgb(1.0, 0.55, 0.15),
            SpawnKind::Ffa => Color::srgb(0.9, 0.9, 0.9),
        }
    }
}

/// What a left click in the 3D view does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    /// Pick and drag things.
    Select,
    /// Drop copies of a piece.
    Place(&'static Piece),
    /// Drop spawn spots.
    Spawn(SpawnKind),
}

/// What the handles on the selection do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoMode {
    /// Three arrows: drag one to move along it.
    Move,
    /// Three rings: drag one to turn around it.
    Rotate,
}

/// What's selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sel {
    Object(usize),
    Spawn(SpawnKind, usize),
}

/// Which palette page is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Pieces(ink_map::Category),
    Spawns,
}

/// A text box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    FileName,
    Name,
    Style,
    Blurb,
}

/// Snap to a grid when placing and moving.
pub const SNAPS: [f32; 4] = [0.0, 0.25, 0.5, 1.0];
/// Turn step, degrees.
pub const TURN_STEPS: [f32; 3] = [90.0, 45.0, 15.0];

pub const SKIES: [(&str, [f32; 3]); 6] = [
    ("Day", [0.55, 0.76, 0.96]),
    ("Clear", [0.6, 0.78, 0.95]),
    ("Pale", [0.66, 0.8, 0.95]),
    ("Haze", [0.7, 0.82, 0.9]),
    ("Sunset", [0.93, 0.72, 0.56]),
    ("Dusk", [0.36, 0.38, 0.55]),
];

pub const TINTS: [(&str, [f32; 3]); 6] = [
    ("Natural", [1.0, 1.0, 1.0]),
    ("Forest", [0.74, 0.88, 0.7]),
    ("Quarry", [0.95, 0.86, 0.74]),
    ("Road", [0.8, 0.74, 0.66]),
    ("Asphalt", [0.52, 0.53, 0.56]),
    ("Sand", [1.0, 0.92, 0.72]),
];

pub const SIZES: [&str; 3] = ["Small", "Medium", "Large"];

/// The map being edited, and the editor around it.
#[derive(Resource)]
pub struct Editor {
    pub map: MapFile,
    /// Where Save writes (none: not saved yet).
    pub path: Option<PathBuf>,
    /// The file name, without `.json`.
    pub file_name: String,
    pub dirty: bool,
    pub sel: Option<Sel>,
    pub tool: Tool,
    pub page: Page,
    /// The turn new pieces are placed with, degrees.
    pub place_yaw: f32,
    /// Place new pieces as scenery (nothing collides with them).
    pub place_scenery: bool,
    pub snap: usize,
    pub turn_step: usize,
    pub focus: Option<Field>,
    pub status: String,
    /// Bumped when the scene must be rebuilt (not for moves and turns).
    pub structure: u64,
    /// The Open list, while it's showing.
    pub open_list: Option<Vec<(String, PathBuf)>>,
    /// A New or Open waiting for a second click because of unsaved changes.
    pub discard_armed: bool,
    pub gizmo: GizmoMode,
    undo: Vec<MapFile>,
    redo: Vec<MapFile>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::with_map(MapFile::blank("New map"), None, "new-map".into())
    }
}

impl Editor {
    pub fn with_map(map: MapFile, path: Option<PathBuf>, file_name: String) -> Self {
        Self {
            map,
            path,
            file_name,
            dirty: false,
            sel: None,
            tool: Tool::Select,
            page: Page::Pieces(ink_map::Category::Bunkers),
            place_yaw: 0.0,
            place_scenery: false,
            snap: 2,
            turn_step: 0,
            focus: None,
            status: "Pick a piece on the left, then click in the field to place it.".into(),
            structure: 0,
            open_list: None,
            discard_armed: false,
            gizmo: GizmoMode::Move,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Replaces the whole map (New / Open), keeping the camera.
    pub fn replace(&mut self, map: MapFile, path: Option<PathBuf>, file_name: String) {
        let structure = self.structure + 1;
        let page = self.page;
        *self = Self::with_map(map, path, file_name);
        self.structure = structure;
        self.page = page;
    }

    /// Call before every change: remembers the map for undo.
    pub fn snapshot(&mut self) {
        self.undo.push(self.map.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        self.discard_armed = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.map, prev));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.map, next));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.sel = self.sel.filter(|s| self.exists(*s));
        self.dirty = true;
        self.structure += 1;
    }

    pub fn exists(&self, sel: Sel) -> bool {
        match sel {
            Sel::Object(i) => i < self.map.objects.len(),
            Sel::Spawn(kind, i) => i < self.spawns(kind).len(),
        }
    }

    pub fn spawns(&self, kind: SpawnKind) -> &Vec<[f32; 2]> {
        match kind {
            SpawnKind::Blue => &self.map.spawns.blue,
            SpawnKind::Orange => &self.map.spawns.orange,
            SpawnKind::Ffa => &self.map.spawns.ffa,
        }
    }

    pub fn spawns_mut(&mut self, kind: SpawnKind) -> &mut Vec<[f32; 2]> {
        match kind {
            SpawnKind::Blue => &mut self.map.spawns.blue,
            SpawnKind::Orange => &mut self.map.spawns.orange,
            SpawnKind::Ffa => &mut self.map.spawns.ffa,
        }
    }

    pub fn snap_value(&self) -> f32 {
        SNAPS[self.snap]
    }

    pub fn turn_value(&self) -> f32 {
        TURN_STEPS[self.turn_step]
    }

    /// Rounds a ground point to the grid.
    pub fn snapped(&self, p: Vec2) -> Vec2 {
        let s = self.snap_value();
        if s > 0.0 { (p / s).round() * s } else { p }
    }

    // --- Edits ------------------------------------------------------------

    pub fn place_piece(&mut self, piece: &'static Piece, at: Vec2) {
        self.snapshot();
        let mut o = Object::new(piece.id, [at.x, 0.0, at.y], self.place_yaw);
        o.solid = !self.place_scenery;
        self.map.objects.push(o);
        // Not selected: turning while placing turns the next piece, not this one.
        self.sel = None;
        self.structure += 1;
    }

    pub fn place_spawn(&mut self, kind: SpawnKind, at: Vec2) {
        self.snapshot();
        let list = self.spawns_mut(kind);
        list.push([at.x, at.y]);
        let i = list.len() - 1;
        self.sel = Some(Sel::Spawn(kind, i));
    }

    /// Where the selection is on the ground, and its height.
    pub fn sel_pos(&self) -> Option<Vec3> {
        match self.sel? {
            Sel::Object(i) => self.map.objects.get(i).map(|o| Vec3::from_array(o.pos)),
            Sel::Spawn(kind, i) => self.spawns(kind).get(i).map(|&[x, z]| Vec3::new(x, 0.0, z)),
        }
    }

    /// Moves the selection to `at` on the ground (keeps its height). No snapshot:
    /// drags take one when they start.
    pub fn move_sel_to(&mut self, at: Vec2) {
        match self.sel {
            Some(Sel::Object(i)) => {
                if let Some(o) = self.map.objects.get_mut(i) {
                    o.pos[0] = at.x;
                    o.pos[2] = at.y;
                    self.dirty = true;
                }
            }
            Some(Sel::Spawn(kind, i)) => {
                if let Some(p) = self.spawns_mut(kind).get_mut(i) {
                    *p = [at.x, at.y];
                    self.dirty = true;
                }
            }
            None => {}
        }
    }

    /// Turns the selected piece (or the one about to be placed).
    pub fn turn(&mut self, dir: f32) {
        let step = self.turn_value() * dir;
        if let Some(Sel::Object(i)) = self.sel
            && i < self.map.objects.len()
            && !matches!(self.tool, Tool::Place(_))
        {
            self.snapshot();
            let o = &mut self.map.objects[i];
            o.yaw = (o.yaw + step).rem_euclid(360.0);
            self.place_yaw = o.yaw;
        } else {
            self.place_yaw = (self.place_yaw + step).rem_euclid(360.0);
        }
    }

    /// Moves the selection to `at` (spawns stay on the ground). No snapshot.
    pub fn move_sel_to3(&mut self, at: Vec3) {
        match self.sel {
            Some(Sel::Object(i)) => {
                if let Some(o) = self.map.objects.get_mut(i) {
                    o.pos = [at.x, at.y.max(-3.0), at.z];
                    self.dirty = true;
                }
            }
            Some(Sel::Spawn(..)) => self.move_sel_to(Vec2::new(at.x, at.z)),
            None => {}
        }
    }

    /// The selected piece's rotation.
    pub fn sel_rotation(&self) -> Option<Quat> {
        match self.sel {
            Some(Sel::Object(i)) => self.map.objects.get(i).map(|o| o.rotation()),
            _ => None,
        }
    }

    /// Turns the selected piece to `rot`. No snapshot.
    pub fn set_sel_rotation(&mut self, rot: Quat) {
        if let Some(Sel::Object(i)) = self.sel
            && let Some(o) = self.map.objects.get_mut(i)
        {
            o.set_rotation(rot);
            self.dirty = true;
        }
    }

    /// Takes the tilt off the selected piece (keeps its yaw).
    pub fn level(&mut self) {
        if let Some(Sel::Object(i)) = self.sel
            && i < self.map.objects.len()
        {
            self.snapshot();
            let o = &mut self.map.objects[i];
            o.pitch = 0.0;
            o.roll = 0.0;
        }
    }

    pub fn raise(&mut self, dy: f32) {
        if let Some(Sel::Object(i)) = self.sel
            && i < self.map.objects.len()
        {
            self.snapshot();
            let o = &mut self.map.objects[i];
            o.pos[1] = ((o.pos[1] + dy) * 1000.0).round() / 1000.0;
            o.pos[1] = o.pos[1].max(0.0);
        }
    }

    /// Stacks the selected piece on top of whatever is under it (or drops it to the ground).
    pub fn set_height(&mut self, y: f32) {
        if let Some(Sel::Object(i)) = self.sel
            && i < self.map.objects.len()
        {
            self.snapshot();
            self.map.objects[i].pos[1] = y.max(0.0);
        }
    }

    pub fn toggle_solid(&mut self) {
        if let Some(Sel::Object(i)) = self.sel
            && i < self.map.objects.len()
        {
            self.snapshot();
            let o = &mut self.map.objects[i];
            o.solid = !o.solid;
        }
    }

    pub fn scale_by(&mut self, f: f32) {
        if let Some(Sel::Object(i)) = self.sel
            && i < self.map.objects.len()
        {
            self.snapshot();
            let o = &mut self.map.objects[i];
            for v in &mut o.scale {
                *v = ((*v * f).clamp(0.25, 4.0) * 100.0).round() / 100.0;
            }
        }
    }

    pub fn delete(&mut self) {
        match self.sel {
            Some(Sel::Object(i)) if i < self.map.objects.len() => {
                self.snapshot();
                self.map.objects.remove(i);
                self.structure += 1;
            }
            Some(Sel::Spawn(kind, i)) if i < self.spawns(kind).len() => {
                self.snapshot();
                self.spawns_mut(kind).remove(i);
            }
            _ => return,
        }
        self.sel = None;
    }

    /// A copy of the selection, nudged aside.
    pub fn duplicate(&mut self) {
        let nudge = self.snap_value().max(0.5) * 2.0;
        match self.sel {
            Some(Sel::Object(i)) if i < self.map.objects.len() => {
                self.snapshot();
                let mut o = self.map.objects[i].clone();
                o.pos[0] += nudge;
                self.map.objects.push(o);
                self.sel = Some(Sel::Object(self.map.objects.len() - 1));
                self.structure += 1;
            }
            Some(Sel::Spawn(kind, i)) if i < self.spawns(kind).len() => {
                self.snapshot();
                let [x, z] = self.spawns(kind)[i];
                let list = self.spawns_mut(kind);
                list.push([x + nudge, z]);
                let n = list.len() - 1;
                self.sel = Some(Sel::Spawn(kind, n));
            }
            _ => {}
        }
    }

    /// A copy of the selected piece turned half way round the middle of the
    /// field, so both teams' halves match (how the built-in maps are made).
    pub fn mirror(&mut self) {
        match self.sel {
            Some(Sel::Object(i)) if i < self.map.objects.len() => {
                self.snapshot();
                let mut o = self.map.objects[i].clone();
                o.pos[0] = -o.pos[0];
                o.pos[2] = -o.pos[2];
                if o.pitch == 0.0 && o.roll == 0.0 {
                    o.yaw = (o.yaw + 180.0).rem_euclid(360.0);
                } else {
                    let rot = Quat::from_rotation_y(std::f32::consts::PI) * o.rotation();
                    o.set_rotation(rot);
                }
                self.map.objects.push(o);
                self.sel = Some(Sel::Object(self.map.objects.len() - 1));
                self.structure += 1;
                self.status = "Mirrored a copy to the other half.".into();
            }
            Some(Sel::Spawn(kind, i)) if i < self.spawns(kind).len() => {
                self.snapshot();
                let [x, z] = self.spawns(kind)[i];
                let other = match kind {
                    SpawnKind::Blue => SpawnKind::Orange,
                    SpawnKind::Orange => SpawnKind::Blue,
                    SpawnKind::Ffa => SpawnKind::Ffa,
                };
                let list = self.spawns_mut(other);
                list.push([-x, -z]);
                let n = list.len() - 1;
                self.sel = Some(Sel::Spawn(other, n));
            }
            _ => {}
        }
    }

    /// Orange spawns become blue's, turned round the middle.
    pub fn mirror_team_spawns(&mut self) {
        self.snapshot();
        self.map.spawns.orange = self.map.spawns.blue.iter().map(|&[x, z]| [-x, -z]).collect();
        self.sel = None;
        self.status = format!(
            "Orange now has {} spawns mirrored from blue.",
            self.map.spawns.orange.len()
        );
    }

    /// Grows or shrinks the field. The base ground patch follows when it
    /// covered the whole field.
    pub fn resize(&mut self, dx: f32, dz: f32) {
        let old = self.map.half;
        let new = [(old[0] + dx).clamp(5.0, 100.0), (old[1] + dz).clamp(5.0, 100.0)];
        if new == old {
            return;
        }
        self.snapshot();
        self.map.half = new;
        if let Some(base) = self.map.ground.first_mut()
            && base.center == [0.0, 0.0]
            && base.size == [old[0] * 2.0, old[1] * 2.0]
        {
            base.size = [new[0] * 2.0, new[1] * 2.0];
        }
        self.structure += 1;
    }

    pub fn cycle_perimeter(&mut self) {
        self.snapshot();
        let i = Perimeter::ALL
            .iter()
            .position(|p| *p == self.map.perimeter)
            .unwrap_or(0);
        self.map.perimeter = Perimeter::ALL[(i + 1) % Perimeter::ALL.len()];
        self.structure += 1;
    }

    pub fn cycle_sky(&mut self) {
        self.snapshot();
        let i = nearest(&SKIES, self.map.sky);
        self.map.sky = SKIES[(i + 1) % SKIES.len()].1;
    }

    pub fn cycle_size_label(&mut self) {
        self.snapshot();
        let i = SIZES.iter().position(|s| *s == self.map.size).map_or(0, |i| i + 1);
        self.map.size = SIZES[i % SIZES.len()].to_string();
    }

    fn base_patch(&mut self) -> &mut ink_map::Patch {
        if self.map.ground.is_empty() {
            let h = self.map.half;
            self.map.ground.push(ink_map::Patch {
                center: [0.0, 0.0],
                size: [h[0] * 2.0, h[1] * 2.0],
                texture: GroundTex::Turf,
                tint: [1.0; 3],
                tile: 3.0,
            });
        }
        &mut self.map.ground[0]
    }

    pub fn cycle_ground_texture(&mut self) {
        self.snapshot();
        let base = self.base_patch();
        base.texture = match base.texture {
            GroundTex::Turf => GroundTex::Dirt,
            GroundTex::Dirt => GroundTex::Turf,
        };
        self.structure += 1;
    }

    pub fn cycle_ground_tint(&mut self) {
        self.snapshot();
        let base = self.base_patch();
        let i = nearest(&TINTS, base.tint);
        base.tint = TINTS[(i + 1) % TINTS.len()].1;
        self.structure += 1;
    }

    pub fn field_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::FileName => &mut self.file_name,
            Field::Name => &mut self.map.name,
            Field::Style => &mut self.map.style,
            Field::Blurb => &mut self.map.blurb,
        }
    }
}

/// The preset closest to `c`.
pub fn nearest(presets: &[(&str, [f32; 3])], c: [f32; 3]) -> usize {
    let d = |p: [f32; 3]| (0..3).map(|i| (p[i] - c[i]).abs()).sum::<f32>();
    presets
        .iter()
        .enumerate()
        .min_by(|a, b| d(a.1.1).total_cmp(&d(b.1.1)))
        .map_or(0, |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_restore_the_map() {
        let mut ed = Editor::default();
        let piece = ink_map::piece("crate").unwrap();
        ed.place_piece(piece, Vec2::new(1.0, 2.0));
        ed.place_piece(piece, Vec2::new(3.0, 4.0));
        assert_eq!(ed.map.objects.len(), 2);
        ed.undo();
        assert_eq!(ed.map.objects.len(), 1);
        ed.redo();
        assert_eq!(ed.map.objects.len(), 2);
        ed.sel = Some(Sel::Object(1));
        ed.delete();
        assert_eq!(ed.sel, None);
        assert_eq!(ed.map.objects.len(), 1);
        ed.undo();
        assert_eq!(ed.map.objects[1].pos, [3.0, 0.0, 4.0]);
    }

    #[test]
    fn mirroring_turns_round_the_middle() {
        let mut ed = Editor::default();
        ed.place_yaw = 90.0;
        ed.place_piece(ink_map::piece("snake").unwrap(), Vec2::new(4.0, 10.0));
        ed.sel = Some(Sel::Object(0));
        ed.mirror();
        let o = &ed.map.objects[1];
        assert_eq!(o.pos, [-4.0, 0.0, -10.0]);
        assert_eq!(o.yaw, 270.0);
        ed.map.spawns.blue = vec![[1.0, 20.0], [-2.0, 21.0]];
        ed.mirror_team_spawns();
        assert_eq!(ed.map.spawns.orange, vec![[-1.0, -20.0], [2.0, -21.0]]);
    }

    #[test]
    fn turning_while_placing_leaves_the_placed_piece_alone() {
        let mut ed = Editor::default();
        let piece = ink_map::piece("crate").unwrap();
        ed.tool = Tool::Place(piece);
        ed.place_piece(piece, Vec2::ZERO);
        assert_eq!(ed.sel, None);
        ed.turn(1.0);
        assert_eq!(ed.map.objects[0].yaw, 0.0);
        assert_eq!(ed.place_yaw, 90.0);
        // Even with something selected, the place tool turns the next piece.
        ed.sel = Some(Sel::Object(0));
        ed.turn(1.0);
        assert_eq!(ed.map.objects[0].yaw, 0.0);
        assert_eq!(ed.place_yaw, 180.0);
    }

    #[test]
    fn tilts_round_trip_through_a_rotation() {
        let mut o = ink_map::Object::new("crate", [0.0; 3], 30.0);
        o.pitch = 20.0;
        o.roll = -15.0;
        let rot = o.rotation();
        let mut back = o.clone();
        back.set_rotation(rot);
        assert!(back.rotation().angle_between(rot) < 1e-3, "{back:?}");
        assert!((back.yaw - 30.0).abs() < 1e-2 && (back.pitch - 20.0).abs() < 1e-2 && (back.roll + 15.0).abs() < 1e-2, "{back:?}");
    }

    #[test]
    fn resizing_keeps_the_base_ground_covering_the_field() {
        let mut ed = Editor::default();
        ed.resize(5.0, -3.0);
        assert_eq!(ed.map.half, [30.0, 22.0]);
        assert_eq!(ed.map.ground[0].size, [60.0, 44.0]);
    }
}
