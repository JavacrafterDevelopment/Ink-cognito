//! 3D surface texture painting engine: world-space brush projection across UV
//! triangles (seamless across UV islands and corners!), smooth stroke
//! interpolation, mirror-X symmetry, brush tips, eraser, eyedropper, flood
//! fill, 2D UV canvas painting, and procedural texture generators.

use crate::model::{MeshPart, TextureSlot};
use bevy::prelude::{Vec2, Vec3};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintTool {
    Brush,
    Eraser,
    FloodFill,
    FillAll,
    Eyedropper,
}

impl PaintTool {
    pub const ALL: [PaintTool; 5] = [
        PaintTool::Brush,
        PaintTool::Eraser,
        PaintTool::FloodFill,
        PaintTool::FillAll,
        PaintTool::Eyedropper,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PaintTool::Brush => "Brush",
            PaintTool::Eraser => "Eraser",
            PaintTool::FloodFill => "Bucket Fill",
            PaintTool::FillAll => "Fill All",
            PaintTool::Eyedropper => "Eyedropper",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushTip {
    Soft,
    Hard,
    Square,
    Splatter,
}

impl BrushTip {
    pub const ALL: [BrushTip; 4] = [
        BrushTip::Soft,
        BrushTip::Hard,
        BrushTip::Square,
        BrushTip::Splatter,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BrushTip::Soft => "Soft",
            BrushTip::Hard => "Hard",
            BrushTip::Square => "Square",
            BrushTip::Splatter => "Splatter",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProceduralPattern {
    Checker,
    Camo,
    Stripes,
    Grunge,
    UvGrid,
}

impl ProceduralPattern {
    pub const ALL: [ProceduralPattern; 5] = [
        ProceduralPattern::Checker,
        ProceduralPattern::Camo,
        ProceduralPattern::Stripes,
        ProceduralPattern::Grunge,
        ProceduralPattern::UvGrid,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ProceduralPattern::Checker => "Checker",
            ProceduralPattern::Camo => "Camo",
            ProceduralPattern::Stripes => "Stripes",
            ProceduralPattern::Grunge => "Grunge",
            ProceduralPattern::UvGrid => "UV Grid",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BrushSettings {
    pub tool: PaintTool,
    pub tip: BrushTip,
    /// Brush color in sRGB `[0..1]` RGBA.
    pub color: [f32; 4],
    /// Secondary / eraser base color in sRGB `[0..1]` RGBA.
    pub erase_color: [f32; 4],
    /// World-space 3D brush radius.
    pub radius: f32,
    /// Brush opacity `[0.05..1.0]`.
    pub opacity: f32,
    /// Brush edge hardness `[0.0..1.0]`.
    pub hardness: f32,
    /// Mirror brush strokes across local X = 0.
    pub mirror_x: bool,
    /// Resolution used when auto-creating a paintable texture.
    pub new_tex_size: u32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            tool: PaintTool::Brush,
            tip: BrushTip::Soft,
            color: [0.15, 0.62, 1.0, 1.0],
            erase_color: [0.85, 0.86, 0.88, 1.0],
            radius: 0.14,
            opacity: 0.95,
            hardness: 0.55,
            mirror_x: false,
            new_tex_size: 1024,
        }
    }
}

impl BrushSettings {
    pub fn active_rgba8(&self) -> [u8; 4] {
        let c = match self.tool {
            PaintTool::Eraser => self.erase_color,
            _ => self.color,
        };
        [
            (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }
}

/// Paints a continuous 3D brushstroke segment from `from_local` to `to_local`
/// across the triangles of `part`, projecting in 3D space so strokes wrap
/// seamlessly across UV seams and curved edges.
#[allow(clippy::too_many_arguments)]
pub fn paint_stroke_3d(
    part: &MeshPart,
    tex: &mut TextureSlot,
    from_local: Vec3,
    to_local: Vec3,
    local_normal: Vec3,
    local_radius: f32,
    settings: &BrushSettings,
    stroke_seed: u64,
) -> bool {
    if !part.has_uvs() || tex.width == 0 || tex.height == 0 {
        return false;
    }
    let dist = from_local.distance(to_local);
    let step = (local_radius * 0.32).max(1e-4);
    let steps = ((dist / step).ceil() as usize).clamp(1, 24);
    let rgba = settings.active_rgba8();

    let mut painted = false;
    for s in 0..=steps {
        let t = if steps == 0 { 1.0 } else { s as f32 / steps as f32 };
        let center = from_local.lerp(to_local, t);
        let seed = stroke_seed.wrapping_mul(6364136223846793005).wrapping_add(s as u64);
        painted |= paint_dab_3d(part, tex, center, local_normal, local_radius, rgba, settings, seed);
        if settings.mirror_x {
            let mc = Vec3::new(-center.x, center.y, center.z);
            let mn = Vec3::new(-local_normal.x, local_normal.y, local_normal.z);
            painted |= paint_dab_3d(part, tex, mc, mn, local_radius, rgba, settings, seed ^ 0x9E3779B9);
        }
    }
    painted
}

#[allow(clippy::too_many_arguments)]
fn paint_dab_3d(
    part: &MeshPart,
    tex: &mut TextureSlot,
    center: Vec3,
    normal: Vec3,
    radius: f32,
    rgba: [u8; 4],
    settings: &BrushSettings,
    seed: u64,
) -> bool {
    let r = radius.max(1e-4);
    let r2 = r * r;
    let lo_b = center - Vec3::splat(r);
    let hi_b = center + Vec3::splat(r);
    let w = tex.width as i32;
    let h = tex.height as i32;
    let fw = tex.width as f32;
    let fh = tex.height as f32;

    // Build orthonormal tangent basis for Square / Splatter tips
    let n_ref = if normal.length_squared() > 1e-6 { normal.normalize() } else { Vec3::Y };
    let up = if n_ref.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let tangent_u = up.cross(n_ref).normalize_or_zero();
    let tangent_v = n_ref.cross(tangent_u).normalize_or_zero();

    // Pre-generate deterministic splatter sub-droplets if tip == Splatter
    let mut droplets: [(Vec3, f32); 9] = [(center, r * 0.55); 9];
    if settings.tip == BrushTip::Splatter {
        droplets[0] = (center, r * 0.45);
        for (k, slot) in droplets.iter_mut().enumerate().skip(1) {
            let h1 = hash_f32(seed.wrapping_add(k as u64 * 31));
            let h2 = hash_f32(seed.wrapping_add(k as u64 * 97 + 13));
            let h3 = hash_f32(seed.wrapping_add(k as u64 * 173 + 7));
            let angle = h1 * std::f32::consts::TAU;
            let dist = (0.25 + h2 * 0.72) * r;
            let dr = (0.10 + h3 * 0.26) * r;
            let pos = center + (tangent_u * angle.cos() + tangent_v * angle.sin()) * dist;
            *slot = (pos, dr);
        }
    }

    let pixels = tex.pixels_mut();
    let mut painted = false;
    let opacity = settings.opacity.clamp(0.01, 1.0);
    let hardness = settings.hardness.clamp(0.0, 0.99);

    for tri in part.indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if i0 >= part.positions.len() || i1 >= part.positions.len() || i2 >= part.positions.len() {
            continue;
        }
        let p0 = Vec3::from_array(part.positions[i0]);
        let p1 = Vec3::from_array(part.positions[i1]);
        let p2 = Vec3::from_array(part.positions[i2]);

        // Quick 3D AABB reject
        let t_lo = p0.min(p1).min(p2);
        let t_hi = p0.max(p1).max(p2);
        if t_hi.x < lo_b.x || t_lo.x > hi_b.x
            || t_hi.y < lo_b.y || t_lo.y > hi_b.y
            || t_hi.z < lo_b.z || t_lo.z > hi_b.z
        {
            continue;
        }
        // Prevent bleeding onto back-facing opposite sides of thin geometry
        let fn_ = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        if fn_ != Vec3::ZERO && fn_.dot(n_ref) < -0.25 {
            continue;
        }

        let uv0 = Vec2::from_array(part.uvs[i0]);
        let uv1 = Vec2::from_array(part.uvs[i1]);
        let uv2 = Vec2::from_array(part.uvs[i2]);

        let uv_lo = uv0.min(uv1).min(uv2);
        let uv_hi = uv0.max(uv1).max(uv2);

        // Pixel bounds with a 1-texel seam dilation margin
        let x0 = (uv_lo.x * fw).floor() as i32 - 1;
        let x1 = (uv_hi.x * fw).ceil() as i32 + 1;
        let y0 = (uv_lo.y * fh).floor() as i32 - 1;
        let y1 = (uv_hi.y * fh).ceil() as i32 + 1;

        // Guard against degenerate giant wrapped UVs
        if (x1 - x0) > w * 2 || (y1 - y0) > h * 2 {
            continue;
        }

        let v0 = uv1 - uv0;
        let v1 = uv2 - uv0;
        let denom = v0.x * v1.y - v1.x * v0.y;
        if denom.abs() < 1e-10 {
            continue;
        }
        let inv_denom = 1.0 / denom;
        // Edge dilation tolerance in barycentric space (~1.5 texels)
        let edge_pad = (1.5 / fw.min(fh)).max(0.002) * (1.0 / denom.abs().sqrt().max(0.05)).min(0.15);

        for py in y0..=y1 {
            let vy = (py as f32 + 0.5) / fh;
            let wy = py.rem_euclid(h) as usize;
            for px in x0..=x1 {
                let vx = (px as f32 + 0.5) / fw;
                let v2 = Vec2::new(vx, vy) - uv0;
                let b1 = (v2.x * v1.y - v1.x * v2.y) * inv_denom;
                let b2 = (v0.x * v2.y - v2.x * v0.y) * inv_denom;
                let b0 = 1.0 - b1 - b2;

                if b0 < -edge_pad || b1 < -edge_pad || b2 < -edge_pad {
                    continue;
                }
                // Clamp slightly outside texels onto the triangle edge for clean seam dilation
                let (c0, c1, c2) = if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                    let s0 = b0.max(0.0);
                    let s1 = b1.max(0.0);
                    let s2 = b2.max(0.0);
                    let sum = (s0 + s1 + s2).max(1e-6);
                    (s0 / sum, s1 / sum, s2 / sum)
                } else {
                    (b0, b1, b2)
                };

                let world_p = p0 * c0 + p1 * c1 + p2 * c2;
                let falloff = match settings.tip {
                    BrushTip::Soft => {
                        let d2 = world_p.distance_squared(center);
                        if d2 > r2 {
                            continue;
                        }
                        let norm_d = d2.sqrt() / r;
                        if norm_d <= hardness {
                            1.0
                        } else {
                            let t = 1.0 - ((norm_d - hardness) / (1.0 - hardness)).clamp(0.0, 1.0);
                            t * t * (3.0 - 2.0 * t)
                        }
                    }
                    BrushTip::Hard => {
                        let d2 = world_p.distance_squared(center);
                        if d2 > r2 {
                            continue;
                        }
                        let norm_d = d2.sqrt() / r;
                        if norm_d < 0.88 { 1.0 } else { (1.0 - (norm_d - 0.88) / 0.12).clamp(0.0, 1.0) }
                    }
                    BrushTip::Square => {
                        let rel = world_p - center;
                        let du = rel.dot(tangent_u).abs();
                        let dv = rel.dot(tangent_v).abs();
                        let dn = rel.dot(n_ref).abs();
                        if du > r || dv > r || dn > r {
                            continue;
                        }
                        let m = du.max(dv) / r;
                        if m <= hardness { 1.0 } else { 1.0 - ((m - hardness) / (1.0 - hardness)).clamp(0.0, 1.0) }
                    }
                    BrushTip::Splatter => {
                        let mut best = 0.0f32;
                        for &(dc, dr) in &droplets {
                            let d2 = world_p.distance_squared(dc);
                            if d2 < dr * dr {
                                let nd = d2.sqrt() / dr;
                                let f = if nd < 0.75 { 1.0 } else { 1.0 - (nd - 0.75) / 0.25 };
                                best = best.max(f);
                            }
                        }
                        if best <= 0.0 {
                            continue;
                        }
                        best
                    }
                };

                let alpha = (opacity * falloff).clamp(0.0, 1.0);
                if alpha <= 1e-4 {
                    continue;
                }
                let wx = px.rem_euclid(w) as usize;
                let idx = (wy * (w as usize) + wx) * 4;
                blend_pixel(&mut pixels[idx..idx + 4], rgba, alpha);
                painted = true;
            }
        }
    }

    painted
}

/// Paints directly onto a 2D UV coordinate `(u, v)` in `[0..1]`, used by the
/// 2D Texture Canvas preview in the Paint panel!
pub fn paint_dab_2d(tex: &mut TextureSlot, uv: Vec2, pixel_radius: f32, settings: &BrushSettings) -> bool {
    if tex.width == 0 || tex.height == 0 {
        return false;
    }
    let w = tex.width as i32;
    let h = tex.height as i32;
    let cx = uv.x.clamp(0.0, 1.0) * tex.width as f32;
    let cy = uv.y.clamp(0.0, 1.0) * tex.height as f32;
    let r = pixel_radius.max(1.5);
    let r2 = r * r;
    let rgba = settings.active_rgba8();
    let opacity = settings.opacity.clamp(0.01, 1.0);
    let hardness = settings.hardness.clamp(0.0, 0.99);

    let x0 = (cx - r).floor().max(0.0) as i32;
    let x1 = (cx + r).ceil().min((w - 1) as f32) as i32;
    let y0 = (cy - r).floor().max(0.0) as i32;
    let y1 = (cy + r).ceil().min((h - 1) as f32) as i32;

    let pixels = tex.pixels_mut();
    let mut painted = false;
    for y in y0..=y1 {
        let dy = y as f32 + 0.5 - cy;
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let falloff = match settings.tip {
                BrushTip::Square => {
                    let m = dx.abs().max(dy.abs()) / r;
                    if m > 1.0 { continue; }
                    if m <= hardness { 1.0 } else { 1.0 - (m - hardness) / (1.0 - hardness) }
                }
                BrushTip::Hard => {
                    let d2 = dx * dx + dy * dy;
                    if d2 > r2 { continue; }
                    1.0
                }
                _ => {
                    let d2 = dx * dx + dy * dy;
                    if d2 > r2 { continue; }
                    let nd = d2.sqrt() / r;
                    if nd <= hardness { 1.0 } else {
                        let t = 1.0 - (nd - hardness) / (1.0 - hardness);
                        t * t * (3.0 - 2.0 * t)
                    }
                }
            };
            let idx = ((y as usize) * (w as usize) + (x as usize)) * 4;
            blend_pixel(&mut pixels[idx..idx + 4], rgba, (opacity * falloff).clamp(0.0, 1.0));
            painted = true;
        }
    }
    painted
}

/// Fills the entire texture with `rgba`.
pub fn fill_texture(tex: &mut TextureSlot, rgba: [u8; 4]) {
    let pixels = tex.pixels_mut();
    for px in pixels.chunks_exact_mut(4) {
        px.copy_from_slice(&rgba);
    }
}

/// Contiguous BFS flood fill in UV texture space starting at `uv`.
pub fn flood_fill_uv(tex: &mut TextureSlot, uv: [f32; 2], fill_rgba: [u8; 4], tolerance: u8) -> bool {
    if tex.width == 0 || tex.height == 0 {
        return false;
    }
    let w = tex.width as usize;
    let h = tex.height as usize;
    let sx = ((uv[0].rem_euclid(1.0) * w as f32).floor() as usize).min(w - 1);
    let sy = ((uv[1].rem_euclid(1.0) * h as f32).floor() as usize).min(h - 1);
    let target = tex.get_pixel(sx as u32, sy as u32);
    if color_within(target, fill_rgba, 2) {
        return false;
    }

    let pixels = tex.pixels_mut();
    let mut visited = vec![false; w * h];
    let mut queue = VecDeque::new();
    visited[sy * w + sx] = true;
    queue.push_back((sx, sy));

    while let Some((x, y)) = queue.pop_front() {
        let idx = (y * w + x) * 4;
        pixels[idx..idx + 4].copy_from_slice(&fill_rgba);

        let neighbors = [
            (x.wrapping_sub(1), y, x > 0),
            (x + 1, y, x + 1 < w),
            (x, y.wrapping_sub(1), y > 0),
            (x, y + 1, y + 1 < h),
        ];
        for (nx, ny, ok) in neighbors {
            if !ok {
                continue;
            }
            let flat = ny * w + nx;
            if visited[flat] {
                continue;
            }
            let ni = flat * 4;
            let px = [pixels[ni], pixels[ni + 1], pixels[ni + 2], pixels[ni + 3]];
            if color_within(px, target, tolerance) {
                visited[flat] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    true
}

/// Generates a procedural texture pattern using the primary & secondary colors.
pub fn generate_procedural(
    tex: &mut TextureSlot,
    pattern: ProceduralPattern,
    primary: [u8; 4],
    secondary: [u8; 4],
) {
    let w = tex.width as usize;
    let h = tex.height as usize;
    if w == 0 || h == 0 {
        return;
    }
    let pixels = tex.pixels_mut();
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let c = match pattern {
                ProceduralPattern::Checker => {
                    let cx = (u * 16.0).floor() as i32;
                    let cy = (v * 16.0).floor() as i32;
                    if (cx + cy) % 2 == 0 { primary } else { secondary }
                }
                ProceduralPattern::Stripes => {
                    let s = ((u + v) * 14.0).floor() as i32;
                    if s % 2 == 0 { primary } else { secondary }
                }
                ProceduralPattern::Camo => {
                    let n1 = fbm(u * 5.0, v * 5.0);
                    let n2 = fbm(u * 9.0 + 13.7, v * 9.0 + 4.3);
                    let dark = [
                        (primary[0] as f32 * 0.45) as u8,
                        (primary[1] as f32 * 0.45) as u8,
                        (primary[2] as f32 * 0.45) as u8,
                        255,
                    ];
                    if n1 > 0.58 {
                        primary
                    } else if n2 > 0.52 {
                        secondary
                    } else {
                        dark
                    }
                }
                ProceduralPattern::Grunge => {
                    let n = fbm(u * 12.0, v * 12.0);
                    lerp_rgba(secondary, primary, n)
                }
                ProceduralPattern::UvGrid => {
                    let gx = (u * 16.0).fract();
                    let gy = (v * 16.0).fract();
                    let border = gx < 0.06 || gy < 0.06;
                    if border {
                        [245, 245, 250, 255]
                    } else {
                        let r = (u * 230.0 + 25.0) as u8;
                        let g = ((1.0 - v) * 210.0 + 30.0) as u8;
                        let b = (((u + v) * 0.5) * 200.0 + 40.0) as u8;
                        [r, g, b, 255]
                    }
                }
            };
            let idx = (y * w + x) * 4;
            pixels[idx..idx + 4].copy_from_slice(&c);
        }
    }
}

fn blend_pixel(dst: &mut [u8], src: [u8; 4], alpha: f32) {
    let a = alpha * (src[3] as f32 / 255.0);
    let inv = 1.0 - a;
    dst[0] = (src[0] as f32 * a + dst[0] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[1] = (src[1] as f32 * a + dst[1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[2] = (src[2] as f32 * a + dst[2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[3] = ((a + (dst[3] as f32 / 255.0) * inv) * 255.0).round().clamp(0.0, 255.0) as u8;
}

fn color_within(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a[0].abs_diff(b[0]) <= tol
        && a[1].abs_diff(b[1]) <= tol
        && a[2].abs_diff(b[2]) <= tol
        && a[3].abs_diff(b[3]) <= tol
}

fn lerp_rgba(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round() as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round() as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round() as u8,
        255,
    ]
}

fn hash_f32(mut x: u64) -> f32 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    (x & 0x00FF_FFFF) as f32 / 16_777_216.0
}

fn value_noise(x: f32, y: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let h = |gx: i32, gy: i32| {
        let k = (gx as u32 as u64).wrapping_mul(73856093) ^ (gy as u32 as u64).wrapping_mul(19349663);
        hash_f32(k)
    };
    let a = h(ix, iy);
    let b = h(ix + 1, iy);
    let c = h(ix, iy + 1);
    let d = h(ix + 1, iy + 1);
    (a + (b - a) * ux) + ((c + (d - c) * ux) - (a + (b - a) * ux)) * uy
}

fn fbm(x: f32, y: f32) -> f32 {
    let mut v = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    for _ in 0..4 {
        v += amp * value_noise(x * freq, y * freq);
        freq *= 2.03;
        amp *= 0.5;
    }
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelDoc;

    #[test]
    fn painting_in_3d_modifies_hit_texels_and_leaves_opposite_side_alone() {
        let mut doc = ModelDoc::starter_cube();
        doc.materials[0].set_base_color_srgb([1.0, 1.0, 1.0, 1.0]);
        let (ti, _, _) = doc.ensure_paintable_texture(0, 128).unwrap();

        let hit = doc.raycast(Vec3::new(0.0, 0.5, 3.0), Vec3::NEG_Z).unwrap().1;
        let mut brush = BrushSettings::default();
        brush.color = [1.0, 0.0, 0.0, 1.0];
        brush.opacity = 1.0;
        brush.radius = 0.25;
        brush.tip = BrushTip::Hard;

        let painted = paint_stroke_3d(
            &doc.parts[0],
            &mut doc.textures[ti],
            hit.local_pos,
            hit.local_pos,
            hit.local_normal,
            brush.radius,
            &brush,
            1,
        );
        assert!(painted);
        let front_px = doc.textures[ti].sample_uv(hit.uv.unwrap());
        assert!(front_px[0] > 200 && front_px[1] < 50, "front pixel should be red: {front_px:?}");

        // Opposite back face (-Z) should still be untouched white!
        let back_hit = doc.raycast(Vec3::new(0.0, 0.5, -3.0), Vec3::Z).unwrap().1;
        let back_px = doc.textures[ti].sample_uv(back_hit.uv.unwrap());
        assert_eq!(back_px, [255, 255, 255, 255], "back face must not get painted!");
    }
}
