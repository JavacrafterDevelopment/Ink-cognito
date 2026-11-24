//! Ink-cognito map files, shared by the game and the map editor.
//!
//! A map is a JSON file ([`MapFile`]): the field's size, sky and ground
//! patches, the edge around it, every placed piece, and where players spawn.
//! [`build`] turns one into the models to show and the shapes that collide,
//! the same way in the game and in the editor.

pub mod catalog;

use bevy_math::{Quat, Rect, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::f32::consts::FRAC_PI_2;

pub use catalog::{Category, PIECES, Part, Piece, piece};

/// The newest map file format this code reads and writes.
pub const FORMAT: u32 = 1;

/// The file extension map files use.
pub const EXTENSION: &str = "json";

/// A map, as saved on disk.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MapFile {
    #[serde(default = "format_one")]
    pub format: u32,
    /// Shown in the map picker.
    pub name: String,
    /// A couple of words on how it plays ("Close quarters").
    #[serde(default)]
    pub style: String,
    /// A sentence for the map picker.
    #[serde(default)]
    pub blurb: String,
    /// "Small", "Medium" or "Large".
    #[serde(default)]
    pub size: String,
    /// Half the playable size along x and z (meters). The field is centered on 0, 0.
    pub half: [f32; 2],
    /// Sky color, sRGB 0..1.
    #[serde(default = "default_sky")]
    pub sky: [f32; 3],
    /// What stands around the edge.
    #[serde(default)]
    pub perimeter: Perimeter,
    /// Flat ground patches, drawn in order (later ones on top).
    #[serde(default)]
    pub ground: Vec<Patch>,
    /// Everything placed on the field.
    #[serde(default)]
    pub objects: Vec<Object>,
    #[serde(default)]
    pub spawns: Spawns,
}

fn format_one() -> u32 {
    1
}

fn default_sky() -> [f32; 3] {
    [0.55, 0.76, 0.96]
}

fn white() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

fn default_tile() -> f32 {
    3.0
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_one(s: &[f32; 3]) -> bool {
    *s == [1.0, 1.0, 1.0]
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// The ground textures.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroundTex {
    #[default]
    Turf,
    Dirt,
}

/// A flat rectangle of ground.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Patch {
    pub center: [f32; 2],
    pub size: [f32; 2],
    #[serde(default)]
    pub texture: GroundTex,
    /// Multiplies the texture, sRGB 0..1.
    #[serde(default = "white")]
    pub tint: [f32; 3],
    /// Meters per texture repeat.
    #[serde(default = "default_tile")]
    pub tile: f32,
}

/// What stands around the edge of the field. Every kind also gets invisible
/// walls that stop players and paint.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Perimeter {
    #[default]
    Nets,
    Concrete,
    Fence,
    /// Only the invisible walls.
    None,
}

impl Perimeter {
    pub const ALL: [Perimeter; 4] = [
        Perimeter::Nets,
        Perimeter::Concrete,
        Perimeter::Fence,
        Perimeter::None,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Perimeter::Nets => "Nets",
            Perimeter::Concrete => "Concrete",
            Perimeter::Fence => "Fence",
            Perimeter::None => "None",
        }
    }
}

/// One placed piece.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Object {
    /// A piece id from the catalog.
    pub piece: String,
    /// Where its origin sits (y = 0 is the ground).
    pub pos: [f32; 3],
    /// Turn around the vertical axis, degrees (counterclockwise seen from above).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub yaw: f32,
    #[serde(default = "white", skip_serializing_if = "is_one")]
    pub scale: [f32; 3],
    /// False: just scenery, nothing collides with it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub solid: bool,
}

impl Object {
    pub fn new(piece: &str, pos: [f32; 3], yaw: f32) -> Self {
        Self {
            piece: piece.to_string(),
            pos,
            yaw,
            scale: [1.0; 3],
            solid: true,
        }
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(yaw_radians(self.yaw))
    }
}

/// Degrees to radians, exact for quarter turns.
pub fn yaw_radians(degrees: f32) -> f32 {
    let turns = degrees / 90.0;
    if turns.fract() == 0.0 {
        turns * FRAC_PI_2
    } else {
        degrees.to_radians()
    }
}

/// Spawn spots on the ground, as x, z.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Spawns {
    /// Blue team (the +z end).
    #[serde(default)]
    pub blue: Vec<[f32; 2]>,
    /// Orange team (the -z end).
    #[serde(default)]
    pub orange: Vec<[f32; 2]>,
    /// Free-for-all.
    #[serde(default)]
    pub ffa: Vec<[f32; 2]>,
}

impl MapFile {
    /// An empty field to start from.
    pub fn blank(name: &str) -> Self {
        Self {
            format: FORMAT,
            name: name.to_string(),
            style: "Custom".to_string(),
            blurb: String::new(),
            size: "Medium".to_string(),
            half: [25.0, 25.0],
            sky: default_sky(),
            perimeter: Perimeter::Nets,
            ground: vec![Patch {
                center: [0.0, 0.0],
                size: [50.0, 50.0],
                texture: GroundTex::Turf,
                tint: white(),
                tile: 3.0,
            }],
            objects: Vec::new(),
            spawns: Spawns::default(),
        }
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let map: MapFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if map.format > FORMAT {
            return Err(format!(
                "made by a newer version (format {}, this reads up to {FORMAT})",
                map.format
            ));
        }
        if !(map.half[0] > 1.0 && map.half[1] > 1.0) {
            return Err("the field is too small".into());
        }
        Ok(map)
    }

    /// Pretty JSON, with short arrays and objects (positions, placed
    /// pieces) kept on one line so the file stays readable and diffs well.
    pub fn to_json(&self) -> String {
        let pretty = serde_json::to_string_pretty(&self.tidied()).expect("map files always serialize");
        let mut out = String::with_capacity(pretty.len());
        let mut stack: Vec<usize> = Vec::new();
        let (mut in_str, mut escaped) = (false, false);
        for c in pretty.chars() {
            out.push(c);
            if in_str {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '[' | '{' => stack.push(out.len() - 1),
                ']' | '}' => {
                    let Some(start) = stack.pop() else { continue };
                    let group = &out[start..];
                    if group[1..group.len() - 1].contains('{') {
                        continue;
                    }
                    let flat = one_line(group);
                    if flat.len() <= 120 {
                        out.truncate(start);
                        out.push_str(&flat);
                    }
                }
                _ => {}
            }
        }
        out.push('\n');
        out
    }

    /// A copy with every number rounded to the millimeter (and the thousandth
    /// of a degree), so files don't fill up with float noise like 2.5999985.
    pub fn tidied(&self) -> Self {
        fn r(v: f32) -> f32 {
            let t = (v * 1000.0).round() / 1000.0;
            if t == 0.0 { 0.0 } else { t }
        }
        fn rs<const N: usize>(a: [f32; N]) -> [f32; N] {
            a.map(r)
        }
        let mut m = self.clone();
        m.half = rs(m.half);
        m.sky = rs(m.sky);
        for p in &mut m.ground {
            p.center = rs(p.center);
            p.size = rs(p.size);
            p.tint = rs(p.tint);
            p.tile = r(p.tile);
        }
        for o in &mut m.objects {
            o.pos = rs(o.pos);
            o.yaw = r(o.yaw);
            o.scale = rs(o.scale);
        }
        for list in [&mut m.spawns.blue, &mut m.spawns.orange, &mut m.spawns.ffa] {
            for p in list.iter_mut() {
                *p = rs(*p);
            }
        }
        m
    }

    pub fn half(&self) -> Vec2 {
        Vec2::from_array(self.half)
    }

    /// Problems worth fixing before playing it.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for o in &self.objects {
            if piece(&o.piece).is_none() {
                out.push(format!("unknown piece \"{}\"", o.piece));
            }
        }
        if self.spawns.blue.is_empty() || self.spawns.orange.is_empty() {
            out.push("each team needs at least one spawn".into());
        }
        if self.spawns.ffa.is_empty() {
            out.push("no free-for-all spawns".into());
        }
        out
    }
}

/// Joins pretty JSON onto one line (raw newlines only appear between tokens).
fn one_line(group: &str) -> String {
    let mut out = String::with_capacity(group.len());
    let mut chars = group.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            if out.ends_with(',') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

// --- Building -------------------------------------------------------------

/// A solid thing on the field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Box {
        min: Vec3,
        max: Vec3,
    },
    /// Upright cylinder standing on `base`.
    Cylinder {
        base: Vec3,
        radius: f32,
        height: f32,
    },
    /// A slope to walk up: `min.y` at one end, `max.y` at the other. `rise`
    /// points (on the ground plane) from the low end to the high end.
    Ramp {
        min: Vec3,
        max: Vec3,
        rise: Vec2,
    },
}

impl Shape {
    /// The footprint on the ground (x, z), for the minimap.
    pub fn footprint(&self) -> Rect {
        match *self {
            Shape::Box { min, max } | Shape::Ramp { min, max, .. } => {
                Rect::new(min.x, min.z, max.x, max.z)
            }
            Shape::Cylinder { base, radius, .. } => Rect::new(
                base.x - radius,
                base.z - radius,
                base.x + radius,
                base.z + radius,
            ),
        }
    }

    pub fn height(&self) -> f32 {
        match *self {
            Shape::Box { max, .. } | Shape::Ramp { max, .. } => max.y,
            Shape::Cylinder { base, height, .. } => base.y + height,
        }
    }
}

/// A collision shape and whether paint sticks to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Solid {
    pub shape: Shape,
    pub decals: bool,
}

/// A model to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// Path under `assets/`.
    pub path: &'static str,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    /// Which `MapFile::objects` entry it came from (none: the edge).
    pub object: Option<usize>,
}

/// A map turned into what to show and what collides.
#[derive(Debug, Clone, Default)]
pub struct Built {
    pub models: Vec<Model>,
    pub solids: Vec<Solid>,
    /// Spawn spots on the ground: blue, orange.
    pub spawns: [Vec<Vec3>; 2],
    pub ffa: Vec<Vec3>,
}

/// The collision shapes of `piece` placed at `at`, turned `rot`, scaled `scale`.
pub fn piece_solids(piece: &Piece, at: Vec3, rot: Quat, scale: Vec3) -> Vec<Solid> {
    piece
        .parts
        .iter()
        .map(|part| {
            let shape = match *part {
                Part::Box(min, max) => {
                    let (lo, hi) = turned(rot, Vec3::from_array(min) * scale, Vec3::from_array(max) * scale);
                    Shape::Box {
                        min: lo + at,
                        max: hi + at,
                    }
                }
                Part::Cyl(cx, cz, r, y0, h) => Shape::Cylinder {
                    base: at + rot * (Vec3::new(cx, y0, cz) * scale),
                    radius: r * scale.x.max(scale.z),
                    height: h * scale.y,
                },
                Part::Ramp(min, max) => {
                    let (lo, hi) = turned(rot, Vec3::from_array(min) * scale, Vec3::from_array(max) * scale);
                    let up = rot * Vec3::NEG_Z;
                    Shape::Ramp {
                        min: lo + at,
                        max: hi + at,
                        rise: Vec2::new(up.x, up.z),
                    }
                }
            };
            Solid {
                shape,
                decals: piece.decals,
            }
        })
        .collect()
}

/// The axis-aligned box around a box turned `rot` (exact for quarter turns).
fn turned(rot: Quat, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for x in [min.x, max.x] {
        for z in [min.z, max.z] {
            let p = rot * Vec3::new(x, 0.0, z);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    // Quarter turns: snap away float noise.
    let snap = |v: Vec3| (v * 1000.0).round() / 1000.0;
    (snap(lo).with_y(min.y), snap(hi).with_y(max.y))
}

/// Turns a map file into models, colliders and spawn spots. Objects naming an
/// unknown piece are skipped.
pub fn build(map: &MapFile) -> Built {
    let mut out = Built::default();
    for (i, o) in map.objects.iter().enumerate() {
        let Some(piece) = piece(&o.piece) else {
            continue;
        };
        let at = Vec3::from_array(o.pos);
        let rot = o.rotation();
        let scale = Vec3::from_array(o.scale);
        out.models.push(Model {
            path: piece.model,
            translation: at,
            rotation: rot,
            scale,
            object: Some(i),
        });
        if o.solid {
            out.solids.extend(piece_solids(piece, at, rot, scale));
        }
    }
    perimeter(map.perimeter, map.half(), &mut out);
    let ground = |v: &Vec<[f32; 2]>| v.iter().map(|&[x, z]| Vec3::new(x, 0.0, z)).collect();
    out.spawns = [ground(&map.spawns.blue), ground(&map.spawns.orange)];
    out.ffa = ground(&map.spawns.ffa);
    out
}

/// The edge: what it looks like, plus invisible walls that stop players and paint.
fn perimeter(kind: Perimeter, h: Vec2, out: &mut Built) {
    let mut model = |path: &'static str, translation: Vec3, yaw: f32, sx: f32| {
        out.models.push(Model {
            path,
            translation,
            rotation: Quat::from_rotation_y(yaw),
            scale: Vec3::new(sx, 1.0, 1.0),
            object: None,
        });
    };
    match kind {
        Perimeter::Nets => {
            let section = 2.474;
            for (len, along_x) in [(h.x * 2.0, true), (h.y * 2.0, false)] {
                let count = (len / section).ceil() as i32;
                let scale = len / (count as f32 * section);
                for i in 0..count {
                    let along = -len * 0.5 + (i as f32 + 0.5) * section * scale;
                    for side in [-1.0, 1.0] {
                        let (x, z, rot) = if along_x {
                            (along, side * h.y, 0.0)
                        } else {
                            (side * h.x, along, FRAC_PI_2)
                        };
                        model(
                            "models/props/netting_fence_section.glb",
                            Vec3::new(x, 0.0, z),
                            rot,
                            scale,
                        );
                    }
                }
            }
        }
        Perimeter::Concrete | Perimeter::Fence => {
            let (path, depth) = match kind {
                Perimeter::Concrete => ("models/map-kit/walls/concrete_wall_2m.glb", 0.15),
                _ => ("models/map-kit/structures/fence_wood_2m.glb", 0.06),
            };
            for (len, along_x) in [(h.x * 2.0, true), (h.y * 2.0, false)] {
                let count = (len / 2.0).round().max(1.0) as i32;
                let scale = len / (count as f32 * 2.0);
                for i in 0..count {
                    let along = -len * 0.5 + (i as f32 + 0.5) * 2.0 * scale;
                    for side in [-1.0, 1.0] {
                        let (x, z, rot) = if along_x {
                            (along, side * (h.y + depth), 0.0)
                        } else {
                            (side * (h.x + depth), along, FRAC_PI_2)
                        };
                        model(path, Vec3::new(x, 0.0, z), rot, scale);
                    }
                }
            }
            // Corner posts.
            for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
                model(
                    "models/map-kit/props/boundary_post.glb",
                    Vec3::new(sx * (h.x + depth), 0.0, sz * (h.y + depth)),
                    0.0,
                    1.0,
                );
            }
        }
        Perimeter::None => {}
    }
    let wall = 2.4;
    for (min, max) in [
        (
            Vec3::new(-h.x - 1.0, 0.0, -h.y - 1.0),
            Vec3::new(h.x + 1.0, wall, -h.y),
        ),
        (
            Vec3::new(-h.x - 1.0, 0.0, h.y),
            Vec3::new(h.x + 1.0, wall, h.y + 1.0),
        ),
        (Vec3::new(-h.x - 1.0, 0.0, -h.y), Vec3::new(-h.x, wall, h.y)),
        (Vec3::new(h.x, 0.0, -h.y), Vec3::new(h.x + 1.0, wall, h.y)),
    ] {
        out.solids.push(Solid {
            shape: Shape::Box { min, max },
            decals: false,
        });
    }
}

/// The file names (without `.json`) of the maps that come with the game. The
/// game compiles these in, so a custom map can't take one of these names.
pub const BUILT_IN_IDS: [&str; 5] = ["SplatPark", "Dockyard", "RingRoad", "ThreeLanes", "PineHollow"];

/// Where custom maps live: `Ink-cognito/maps` in the game's settings folder
/// (`%APPDATA%` on Windows, Application Support on macOS, `~/.config` elsewhere).
pub fn custom_maps_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| {
            let mut p = PathBuf::from(h);
            if cfg!(target_os = "macos") {
                p.push("Library/Application Support");
            } else {
                p.push(".config");
            }
            p
        })
    })?;
    Some(base.join("Ink-cognito").join("maps"))
}

/// Turns a file name into a map id: letters, digits, `_` and `-`, at most 31 bytes.
pub fn id_from_stem(stem: &str) -> String {
    let mut id: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    id.truncate(31);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let mut map = MapFile::blank("Test");
        map.objects.push(Object::new("crate", [1.0, 0.0, 2.0], 90.0));
        let mut deco = Object::new("pine", [3.0, 0.0, 4.0], 17.5);
        deco.solid = false;
        deco.scale = [1.5, 1.5, 1.5];
        map.objects.push(deco);
        map.spawns.blue.push([0.0, 10.0]);
        let back = MapFile::from_json(&map.to_json()).unwrap();
        assert_eq!(map, back);
    }

    #[test]
    fn scenery_has_no_colliders() {
        let mut map = MapFile::blank("Test");
        map.perimeter = Perimeter::None;
        map.objects.push(Object::new("crate", [0.0; 3], 0.0));
        let mut deco = Object::new("crate", [5.0, 0.0, 0.0], 0.0);
        deco.solid = false;
        map.objects.push(deco);
        let built = build(&map);
        assert_eq!(built.models.len(), 2);
        // One crate plus the four invisible edge walls.
        assert_eq!(built.solids.len(), 5);
    }

    #[test]
    fn quarter_turns_swap_box_sides() {
        let p = piece("snake").unwrap();
        let s = piece_solids(p, Vec3::ZERO, Quat::from_rotation_y(yaw_radians(90.0)), Vec3::ONE);
        let Shape::Box { min, max } = s[0].shape else {
            panic!()
        };
        assert!((max.x - 0.356).abs() < 1e-3 && (max.z - 1.8).abs() < 1e-3, "{min} {max}");
    }

    /// The built-in maps' files are exactly what the editor would save, so
    /// editing one in the editor only shows real changes in a diff. Run with
    /// INK_MAP_REFORMAT=1 to rewrite them.
    #[test]
    fn built_in_files_are_in_editor_format() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps");
        for id in BUILT_IN_IDS {
            let path = dir.join(format!("{id}.json"));
            // Line endings don't count (a Windows checkout may have CRLF).
            let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            let map = MapFile::from_json(&text).unwrap();
            let canonical = map.to_json();
            if std::env::var_os("INK_MAP_REFORMAT").is_some() {
                std::fs::write(&path, &canonical).unwrap();
            } else {
                assert!(text == canonical, "{id}.json isn't in editor format");
            }
            assert_eq!(MapFile::from_json(&canonical).unwrap(), map.tidied());
        }
    }

    #[test]
    fn every_model_exists() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        for p in PIECES {
            assert!(assets.join(p.model).exists(), "{} is missing", p.model);
        }
    }
}
