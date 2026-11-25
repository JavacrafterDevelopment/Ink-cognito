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

#[derive(Resource, Default)]
pub struct Colliders(pub Vec<Collider>);

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
        for c in &self.0 {
            let hit = c.shape.segment(from, to);
            if let Some((t, normal)) = hit {
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

    /// Pushes a standing cylinder (a player) out of everything it overlaps.
    /// `feet` is the bottom, `top` the head. Returns the height of the
    /// highest surface right under the feet, to stand on.
    pub fn resolve_player(&self, pos: &mut Vec3, radius: f32, feet: f32, top: f32) -> f32 {
        let mut floor = 0.0f32;
        for c in &self.0 {
            match &c.shape {
                &Shape::Box { min, max } => {
                    let (stand, push) = player_vs_box(Vec2::new(pos.x, pos.z), min, max, radius, feet, top);
                    if let Some(y) = stand {
                        floor = floor.max(y);
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
                        floor = floor.max(y);
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
                        continue;
                    }
                    let top_y = base.y + height;
                    if feet >= top_y - 0.25 {
                        if dist2 < r * r {
                            floor = floor.max(top_y);
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
                        floor = floor.max(y);
                    }
                    pos.x += push.x;
                    pos.z += push.y;
                }
            }
        }
        floor
    }

    /// Keeps a small sphere (a ragdoll joint) outside every shape.
    pub fn push_out_point(&self, p: &mut Vec3, radius: f32) -> bool {
        let mut touched = false;
        for c in &self.0 {
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
                        continue;
                    }
                    let d = Vec2::new(p.x - base.x, p.z - base.z);
                    let reach = r + radius;
                    let dist = d.length();
                    if dist >= reach {
                        continue;
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
        }
        touched
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
mod tests {
    use super::*;

    #[test]
    fn segments_hit_boxes_cylinders_and_ground() {
        let c = Colliders(vec![
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
        let c = Colliders(vec![Collider {
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
        let c = Colliders(vec![hull(Convex::wedge(
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
        let c = Colliders(vec![hull(Convex::wedge(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0), place))]);
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
        let c = Colliders(vec![hull(Convex::cuboid(
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
}
