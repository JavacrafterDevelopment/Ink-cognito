//! The simple shapes players, paintballs and ragdolls collide with. The maps
//! themselves (which models go where) are in `maps.rs`.

use bevy::prelude::*;

/// A solid thing on the field.
#[derive(Debug, Clone, Copy)]
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

/// Height of a ramp's surface over `(x, z)`, if that's on it.
fn ramp_height(min: Vec3, max: Vec3, rise: Vec2, x: f32, z: f32) -> Option<f32> {
    if x < min.x || x > max.x || z < min.z || z > max.z {
        return None;
    }
    let t = if rise.x.abs() > rise.y.abs() {
        let f = (x - min.x) / (max.x - min.x).max(1e-4);
        if rise.x > 0.0 { f } else { 1.0 - f }
    } else {
        let f = (z - min.z) / (max.z - min.z).max(1e-4);
        if rise.y > 0.0 { f } else { 1.0 - f }
    };
    Some(min.y + (max.y - min.y) * t.clamp(0.0, 1.0))
}

#[derive(Debug, Clone, Copy)]
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
            let hit = match c.shape {
                Shape::Box { min, max } => segment_box(from, to, min, max),
                Shape::Cylinder {
                    base,
                    radius,
                    height,
                } => segment_cylinder(from, to, base, radius, height),
                // Paint rolls off slopes: only the ground under them counts.
                Shape::Ramp { .. } => None,
            };
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
            match c.shape {
                Shape::Box { min, max } => {
                    let closest = Vec2::new(pos.x.clamp(min.x, max.x), pos.z.clamp(min.z, max.z));
                    let p = Vec2::new(pos.x, pos.z);
                    let d = p - closest;
                    let dist2 = d.length_squared();
                    let overlap = dist2 < radius * radius
                        || (p.x > min.x && p.x < max.x && p.y > min.z && p.y < max.z);
                    if !overlap {
                        continue;
                    }
                    if feet >= max.y - 0.25 {
                        floor = floor.max(max.y);
                    } else if top > min.y {
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
                        pos.x += push.x;
                        pos.z += push.y;
                    }
                }
                Shape::Cylinder {
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
                Shape::Ramp { min, max, rise } => {
                    if let Some(h) = ramp_height(min, max, rise, pos.x, pos.z)
                        && feet >= h - 0.45
                    {
                        floor = floor.max(h);
                    }
                }
            }
        }
        floor
    }

    /// Keeps a small sphere (a ragdoll joint) outside every shape.
    pub fn push_out_point(&self, p: &mut Vec3, radius: f32) -> bool {
        let mut touched = false;
        for c in &self.0 {
            match c.shape {
                Shape::Box { min, max } => {
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
                Shape::Cylinder {
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
                Shape::Ramp { min, max, rise } => {
                    if let Some(h) = ramp_height(min, max, rise, p.x, p.z)
                        && p.y < h + radius
                        && p.y > h - 0.6
                    {
                        p.y = h + radius;
                        touched = true;
                    }
                }
            }
        }
        touched
    }
}

/// Slab test. Returns (t, outward normal) of where the segment enters the box.
fn segment_box(from: Vec3, to: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
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

fn segment_cylinder(
    from: Vec3,
    to: Vec3,
    base: Vec3,
    radius: f32,
    height: f32,
) -> Option<(f32, Vec3)> {
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
        if Vec2::new(p.x - base.x, p.z - base.z).length_squared() <= radius * radius
            && best.is_none_or(|b| t < b.0)
        {
            best = Some((t, Vec3::Y));
        }
    }
    best
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
    fn ramps_lift_players_toward_their_high_end() {
        let c = Colliders(vec![Collider {
            shape: Shape::Ramp {
                min: Vec3::new(-1.0, 0.0, -1.0),
                max: Vec3::new(1.0, 1.0, 1.0),
                rise: Vec2::new(0.0, -1.0),
            },
            decals: false,
        }]);
        let mut p = Vec3::new(0.0, 0.9, 1.0);
        assert!(c.resolve_player(&mut p, 0.4, 0.0, 1.8).abs() < 1e-4);
        let mut p = Vec3::new(0.0, 1.4, 0.0);
        let floor = c.resolve_player(&mut p, 0.4, 0.45, 2.3);
        assert!((floor - 0.5).abs() < 1e-4);
        let mut p = Vec3::new(0.0, 1.9, -1.0);
        assert!((c.resolve_player(&mut p, 0.4, 0.95, 2.8) - 1.0).abs() < 1e-4);
        // Off the side: nothing.
        let mut p = Vec3::new(3.0, 0.9, 0.0);
        assert_eq!(c.resolve_player(&mut p, 0.4, 0.0, 1.8), 0.0);
    }
}
