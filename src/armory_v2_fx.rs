//! Armory v2 FX: rolling paintballs inside hoppers + sloshing water in the water guns.
//!
//! From the LowPoly Armory V2 pack (`game-assets/src/armory_v2_fx.rs`), ported to Bevy 0.20.
//! Gun parts are spawned as usual (`spawn_gun_parts`); this plugin finds their nodes by name.
//!
//! What happens automatically:
//! * Every node named `hopper_volume_<shape>` gets a [`Hopper`] component. Its sibling nodes named
//!   `paintball_00`, `paintball_01`, ... become verlet-simulated balls that fall, roll, bump into each
//!   other and react to the gun moving / tilting (gravity + the gun's own acceleration).
//! * Every node named `water` (water guns v2) gets a [`WaterSlosh`] component. A damped spring drives the
//!   4 morph targets (roll+/roll-/pitch+/pitch-) so the surface stays level as the gun tilts and
//!   sloshes when it moves. (Don't also play the baked "Slosh"/"Idle" clips on the same model.)
//!
//! Firing: `Hopper::set_fill` shows that many balls (combat keeps the first-person hopper in step
//! with the ammo count); balls beyond `visible_balls` are hidden.

use bevy::gltf::GltfExtras;
use bevy::mesh::morph::MorphWeights;
use bevy::prelude::*;

pub struct ArmoryV2FxPlugin;

impl Plugin for ArmoryV2FxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (setup_hoppers, setup_water, simulate_hoppers, simulate_water).chain(),
        );
    }
}

// ---------------------------------------------------------------- tiny extras parser
fn extra_num(json: &str, key: &str) -> Option<f32> {
    let k = format!("\"{key}\":");
    let i = json.find(&k)? + k.len();
    let rest = &json[i..];
    let end = rest.find([',', '}', ']'])?;
    rest[..end].trim().parse().ok()
}
fn extra_vec3(json: &str, key: &str) -> Option<Vec3> {
    let k = format!("\"{key}\":[");
    let i = json.find(&k)? + k.len();
    let rest = &json[i..];
    let v: Vec<f32> = rest[..rest.find(']')?]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
}

/// The gun's effective gravity in some local frame: real gravity minus the frame's own acceleration.
#[derive(Default)]
struct MotionTracker {
    last_pos: Option<Vec3>,
    last_vel: Vec3,
}
impl MotionTracker {
    fn effective_gravity_world(&mut self, pos: Vec3, dt: f32) -> Vec3 {
        let vel = match self.last_pos {
            Some(p) => (pos - p) / dt,
            None => Vec3::ZERO,
        };
        let acc = ((vel - self.last_vel) / dt).clamp_length_max(60.0);
        self.last_pos = Some(pos);
        self.last_vel = vel;
        Vec3::new(0.0, -9.81, 0.0) - acc
    }
}

// ---------------------------------------------------------------- hoppers
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HopperShape {
    Box,
    Ellipsoid,
    CylinderZ,
    ConeY,
}

#[derive(Component)]
pub struct Hopper {
    pub shape: HopperShape,
    pub half: Vec3,
    pub radius: f32,
    pub radius_bottom: f32,
    pub radius_top: f32,
    pub ball_radius: f32,
    /// balls with index >= visible_balls are hidden (fired)
    pub visible_balls: usize,
    balls: Vec<Entity>,
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    local: Transform, // the volume node's transform in the gun frame (shared parent with the balls)
    motion: MotionTracker,
}

impl Hopper {
    pub fn ball_count(&self) -> usize {
        self.balls.len()
    }
    /// Show `n` balls (clamped to the hopper's count). Balls coming back
    /// (a reload) drop in from the top; fewer hides the extra ones.
    pub fn set_fill(&mut self, n: usize) {
        let n = n.min(self.balls.len());
        if n > self.visible_balls {
            let top = self.half.y - self.ball_radius;
            for i in self.visible_balls..n {
                let p = Vec3::new(
                    self.pos[i].x * 0.5,
                    top - (i - self.visible_balls) as f32 * 0.002,
                    self.pos[i].z * 0.5,
                );
                self.pos[i] = p;
                self.prev[i] = p;
            }
        }
        self.visible_balls = n;
    }

    fn contain(&self, p: Vec3) -> Vec3 {
        let r = self.ball_radius;
        match self.shape {
            HopperShape::Box => p.clamp(-(self.half - r), self.half - r),
            HopperShape::Ellipsoid => {
                let e = (self.half - r).max(Vec3::splat(1e-4));
                let k = (p / e).length();
                if k > 1.0 { p / k } else { p }
            }
            HopperShape::CylinderZ => {
                let rr = (self.radius - r).max(1e-4);
                let xy = Vec2::new(p.x, p.y).clamp_length_max(rr);
                Vec3::new(xy.x, xy.y, p.z.clamp(-(self.half.z - r), self.half.z - r))
            }
            HopperShape::ConeY => {
                let h = self.half.y;
                let y = p.y.clamp(-(h - r), h - r);
                let rr = (self.radius_bottom
                    + (self.radius_top - self.radius_bottom) * (y + h) / (2.0 * h)
                    - r * 1.15)
                    .max(1e-4);
                let xz = Vec2::new(p.x, p.z).clamp_length_max(rr);
                Vec3::new(xz.x, y, xz.y)
            }
        }
    }
}

fn setup_hoppers(
    mut commands: Commands,
    added: Query<(Entity, &Name, &GltfExtras, &Transform, &ChildOf), Added<GltfExtras>>,
    children: Query<&Children>,
    balls: Query<(&Name, &Transform)>,
) {
    for (entity, name, extras, tf, child_of) in &added {
        if !name.as_str().starts_with("hopper_volume_") {
            continue;
        }
        let j = extras.value.as_str();
        let shape = if j.contains("\"cone_y\"") {
            HopperShape::ConeY
        } else if j.contains("\"cyl_z\"") {
            HopperShape::CylinderZ
        } else if j.contains("\"ellipsoid\"") {
            HopperShape::Ellipsoid
        } else {
            HopperShape::Box
        };
        let half = extra_vec3(j, "half_extents").unwrap_or(tf.scale);
        let local = Transform::from_translation(tf.translation).with_rotation(tf.rotation);
        let inv = local.rotation.inverse();
        let mut list: Vec<(String, Entity, Vec3)> = Vec::new();
        if let Ok(kids) = children.get(child_of.parent()) {
            for kid in kids.iter() {
                if let Ok((n, t)) = balls.get(kid)
                    && n.as_str().starts_with("paintball_")
                {
                    list.push((
                        n.as_str().to_string(),
                        kid,
                        inv * (t.translation - local.translation),
                    ));
                }
            }
        }
        list.sort_by(|a, b| a.0.cmp(&b.0));
        let pos: Vec<Vec3> = list.iter().map(|b| b.2).collect();
        commands.entity(entity).insert(Hopper {
            shape,
            half,
            radius: extra_num(j, "radius").unwrap_or(half.x),
            radius_bottom: extra_num(j, "radius_bottom").unwrap_or(half.x),
            radius_top: extra_num(j, "radius_top").unwrap_or(half.x),
            ball_radius: extra_num(j, "ball_radius").unwrap_or(0.0086),
            visible_balls: list.len(),
            balls: list.iter().map(|b| b.1).collect(),
            prev: pos.clone(),
            pos,
            local,
            motion: MotionTracker::default(),
        });
    }
}

fn simulate_hoppers(
    time: Res<Time>,
    mut hoppers: Query<(&mut Hopper, &GlobalTransform)>,
    mut balls: Query<(&mut Transform, &mut Visibility), Without<Hopper>>,
) {
    let dt = time.delta_secs().clamp(1.0 / 240.0, 1.0 / 30.0);
    for (mut h, gt) in &mut hoppers {
        let (_, rot, pos) = gt.to_scale_rotation_translation();
        let g_local = rot.inverse() * h.motion.effective_gravity_world(pos, dt);
        let n = h.visible_balls.min(h.pos.len());
        let r2 = h.ball_radius * 2.0;
        const SUB: usize = 2;
        let sdt = dt / SUB as f32;
        for _ in 0..SUB {
            for i in 0..n {
                let v = (h.pos[i] - h.prev[i]) * 0.992;
                h.prev[i] = h.pos[i];
                h.pos[i] += v + g_local * sdt * sdt;
            }
            for _ in 0..4 {
                for i in 0..n {
                    for k in (i + 1)..n {
                        let d = h.pos[k] - h.pos[i];
                        let l = d.length();
                        if l < r2 && l > 1e-6 {
                            let push = d * (0.5 * (r2 - l) / l);
                            h.pos[i] -= push;
                            h.pos[k] += push;
                        }
                    }
                }
                for i in 0..n {
                    let c = h.contain(h.pos[i]);
                    h.pos[i] = c;
                }
            }
        }
        for (i, &e) in h.balls.iter().enumerate() {
            if let Ok((mut t, mut vis)) = balls.get_mut(e) {
                if i < n {
                    t.translation = h.local.translation + h.local.rotation * h.pos[i];
                    *vis = Visibility::Inherited;
                } else {
                    *vis = Visibility::Hidden;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- water
#[derive(Component)]
pub struct WaterSlosh {
    pub roll_slope: f32,
    pub pitch_slope: f32,
    /// spring stiffness (higher = faster slosh) and damping ratio
    pub stiffness: f32,
    pub damping: f32,
    roll: f32,
    pitch: f32,
    v_roll: f32,
    v_pitch: f32,
    motion: MotionTracker,
}

fn setup_water(
    mut commands: Commands,
    added: Query<(Entity, &Name, &GltfExtras), Added<GltfExtras>>,
) {
    for (entity, name, extras) in &added {
        if name.as_str() != "water" {
            continue;
        }
        let j = extras.value.as_str();
        commands.entity(entity).insert(WaterSlosh {
            roll_slope: extra_num(j, "roll_slope").unwrap_or(0.5),
            pitch_slope: extra_num(j, "pitch_slope").unwrap_or(0.3),
            stiffness: 55.0,
            damping: 0.12,
            roll: 0.0,
            pitch: 0.0,
            v_roll: 0.0,
            v_pitch: 0.0,
            motion: MotionTracker::default(),
        });
    }
}

fn simulate_water(
    time: Res<Time>,
    mut q: Query<(&mut WaterSlosh, &GlobalTransform, &mut MorphWeights)>,
) {
    let dt = time.delta_secs().clamp(1.0 / 240.0, 1.0 / 30.0);
    for (mut w, gt, mut weights) in &mut q {
        let (_, rot, pos) = gt.to_scale_rotation_translation();
        let g = rot.inverse() * w.motion.effective_gravity_world(pos, dt);
        // the free surface is perpendicular to effective gravity; u = its "up" normal in the gun's frame.
        // surface height in the gun frame: y = c - (u.x/u.y)·x - (u.z/u.y)·z
        let u = (-g).try_normalize().unwrap_or(Vec3::Y);
        let uy = u.y.max(0.25); // gun upside-down: just pin the surface at max tilt
        let target_roll = ((-u.z / uy) / w.roll_slope).clamp(-1.0, 1.0);
        let target_pitch = ((-u.x / uy) / w.pitch_slope).clamp(-1.0, 1.0);
        let k = w.stiffness;
        let c = 2.0 * w.damping * k.sqrt();
        let a_r = k * (target_roll - w.roll) - c * w.v_roll;
        let a_p = k * (target_pitch - w.pitch) - c * w.v_pitch;
        w.v_roll += a_r * dt;
        w.v_pitch += a_p * dt;
        w.roll = (w.roll + w.v_roll * dt).clamp(-1.2, 1.2);
        w.pitch = (w.pitch + w.v_pitch * dt).clamp(-1.2, 1.2);
        let ws = weights.weights_mut();
        if ws.len() >= 4 {
            ws[0] = w.roll.clamp(0.0, 1.0);
            ws[1] = (-w.roll).clamp(0.0, 1.0);
            ws[2] = w.pitch.clamp(0.0, 1.0);
            ws[3] = (-w.pitch).clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A gravity-hopper-sized cone with balls stacked in a column: after a
    /// second of simulation they have fallen, stay inside and don't overlap.
    #[test]
    fn hopper_balls_settle_inside() {
        let mut world = World::new();
        world.init_resource::<Time>();
        let half = Vec3::new(0.076, 0.03, 0.076);
        let start: Vec<Vec3> = (0..12)
            .map(|i| {
                Vec3::new(
                    (i % 3) as f32 * 0.01 - 0.01,
                    0.02,
                    (i / 3) as f32 * 0.01 - 0.015,
                )
            })
            .collect();
        let balls: Vec<Entity> = start
            .iter()
            .map(|_| {
                world
                    .spawn((Transform::default(), Visibility::default()))
                    .id()
            })
            .collect();
        let mut hopper = Hopper {
            shape: HopperShape::ConeY,
            half,
            radius: half.x,
            radius_bottom: 0.033,
            radius_top: 0.076,
            ball_radius: 0.0086,
            visible_balls: balls.len(),
            balls: balls.clone(),
            prev: start.clone(),
            pos: start,
            local: Transform::default(),
            motion: MotionTracker::default(),
        };
        hopper.set_fill(10);
        world.spawn((hopper, GlobalTransform::default()));
        for _ in 0..240 {
            world.run_system_once(simulate_hoppers).unwrap();
        }
        let mut q = world.query::<&Hopper>();
        let h = q.single(&world).unwrap();
        for i in 0..h.visible_balls {
            let p = h.pos[i];
            assert!(p.y < 0.0, "ball {i} did not fall: {p}");
            assert!(h.contain(p).distance(p) < 1e-4, "ball {i} escaped: {p}");
            for k in (i + 1)..h.visible_balls {
                assert!(p.distance(h.pos[k]) > 2.0 * h.ball_radius * 0.9);
            }
        }
        // The two fired balls are hidden.
        assert_eq!(
            world.get::<Visibility>(balls[11]),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            world.get::<Visibility>(balls[0]),
            Some(&Visibility::Inherited)
        );
    }
}
