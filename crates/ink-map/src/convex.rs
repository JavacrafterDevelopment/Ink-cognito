//! Convex shapes turned any way: tilted boxes, ramps, rods and the hulls the
//! catalog fits to odd-shaped models. Everything here is in field space.

use bevy_math::{Quat, Vec2, Vec3};

/// A convex shape on the field.
#[derive(Debug, Clone, PartialEq)]
pub struct Convex {
    pub points: Vec<Vec3>,
    /// Faces: outward normal and `d`, inside is `n . p <= d`.
    pub planes: Vec<(Vec3, f32)>,
    /// Pairs of `points` indices.
    pub edges: Vec<[u16; 2]>,
    /// The axis-aligned box around it.
    pub lo: Vec3,
    pub hi: Vec3,
}

/// Where a convex shape is: placed at `at`, turned `rot`, scaled `scale`
/// (in its own axes, before turning).
#[derive(Debug, Clone, Copy)]
pub struct Place {
    pub at: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
}

impl Place {
    pub fn point(&self, p: Vec3) -> Vec3 {
        self.at + self.rot * (p * self.scale)
    }
}

/// The corners of a box with half size `half`, in the order [`BOX_EDGES`] uses.
fn box_points(half: Vec3) -> Vec<Vec3> {
    crate::box_corners(half).to_vec()
}

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

impl Convex {
    /// Builds one from a shape in its own space (`points`, `planes`, `edges`)
    /// moved to `place`.
    pub fn new(points: &[Vec3], planes: &[(Vec3, f32)], edges: &[[u16; 2]], place: Place) -> Self {
        let world: Vec<Vec3> = points.iter().map(|&p| place.point(p)).collect();
        let planes = planes
            .iter()
            .map(|&(n, d)| {
                // A point on the plane moves with the shape; the normal turns
                // with it and stretches the other way to the scale.
                let on = place.point(n * d);
                let wn = (place.rot * (n / place.scale)).normalize();
                (wn, wn.dot(on))
            })
            .collect();
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for &p in &world {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        Self {
            points: world,
            planes,
            edges: edges.to_vec(),
            lo,
            hi,
        }
    }

    /// A box centered on `center` (in its own space) with half size `half`.
    pub fn cuboid(center: Vec3, half: Vec3, place: Place) -> Self {
        let pts: Vec<Vec3> = box_points(half).into_iter().map(|p| p + center).collect();
        let planes = [
            (Vec3::X, center.x + half.x),
            (Vec3::NEG_X, -(center.x - half.x)),
            (Vec3::Y, center.y + half.y),
            (Vec3::NEG_Y, -(center.y - half.y)),
            (Vec3::Z, center.z + half.z),
            (Vec3::NEG_Z, -(center.z - half.z)),
        ];
        Self::new(&pts, &planes, &BOX_EDGES, place)
    }

    /// A ramp filling the box `min..max`, rising toward -Z.
    pub fn wedge(min: Vec3, max: Vec3, place: Place) -> Self {
        let pts = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(max.x, max.y, min.z),
        ];
        let slope = Vec3::new(0.0, max.z - min.z, max.y - min.y).normalize();
        let planes = [
            (Vec3::X, max.x),
            (Vec3::NEG_X, -min.x),
            (Vec3::NEG_Y, -min.y),
            (Vec3::NEG_Z, -min.z),
            (slope, slope.dot(Vec3::new(0.0, min.y, max.z))),
        ];
        let edges = [[0, 1], [2, 3], [4, 5], [0, 2], [1, 3], [0, 4], [1, 5], [2, 4], [3, 5]];
        Self::new(&pts, &planes, &edges, place)
    }

    /// A cylinder from `a` to `b` (its own space), as a 12-sided prism around it.
    pub fn rod(a: Vec3, b: Vec3, radius: f32, place: Place) -> Self {
        const SIDES: usize = 12;
        let axis = (b - a).normalize_or(Vec3::Y);
        let u = axis.any_orthonormal_vector();
        let w = axis.cross(u);
        // Corners a little outside the circle, so the sides touch it.
        let r = radius / (std::f32::consts::PI / SIDES as f32).cos();
        let mut pts = Vec::with_capacity(SIDES * 2);
        let mut planes = Vec::with_capacity(SIDES + 2);
        let mut edges = Vec::with_capacity(SIDES * 3);
        for end in [a, b] {
            for i in 0..SIDES {
                let ang = i as f32 / SIDES as f32 * std::f32::consts::TAU;
                pts.push(end + (u * ang.cos() + w * ang.sin()) * r);
            }
        }
        for i in 0..SIDES {
            let mid = (i as f32 + 0.5) / SIDES as f32 * std::f32::consts::TAU;
            let n = u * mid.cos() + w * mid.sin();
            planes.push((n, n.dot(a) + radius));
            let j = (i + 1) % SIDES;
            edges.push([i as u16, j as u16]);
            edges.push([(SIDES + i) as u16, (SIDES + j) as u16]);
            edges.push([i as u16, (SIDES + i) as u16]);
        }
        planes.push((axis, axis.dot(b)));
        planes.push((-axis, -axis.dot(a)));
        Self::new(&pts, &planes, &edges, place)
    }

    /// One of the catalog's fitted hulls.
    pub fn hull(h: &crate::catalog::Hull, place: Place) -> Self {
        let pts: Vec<Vec3> = h.points.iter().map(|&p| Vec3::from_array(p)).collect();
        let planes: Vec<(Vec3, f32)> = h
            .planes
            .iter()
            .map(|&[x, y, z, d]| (Vec3::new(x, y, z), d))
            .collect();
        Self::new(&pts, &planes, h.edges, place)
    }

    /// Where a vertical line through `(x, z)` is inside: bottom, top, and how
    /// upright the top face is (its normal's y).
    pub fn column(&self, x: f32, z: f32) -> Option<(f32, f32, f32)> {
        if x < self.lo.x || x > self.hi.x || z < self.lo.z || z > self.hi.z {
            return None;
        }
        let (mut lo, mut hi, mut up) = (f32::MIN, f32::MAX, 1.0);
        for &(n, d) in &self.planes {
            let rest = d - n.x * x - n.z * z;
            if n.y.abs() < 1e-5 {
                if rest < -1e-4 {
                    return None;
                }
            } else if n.y > 0.0 {
                let y = rest / n.y;
                if y < hi {
                    hi = y;
                    up = n.y;
                }
            } else {
                lo = lo.max(rest / n.y);
            }
        }
        (lo <= hi + 1e-4 && hi < f32::MAX).then_some((lo, hi, up))
    }

    /// The shape cut flat at height `y`, as a convex outline (x, z) going
    /// counterclockwise seen from above. Empty if it doesn't reach `y`.
    pub fn section(&self, y: f32) -> Vec<Vec2> {
        if y < self.lo.y || y > self.hi.y {
            return Vec::new();
        }
        let mut pts: Vec<Vec2> = Vec::new();
        for &[a, b] in &self.edges {
            let (a, b) = (self.points[a as usize], self.points[b as usize]);
            if (a.y - y) * (b.y - y) > 0.0 {
                continue;
            }
            if (a.y - b.y).abs() < 1e-6 {
                pts.push(Vec2::new(a.x, a.z));
                pts.push(Vec2::new(b.x, b.z));
            } else {
                let t = (y - a.y) / (b.y - a.y);
                let p = a.lerp(b, t);
                pts.push(Vec2::new(p.x, p.z));
            }
        }
        convex_outline(pts)
    }

    /// Where the segment `from -> to` enters: (0..=1 along it, outward
    /// normal). Starting inside counts as no hit.
    pub fn segment(&self, from: Vec3, to: Vec3) -> Option<(f32, Vec3)> {
        let d = to - from;
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        let mut normal = None;
        for &(n, dist) in &self.planes {
            let den = n.dot(d);
            let gap = dist - n.dot(from);
            if den.abs() < 1e-9 {
                if gap < 0.0 {
                    return None;
                }
                continue;
            }
            let t = gap / den;
            if den < 0.0 {
                if t > t0 {
                    t0 = t;
                    normal = Some(n);
                }
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
        normal.map(|n| (t0, n))
    }

    /// How far `p` is outside (positive) or inside (negative), and the face it's nearest.
    pub fn depth(&self, p: Vec3) -> (f32, Vec3) {
        let mut best = (f32::MIN, Vec3::Y);
        for &(n, d) in &self.planes {
            let s = n.dot(p) - d;
            if s > best.0 {
                best = (s, n);
            }
        }
        best
    }
}

/// Orders points into a convex outline (counterclockwise), dropping inner ones.
fn convex_outline(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts;
    }
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| a.distance_squared(*b) < 1e-10);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    // Andrew's monotone chain: the lower side, then the upper.
    let mut hull: Vec<Vec2> = Vec::with_capacity(pts.len() * 2);
    for &p in &pts {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 1e-9 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 1e-9 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// How far to move a circle (center `p`, `radius`) to get it out of a convex
/// outline, if they overlap.
pub fn circle_out_of(outline: &[Vec2], p: Vec2, radius: f32) -> Option<Vec2> {
    let n = outline.len();
    if n == 0 {
        return None;
    }
    if n < 3 {
        // A line or a point: push away from the nearest spot on it.
        let (a, b) = (outline[0], outline[n - 1]);
        let c = closest_on_segment(a, b, p);
        let d = p - c;
        let dist = d.length();
        return (dist < radius).then(|| if dist > 1e-6 { d / dist * (radius - dist) } else { Vec2::X * radius });
    }
    let mut inside = true;
    let mut least = (f32::MAX, Vec2::ZERO);
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let edge = b - a;
        let out = Vec2::new(edge.y, -edge.x).normalize_or_zero();
        let s = out.dot(p - a);
        if s > 0.0 {
            inside = false;
        }
        if -s < least.0 {
            least = (-s, out);
        }
    }
    if inside {
        return Some(least.1 * (least.0 + radius));
    }
    let mut best = (f32::MAX, Vec2::ZERO);
    for i in 0..n {
        let c = closest_on_segment(outline[i], outline[(i + 1) % n], p);
        let d = p.distance_squared(c);
        if d < best.0 {
            best = (d, c);
        }
    }
    let dist = best.0.sqrt();
    (dist < radius).then(|| (p - best.1) / dist.max(1e-6) * (radius - dist))
}

fn closest_on_segment(a: Vec2, b: Vec2, p: Vec2) -> Vec2 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-12 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here() -> Place {
        Place {
            at: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    #[test]
    fn a_wedge_rises_toward_minus_z() {
        let w = Convex::wedge(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0), here());
        let (_, top, up) = w.column(0.0, 1.0).unwrap();
        assert!(top.abs() < 1e-4);
        let (_, top, _) = w.column(0.0, -1.0).unwrap();
        assert!((top - 1.0).abs() < 1e-4);
        let (_, top, _) = w.column(0.3, 0.0).unwrap();
        assert!((top - 0.5).abs() < 1e-4 && up < 0.95);
        assert!(w.column(1.5, 0.0).is_none());
        // Cut at half height: the back half of the footprint.
        let s = w.section(0.5);
        let lo = s.iter().fold(Vec2::MAX, |a, &b| a.min(b));
        let hi = s.iter().fold(Vec2::MIN, |a, &b| a.max(b));
        assert!((lo - Vec2::new(-1.0, -1.0)).length() < 1e-4 && (hi - Vec2::new(1.0, 0.0)).length() < 1e-4, "{s:?}");
    }

    #[test]
    fn turned_and_scaled_boxes_keep_their_faces() {
        let place = Place {
            at: Vec3::new(5.0, 0.0, 0.0),
            rot: Quat::from_rotation_z(0.5) * Quat::from_rotation_y(0.3),
            scale: Vec3::new(2.0, 1.0, 0.5),
        };
        let c = Convex::cuboid(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0), place);
        // Every corner lies on three faces and inside the rest.
        for &p in &c.points {
            let on = c.planes.iter().filter(|(n, d)| (n.dot(p) - d).abs() < 1e-4).count();
            assert_eq!(on, 3);
            assert!(c.planes.iter().all(|(n, d)| n.dot(p) - d < 1e-4));
        }
        let mid = place.point(Vec3::new(0.0, 0.5, 0.0));
        assert!(c.depth(mid).0 < -0.2);
    }

    #[test]
    fn rods_are_round_enough() {
        let r = Convex::rod(Vec3::new(-2.0, 0.5, 0.0), Vec3::new(2.0, 0.5, 0.0), 0.5, here());
        let (lo, hi, _) = r.column(0.0, 0.0).unwrap();
        // The 12-sided prism reaches a little past the circle at its corners.
        assert!(lo.abs() < 0.03 && (hi - 1.0).abs() < 0.03, "{lo} {hi}");
        let hit = r.segment(Vec3::new(0.0, 0.5, -3.0), Vec3::new(0.0, 0.5, 3.0)).unwrap();
        assert!((hit.0 * 6.0 - 2.5).abs() < 0.03, "{}", hit.0 * 6.0);
        assert!(r.segment(Vec3::new(0.0, 1.2, -3.0), Vec3::new(0.0, 1.2, 3.0)).is_none());
    }

    #[test]
    fn circles_leave_outlines_the_short_way() {
        let sq = [Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)];
        let push = circle_out_of(&sq, Vec2::new(0.8, 0.0), 0.5).unwrap();
        assert!((push - Vec2::new(0.7, 0.0)).length() < 1e-4, "{push}");
        let push = circle_out_of(&sq, Vec2::new(1.3, 0.0), 0.5).unwrap();
        assert!((push - Vec2::new(0.2, 0.0)).length() < 1e-4, "{push}");
        assert!(circle_out_of(&sq, Vec2::new(1.6, 0.0), 0.5).is_none());
    }
}
