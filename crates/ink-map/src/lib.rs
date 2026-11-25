//! Ink-cognito map files, shared by the game and the map editor.
//!
//! A map is a JSON file ([`MapFile`]): the field's size, sky and ground
//! patches, the edge around it, every placed piece, and where players spawn.
//! [`build`] turns one into the models to show and the shapes that collide,
//! the same way in the game and in the editor.

pub mod catalog;
pub mod convex;
pub mod daylight;
mod pieces;

use bevy_math::{Quat, Rect, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::f32::consts::FRAC_PI_2;

pub use catalog::{Category, Hull, PIECES, Part, Piece, piece};
pub use convex::{Convex, Place};
pub use daylight::{Daylight, Fog, TIMES};
use std::sync::Arc;

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
    /// Sky color, sRGB 0..1. It also sets the time of day the map is lit
    /// for: see [`Daylight::for_sky`].
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
    /// Tilt forward / back (around the piece's own X axis after the yaw), degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch: f32,
    /// Tilt sideways (around the piece's own Z axis), degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub roll: f32,
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
            pitch: 0.0,
            roll: 0.0,
            scale: [1.0; 3],
            solid: true,
        }
    }

    /// Yaw, then pitch, then roll.
    pub fn rotation(&self) -> Quat {
        let yaw = Quat::from_rotation_y(yaw_radians(self.yaw));
        if self.pitch == 0.0 && self.roll == 0.0 {
            return yaw;
        }
        yaw * Quat::from_rotation_x(self.pitch.to_radians()) * Quat::from_rotation_z(self.roll.to_radians())
    }

    /// Sets yaw, pitch and roll (degrees, to a thousandth) from a rotation.
    pub fn set_rotation(&mut self, rot: Quat) {
        let (y, x, z) = rot.normalize().to_euler(bevy_math::EulerRot::YXZ);
        let deg = |r: f32| {
            let d = ((r.to_degrees() * 1000.0).round() / 1000.0).rem_euclid(360.0);
            if (d - 360.0).abs() < 1e-3 { 0.0 } else { d }
        };
        // Pitch reads best as -90..90.
        let tilt = |r: f32| {
            let d = deg(r);
            if d > 180.0 { d - 360.0 } else { d }
        };
        self.yaw = deg(y);
        self.pitch = tilt(x);
        self.roll = tilt(z);
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
            o.pitch = r(o.pitch);
            o.roll = r(o.roll);
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
#[derive(Debug, Clone, PartialEq)]
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
    /// An upright box turned around the vertical: `center`, half its size
    /// along its own axes, and its rotation.
    Oriented {
        center: Vec3,
        half: Vec3,
        rot: Quat,
    },
    /// Any other convex shape: tilted boxes, ramps, rods, fitted hulls.
    Hull(Arc<Convex>),
}

impl Shape {
    /// The axis-aligned box around it.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        match self {
            Shape::Box { min, max } => (*min, *max),
            Shape::Cylinder { base, radius, height } => (
                *base - Vec3::new(*radius, 0.0, *radius),
                *base + Vec3::new(*radius, *height, *radius),
            ),
            Shape::Oriented { center, half, rot } => {
                let mut lo = Vec3::splat(f32::MAX);
                let mut hi = Vec3::splat(f32::MIN);
                for c in box_corners(*half) {
                    let p = *center + *rot * c;
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
                (lo, hi)
            }
            Shape::Hull(h) => (h.lo, h.hi),
        }
    }

    /// The footprint on the ground (x, z), for the minimap.
    pub fn footprint(&self) -> Rect {
        let (lo, hi) = self.bounds();
        Rect::new(lo.x, lo.z, hi.x, hi.z)
    }

    pub fn height(&self) -> f32 {
        self.bounds().1.y
    }

    /// Its edges, to draw it.
    pub fn edges(&self) -> Vec<(Vec3, Vec3)> {
        const BOX: [(usize, usize); 12] =
            [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
        let boxed = |c: [Vec3; 8]| BOX.iter().map(|&(a, b)| (c[a], c[b])).collect();
        match self {
            Shape::Box { min, max } => {
                let (c, h) = ((*min + *max) * 0.5, (*max - *min) * 0.5);
                boxed(box_corners(h).map(|p| p + c))
            }
            Shape::Oriented { center, half, rot } => boxed(box_corners(*half).map(|p| *center + *rot * p)),
            Shape::Cylinder { base, radius, height } => {
                let n = 16;
                let at = |i: usize, y: f32| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    *base + Vec3::new(a.cos() * radius, y, a.sin() * radius)
                };
                let mut out = Vec::new();
                for i in 0..n {
                    out.push((at(i, 0.0), at(i + 1, 0.0)));
                    out.push((at(i, *height), at(i + 1, *height)));
                    if i % 4 == 0 {
                        out.push((at(i, 0.0), at(i, *height)));
                    }
                }
                out
            }
            Shape::Hull(h) => h.edges.iter().map(|&[a, b]| (h.points[a as usize], h.points[b as usize])).collect(),
        }
    }

    /// Where the segment `from -> to` first enters it: (0..=1 along it,
    /// outward normal). Starting inside counts as no hit.
    pub fn segment(&self, from: Vec3, to: Vec3) -> Option<(f32, Vec3)> {
        match self {
            Shape::Box { min, max } => segment_box(from, to, *min, *max),
            Shape::Cylinder { base, radius, height } => segment_cylinder(from, to, *base, *radius, *height),
            Shape::Oriented { center, half, rot } => {
                // In the box's own space it's a plain box.
                let inv = rot.inverse();
                segment_box(inv * (from - *center), inv * (to - *center), -*half, *half).map(|(t, n)| (t, *rot * n))
            }
            Shape::Hull(h) => {
                let (lo, hi) = (from.min(to), from.max(to));
                if hi.cmplt(h.lo).any() || lo.cmpgt(h.hi).any() {
                    return None;
                }
                h.segment(from, to)
            }
        }
    }
}

/// Slab test. Returns (t, outward normal) of where the segment enters the box.
pub fn segment_box(from: Vec3, to: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let d = to - from;
    let mut t_enter = 0.0f32;
    let mut t_exit = 1.0f32;
    let mut normal = Vec3::ZERO;
    for axis in 0..3 {
        let (o, dir, lo, hi) = (from[axis], d[axis], min[axis], max[axis]);
        if dir.abs() < 1e-8 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let mut t0 = (lo - o) / dir;
        let mut t1 = (hi - o) / dir;
        let mut n = Vec3::ZERO;
        n[axis] = -1.0;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
            n[axis] = 1.0;
        }
        if t0 > t_enter {
            t_enter = t0;
            normal = n;
        }
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }
    // Starting inside counts as no hit (the ball just left a surface).
    (normal != Vec3::ZERO).then_some((t_enter, normal))
}

fn segment_cylinder(from: Vec3, to: Vec3, base: Vec3, radius: f32, height: f32) -> Option<(f32, Vec3)> {
    let top = base.y + height;
    let d = to - from;
    // Side wall.
    let o = Vec2::new(from.x - base.x, from.z - base.z);
    let v = Vec2::new(d.x, d.z);
    let a = v.length_squared();
    let mut best: Option<(f32, Vec3)> = None;
    if a > 1e-10 {
        let b = 2.0 * o.dot(v);
        let c = o.length_squared() - radius * radius;
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 && c > 0.0 {
            let t = (-b - disc.sqrt()) / (2.0 * a);
            if (0.0..=1.0).contains(&t) {
                let p = from + d * t;
                if p.y >= base.y && p.y <= top {
                    let n = Vec3::new(p.x - base.x, 0.0, p.z - base.z).normalize_or(Vec3::X);
                    best = Some((t, n));
                }
            }
        }
    }
    // Top cap.
    if from.y > top && to.y <= top {
        let t = (from.y - top) / (from.y - to.y);
        let p = from + d * t;
        if Vec2::new(p.x - base.x, p.z - base.z).length_squared() <= radius * radius && best.is_none_or(|b| t < b.0) {
            best = Some((t, Vec3::Y));
        }
    }
    best
}

/// The eight corners of a box centered on 0 with half size `half`.
pub fn box_corners(half: Vec3) -> [Vec3; 8] {
    let h = half;
    [
        Vec3::new(-h.x, -h.y, -h.z),
        Vec3::new(h.x, -h.y, -h.z),
        Vec3::new(-h.x, h.y, -h.z),
        Vec3::new(h.x, h.y, -h.z),
        Vec3::new(-h.x, -h.y, h.z),
        Vec3::new(h.x, -h.y, h.z),
        Vec3::new(-h.x, h.y, h.z),
        Vec3::new(h.x, h.y, h.z),
    ]
}

/// Whether `rot` only turns around the vertical axis.
pub fn is_upright(rot: Quat) -> bool {
    (rot * Vec3::Y).y > 0.9999
}

/// Whether `rot` is an upright quarter turn (0, 90, 180 or 270 degrees), where
/// boxes stay lined up with the field.
pub fn is_quarter_turn(rot: Quat) -> bool {
    if !is_upright(rot) {
        return false;
    }
    let f = rot * Vec3::X;
    // Lined up with x or z.
    f.x.abs() > 0.9999 || f.z.abs() > 0.9999
}

/// A collision shape and whether paint sticks to it.
#[derive(Debug, Clone, PartialEq)]
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
///
/// Boxes turned a quarter turn stay lined up with the field (exact, and what
/// the built-in maps mostly use); other upright turns give turned boxes.
/// Anything tilted, every ramp, rod and fitted hull becomes a convex shape
/// that collides as it looks.
pub fn piece_solids(piece: &Piece, at: Vec3, rot: Quat, scale: Vec3) -> Vec<Solid> {
    let quarter = is_quarter_turn(rot);
    let upright = is_upright(rot);
    let place = Place { at, rot, scale };
    let hull = |c: Convex| Shape::Hull(Arc::new(c));
    piece
        .parts
        .iter()
        .map(|part| {
            let shape = match *part {
                Part::Box(min, max) => {
                    let (min, max) = (Vec3::from_array(min), Vec3::from_array(max));
                    if quarter {
                        let (lo, hi) = turned(rot, min * scale, max * scale);
                        Shape::Box { min: lo + at, max: hi + at }
                    } else if upright {
                        let (min, max) = (min * scale, max * scale);
                        Shape::Oriented {
                            center: at + rot * ((min + max) * 0.5),
                            half: (max - min).abs() * 0.5,
                            rot,
                        }
                    } else {
                        hull(Convex::cuboid((min + max) * 0.5, (max - min) * 0.5, place))
                    }
                }
                Part::Cyl(cx, cz, r, y0, h) if upright && (scale.x - scale.z).abs() < 1e-4 => Shape::Cylinder {
                    base: at + rot * (Vec3::new(cx, y0, cz) * scale),
                    radius: r * scale.x,
                    height: h * scale.y,
                },
                Part::Cyl(cx, cz, r, y0, h) => {
                    if (scale.x - scale.z).abs() < 1e-4 {
                        hull(Convex::rod(Vec3::new(cx, y0, cz), Vec3::new(cx, y0 + h, cz), r, place))
                    } else {
                        // Squashed round: the box around it.
                        let half = Vec3::new(r, h * 0.5, r);
                        hull(Convex::cuboid(Vec3::new(cx, y0 + h * 0.5, cz), half, place))
                    }
                }
                Part::Ramp(min, max) => hull(Convex::wedge(Vec3::from_array(min), Vec3::from_array(max), place)),
                Part::Turned(c, h, q) => {
                    let local = Quat::from_array(q).normalize();
                    let total = rot * local;
                    let (c, h) = (Vec3::from_array(c), Vec3::from_array(h));
                    let even = (scale.x - scale.y).abs() < 1e-4 && (scale.y - scale.z).abs() < 1e-4;
                    if is_upright(total) && even {
                        Shape::Oriented { center: at + rot * (c * scale), half: h * scale.x, rot: total }
                    } else {
                        // Turn it in the piece's space first, then place it.
                        let pts: Vec<Vec3> = box_corners(h).iter().map(|&p| c + local * p).collect();
                        let planes: Vec<(Vec3, f32)> = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z]
                            .iter()
                            .map(|&n| {
                                let wn = local * n;
                                (wn, wn.dot(c) + (n * h).abs().max_element())
                            })
                            .collect();
                        hull(Convex::new(&pts, &planes, &BOX_EDGES, place))
                    }
                }
                Part::Rod(a, b, r) => hull(Convex::rod(Vec3::from_array(a), Vec3::from_array(b), r, place)),
                Part::Hull(h) => hull(Convex::hull(h, place)),
            };
            Solid {
                shape,
                decals: piece.decals,
            }
        })
        .collect()
}

/// The edges of a box from [`box_corners`].
const BOX_EDGES: [[u16; 2]; 12] = [
    [0, 1],
    [2, 3],
    [4, 5],
    [6, 7],
    [0, 2],
    [1, 3],
    [4, 6],
    [5, 7],
    [0, 4],
    [1, 5],
    [2, 6],
    [3, 7],
];

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
pub const BUILT_IN_IDS: [&str; 6] = [
    "SplatPark",
    "Dockyard",
    "RingRoad",
    "ThreeLanes",
    "PineHollow",
    "City",
];

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

    fn test_piece(parts: &'static [Part]) -> Piece {
        Piece { id: "test", label: "Test", category: Category::Bunkers, model: "", parts, decals: true }
    }
    static SNAKE: std::sync::LazyLock<Piece> =
        std::sync::LazyLock::new(|| test_piece(&[Part::Box([-1.8, 0.0, -0.356], [1.8, 0.75, 0.356])]));
    static TALL_WALL: std::sync::LazyLock<Piece> =
        std::sync::LazyLock::new(|| test_piece(&[Part::Box([-1.006, 0.0, -0.306], [1.006, 2.0, 0.306])]));
    static CAN: std::sync::LazyLock<Piece> = std::sync::LazyLock::new(|| test_piece(&[Part::Cyl(0.0, 0.0, 0.537, 0.0, 1.31)]));

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
        let p = &SNAKE;
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
    fn odd_turns_and_tilts_give_turned_boxes() {
        let wall = &TALL_WALL;
        let mut o = Object::new("tall_wall", [0.0; 3], 90.0);
        let quarter = piece_solids(wall, Vec3::ZERO, o.rotation(), Vec3::ONE);
        assert!(matches!(quarter[0].shape, Shape::Box { .. }));
        o.yaw = 30.0;
        let s = piece_solids(wall, Vec3::ZERO, o.rotation(), Vec3::ONE);
        let Shape::Oriented { half, .. } = s[0].shape else { panic!() };
        assert!((half - Vec3::new(1.006, 1.0, 0.306)).length() < 1e-4);
        // A tilted can (a cylinder) becomes a rod lying the way it was tipped.
        let can = &CAN;
        let mut o = Object::new("can", [0.0; 3], 0.0);
        o.pitch = 90.0;
        let s = piece_solids(can, Vec3::ZERO, o.rotation(), Vec3::ONE);
        assert!(s.iter().all(|s| matches!(s.shape, Shape::Hull(_))));
        // Lying down, it's low and long.
        let (lo, hi) = s.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), s| {
            let (a, b) = s.shape.bounds();
            (lo.min(a), hi.max(b))
        });
        assert!(hi.y - lo.y < 1.2 && hi.z - lo.z > 1.2, "{lo} {hi}");
    }

    #[test]
    fn every_part_fits_inside_its_models_reach() {
        // Hulls are convex and every corner is inside all of their faces.
        for p in PIECES {
            for part in p.parts {
                if let Part::Hull(h) = part {
                    assert!(h.planes.len() >= 4 && h.points.len() >= 4, "{}", p.id);
                    for &c in h.points {
                        for &[x, y, z, d] in h.planes {
                            assert!(x * c[0] + y * c[1] + z * c[2] - d < 2e-3, "{}: corner {c:?} outside", p.id);
                        }
                    }
                    for &[a, b] in h.edges {
                        assert!((a as usize) < h.points.len() && (b as usize) < h.points.len(), "{}", p.id);
                    }
                }
            }
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
