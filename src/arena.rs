//! The simple shapes players, paintballs and ragdolls collide with. The maps
//! themselves (which models go where) are in `maps.rs`.

use bevy::prelude::*;

/// A solid thing on the field (shared with the map editor).
pub use ink_map::Shape;
use ink_map::{Convex, convex::circle_out_of};

#[derive(Debug, Clone)]
pub struct Collider {
    pub shape: Shape,
    /// Whether paint splats stick to it (false for odd shapes, where a flat
    /// decal on its bounding box would float in the air).
    pub decals: bool,
}

/// Every solid thing on the current map, with a grid over them so a query
/// only looks at the shapes near it instead of the whole map.
#[derive(Resource, Default)]
pub struct Colliders {
    list: Vec<Collider>,
    grid: Grid,
}

/// A hit along a segment.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    /// 0..=1 along the segment.
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    pub decals: bool,
}

impl Colliders {
    pub fn new(list: Vec<Collider>) -> Self {
        let grid = Grid::build(&list);
        Self { list, grid }
    }

    pub fn list(&self) -> &[Collider] {
        &self.list
    }

    /// The first thing (ground included) the segment `from -> to` hits.
    pub fn segment_hit(&self, from: Vec3, to: Vec3) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        let mut consider = |hit: RayHit| {
            if best.is_none_or(|b| hit.t < b.t) {
                best = Some(hit);
            }
        };
        if to.y <= 0.0 && from.y > 0.0 {
            let t = from.y / (from.y - to.y);
            consider(RayHit {
                t,
                point: from.lerp(to, t).with_y(0.0),
                normal: Vec3::Y,
                decals: true,
            });
        }
        let mut test = |c: &Collider| {
            if let Some((t, normal)) = c.shape.segment(from, to) {
                consider(RayHit {
                    t,
                    point: from.lerp(to, t),
                    normal,
                    decals: c.decals,
                });
            }
        };
        if from.is_finite() && to.is_finite() {
            // Only the shapes in the cells the segment crosses, in the same
            // order as the full list, so ties resolve the same way.
            self.grid.with_segment(from.xz(), to.xz(), |found| {
                for &i in found {
                    test(&self.list[i as usize]);
                }
            });
        } else {
            self.list.iter().for_each(test);
        }
        best
    }

    /// Pushes a standing cylinder (a player) out of everything it overlaps.
    /// `feet` is the bottom, `top` the head. Returns the height of the
    /// highest surface right under the feet, to stand on.
    pub fn resolve_player(&self, pos: &mut Vec3, radius: f32, feet: f32, top: f32) -> f32 {
        // Each shape only pushes a player whose circle overlaps its bounds,
        // so the shapes near the start are enough as long as the pushes
        // don't carry the player further than MOVE_SLACK; if they do, the
        // whole list is walked instead.
        let start = *pos;
        let reach = Vec2::splat(radius + MOVE_SLACK);
        let near = self.grid.with_area(start.xz() - reach, start.xz() + reach, |found| {
            let mut p = start;
            let mut floor = 0.0f32;
            for &i in found {
                player_vs(&self.list[i as usize], &mut p, radius, feet, top, &mut floor);
                if !within_slack(start, p) {
                    return None;
                }
            }
            Some((p, floor))
        });
        if let Some((p, floor)) = near {
            *pos = p;
            return floor;
        }
        let mut floor = 0.0f32;
        for c in &self.list {
            player_vs(c, pos, radius, feet, top, &mut floor);
        }
        floor
    }

    /// Keeps a small sphere (a ragdoll joint) outside every shape.
    pub fn push_out_point(&self, p: &mut Vec3, radius: f32) -> bool {
        // Same scheme as `resolve_player`. Turned boxes reach a little past
        // their bounds once grown by the radius, hence the extra margin.
        let start = *p;
        let reach = Vec2::splat(radius * 1.5 + MOVE_SLACK);
        let near = self.grid.with_area(start.xz() - reach, start.xz() + reach, |found| {
            let mut q = start;
            let mut touched = false;
            for &i in found {
                touched |= point_vs(&self.list[i as usize], &mut q, radius);
                if !within_slack(start, q) {
                    return None;
                }
            }
            Some((q, touched))
        });
        if let Some((q, touched)) = near {
            *p = q;
            return touched;
        }
        let mut touched = false;
        for c in &self.list {
            touched |= point_vs(c, p, radius);
        }
        touched
    }
}

/// How far (on the ground plane) pushes may move a player or joint before a
/// query falls back to checking every shape.
const MOVE_SLACK: f32 = 1.0;

fn within_slack(start: Vec3, now: Vec3) -> bool {
    (now.xz() - start.xz()).abs().max_element() <= MOVE_SLACK
}

/// One shape against a standing player; see [`Colliders::resolve_player`].
fn player_vs(c: &Collider, pos: &mut Vec3, radius: f32, feet: f32, top: f32, floor: &mut f32) {
    match &c.shape {
        &Shape::Box { min, max } => {
            let (stand, push) = player_vs_box(Vec2::new(pos.x, pos.z), min, max, radius, feet, top);
            if let Some(y) = stand {
                *floor = floor.max(y);
            }
            pos.x += push.x;
            pos.z += push.y;
        }
        &Shape::Oriented { center, half, rot } => {
            // Work in the box's own (turned) space, then turn the push back.
            let local = rot.inverse() * (*pos - center);
            let (stand, push) = player_vs_box(
                Vec2::new(local.x, local.z),
                Vec3::new(-half.x, center.y - half.y, -half.z),
                Vec3::new(half.x, center.y + half.y, half.z),
                radius,
                feet,
                top,
            );
            if let Some(y) = stand {
                *floor = floor.max(y);
            }
            let push = rot * Vec3::new(push.x, 0.0, push.y);
            pos.x += push.x;
            pos.z += push.z;
        }
        &Shape::Cylinder {
            base,
            radius: r,
            height,
        } => {
            let d = Vec2::new(pos.x - base.x, pos.z - base.z);
            let reach = r + radius;
            let dist2 = d.length_squared();
            if dist2 >= reach * reach {
                return;
            }
            let top_y = base.y + height;
            if feet >= top_y - 0.25 {
                if dist2 < r * r {
                    *floor = floor.max(top_y);
                }
            } else if top > base.y {
                let dist = dist2.sqrt().max(1e-4);
                let dir = if dist2 > 1e-8 { d / dist } else { Vec2::X };
                let push = dir * (reach - dist);
                pos.x += push.x;
                pos.z += push.y;
            }
        }
        Shape::Hull(h) => {
            let (stand, push) = player_vs_hull(h, Vec2::new(pos.x, pos.z), radius, feet, top);
            if let Some(y) = stand {
                *floor = floor.max(y);
            }
            pos.x += push.x;
            pos.z += push.y;
        }
    }
}

/// One shape against a ragdoll joint; see [`Colliders::push_out_point`].
fn point_vs(c: &Collider, p: &mut Vec3, radius: f32) -> bool {
    let mut touched = false;
    match &c.shape {
        &Shape::Box { min, max } => {
            let lo = min - Vec3::splat(radius);
            let hi = max + Vec3::splat(radius);
            if p.x > lo.x
                && p.x < hi.x
                && p.y > lo.y
                && p.y < hi.y
                && p.z > lo.z
                && p.z < hi.z
            {
                let exits = [
                    (p.x - lo.x, Vec3::new(lo.x, p.y, p.z)),
                    (hi.x - p.x, Vec3::new(hi.x, p.y, p.z)),
                    (hi.y - p.y, Vec3::new(p.x, hi.y, p.z)),
                    (p.z - lo.z, Vec3::new(p.x, p.y, lo.z)),
                    (hi.z - p.z, Vec3::new(p.x, p.y, hi.z)),
                ];
                if let Some(e) = exits.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
                    *p = e.1;
                    touched = true;
                }
            }
        }
        &Shape::Cylinder {
            base,
            radius: r,
            height,
        } => {
            let top = base.y + height + radius;
            if p.y > top || p.y < base.y {
                return false;
            }
            let d = Vec2::new(p.x - base.x, p.z - base.z);
            let reach = r + radius;
            let dist = d.length();
            if dist >= reach {
                return false;
            }
            if top - p.y < reach - dist {
                p.y = top;
            } else {
                let dir = if dist > 1e-4 { d / dist } else { Vec2::X };
                p.x = base.x + dir.x * reach;
                p.z = base.z + dir.y * reach;
            }
            touched = true;
        }
        &Shape::Oriented { center, half, rot } => {
            let local = rot.inverse() * (*p - center);
            let h = half + Vec3::splat(radius);
            if local.abs().cmplt(h).all() {
                // Out through the nearest face.
                let mut best = (f32::MAX, local);
                for axis in 0..3 {
                    for sign in [-1.0f32, 1.0] {
                        let gap = h[axis] - sign * local[axis];
                        if gap < best.0 {
                            let mut q = local;
                            q[axis] = sign * h[axis];
                            best = (gap, q);
                        }
                    }
                }
                *p = center + rot * best.1;
                touched = true;
            }
        }
        Shape::Hull(h) => {
            let (out, n) = h.depth(*p);
            if out < radius {
                *p += n * (radius - out);
                touched = true;
            }
        }
    }
    touched
}

/// A flat (x, z) grid over the map. Each cell lists, in list order, every
/// shape whose bounds touch it. Queries gather the cells they cover, so
/// a paintball only tests the few shapes along its path.
#[derive(Default)]
struct Grid {
    origin: Vec2,
    cell: f32,
    cols: usize,
    rows: usize,
    /// Cell `c`'s shapes are `items[starts[c]..starts[c + 1]]`.
    starts: Vec<u32>,
    items: Vec<u32>,
    /// How many shapes there are.
    count: u32,
}

/// Roughly a bunker's size: small enough that a cell holds few shapes, big
/// enough that big shapes don't fill many cells.
const CELL: f32 = 4.0;
/// Keeps the grid small however large an editor map gets.
const MAX_CELLS_PER_SIDE: f32 = 256.0;
/// Bounds and queries grow by this so shapes touching a cell's edge count.
const PAD: f32 = 1e-3;

thread_local! {
    /// Reused for the shapes a query finds, so queries don't allocate.
    static FOUND: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Grid {
    fn build(list: &[Collider]) -> Self {
        let bounds: Vec<(Vec2, Vec2)> = list
            .iter()
            .map(|c| {
                let (lo, hi) = c.shape.bounds();
                (lo.xz() - Vec2::splat(PAD), hi.xz() + Vec2::splat(PAD))
            })
            .collect();
        let Some((lo, hi)) = bounds.iter().copied().reduce(|a, b| (a.0.min(b.0), a.1.max(b.1))) else {
            return Self::default();
        };
        if !lo.is_finite() || !hi.is_finite() {
            // Not expected; one cell holding everything still works.
            return Self {
                origin: Vec2::ZERO,
                cell: f32::INFINITY,
                cols: 1,
                rows: 1,
                starts: vec![0, list.len() as u32],
                items: (0..list.len() as u32).collect(),
                count: list.len() as u32,
            };
        }
        let size = (hi - lo).max(Vec2::splat(PAD));
        let cell = CELL.max(size.max_element() / MAX_CELLS_PER_SIDE);
        let mut grid = Self {
            origin: lo,
            cell,
            cols: ((size.x / cell).ceil() as usize).max(1),
            rows: ((size.y / cell).ceil() as usize).max(1),
            starts: Vec::new(),
            items: Vec::new(),
            count: list.len() as u32,
        };
        // Count, then fill: shapes go in list order so every cell is sorted.
        let mut counts = vec![0u32; grid.cols * grid.rows + 1];
        for &(lo, hi) in &bounds {
            grid.each_cell(lo, hi, |c| counts[c + 1] += 1);
        }
        for c in 1..counts.len() {
            counts[c] += counts[c - 1];
        }
        let mut fill = counts.clone();
        let mut items = vec![0; *counts.last().unwrap() as usize];
        for (i, &(lo, hi)) in bounds.iter().enumerate() {
            grid.each_cell(lo, hi, |c| {
                items[fill[c] as usize] = i as u32;
                fill[c] += 1;
            });
        }
        grid.items = items;
        grid.starts = counts;
        grid
    }

    /// Calls `f` with every cell index the box `lo..hi` touches.
    fn each_cell(&self, lo: Vec2, hi: Vec2, mut f: impl FnMut(usize)) {
        let Some((c0, c1)) = self.span(lo.x - self.origin.x, hi.x - self.origin.x, self.cols) else {
            return;
        };
        let Some((r0, r1)) = self.span(lo.y - self.origin.y, hi.y - self.origin.y, self.rows) else {
            return;
        };
        for r in r0..=r1 {
            for c in c0..=c1 {
                f(r * self.cols + c);
            }
        }
    }

    /// The cells `a..b` (distances from the origin) covers out of `n`.
    fn span(&self, a: f32, b: f32, n: usize) -> Option<(usize, usize)> {
        let (a, b) = (a / self.cell, b / self.cell);
        if !(b >= 0.0 && a < n as f32) {
            return None;
        }
        let first = (a.max(0.0) as usize).min(n - 1);
        let last = (b.max(0.0) as usize).min(n - 1);
        Some((first, last))
    }

    /// Runs `f` on the shapes (sorted, each once) in the cells `push` adds.
    fn gather<R>(&self, push: impl FnOnce(&mut Vec<u32>), f: impl FnOnce(&[u32]) -> R) -> R {
        let run = |found: &mut Vec<u32>| {
            found.clear();
            push(found);
            found.sort_unstable();
            found.dedup();
            f(found)
        };
        FOUND.with(|found| match found.try_borrow_mut() {
            Ok(mut found) => run(&mut found),
            // A query from inside `f` (none today) gets its own list.
            Err(_) => run(&mut Vec::new()),
        })
    }

    fn cell_items(&self, c: usize) -> &[u32] {
        &self.items[self.starts[c] as usize..self.starts[c + 1] as usize]
    }

    /// The shapes whose bounds might touch the box `lo..hi`.
    fn with_area<R>(&self, lo: Vec2, hi: Vec2, f: impl FnOnce(&[u32]) -> R) -> R {
        let (lo, hi) = (lo - Vec2::splat(PAD), hi + Vec2::splat(PAD));
        self.gather(
            |out| {
                if self.cols > 0 && lo.is_finite() && hi.is_finite() {
                    self.each_cell(lo, hi, |c| out.extend_from_slice(self.cell_items(c)));
                } else {
                    out.extend(0..self.count);
                }
            },
            f,
        )
    }

    /// The shapes whose bounds might touch the segment `from -> to` (seen
    /// from above): row by row, only the columns the segment crosses there.
    fn with_segment<R>(&self, from: Vec2, to: Vec2, f: impl FnOnce(&[u32]) -> R) -> R {
        self.gather(
            |out| {
                if self.cols == 0 {
                    return;
                }
                let d = to - from;
                let (z_lo, z_hi) = (from.y.min(to.y) - PAD, from.y.max(to.y) + PAD);
                let Some((r0, r1)) = self.span(z_lo - self.origin.y, z_hi - self.origin.y, self.rows) else {
                    return;
                };
                for r in r0..=r1 {
                    // The part of the segment inside this row's band.
                    let band_lo = self.origin.y + r as f32 * self.cell - PAD;
                    let band_hi = band_lo + self.cell + 2.0 * PAD;
                    let (t0, t1) = if d.y.abs() < 1e-9 {
                        (0.0, 1.0)
                    } else {
                        let (a, b) = ((band_lo - from.y) / d.y, (band_hi - from.y) / d.y);
                        (a.min(b).max(0.0), a.max(b).min(1.0))
                    };
                    if t0 > t1 {
                        continue;
                    }
                    let (xa, xb) = (from.x + d.x * t0, from.x + d.x * t1);
                    let (x_lo, x_hi) = (xa.min(xb) - PAD, xa.max(xb) + PAD);
                    let Some((c0, c1)) = self.span(x_lo - self.origin.x, x_hi - self.origin.x, self.cols) else {
                        continue;
                    };
                    for c in c0..=c1 {
                        out.extend_from_slice(self.cell_items(r * self.cols + c));
                    }
                }
            },
            f,
        )
    }
}

/// A standing player (center `p` on the ground plane) against a box lined up
/// with the axes it's given in. Returns the height to stand on, if the player
/// is on top, and how far to push them out on the ground plane.
fn player_vs_box(p: Vec2, min: Vec3, max: Vec3, radius: f32, feet: f32, top: f32) -> (Option<f32>, Vec2) {
    let closest = Vec2::new(p.x.clamp(min.x, max.x), p.y.clamp(min.z, max.z));
    let d = p - closest;
    let dist2 = d.length_squared();
    let overlap = dist2 < radius * radius || (p.x > min.x && p.x < max.x && p.y > min.z && p.y < max.z);
    if !overlap {
        return (None, Vec2::ZERO);
    }
    if feet >= max.y - 0.25 {
        return (Some(max.y), Vec2::ZERO);
    }
    if top <= min.y {
        return (None, Vec2::ZERO);
    }
    let push = if dist2 > 1e-8 {
        let dist = dist2.sqrt();
        d / dist * (radius - dist)
    } else {
        // Center inside the box: leave by the nearest side.
        let exits = [
            (p.x - min.x, Vec2::new(-(p.x - min.x + radius), 0.0)),
            (max.x - p.x, Vec2::new(max.x - p.x + radius, 0.0)),
            (p.y - min.z, Vec2::new(0.0, -(p.y - min.z + radius))),
            (max.z - p.y, Vec2::new(0.0, max.z - p.y + radius)),
        ];
        exits
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|e| e.1)
            .unwrap_or_default()
    };
    (None, push)
}

/// A standing player against a convex shape: stands on it if its surface
/// under them is within a step of their feet, else is pushed out sideways
/// by its outline at the heights their body spans.
fn player_vs_hull(h: &Convex, p: Vec2, radius: f32, feet: f32, top: f32) -> (Option<f32>, Vec2) {
    if p.x + radius < h.lo.x || p.x - radius > h.hi.x || p.y + radius < h.lo.z || p.y - radius > h.hi.z {
        return (None, Vec2::ZERO);
    }
    if top <= h.lo.y {
        return (None, Vec2::ZERO);
    }
    // The surface under the middle of the feet, else under their rim.
    let mut surface = h.column(p.x, p.y).map(|(_, y, up)| (y, up));
    if surface.is_none() {
        for i in 0..8 {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            let q = p + Vec2::new(a.cos(), a.sin()) * radius * 0.9;
            if let Some((_, y, up)) = h.column(q.x, q.y)
                && surface.is_none_or(|s| y > s.0)
            {
                surface = Some((y, up));
            }
        }
    }
    if let Some((y, up)) = surface {
        // Flat tops take a small step like boxes; walkable slopes a ramp's.
        let step = if up > 0.95 {
            0.25
        } else if up > 0.6 {
            0.45
        } else {
            0.05
        };
        if feet >= y - step {
            return (Some(y), Vec2::ZERO);
        }
    }
    let (y0, y1) = ((feet + 0.05).max(h.lo.y + 0.01), top.min(h.hi.y - 0.01));
    if y1 < y0 {
        return (None, Vec2::ZERO);
    }
    let mut best = Vec2::ZERO;
    for i in 0..4 {
        let y = y0 + (y1 - y0) * i as f32 / 3.0;
        if let Some(push) = circle_out_of(&h.section(y), p, radius)
            && push.length_squared() > best.length_squared()
        {
            best = push;
        }
    }
    (None, best)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn segments_hit_boxes_cylinders_and_ground() {
        let c = Colliders::new(vec![
            Collider {
                shape: Shape::Box {
                    min: Vec3::new(-1.0, 0.0, -1.0),
                    max: Vec3::new(1.0, 2.0, 1.0),
                },
                decals: true,
            },
            Collider {
                shape: Shape::Cylinder {
                    base: Vec3::new(10.0, 0.0, 0.0),
                    radius: 1.0,
                    height: 2.0,
                },
                decals: true,
            },
        ]);
        let hit = c
            .segment_hit(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0))
            .unwrap();
        assert!((hit.point.x + 1.0).abs() < 1e-4);
        assert_eq!(hit.normal, Vec3::NEG_X);
        let hit = c
            .segment_hit(Vec3::new(5.0, 1.0, 0.0), Vec3::new(15.0, 1.0, 0.0))
            .unwrap();
        assert!((hit.point.x - 9.0).abs() < 1e-4);
        let hit = c
            .segment_hit(Vec3::new(5.0, 1.0, 5.0), Vec3::new(5.0, -1.0, 5.0))
            .unwrap();
        assert_eq!(hit.normal, Vec3::Y);
        assert!(
            c.segment_hit(Vec3::new(5.0, 1.0, 5.0), Vec3::new(6.0, 1.0, 5.0))
                .is_none()
        );

        let mut p = Vec3::new(1.2, 0.9, 0.0);
        c.resolve_player(&mut p, 0.42, 0.0, 1.8);
        assert!(p.x >= 1.42 - 1e-4);
        let mut p = Vec3::new(0.0, 3.0, 0.0);
        assert_eq!(c.resolve_player(&mut p, 0.42, 2.1, 3.9), 2.0);
    }

    #[test]
    fn turned_boxes_collide_as_turned() {
        // A 4 x 0.4 wall turned 45 degrees around the middle.
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let c = Colliders::new(vec![Collider {
            shape: Shape::Oriented {
                center: Vec3::new(0.0, 1.0, 0.0),
                half: Vec3::new(2.0, 1.0, 0.2),
                rot,
            },
            decals: true,
        }]);
        // A shot along x through the middle hits the wall's face...
        let hit = c
            .segment_hit(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0))
            .unwrap();
        assert!(hit.point.x.abs() < 0.4, "{}", hit.point);
        assert!((hit.normal - rot * Vec3::NEG_Z).length() < 1e-4 || (hit.normal - rot * Vec3::Z).length() < 1e-4);
        // ...but one past where the old bounding box reached misses.
        assert!(c
            .segment_hit(Vec3::new(-5.0, 1.0, -1.3), Vec3::new(-0.6, 1.0, -1.3))
            .is_none());
        // A player standing in the old bounding box's empty corner isn't pushed.
        let mut p = Vec3::new(1.2, 0.9, 1.2);
        c.resolve_player(&mut p, 0.3, 0.0, 1.8);
        assert_eq!(p, Vec3::new(1.2, 0.9, 1.2));
        // One walking into the face is pushed out along its normal.
        let mut p = Vec3::new(0.1, 0.9, -0.1);
        c.resolve_player(&mut p, 0.3, 0.0, 1.8);
        let local = rot.inverse() * (p - Vec3::new(0.0, 0.9, 0.0));
        assert!(local.z.abs() >= 0.5 - 1e-3, "{local}");
        // Standing on top.
        let mut p = Vec3::new(0.5, 2.9, -0.5);
        assert!((c.resolve_player(&mut p, 0.3, 2.05, 3.8) - 2.0).abs() < 1e-4);
    }

    fn here() -> ink_map::Place {
        ink_map::Place {
            at: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    fn hull(c: Convex) -> Collider {
        Collider {
            shape: Shape::Hull(std::sync::Arc::new(c)),
            decals: false,
        }
    }

    #[test]
    fn ramps_lift_players_toward_their_high_end() {
        let c = Colliders::new(vec![hull(Convex::wedge(
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
            here(),
        ))]);
        let mut p = Vec3::new(0.0, 0.9, 1.0);
        assert!(c.resolve_player(&mut p, 0.4, 0.0, 1.8).abs() < 1e-4);
        let mut p = Vec3::new(0.0, 1.4, 0.0);
        let floor = c.resolve_player(&mut p, 0.4, 0.45, 2.3);
        assert!((floor - 0.5).abs() < 1e-4);
        let mut p = Vec3::new(0.0, 1.9, -0.9);
        assert!((c.resolve_player(&mut p, 0.4, 0.95, 2.8) - 0.95).abs() < 1e-4);
        // Off the side: nothing.
        let mut p = Vec3::new(3.0, 0.9, 0.0);
        assert_eq!(c.resolve_player(&mut p, 0.4, 0.0, 1.8), 0.0);
        // Walking into its tall side from the ground pushes you away.
        let mut p = Vec3::new(1.2, 0.9, -0.6);
        assert_eq!(c.resolve_player(&mut p, 0.4, 0.0, 1.8), 0.0);
        assert!(p.x >= 1.4 - 1e-3, "{p}");
        // Paint hits its slope instead of passing through.
        let hit = c.segment_hit(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, -1.0, 0.0)).unwrap();
        assert!((hit.point.y - 0.5).abs() < 1e-3, "{}", hit.point);
    }

    #[test]
    fn ramps_turned_any_way_slope_the_way_they_face() {
        // Turned 30 degrees: the high end is 30 degrees around from -Z.
        let rot = Quat::from_rotation_y(30f32.to_radians());
        let place = ink_map::Place { rot, ..here() };
        let c = Colliders::new(vec![hull(Convex::wedge(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0), place))]);
        let up = rot * Vec3::NEG_Z;
        let mut p = up * 0.6 + Vec3::Y * 1.5;
        let floor = c.resolve_player(&mut p, 0.4, 0.75, 2.6);
        assert!((floor - 0.8).abs() < 1e-3, "{floor}");
        let mut p = -up * 0.6 + Vec3::Y * 1.0;
        let floor = c.resolve_player(&mut p, 0.4, 0.15, 2.0);
        assert!((floor - 0.2).abs() < 1e-3, "{floor}");
    }

    #[test]
    fn tilted_boxes_collide_as_they_look() {
        // A 4 x 2 x 0.4 slab leaning 30 degrees back, like a fallen wall.
        let rot = Quat::from_rotation_x(-30f32.to_radians());
        let c = Colliders::new(vec![hull(Convex::cuboid(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.2),
            ink_map::Place { rot, ..here() },
        ))]);
        // Its box around reaches z = -0.67 at the top; at the ground the slab
        // ends near z = +0.17, so a player standing at z = 0.8 is clear.
        let mut p = Vec3::new(0.0, 0.9, 0.8);
        assert_eq!(c.resolve_player(&mut p, 0.4, 0.0, 1.8), 0.0);
        assert_eq!(p, Vec3::new(0.0, 0.9, 0.8));
        // A shot over its leaning top misses; one into its face hits.
        assert!(c.segment_hit(Vec3::new(0.0, 1.85, 3.0), Vec3::new(0.0, 1.85, 0.6)).is_none());
        let hit = c.segment_hit(Vec3::new(0.0, 1.0, 3.0), Vec3::new(0.0, 1.0, -3.0)).unwrap();
        assert!(hit.normal.z > 0.8, "{}", hit.normal);
        // Walking into it pushes you back out.
        let mut p = Vec3::new(0.0, 0.9, 0.3);
        c.resolve_player(&mut p, 0.4, 0.0, 1.8);
        assert!(p.z > 0.4, "{p}");
        // Ragdoll joints are kept out.
        let mut q = Vec3::new(0.0, 1.0, -0.4);
        assert!(c.push_out_point(&mut q, 0.1));
    }

    /// A small repeatable random source for the tests below.
    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.f()
        }
        fn v(&mut self, lo: Vec3, hi: Vec3) -> Vec3 {
            Vec3::new(self.range(lo.x, hi.x), self.range(lo.y, hi.y), self.range(lo.z, hi.z))
        }
    }

    /// A crowded field of every kind of shape, some of them huge.
    pub(crate) fn random_field(seed: u64, n: usize) -> Vec<Collider> {
        let mut r = Rng(seed);
        let (lo, hi) = (Vec3::new(-60.0, 0.0, -45.0), Vec3::new(60.0, 3.0, 45.0));
        (0..n)
            .map(|i| {
                let at = r.v(lo, hi).with_y(r.range(0.0, 1.5));
                let size = if i % 40 == 0 {
                    Vec3::new(r.range(10.0, 50.0), 2.0, r.range(0.3, 1.0))
                } else {
                    r.v(Vec3::splat(0.2), Vec3::new(3.0, 2.5, 3.0))
                };
                let yaw = Quat::from_rotation_y(r.range(0.0, 6.28));
                let shape = match i % 5 {
                    0 => Shape::Box { min: at, max: at + size },
                    1 => Shape::Cylinder {
                        base: at,
                        radius: size.x * 0.5,
                        height: size.y,
                    },
                    2 => Shape::Oriented {
                        center: at + Vec3::Y * size.y * 0.5,
                        half: size * 0.5,
                        rot: yaw,
                    },
                    3 => Shape::Hull(std::sync::Arc::new(Convex::wedge(
                        -size * 0.5,
                        size * 0.5,
                        ink_map::Place {
                            at,
                            rot: yaw * Quat::from_rotation_x(r.range(-0.5, 0.5)),
                            scale: Vec3::ONE,
                        },
                    ))),
                    _ => Shape::Hull(std::sync::Arc::new(Convex::cuboid(
                        Vec3::ZERO,
                        size * 0.5,
                        ink_map::Place {
                            at,
                            rot: yaw * Quat::from_rotation_z(r.range(-0.6, 0.6)),
                            scale: Vec3::ONE,
                        },
                    ))),
                };
                Collider { shape, decals: i % 3 != 0 }
            })
            .collect()
    }

    /// The plain walk over every shape that the grid replaced.
    pub(crate) fn segment_hit_slow(c: &Colliders, from: Vec3, to: Vec3) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        let mut consider = |hit: RayHit| {
            if best.is_none_or(|b| hit.t < b.t) {
                best = Some(hit);
            }
        };
        if to.y <= 0.0 && from.y > 0.0 {
            let t = from.y / (from.y - to.y);
            consider(RayHit {
                t,
                point: from.lerp(to, t).with_y(0.0),
                normal: Vec3::Y,
                decals: true,
            });
        }
        for c in c.list() {
            if let Some((t, normal)) = c.shape.segment(from, to) {
                consider(RayHit {
                    t,
                    point: from.lerp(to, t),
                    normal,
                    decals: c.decals,
                });
            }
        }
        best
    }

    pub(crate) fn same_hit(a: Option<RayHit>, b: Option<RayHit>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.t == b.t && a.point == b.point && a.normal == b.normal && a.decals == b.decals,
            _ => false,
        }
    }

    #[test]
    fn grid_finds_exactly_what_checking_everything_does() {
        for seed in 1..4u64 {
            let c = Colliders::new(random_field(seed * 7919, 600));
            let mut r = Rng(seed * 104729);
            let (lo, hi) = (Vec3::new(-70.0, -0.5, -55.0), Vec3::new(70.0, 4.0, 55.0));
            let mut hits = 0;
            for i in 0..20_000 {
                let from = r.v(lo, hi);
                // Mostly paintball-sized steps, some across the whole map.
                let to = if i % 10 == 0 { r.v(lo, hi) } else { from + r.v(Vec3::splat(-2.0), Vec3::splat(2.0)) };
                let (fast, slow) = (c.segment_hit(from, to), segment_hit_slow(&c, from, to));
                assert!(same_hit(fast, slow), "{from} -> {to}: {fast:?} vs {slow:?}");
                hits += fast.is_some() as u32;
            }
            assert!(hits > 2000, "too few hits to mean anything: {hits}");

            let mut moved = 0;
            for _ in 0..20_000 {
                let start = r.v(lo, hi).with_y(0.0);
                let feet = r.range(0.0, 2.5);
                let (mut fast, mut slow) = (start, start);
                let a = c.resolve_player(&mut fast, 0.42, feet, feet + 1.8);
                let mut b = 0.0f32;
                for s in c.list() {
                    player_vs(s, &mut slow, 0.42, feet, feet + 1.8, &mut b);
                }
                assert!(fast == slow && a == b, "{start}: {fast} {a} vs {slow} {b}");
                moved += (fast != start) as u32;
            }
            assert!(moved > 1000, "too few pushes to mean anything: {moved}");

            for _ in 0..20_000 {
                let start = r.v(lo, hi);
                let (mut fast, mut slow) = (start, start);
                let a = c.push_out_point(&mut fast, 0.12);
                let mut b = false;
                for s in c.list() {
                    b |= point_vs(s, &mut slow, 0.12);
                }
                assert!(fast == slow && a == b, "{start}: {fast} {a} vs {slow} {b}");
            }
        }
    }

    #[test]
    fn grid_handles_empty_and_odd_input() {
        let empty = Colliders::default();
        assert!(empty.segment_hit(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0)).is_some());
        assert!(empty.segment_hit(Vec3::new(0.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0)).is_none());
        let c = Colliders::new(random_field(3, 50));
        assert!(c.segment_hit(Vec3::NAN, Vec3::ONE).is_none());
        let mut p = Vec3::new(1e30, 0.0, -1e30);
        c.resolve_player(&mut p, 0.42, 0.0, 1.8);
        assert_eq!(p, Vec3::new(1e30, 0.0, -1e30));
    }

    /// `cargo test --release --bin ink-cognito grid_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn grid_speed() {
        for n in [100, 500, 2000] {
            let c = Colliders::new(random_field(11, n));
            let mut r = Rng(5);
            // 400 balls (Ultra Bang) flying for 120 frames.
            let balls: Vec<(Vec3, Vec3)> = (0..400)
                .map(|_| {
                    let at = r.v(Vec3::new(-60.0, 0.5, -45.0), Vec3::new(60.0, 3.0, 45.0));
                    (at, r.v(Vec3::splat(-1.0), Vec3::splat(1.0)))
                })
                .collect();
            let time = |f: &dyn Fn(Vec3, Vec3) -> Option<RayHit>| {
                let t = std::time::Instant::now();
                let mut n = 0;
                for frame in 0..120 {
                    for &(p, v) in &balls {
                        let from = p + v * frame as f32 * 0.5;
                        n += f(from, from + v * 0.5).is_some() as u32;
                    }
                }
                (t.elapsed().as_secs_f64() * 1000.0 / 120.0, n)
            };
            let (slow, a) = time(&|f, t| segment_hit_slow(&c, f, t));
            let (fast, b) = time(&|f, t| c.segment_hit(f, t));
            assert_eq!(a, b);
            println!("{n} shapes, 400 balls: {slow:.3} ms/frame before, {fast:.3} ms/frame now");
        }
    }
}
