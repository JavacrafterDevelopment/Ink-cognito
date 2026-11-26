//! Procedural legs and hips for the rigged player models.
//!
//! The pack's clips still drive the upper body (spine sway, reload, hit react,
//! victory), but the legs and hips are worked out every frame from how the
//! model actually moves through the world:
//!
//! - **Stride planning.** A gait phase advances with speed. Each foot is
//!   either planted (locked in the world, so it never slides) or swinging
//!   toward where the body will be at its next mid-stance. Faster means longer
//!   strides, a higher cadence and less time on the ground, until both feet
//!   leave it (running). Strafing and backpedalling fall out of the same maths
//!   because targets follow the real velocity.
//! - **Standing still.** Feet stay put until the body turns or drifts too far
//!   from them, then step back under the hips one at a time.
//! - **Foot IK.** Each leg is a two-bone chain (thigh 0.42 m, shin 0.43 m)
//!   reaching a foot planted on whatever is below it: bunker tops, ramps, the
//!   field. The hips drop when a foot would be out of reach.
//! - **Body.** The hips bob (high mid-stance when walking, low mid-stance when
//!   running), sway over the planted foot, twist with the stride and lean into
//!   speed and acceleration. Crouching lowers the hips and the knees follow.
//! - **Jumps.** A push off on takeoff, legs tucked on the way up, reaching
//!   for the ground on the way down, and a landing dip that grows with how
//!   hard they came down.
//!
//! Nothing here is networked: remote players are animated from their smoothed
//! position and `FLAG_*` bits, which every client already has. Every foot
//! landing is sent as a [`FootPlant`] message, which `combat.rs` turns into
//! the footstep sound, so steps are heard exactly when they are seen.

use crate::{
    arena::Colliders,
    player_model::{PlayerPose, Rig},
    protocol::{FLAG_AIRBORNE, FLAG_CROUCH},
};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};

/// Thigh length (UpLeg → Leg) in the rig.
const THIGH: f32 = 0.42;
/// Shin length (Leg → Foot) in the rig.
const SHIN: f32 = 0.43;
/// The hips bone's rest height above the feet.
const HIPS_REST: Vec3 = Vec3::new(0.0, 0.98, 0.0);
/// Where each thigh starts, in the hips bone's space (left is +X).
const HIP_JOINT: [Vec3; 2] = [Vec3::new(0.1, -0.04, 0.0), Vec3::new(-0.1, -0.04, 0.0)];
/// The ankle bone's height above the sole.
const ANKLE_UP: f32 = 0.09;
/// How far each foot sits out from the body's centre line.
const STANCE_WIDTH: f32 = 0.11;
/// Below this speed (m/s) the gait stops and the feet just settle.
const IDLE_SPEED: f32 = 0.45;
/// The longest a planted foot may trail or lead the body before it has to step.
const MAX_REACH: f32 = 0.62;

/// A foot came down: on a step, or both feet on landing a jump.
#[derive(Message, Debug, Clone, Copy)]
pub struct FootPlant {
    /// The player model (the entity carrying the `Rig`).
    pub model: Entity,
    /// Where the sole touched, in world space.
    pub pos: Vec3,
    /// True for the landing of a jump or fall (both feet at once).
    pub landing: bool,
}

#[derive(Clone, Copy, Default)]
enum FootState {
    #[default]
    Planted,
    /// Moving to a new plant. `t` runs 0 → 1 over `dur` seconds.
    Swing {
        from: Vec3,
        from_yaw: f32,
        t: f32,
        dur: f32,
    },
    /// In the air, posed in the model's own space.
    Air,
}

#[derive(Clone, Copy, Default)]
struct Foot {
    state: FootState,
    /// The sole, world space (planted and swinging).
    pos: Vec3,
    /// World yaw the foot points along.
    yaw: f32,
    /// The ground's normal under the foot (world).
    normal: Vec3,
    /// Toe up (+) / down (-) while swinging, radians.
    toe_up: f32,
    /// The sole in model space while in the air.
    air: Vec3,
}

/// Procedural leg state for one rigged player model.
#[derive(Component, Default)]
pub struct Locomotion {
    ready: bool,
    /// The model root's world transform this frame (feet at its origin).
    world: Transform,
    last_pos: Vec3,
    /// Smoothed world velocity (horizontal) and vertical speed.
    vel: Vec3,
    vy: f32,
    /// Smoothed acceleration (horizontal, world).
    acc: Vec3,
    /// Gait cycle position 0..1. Foot 0 (left) is mid-stance at `duty / 2`.
    phase: f32,
    /// 0 standing still → 1 moving, eased.
    moving: f32,
    crouch: f32,
    airborne: bool,
    feet: [Foot; 2],
    /// Extra hips drop so a foot stays within reach (smoothed).
    reach_drop: f32,
    /// Landing dip spring.
    dip: f32,
    dip_vel: f32,
    /// Lean (x: forward pitch, y: sideways roll), smoothed.
    lean: Vec2,
    /// The hips' rotation added here, in model space, so the chest can undo
    /// it and keep aiming where the player looks.
    pub hips_rot: Quat,
    /// Whether the legs were posed this frame.
    pub active: bool,
}

pub struct LocomotionPlugin;

impl Plugin for LocomotionPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<FootPlant>();
    }
}

/// Before the legs are posed: where each model is this frame. Kept separate
/// from `drive_legs` because computing a fresh world transform reads every
/// `Transform` up the hierarchy while `drive_legs` writes the bones'.
pub fn sample_model_transforms(
    helper: TransformHelper,
    mut models: Query<(Entity, &mut Locomotion)>,
) {
    for (e, mut loco) in &mut models {
        if let Ok(g) = helper.compute_global_transform(e) {
            loco.world = g.compute_transform();
        }
    }
}

/// The ground under `p`, searched from a little above to a little below.
fn ground_at(colliders: Option<&Colliders>, p: Vec3, fallback: f32) -> (f32, Vec3) {
    if let Some(c) = colliders
        && let Some(hit) = c.segment_hit(p + Vec3::Y * 0.7, p - Vec3::Y * 0.9)
    {
        let n = if hit.normal.y > 0.5 {
            hit.normal
        } else {
            Vec3::Y
        };
        return (hit.point.y, n);
    }
    (fallback, Vec3::Y)
}

fn yaw_of(rot: Quat) -> f32 {
    let f = rot * Vec3::Z;
    f.x.atan2(f.z)
}

fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Gait numbers for a speed: (step length, cycle seconds, duty factor).
/// Duty is the share of the cycle each foot is on the ground: above 0.5 is a
/// walk (both feet down at times), below 0.5 a run (both feet up at times).
///
/// `side` is how much of the motion is sideways (0..1) and `back` how much
/// is backwards: legs can't reach as far that way, so strides get shorter
/// and quicker.
fn gait(speed: f32, crouch: f32, side: f32, back: f32) -> (f32, f32, f32) {
    let v = speed.max(IDLE_SPEED);
    let short = (1.0 - 0.3 * crouch) * (1.0 - 0.3 * side) * (1.0 - 0.15 * back);
    let step = (0.42 + 0.165 * v).clamp(0.45, 2.0) * short;
    let cycle = 2.0 * step / v;
    // How far a planted foot travels under the body, capped by leg reach.
    let stance = (1.25 * step).min((0.78 - 0.18 * crouch) * (1.0 - 0.4 * side));
    let duty = (stance / (2.0 * step)).clamp(0.2, 0.62);
    (step, cycle, duty)
}

/// Two-bone IK: the knee (or elbow) for a chain from `root` reaching `target`.
/// Returns the mid joint and the reachable end.
pub fn two_bone(root: Vec3, target: Vec3, pole: Vec3, a: f32, b: f32) -> (Vec3, Vec3) {
    let to = target - root;
    let d = to.length().clamp((a - b).abs() + 0.01, a + b - 0.002);
    let dir = to.normalize_or(Vec3::NEG_Y);
    let end = root + dir * d;
    let cos_a = ((a * a + d * d - b * b) / (2.0 * a * d)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let perp = (pole - dir * pole.dot(dir)).normalize_or(Vec3::Z);
    (root + dir * a * cos_a + perp * a * sin_a, end)
}

/// A bone rotation whose local -Y runs along `along` and whose local +Z points
/// as close to `fwd` as it can.
fn bone_basis(along: Vec3, fwd: Vec3) -> Quat {
    let y = (-along).normalize_or(Vec3::Y);
    let z = (fwd - y * fwd.dot(y)).normalize_or(Vec3::Z);
    let x = y.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// Simulates every model's legs and writes the leg and hips bones (after the
/// clips, before the upper body is posed and transforms propagate).
pub fn drive_legs(
    time: Res<Time>,
    colliders: Option<Res<Colliders>>,
    mut models: Query<(Entity, &Rig, &PlayerPose, &mut Locomotion)>,
    mut transforms: Query<&mut Transform, Without<Locomotion>>,
    mut plants: MessageWriter<FootPlant>,
) {
    let dt = time.delta_secs().min(0.05);
    if dt <= 0.0 {
        return;
    }
    let colliders = colliders.as_deref();
    for (model, rig, pose, mut loco) in &mut models {
        let loco = &mut *loco;
        // Celebrating or eliminated: the clips (or the ragdoll) have the body.
        if pose.victory || pose.health <= 0.0 {
            loco.ready = false;
            loco.active = false;
            loco.hips_rot = Quat::IDENTITY;
            continue;
        }
        let world = loco.world;
        let root = world.translation;
        let body_yaw = yaw_of(world.rotation);

        // Home spot for each foot: under its hip, on the ground.
        let home = |i: usize, ahead: Vec3| {
            let side = if i == 0 { 1.0 } else { -1.0 };
            let p = root + world.rotation * Vec3::new(side * STANCE_WIDTH, 0.0, 0.0) + ahead;
            let (y, n) = ground_at(colliders, p, root.y);
            (p.with_y(y), n)
        };

        // A fresh (or teleported) body: stand with both feet under it, silently.
        if !loco.ready || root.distance(loco.last_pos) > 3.0 {
            for i in 0..2 {
                let (p, n) = home(i, Vec3::ZERO);
                loco.feet[i] = Foot {
                    state: FootState::Planted,
                    pos: p,
                    yaw: body_yaw,
                    normal: n,
                    ..default()
                };
            }
            loco.last_pos = root;
            loco.vel = Vec3::ZERO;
            loco.vy = 0.0;
            loco.acc = Vec3::ZERO;
            loco.airborne = false;
            loco.ready = true;
        }

        // How the body moves.
        let raw = (root - loco.last_pos) / dt;
        loco.last_pos = root;
        let k_vel = 1.0 - (-20.0 * dt).exp();
        let prev_vel = loco.vel;
        loco.vel += (raw.with_y(0.0) - loco.vel) * k_vel;
        loco.vy += (raw.y - loco.vy) * k_vel;
        let raw_acc = (loco.vel - prev_vel) / dt;
        loco.acc += (raw_acc.clamp_length_max(30.0) - loco.acc) * (1.0 - (-5.0 * dt).exp());
        let speed = loco.vel.length();
        let move_target = ((speed - IDLE_SPEED) / 1.2).clamp(0.0, 1.0);
        loco.moving += (move_target - loco.moving) * (1.0 - (-8.0 * dt).exp());
        let crouch_target = if pose.flags & FLAG_CROUCH != 0 {
            1.0
        } else {
            0.0
        };
        loco.crouch += (crouch_target - loco.crouch) * (1.0 - (-10.0 * dt).exp());
        let run = ((speed - 3.0) / 3.5).clamp(0.0, 1.0) * (1.0 - loco.crouch);

        let local_vel = world.rotation.inverse() * loco.vel;
        let local_dir = local_vel.normalize_or_zero();
        let (_, cycle, duty) = gait(
            speed,
            loco.crouch,
            local_dir.x.abs(),
            (-local_dir.z).max(0.0),
        );
        let stance_time = duty * cycle;
        let swing_time = ((1.0 - duty) * cycle).clamp(0.12, 0.6);

        let airborne = pose.flags & FLAG_AIRBORNE != 0;
        let inv = world.compute_affine().inverse();
        let to_model = |p: Vec3| inv.transform_point3(p);

        // Takeoff: the feet leave the ground and are posed in model space.
        if airborne && !loco.airborne {
            for f in &mut loco.feet {
                f.air = to_model(f.pos);
                f.state = FootState::Air;
            }
            loco.dip_vel += 0.5; // push off
        }
        // Landing: both feet plant where they are, and the body absorbs it.
        if !airborne && loco.airborne {
            let impact = (-loco.vy).clamp(0.0, 14.0);
            loco.dip_vel -= 0.08 + impact * 0.055;
            for i in 0..2 {
                let p = world.transform_point(loco.feet[i].air);
                let (y, n) = ground_at(colliders, p, root.y);
                let f = &mut loco.feet[i];
                f.pos = p.with_y(y);
                f.normal = n;
                f.yaw = body_yaw;
                f.state = FootState::Planted;
                f.toe_up = 0.0;
            }
            plants.write(FootPlant {
                model,
                pos: root,
                landing: true,
            });
        }
        loco.airborne = airborne;

        if airborne {
            // Up: knees tucked (one leg leading). Down: legs reach for the ground.
            let up = smoothstep((loco.vy + 1.5) / 5.0);
            let below = ground_at(colliders, root, root.y - 2.0).0 - root.y;
            let reach_y = (below + 0.02).clamp(-0.35, 0.05);
            let targets = [
                Vec3::new(0.12, reach_y, 0.10).lerp(Vec3::new(0.12, 0.34, 0.20), up),
                Vec3::new(-0.12, reach_y + 0.06, -0.06).lerp(Vec3::new(-0.12, 0.44, -0.16), up),
            ];
            let k = 1.0 - (-12.0 * dt).exp();
            for (f, t) in loco.feet.iter_mut().zip(targets) {
                f.air += (t - f.air) * k;
                f.toe_up += (-0.35 * up - f.toe_up) * k;
            }
        } else {
            // Advance the gait while moving; when still, the phase holds.
            let prev_phase = loco.phase;
            if speed > IDLE_SPEED {
                // Starting off: the foot furthest behind lifts first.
                let all_planted = loco
                    .feet
                    .iter()
                    .all(|f| matches!(f.state, FootState::Planted));
                if all_planted && loco.moving < 0.35 {
                    let dir = loco.vel.normalize_or_zero();
                    let behind = |f: &Foot| (f.pos - root).dot(dir);
                    let first = if behind(&loco.feet[0]) <= behind(&loco.feet[1]) {
                        0
                    } else {
                        1
                    };
                    // A first step is short and quick: start it part-way
                    // through its swing so the other foot isn't left behind.
                    let head_start = 0.4;
                    loco.phase =
                        (duty + head_start * (1.0 - duty) - 0.5 * first as f32).rem_euclid(1.0);
                    let f = loco.feet[first];
                    loco.feet[first].state = FootState::Swing {
                        from: f.pos,
                        from_yaw: f.yaw,
                        t: head_start,
                        dur: swing_time,
                    };
                }
                loco.phase = (loco.phase + dt / cycle).rem_euclid(1.0);
            }
            let phase = loco.phase;

            for i in 0..2 {
                let other_swinging = matches!(loco.feet[1 - i].state, FootState::Swing { .. });
                let off = 0.5 * i as f32;
                let p_prev = (prev_phase + off).rem_euclid(1.0);
                let p_now = (phase + off).rem_euclid(1.0);
                let crossed_lift = speed > IDLE_SPEED
                    && phase != prev_phase
                    && ((p_prev < duty && p_now >= duty) || (p_prev > p_now && p_now >= duty));

                let f = loco.feet[i];
                match f.state {
                    FootState::Planted | FootState::Air => {
                        let (h, _) = home(i, Vec3::ZERO);
                        let drift = (f.pos - h).with_y(0.0).length();
                        let turned = wrap_angle(f.yaw - body_yaw).abs();
                        let lift = if speed > IDLE_SPEED {
                            // The gait says so, or the foot is about to be left behind.
                            crossed_lift || drift > MAX_REACH
                        } else {
                            // Settling: one foot at a time, when it's out of place.
                            !other_swinging && (drift > 0.2 || turned > 0.7) && {
                                let o = loco.feet[1 - i];
                                let (oh, _) = home(1 - i, Vec3::ZERO);
                                let o_err = (o.pos - oh).with_y(0.0).length()
                                    + wrap_angle(o.yaw - body_yaw).abs() * 0.3;
                                drift + turned * 0.3 >= o_err
                            }
                        };
                        if lift {
                            let dur = if speed > IDLE_SPEED { swing_time } else { 0.26 };
                            if drift > MAX_REACH && !crossed_lift && speed > IDLE_SPEED {
                                // Re-sync the gait to this unplanned step.
                                loco.phase = (duty - off).rem_euclid(1.0);
                            }
                            loco.feet[i].state = FootState::Swing {
                                from: f.pos,
                                from_yaw: f.yaw,
                                t: 0.0,
                                dur,
                            };
                        }
                    }
                    FootState::Swing {
                        from,
                        from_yaw,
                        t,
                        dur,
                    } => {
                        // In a gait the swing follows the phase, so both feet
                        // stay in rhythm however the cadence changes; a
                        // settling step (or a stop mid-stride) runs on its timer.
                        let t = if speed > IDLE_SPEED && loco.moving > 0.2 {
                            // Follow the phase, but never faster than twice (or
                            // slower than half) the swing time at today's pace.
                            let by_phase = if p_now >= duty {
                                (p_now - duty) / (1.0 - duty)
                            } else {
                                1.0
                            };
                            let rate = dt / swing_time;
                            by_phase.clamp(t + 0.5 * rate, t + 2.0 * rate)
                        } else {
                            t + dt / dur.min(swing_time.max(0.26))
                        }
                        .min(1.0);
                        // Aim for where the body will be at this foot's mid-stance.
                        let remaining = (1.0 - t) * dur;
                        let ahead = if speed > IDLE_SPEED {
                            loco.vel * (remaining + 0.5 * stance_time)
                        } else {
                            Vec3::ZERO
                        };
                        let (target, n) = home(i, ahead);
                        let e = smoothstep(t);
                        let lift_h = (0.07 + 0.16 * run + 0.04 * loco.moving)
                            * (1.0 - 0.4 * loco.crouch)
                            * (if speed > IDLE_SPEED { 1.0 } else { 0.6 });
                        // The foot rises early (heel kicks up behind) and comes down flat.
                        let arc = (PI * t.powf(0.75)).sin() * lift_h;
                        let base = from.lerp(target, e);
                        let foot = &mut loco.feet[i];
                        foot.pos = base + Vec3::Y * arc;
                        // Lift over a step up as the foot comes forward.
                        if target.y > from.y {
                            foot.pos.y = foot
                                .pos
                                .y
                                .max(from.y + (target.y - from.y) * (e * 1.6).min(1.0) + arc);
                        }
                        foot.yaw = from_yaw + wrap_angle(body_yaw - from_yaw) * e;
                        foot.normal = foot.normal.lerp(n, e).normalize_or(Vec3::Y);
                        // Toe down pushing off, toe up for the heel strike.
                        let swing_amt = loco.moving * (0.5 + 0.5 * run);
                        foot.toe_up =
                            (-0.75 * (1.0 - t) + 0.3 * t) * (PI * t).sin().sqrt() * swing_amt;
                        if t >= 1.0 {
                            foot.pos = target;
                            foot.normal = n;
                            foot.toe_up = 0.0;
                            foot.state = FootState::Planted;
                            plants.write(FootPlant {
                                model,
                                pos: target,
                                landing: false,
                            });
                        } else {
                            foot.state = FootState::Swing {
                                from,
                                from_yaw,
                                t,
                                dur,
                            };
                        }
                    }
                }
            }
        }

        // Landing dip spring (critically-ish damped).
        let k_spring: f32 = 170.0;
        let damp = 2.0 * k_spring.sqrt() * 0.7;
        loco.dip_vel += (-k_spring * loco.dip - damp * loco.dip_vel) * dt;
        loco.dip += loco.dip_vel * dt;
        loco.dip = loco.dip.clamp(-0.3, 0.12);

        // Hips: bob, sway, twist and lean.
        let mv = loco.moving * if airborne { 0.0 } else { 1.0 };
        let mid = TAU * (loco.phase - 0.5 * duty);
        // Walking vaults over the planted leg (high mid-stance); running
        // compresses into it (low mid-stance).
        let bob_sign = 1.0 - 2.0 * run;
        let bob = bob_sign * (0.022 + 0.022 * run) * (2.0 * mid).cos() * mv;
        let sway = (0.028 - 0.016 * run) * mid.cos() * mv * (1.0 - 0.5 * loco.crouch);
        let twist = -(0.10 + 0.06 * run) * (TAU * loco.phase).cos() * mv;

        let local_acc = world.rotation.inverse() * loco.acc;
        let lean_target = Vec2::new(
            (0.05 * (local_vel.z / 7.0).clamp(-0.6, 1.2)
                + 0.025 * (local_acc.z / 8.0).clamp(-1.0, 1.0))
                * (1.0 - 0.5 * loco.crouch),
            -0.05 * (local_acc.x / 8.0).clamp(-1.0, 1.0)
                - 0.02 * (local_vel.x / 7.0).clamp(-1.0, 1.0),
        );
        loco.lean += (lean_target - loco.lean) * (1.0 - (-6.0 * dt).exp());
        let air_tuck = if airborne { 0.06 } else { 0.0 };
        let hips_rot = Quat::from_rotation_y(twist)
            * Quat::from_rotation_x(loco.lean.x + air_tuck + 0.12 * loco.crouch)
            * Quat::from_rotation_z(loco.lean.y);

        let mut hips =
            HIPS_REST + Vec3::new(sway, bob - 0.34 * loco.crouch - 0.035 * run + loco.dip, 0.0);

        // Feet in model space: the ankle above each sole.
        let mut ankles = [Vec3::ZERO; 2];
        let mut foot_rots = [Quat::IDENTITY; 2];
        for i in 0..2 {
            let f = &loco.feet[i];
            let (sole, yaw_m, normal_m) = match f.state {
                FootState::Air => (f.air, 0.0, Vec3::Y),
                _ => (
                    to_model(f.pos),
                    wrap_angle(f.yaw - body_yaw),
                    (world.rotation.inverse() * f.normal).normalize_or(Vec3::Y),
                ),
            };
            let rot = Quat::from_rotation_arc(Vec3::Y, normal_m)
                * Quat::from_rotation_y(yaw_m)
                * Quat::from_rotation_x(-f.toe_up);
            ankles[i] = sole + rot * Vec3::Y * ANKLE_UP;
            foot_rots[i] = rot;
        }

        // Drop the hips as far as it takes for both (grounded) feet to be reachable.
        let reach = (THIGH + SHIN) * 0.985;
        let mut need: f32 = 0.0;
        if !airborne {
            for i in 0..2 {
                let hip = hips + hips_rot * HIP_JOINT[i];
                let d = (ankles[i] - hip).with_y(0.0).length().min(reach * 0.97);
                let max_v = (reach * reach - d * d).sqrt();
                need = need.max((hip.y - ankles[i].y) - max_v);
            }
        }
        let k_drop = if need > loco.reach_drop { 25.0 } else { 7.0 };
        loco.reach_drop += (need - loco.reach_drop) * (1.0 - (-k_drop * dt).exp());
        // Past this, a squat looks worse than a foot briefly short of the ground.
        hips.y -= loco.reach_drop.clamp(0.0, 0.3);

        // Write the bones.
        let mut set = |name: &str, f: &dyn Fn(&mut Transform)| {
            if let Some(mut t) = rig.bone(name).and_then(|e| transforms.get_mut(e).ok()) {
                f(&mut t);
            }
        };
        set("Hips", &|t| {
            t.translation = hips;
            t.rotation = hips_rot;
        });
        for (i, (up, low, foot, toe)) in [
            ("LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase"),
            ("RightUpLeg", "RightLeg", "RightFoot", "RightToeBase"),
        ]
        .into_iter()
        .enumerate()
        {
            let hip = hips + hips_rot * HIP_JOINT[i];
            let foot_fwd = foot_rots[i] * Vec3::Z;
            // Knees point the way the foot and the hips face.
            let pole = (foot_fwd + hips_rot * Vec3::Z).normalize_or(Vec3::Z)
                + Vec3::new(if i == 0 { 0.08 } else { -0.08 }, 0.0, 0.0);
            let (knee, ankle) = two_bone(hip, ankles[i], pole, THIGH, SHIN);
            // The thigh's forward hint points the way the knee bulges.
            let bulge = knee - (hip + ankle) * 0.5;
            let fwd_hint = if bulge.length_squared() > 1e-6 {
                bulge
            } else {
                pole
            };
            let thigh = bone_basis(knee - hip, fwd_hint);
            // The knee is a hinge: the shin shares the thigh's side axis.
            let side = thigh * Vec3::X;
            let shin_y = (knee - ankle).normalize_or(Vec3::Y);
            let shin_z = side.cross(shin_y).normalize_or(Vec3::Z);
            let shin =
                Quat::from_mat3(&Mat3::from_cols(shin_y.cross(shin_z), shin_y, shin_z)).normalize();
            let thigh_local = hips_rot.inverse() * thigh;
            let shin_local = thigh.inverse() * shin;
            let foot_local = shin.inverse() * foot_rots[i];
            // Toes bend back as the heel lifts off.
            let toe_bend = (-loco.feet[i].toe_up).max(0.0) * 0.8;
            set(up, &|t| t.rotation = thigh_local);
            set(low, &|t| t.rotation = shin_local);
            set(foot, &|t| t.rotation = foot_local);
            set(toe, &|t| t.rotation = Quat::from_rotation_x(-toe_bend));
        }
        loco.hips_rot = hips_rot;
        loco.active = true;
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::{collections::HashMap, time::Duration};

    /// Forward kinematics from the local rotations `drive_legs` writes must
    /// put the ankle where the IK aimed it, with the knee bending forward.
    #[test]
    fn leg_bones_reach_ik_target() {
        let hips_rot = Quat::from_rotation_y(0.2) * Quat::from_rotation_x(0.1);
        let hips = Vec3::new(0.02, 0.9, 0.0);
        let hip = hips + hips_rot * HIP_JOINT[0];
        let target = Vec3::new(0.15, 0.2, 0.35);
        let (knee, ankle) = two_bone(hip, target, Vec3::Z, THIGH, SHIN);
        let thigh = bone_basis(knee - hip, knee - (hip + ankle) * 0.5);
        let side = thigh * Vec3::X;
        let shin_y = (knee - ankle).normalize();
        let shin_z = side.cross(shin_y).normalize();
        let shin = Quat::from_mat3(&Mat3::from_cols(shin_y.cross(shin_z), shin_y, shin_z));
        let thigh_local = hips_rot.inverse() * thigh;
        let shin_local = thigh.inverse() * shin;
        // FK with the rig's rest offsets.
        let thigh_w = hips_rot * thigh_local;
        let knee_fk = hip + thigh_w * Vec3::new(0.0, -THIGH, 0.0);
        let ankle_fk = knee_fk + thigh_w * shin_local * Vec3::new(0.0, -SHIN, 0.0);
        assert!(knee_fk.distance(knee) < 1e-3, "{knee_fk} vs {knee}");
        assert!(ankle_fk.distance(target) < 1e-3, "{ankle_fk} vs {target}");
        assert!(knee.z > (hip.z + ankle.z) * 0.5, "knee should bend forward");
    }

    struct Sim {
        app: App,
        root: Entity,
        model: Entity,
        bones: HashMap<String, Entity>,
    }

    fn sim() -> Sim {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(LocomotionPlugin)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
                1.0 / 60.0,
            )))
            .add_systems(PostUpdate, (sample_model_transforms, drive_legs).chain());
        let mut bones = HashMap::new();
        for name in [
            "Hips",
            "LeftUpLeg",
            "LeftLeg",
            "LeftFoot",
            "LeftToeBase",
            "RightUpLeg",
            "RightLeg",
            "RightFoot",
            "RightToeBase",
        ] {
            bones.insert(
                name.to_string(),
                app.world_mut().spawn(Transform::default()).id(),
            );
        }
        let root = app
            .world_mut()
            .spawn(Transform::from_xyz(0.0, 0.9, 0.0))
            .id();
        let model = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, -0.9, 0.0).with_rotation(Quat::from_rotation_y(PI)),
                ChildOf(root),
                Rig {
                    bones: bones.clone(),
                    anim_player: None,
                    gun_holder: None,
                },
                PlayerPose {
                    health: 100.0,
                    ..default()
                },
                Locomotion::default(),
            ))
            .id();
        Sim {
            app,
            root,
            model,
            bones,
        }
    }

    fn plants(app: &mut App) -> usize {
        let msgs = app.world().resource::<Messages<FootPlant>>();
        msgs.iter_current_update_messages().count()
    }

    #[test]
    fn walking_plants_feet_without_sliding() {
        let mut s = sim();
        let speed = 4.6;
        let mut total = 0;
        let mut slide: f32 = 0.0;
        let mut last_planted: [Option<Vec3>; 2] = [None, None];
        for frame in 0..240 {
            {
                let mut t = s.app.world_mut().get_mut::<Transform>(s.root).unwrap();
                t.translation.z -= speed / 60.0;
            }
            s.app.update();
            total += plants(&mut s.app);
            let loco = s.app.world().get::<Locomotion>(s.model).unwrap();
            assert!(loco.active || frame == 0);
            for i in 0..2 {
                let f = loco.feet[i];
                if matches!(f.state, FootState::Planted) {
                    if let Some(p) = last_planted[i]
                        && frame > 30
                    {
                        slide = slide.max(p.distance(f.pos));
                    }
                    last_planted[i] = Some(f.pos);
                } else {
                    last_planted[i] = None;
                }
            }
        }
        // 4 s at walk speed: about 3.4 steps a second once going.
        assert!((10..=18).contains(&total), "{total} steps");
        assert!(slide < 1e-4, "a planted foot slid {slide} m");
        let hips = s.app.world().get::<Transform>(s.bones["Hips"]).unwrap();
        assert!(
            hips.translation.y > 0.6 && hips.translation.y < 1.0,
            "{}",
            hips.translation
        );
    }

    #[test]
    fn standing_still_takes_no_steps_and_turning_does() {
        let mut s = sim();
        for _ in 0..60 {
            s.app.update();
            assert_eq!(plants(&mut s.app), 0);
        }
        s.app
            .world_mut()
            .get_mut::<Transform>(s.root)
            .unwrap()
            .rotation = Quat::from_rotation_y(1.4);
        let mut total = 0;
        for _ in 0..90 {
            s.app.update();
            total += plants(&mut s.app);
        }
        assert!((1..=4).contains(&total), "{total} settle steps");
    }

    #[test]
    fn jump_landing_sends_one_landing() {
        let mut s = sim();
        for _ in 0..10 {
            s.app.update();
        }
        let mut vy = 6.8f32;
        let mut landings = 0;
        let mut steps = 0;
        for _ in 0..90 {
            let airborne = {
                let mut t = s.app.world_mut().get_mut::<Transform>(s.root).unwrap();
                t.translation.y += vy / 60.0;
                vy -= 20.0 / 60.0;
                if t.translation.y <= 0.9 {
                    t.translation.y = 0.9;
                    vy = 0.0;
                    false
                } else {
                    true
                }
            };
            s.app
                .world_mut()
                .get_mut::<PlayerPose>(s.model)
                .unwrap()
                .flags = if airborne { FLAG_AIRBORNE } else { 0 };
            s.app.update();
            let msgs = s.app.world().resource::<Messages<FootPlant>>();
            for m in msgs.iter_current_update_messages() {
                if m.landing {
                    landings += 1;
                } else {
                    steps += 1;
                }
            }
        }
        assert_eq!(landings, 1);
        assert_eq!(steps, 0);
    }

    /// Every speed and direction the game uses: steps stay in rhythm, planted
    /// feet never slide and the hips stay at a believable height.
    #[test]
    fn gaits_across_speeds_and_directions() {
        for (speed, dir) in [
            (1.5, Vec3::NEG_Z),
            (4.6, Vec3::X),
            (4.6, Vec3::Z),
            (7.0, Vec3::NEG_Z),
            (10.0, Vec3::NEG_Z),
        ] {
            let mut s = sim();
            let mut total = 0;
            let mut min_hips = f32::MAX;
            let mut start_hips = f32::MAX;
            let mut slide: f32 = 0.0;
            let mut last: [Option<Vec3>; 2] = [None, None];
            for frame in 0..180 {
                s.app
                    .world_mut()
                    .get_mut::<Transform>(s.root)
                    .unwrap()
                    .translation += dir * speed / 60.0;
                s.app.update();
                if frame > 30 {
                    total += plants(&mut s.app);
                }
                let y = s
                    .app
                    .world()
                    .get::<Transform>(s.bones["Hips"])
                    .unwrap()
                    .translation
                    .y;
                if frame > 5 {
                    start_hips = start_hips.min(y);
                }
                if frame > 60 {
                    min_hips = min_hips.min(y);
                }
                let loco = s.app.world().get::<Locomotion>(s.model).unwrap();
                for i in 0..2 {
                    let f = loco.feet[i];
                    if matches!(f.state, FootState::Planted) {
                        if let Some(p) = last[i]
                            && frame > 2
                        {
                            slide = slide.max(p.distance(f.pos));
                        }
                        last[i] = Some(f.pos);
                    } else {
                        last[i] = None;
                    }
                }
            }
            let per_sec = total as f32 / 2.48;
            println!(
                "speed {speed} dir {dir}: {per_sec:.1} steps/s, hips min {min_hips:.2} (from a standstill {start_hips:.2})"
            );
            assert!(slide < 1e-4, "slid {slide}");
            assert!(per_sec > 1.5 && per_sec < 6.5, "{per_sec}");
            assert!(min_hips > 0.75, "{min_hips}");
            // Setting off at full speed may dip into the first stride, not squat.
            assert!(start_hips > 0.64, "{start_hips}");
        }
    }
}
