//! Ragdolls for eliminated players: a small Verlet simulation of the
//! skeleton's joints (with the field's bunkers, the ground and other bodies to
//! land on) that drives the rigged model's bones.
//!
//! The host simulates every body and sends their poses to everyone, so all
//! players see the same bodies fall the same way. A player's own screen starts
//! a body the moment someone goes down and hands it over to the host's pose as
//! soon as that arrives. Paintballs shove bodies (on the host). A body sinks
//! away once its player has respawned three times (or left).

use crate::{
    AppState,
    arena::Colliders,
    combat::LocalState,
    net::{NetRole, NetworkState},
    player_model::{PlayerModelAssets, Rig, RiggedModel},
    protocol::{PlayerId, RAGDOLL_JOINTS, RagdollPose, Team, pack_ragdoll_frames},
    world::{LocalPlayer, RemotePlayer},
};
use bevy::{app::AnimationSystems, prelude::*, transform::TransformSystems};
use std::collections::{HashMap, HashSet};

pub struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RagdollSync>()
            .add_systems(OnExit(AppState::InGame), reset_sync)
            .add_systems(
                Update,
                (receive_host_ragdolls, simulate_ragdolls, host_ragdolls)
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                PostUpdate,
                apply_ragdoll_pose
                    .after(AnimationSystems)
                    .before(TransformSystems::Propagate),
            );
    }
}

/// A body goes once its player has respawned this many times since it fell.
pub const RESPAWNS_TO_CLEAR: u32 = 3;
/// At most this many bodies at once (the oldest goes first).
const MAX_BODIES: usize = 24;
/// Seconds a body takes to sink away.
const SINK_TIME: f32 = 1.0;
/// A body the host never told us about (a host without synced ragdolls) goes
/// after this long, like bodies always used to.
const FALLBACK_LIFETIME: f32 = 15.0;
/// How often the host sends moving bodies, and every body.
const SEND_EVERY: f32 = 0.05;
const FULL_EVERY: f32 = 1.0;
/// How fast a player's copy of a body catches up with the host's pose.
const FOLLOW_RATE: f32 = 18.0;
/// Seconds a body keeps simulating after it falls / is hit before it may sleep.
const WAKE_ON_SPAWN: f32 = 6.0;
const WAKE_ON_HIT: f32 = 2.5;
/// Joints slower than this (m/s) let a body sleep.
const SLEEP_SPEED: f32 = 0.35;
/// How hard a paintball shoves a body (m/s at the joints it hits).
const SHOVE_SPEED: f32 = 2.4;
/// A paintball's size, for hitting bodies.
const BALL_RADIUS: f32 = 0.03;

const SUBSTEP: f32 = 1.0 / 120.0;
const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

// Joints, in the model's rest pose (feet at the origin, facing +Z).
const HIP_L: usize = 0;
const HIP_R: usize = 1;
const SH_L: usize = 2;
const SH_R: usize = 3;
const HEAD: usize = 4;
const ELBOW_L: usize = 5;
const HAND_L: usize = 6;
const ELBOW_R: usize = 7;
const HAND_R: usize = 8;
const KNEE_L: usize = 9;
const FOOT_L: usize = 10;
const KNEE_R: usize = 11;
const FOOT_R: usize = 12;
const JOINTS: usize = 13;

/// Rest positions, with the arms hanging (not the T-pose) so a ragdoll that
/// starts from rest looks like a person.
const REST: [Vec3; JOINTS] = [
    Vec3::new(0.1, 0.94, 0.0),
    Vec3::new(-0.1, 0.94, 0.0),
    Vec3::new(0.19, 1.46, 0.0),
    Vec3::new(-0.19, 1.46, 0.0),
    Vec3::new(0.0, 1.72, 0.0),
    Vec3::new(0.24, 1.19, 0.06),
    Vec3::new(0.26, 0.95, 0.14),
    Vec3::new(-0.24, 1.19, 0.06),
    Vec3::new(-0.26, 0.95, 0.14),
    Vec3::new(0.1, 0.52, 0.0),
    Vec3::new(0.1, 0.09, 0.0),
    Vec3::new(-0.1, 0.52, 0.0),
    Vec3::new(-0.1, 0.09, 0.0),
];

/// Joint radius for touching the ground and bunkers.
const RADIUS: [f32; JOINTS] = [
    0.1, 0.1, 0.1, 0.1, 0.13, 0.06, 0.06, 0.06, 0.06, 0.07, 0.07, 0.07, 0.07,
];

/// Sticks: pairs of joints kept at their rest distance.
const STICKS: &[(usize, usize)] = &[
    // Torso (all six pairs keep it rigid) and head.
    (HIP_L, HIP_R),
    (SH_L, SH_R),
    (HIP_L, SH_L),
    (HIP_R, SH_R),
    (HIP_L, SH_R),
    (HIP_R, SH_L),
    (HEAD, SH_L),
    (HEAD, SH_R),
    (HEAD, HIP_L),
    (HEAD, HIP_R),
    // Limbs.
    (SH_L, ELBOW_L),
    (ELBOW_L, HAND_L),
    (SH_R, ELBOW_R),
    (ELBOW_R, HAND_R),
    (HIP_L, KNEE_L),
    (KNEE_L, FOOT_L),
    (HIP_R, KNEE_R),
    (KNEE_R, FOOT_R),
];

/// Limb lengths from the skeleton (T-pose), which the hanging rest keeps.
fn rest_length(a: usize, b: usize) -> f32 {
    match (a, b) {
        (SH_L, ELBOW_L) | (SH_R, ELBOW_R) => 0.28,
        (ELBOW_L, HAND_L) | (ELBOW_R, HAND_R) => 0.25,
        _ => REST[a].distance(REST[b]),
    }
}

/// Joints that shouldn't fold flat: (a, b, minimum distance).
const MIN_SPANS: &[(usize, usize, f32)] = &[
    (SH_L, HAND_L, 0.18),
    (SH_R, HAND_R, 0.18),
    (HIP_L, FOOT_L, 0.4),
    (HIP_R, FOOT_R, 0.4),
    (HAND_L, HIP_L, 0.12),
    (HAND_R, HIP_R, 0.12),
    (KNEE_L, KNEE_R, 0.12),
    (FOOT_L, FOOT_R, 0.1),
];

/// Limbs as capsules (joint, joint, radius) so limbs and bodies rest on each
/// other instead of passing through. The head is a ball.
const CAPSULES: [(usize, usize, f32); 11] = [
    (HIP_L, KNEE_L, 0.08),
    (KNEE_L, FOOT_L, 0.07),
    (HIP_R, KNEE_R, 0.08),
    (KNEE_R, FOOT_R, 0.07),
    (SH_L, ELBOW_L, 0.06),
    (ELBOW_L, HAND_L, 0.055),
    (SH_R, ELBOW_R, 0.06),
    (ELBOW_R, HAND_R, 0.055),
    (HIP_L, SH_L, 0.11),
    (HIP_R, SH_R, 0.11),
    (HEAD, HEAD, 0.13),
];
const C_THIGH_L: usize = 0;
const C_THIGH_R: usize = 2;
const C_ARM_L: [usize; 2] = [4, 5];
const C_ARM_R: [usize; 2] = [6, 7];
const C_TORSO_L: usize = 8;
const C_TORSO_R: usize = 9;
const C_HEAD: usize = 10;

/// Whether two of a body's own capsules should push each other apart: not
/// ones that share a joint, and not ones that always overlap where they meet
/// the torso (that's what the sticks are for).
fn self_pair(i: usize, j: usize) -> bool {
    let (a0, a1, _) = CAPSULES[i];
    let (b0, b1, _) = CAPSULES[j];
    if a0 == b0 || a0 == b1 || a1 == b0 || a1 == b1 {
        return false;
    }
    let torso = |c: usize| c == C_TORSO_L || c == C_TORSO_R;
    let either =
        |f: &dyn Fn(usize) -> bool, g: &dyn Fn(usize) -> bool| (f(i) && g(j)) || (f(j) && g(i));
    !(either(&torso, &torso)
        || either(&|c: usize| c == C_HEAD, &torso)
        || either(&|c: usize| c == C_THIGH_L || c == C_THIGH_R, &torso)
        || either(&|c: usize| C_ARM_L.contains(&c), &|c: usize| c == C_TORSO_L)
        || either(&|c: usize| C_ARM_R.contains(&c), &|c: usize| c == C_TORSO_R))
}

/// Closest points between segments p1-q1 and p2-q2: how far along each (0..=1).
fn closest_params(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (f32, f32) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    if a <= 1e-9 && e <= 1e-9 {
        return (0.0, 0.0);
    }
    if a <= 1e-9 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = d1.dot(r);
    if e <= 1e-9 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = d1.dot(d2);
    let denom = a * e - b * b;
    let mut s = if denom > 1e-9 {
        ((b * f - c * e) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (s, t)
}

/// Where capsule `ca` of `pa` overlaps capsule `cb` of `pb`: how far along
/// each, the direction from b to a, and how deep.
fn contact(
    pa: &[Vec3; JOINTS],
    ca: usize,
    pb: &[Vec3; JOINTS],
    cb: usize,
) -> Option<(f32, f32, Vec3, f32)> {
    let (a0, a1, ra) = CAPSULES[ca];
    let (b0, b1, rb) = CAPSULES[cb];
    let (s, t) = closest_params(pa[a0], pa[a1], pb[b0], pb[b1]);
    let d = pa[a0].lerp(pa[a1], s) - pb[b0].lerp(pb[b1], t);
    let dist = d.length();
    let depth = ra + rb - dist;
    if depth <= 0.0 {
        return None;
    }
    let n = if dist > 1e-5 { d / dist } else { Vec3::Y };
    Some((s, t, n, depth))
}

/// Moves capsule `c` so its point `s` along it moves by `delta`.
fn move_capsule(pos: &mut [Vec3; JOINTS], c: usize, s: f32, delta: Vec3) {
    let (a, b, _) = CAPSULES[c];
    if a == b {
        pos[a] += delta;
        return;
    }
    let (wa, wb) = (1.0 - s, s);
    let k = 1.0 / (wa * wa + wb * wb);
    pos[a] += delta * wa * k;
    pos[b] += delta * wb * k;
}

#[derive(Component)]
pub struct Ragdoll {
    pos: [Vec3; JOINTS],
    prev: [Vec3; JOINTS],
    /// Whose body, and which of their eliminations (their `deaths` count).
    owner: PlayerId,
    death: u32,
    age: f32,
    /// Seconds of simulation left before it may sleep.
    wake: f32,
    asleep: bool,
    /// Simulated since the host last sent it.
    moved: bool,
    /// A player's copy: the host's latest pose, which it follows.
    follow: Option<[Vec3; JOINTS]>,
    /// Host: the owner's respawn count when it fell.
    base_respawns: Option<u32>,
    /// Going away: seconds into sinking.
    sinking: Option<f32>,
}

/// Where a paintball meets a body.
#[derive(Clone, Copy, Debug)]
pub struct BodyHit {
    /// How far along the ball's path (0..=1).
    pub t: f32,
    capsule: usize,
    s: f32,
}

impl Ragdoll {
    fn new(pos: [Vec3; JOINTS], prev: [Vec3; JOINTS], owner: PlayerId, death: u32) -> Self {
        Self {
            pos,
            prev,
            owner,
            death,
            age: 0.0,
            wake: WAKE_ON_SPAWN,
            asleep: false,
            moved: true,
            follow: None,
            base_respawns: None,
            sinking: None,
        }
    }

    fn key(&self) -> (PlayerId, u32) {
        (self.owner, self.death)
    }

    /// Simulated here (the host, or a player's copy the host hasn't sent yet).
    fn simulated(&self) -> bool {
        self.follow.is_none() && self.sinking.is_none()
    }

    fn awake(&self) -> bool {
        self.simulated() && !self.asleep
    }

    fn center(&self) -> Vec3 {
        (self.pos[HIP_L] + self.pos[HIP_R]) * 0.5
    }

    fn max_speed(&self) -> f32 {
        (0..JOINTS)
            .map(|i| (self.pos[i] - self.prev[i]).length())
            .fold(0.0, f32::max)
            / SUBSTEP
    }

    /// Where a paintball going `from -> to` this frame passes through the body.
    pub fn ball_hit(&self, from: Vec3, to: Vec3) -> Option<BodyHit> {
        if self.sinking.is_some() {
            return None;
        }
        let mut best: Option<BodyHit> = None;
        for (c, &(a, b, r)) in CAPSULES.iter().enumerate() {
            let (t, s) = closest_params(from, to, self.pos[a], self.pos[b]);
            let d = from.lerp(to, t).distance(self.pos[a].lerp(self.pos[b], s));
            if d <= r + BALL_RADIUS && best.is_none_or(|h| t < h.t) {
                best = Some(BodyHit { t, capsule: c, s });
            }
        }
        best
    }

    /// A paintball hit the body: knock the limb it hit along the ball's path.
    /// Only bodies simulated here move; a player's copy moves when the host's does.
    pub fn shove(&mut self, hit: &BodyHit, velocity: Vec3, damage: f32) {
        if !self.simulated() {
            return;
        }
        let (a, b, _) = CAPSULES[hit.capsule];
        let strength = (damage / 20.0).clamp(0.5, 1.6) * SHOVE_SPEED;
        let dv = velocity.normalize_or(Vec3::Z) * strength + Vec3::Y * strength * 0.15;
        for i in 0..JOINTS {
            let w = if a == b {
                if i == a { 1.0 } else { 0.0 }
            } else if i == a {
                1.0 - hit.s
            } else if i == b {
                hit.s
            } else {
                0.0
            };
            // The whole body rocks a little, the limb that's hit a lot.
            self.prev[i] -= dv * (0.1 + 0.9 * w) * SUBSTEP;
        }
        self.asleep = false;
        self.wake = self.wake.max(WAKE_ON_HIT);
    }
}

/// The host's bookkeeping (respawn counts, send timers) and, for players, the
/// bodies already gone (so a late packet doesn't bring one back).
#[derive(Resource, Default)]
struct RagdollSync {
    gone: HashSet<(PlayerId, u32)>,
    alive: HashMap<PlayerId, bool>,
    respawns: HashMap<PlayerId, u32>,
    acc: f32,
    send_timer: f32,
    full_timer: f32,
    last_all: Vec<[u32; 2]>,
}

fn reset_sync(mut sync: ResMut<RagdollSync>) {
    *sync = RagdollSync::default();
}

const _: () = assert!(JOINTS == RAGDOLL_JOINTS);

fn encode_joints(pos: &[Vec3; JOINTS]) -> Vec<i32> {
    pos.iter()
        .flat_map(|p| p.to_array())
        .map(|v| (v * 100.0).round() as i32)
        .collect()
}

fn decode_joints(joints: &[i32]) -> Option<[Vec3; JOINTS]> {
    if joints.len() != JOINTS * 3 {
        return None;
    }
    let mut out = [Vec3::ZERO; JOINTS];
    for (i, p) in out.iter_mut().enumerate() {
        *p = Vec3::new(
            joints[i * 3] as f32,
            joints[i * 3 + 1] as f32,
            joints[i * 3 + 2] as f32,
        ) / 100.0;
        if !p.is_finite() {
            return None;
        }
    }
    Some(out)
}

/// The ragdoll's starting pose: joint positions in world space.
pub struct RagdollStart {
    pub joints: [Vec3; JOINTS],
}

impl RagdollStart {
    /// The rest pose, standing at `transform` (feet origin, model facing +Z).
    pub fn from_rest(transform: &Transform) -> Self {
        Self {
            joints: REST.map(|p| transform.transform_point(p)),
        }
    }

    /// The pose a live model is in right now (falls back to its rest pose for
    /// any joint it doesn't have).
    pub fn from_rig(rig: &Rig, model: &GlobalTransform, bones: &Query<&GlobalTransform>) -> Self {
        let rest = model.compute_transform();
        let mut start = Self::from_rest(&rest);
        let at = |name: &str| rig.bone(name).and_then(|e| bones.get(e).ok());
        let names = [
            "LeftUpLeg",
            "RightUpLeg",
            "LeftArm",
            "RightArm",
            "Head",
            "LeftForeArm",
            "LeftHand",
            "RightForeArm",
            "RightHand",
            "LeftLeg",
            "LeftFoot",
            "RightLeg",
            "RightFoot",
        ];
        for (i, name) in names.iter().enumerate() {
            if let Some(g) = at(name) {
                start.joints[i] = if i == HEAD {
                    g.transform_point(Vec3::Y * 0.12)
                } else {
                    g.translation()
                };
            }
        }
        start
    }
}

/// Spawns a ragdoll of `owner`'s model for their `death`th elimination,
/// knocked along `push`.
pub fn spawn_ragdoll(
    commands: &mut Commands,
    assets: &PlayerModelAssets,
    owner: PlayerId,
    death: u32,
    start: RagdollStart,
    push: Vec3,
) {
    let mut rng = (owner ^ death).wrapping_mul(0x9E37_79B9) | 1;
    let mut rand = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng as f32 / u32::MAX as f32 - 0.5
    };
    let push = push.with_y(0.0).normalize_or(Vec3::Z);
    let mut prev = start.joints;
    for (i, p) in prev.iter_mut().enumerate() {
        // Hit up high: the shoulders and head go first, the knees buckle.
        let upper = matches!(i, SH_L | SH_R | HEAD | ELBOW_L | ELBOW_R | HAND_L | HAND_R);
        let knee = matches!(i, KNEE_L | KNEE_R);
        let mut v = push * if upper { 2.6 } else { 0.6 };
        if knee {
            v -= push * 1.2;
        }
        v += Vec3::new(rand(), rand() * 0.5 + 0.3, rand()) * 0.8;
        *p -= v * SUBSTEP;
    }
    spawn_body(
        commands,
        assets,
        Ragdoll::new(start.joints, prev, owner, death),
    );
}

fn spawn_body(commands: &mut Commands, assets: &PlayerModelAssets, ragdoll: Ragdoll) {
    commands.spawn((
        WorldAssetRoot(assets.scene(Team::of(ragdoll.owner))),
        RiggedModel,
        ragdoll,
        DespawnOnExit(AppState::InGame),
    ));
}

/// Players: follow the host's bodies, add the ones we don't have, and let go
/// of the ones the host no longer has.
fn receive_host_ragdolls(
    mut commands: Commands,
    mut net: ResMut<NetworkState>,
    assets: Res<PlayerModelAssets>,
    sync: Res<RagdollSync>,
    mut ragdolls: Query<(Entity, &mut Ragdoll)>,
) {
    let frames = std::mem::take(&mut net.pending_ragdolls);
    if net.active_role() != Some(NetRole::Client) {
        return;
    }
    let mut spawned: HashSet<(PlayerId, u32)> = HashSet::new();
    let mut dropped: HashSet<Entity> = HashSet::new();
    for frame in frames {
        for body in &frame.bodies {
            let key = (body.owner, body.death);
            if sync.gone.contains(&key) || spawned.contains(&key) {
                continue;
            }
            let Some(joints) = decode_joints(&body.joints) else {
                continue;
            };
            let mut found = false;
            for (entity, mut rd) in &mut ragdolls {
                if rd.key() != key || rd.sinking.is_some() || dropped.contains(&entity) {
                    continue;
                }
                if found {
                    // Two of the same body (ours and the host's both started): keep one.
                    commands.entity(entity).despawn();
                    dropped.insert(entity);
                } else {
                    rd.follow = Some(joints);
                    found = true;
                }
            }
            if !found {
                // One we didn't see fall (we just joined, or missed it).
                let mut rd = Ragdoll::new(joints, joints, key.0, key.1);
                rd.follow = Some(joints);
                spawn_body(&mut commands, &assets, rd);
                spawned.insert(key);
            }
        }
        if let Some(all) = &frame.all {
            let keep: HashSet<(PlayerId, u32)> = all.iter().map(|k| (k[0], k[1])).collect();
            for (_, mut rd) in &mut ragdolls {
                // A body of ours the host never had after a moment is a stray too.
                if rd.sinking.is_none()
                    && !keep.contains(&rd.key())
                    && (rd.follow.is_some() || rd.age > 2.0)
                {
                    rd.sinking = Some(0.0);
                }
            }
        }
    }
}

fn simulate_ragdolls(
    mut commands: Commands,
    time: Res<Time>,
    colliders: Res<Colliders>,
    net: Res<NetworkState>,
    mut sync: ResMut<RagdollSync>,
    mut ragdolls: Query<(Entity, &mut Ragdoll)>,
) {
    let dt = time.delta_secs().min(0.1);
    let client = net.active_role() == Some(NetRole::Client);
    let blend = 1.0 - (-FOLLOW_RATE * dt).exp();
    for (entity, mut rd) in &mut ragdolls {
        rd.age += dt;
        if let Some(s) = rd.sinking {
            let s = s + dt;
            rd.sinking = Some(s);
            if s >= SINK_TIME {
                sync.gone.insert(rd.key());
                commands.entity(entity).despawn();
            }
            continue;
        }
        if client && rd.follow.is_none() && rd.age >= FALLBACK_LIFETIME {
            rd.sinking = Some(0.0);
            continue;
        }
        if let Some(target) = rd.follow {
            for (i, &goal) in target.iter().enumerate() {
                let p = rd.pos[i];
                let next = if p.distance_squared(goal) > 4.0 {
                    goal
                } else {
                    p.lerp(goal, blend)
                };
                rd.pos[i] = next;
                rd.prev[i] = next;
            }
        }
    }

    sync.acc = (sync.acc + dt).min(SUBSTEP * 12.0);
    while sync.acc >= SUBSTEP {
        sync.acc -= SUBSTEP;
        for (_, mut rd) in &mut ragdolls {
            if rd.awake() {
                step(&mut rd, &colliders);
            }
        }
        // Bodies that fall on each other pile up.
        let mut pairs = ragdolls.iter_combinations_mut::<2>();
        while let Some([(_, mut a), (_, mut b)]) = pairs.fetch_next() {
            collide_bodies(&mut a, &mut b);
        }
    }

    for (_, mut rd) in &mut ragdolls {
        if rd.awake() && rd.wake <= 0.0 && (rd.max_speed() < SLEEP_SPEED || rd.wake < -4.0) {
            rd.asleep = true;
        }
    }
}

/// Keeps two bodies' limbs out of each other. A sleeping body stays put and
/// the other rests on it.
fn collide_bodies(a: &mut Ragdoll, b: &mut Ragdoll) {
    if !a.simulated() || !b.simulated() {
        return;
    }
    let wa = if a.asleep { 0.0 } else { 1.0 };
    let wb = if b.asleep { 0.0 } else { 1.0 };
    if wa + wb == 0.0 || a.center().distance_squared(b.center()) > 2.4 * 2.4 {
        return;
    }
    let (wa, wb) = (wa / (wa + wb), wb / (wa + wb));
    for ca in 0..CAPSULES.len() {
        for cb in 0..CAPSULES.len() {
            if let Some((s, t, n, depth)) = contact(&a.pos, ca, &b.pos, cb) {
                if wa > 0.0 {
                    move_capsule(&mut a.pos, ca, s, n * depth * wa);
                    a.moved = true;
                }
                if wb > 0.0 {
                    move_capsule(&mut b.pos, cb, t, -n * depth * wb);
                    b.moved = true;
                }
            }
        }
    }
}

/// Host: count respawns, clear away bodies whose player has respawned three
/// times (or left), and send everyone the bodies.
#[allow(clippy::too_many_arguments)]
fn host_ragdolls(
    time: Res<Time>,
    net: Res<NetworkState>,
    local_state: Res<LocalState>,
    mut sync: ResMut<RagdollSync>,
    local_q: Query<&LocalPlayer>,
    remote_q: Query<&RemotePlayer>,
    mut ragdolls: Query<&mut Ragdoll>,
) {
    let role = net.active_role();
    if role == Some(NetRole::Client) {
        return;
    }

    // A player who was out and is back in has respawned.
    let mut lives: Vec<(PlayerId, bool)> = remote_q.iter().map(|r| (r.id, r.alive())).collect();
    if let Ok(me) = local_q.single() {
        lives.push((me.id, local_state.0.health > 0.0));
    }
    let present: HashSet<PlayerId> = lives.iter().map(|l| l.0).collect();
    for (id, alive) in lives {
        let was = sync.alive.insert(id, alive).unwrap_or(alive);
        if alive && !was {
            *sync.respawns.entry(id).or_default() += 1;
        }
    }
    sync.alive.retain(|id, _| present.contains(id));

    let mut live = 0;
    for mut rd in &mut ragdolls {
        if rd.sinking.is_some() {
            continue;
        }
        let respawns = sync.respawns.get(&rd.owner).copied().unwrap_or(0);
        let base = *rd.base_respawns.get_or_insert(respawns);
        if respawns >= base + RESPAWNS_TO_CLEAR || !present.contains(&rd.owner) {
            rd.sinking = Some(0.0);
        } else {
            live += 1;
        }
    }
    while live > MAX_BODIES {
        let oldest = ragdolls
            .iter_mut()
            .filter(|r| r.sinking.is_none())
            .max_by(|a, b| a.age.total_cmp(&b.age));
        match oldest {
            Some(mut rd) => rd.sinking = Some(0.0),
            None => break,
        }
        live -= 1;
    }

    if role != Some(NetRole::Host) {
        return;
    }
    let dt = time.delta_secs();
    sync.send_timer += dt;
    sync.full_timer += dt;
    if sync.send_timer < SEND_EVERY {
        return;
    }
    // Keep the leftover so the send rate doesn't round to whole frames.
    sync.send_timer = (sync.send_timer - SEND_EVERY).min(SEND_EVERY);
    let full = sync.full_timer >= FULL_EVERY;
    if full {
        sync.full_timer = 0.0;
    }
    let mut all = Vec::new();
    let mut bodies = Vec::new();
    for mut rd in &mut ragdolls {
        if rd.sinking.is_some() {
            continue;
        }
        all.push([rd.owner, rd.death]);
        if full || rd.moved {
            rd.moved = false;
            bodies.push(RagdollPose {
                owner: rd.owner,
                death: rd.death,
                joints: encode_joints(&rd.pos),
            });
        }
    }
    all.sort_unstable();
    if bodies.is_empty() && !full && all == sync.last_all {
        return;
    }
    sync.last_all = all.clone();
    net.send_ragdolls(pack_ragdoll_frames(all, bodies));
}

fn step(rd: &mut Ragdoll, colliders: &Colliders) {
    rd.wake -= SUBSTEP;
    rd.moved = true;
    for i in 0..JOINTS {
        let v = (rd.pos[i] - rd.prev[i]) * 0.995;
        rd.prev[i] = rd.pos[i];
        rd.pos[i] += v + GRAVITY * SUBSTEP * SUBSTEP;
    }
    for _ in 0..8 {
        for &(a, b) in STICKS {
            let rest = rest_length(a, b);
            let d = rd.pos[b] - rd.pos[a];
            let len = d.length().max(1e-5);
            let fix = d * ((len - rest) / len * 0.5);
            rd.pos[a] += fix;
            rd.pos[b] -= fix;
        }
        for &(a, b, min) in MIN_SPANS {
            let d = rd.pos[b] - rd.pos[a];
            let len = d.length().max(1e-5);
            if len < min {
                let fix = d * ((len - min) / len * 0.5);
                rd.pos[a] += fix;
                rd.pos[b] -= fix;
            }
        }
        // Limbs rest on each other (a leg on top of the other) instead of
        // sinking through.
        for i in 0..CAPSULES.len() {
            for j in (i + 1)..CAPSULES.len() {
                if !self_pair(i, j) {
                    continue;
                }
                if let Some((s, t, n, depth)) = contact(&rd.pos, i, &rd.pos, j) {
                    move_capsule(&mut rd.pos, i, s, n * depth * 0.5);
                    move_capsule(&mut rd.pos, j, t, -n * depth * 0.5);
                }
            }
        }
        for i in 0..JOINTS {
            let r = RADIUS[i];
            let mut touched = false;
            if rd.pos[i].y < r {
                rd.pos[i].y = r;
                touched = true;
            }
            touched |= colliders.push_out_point(&mut rd.pos[i], r);
            if touched {
                // Friction: lose most sideways speed on contact.
                let p = rd.pos[i];
                let prev = &mut rd.prev[i];
                prev.x = prev.x + (p.x - prev.x) * 0.35;
                prev.z = prev.z + (p.z - prev.z) * 0.35;
            }
        }
    }
}

/// Points the skeleton's bones along the simulated joints.
fn apply_ragdoll_pose(ragdolls: Query<(&Ragdoll, &Rig)>, mut transforms: Query<&mut Transform>) {
    for (rd, rig) in &ragdolls {
        let p = &rd.pos;
        let hips_mid = (p[HIP_L] + p[HIP_R]) * 0.5;
        let sh_mid = (p[SH_L] + p[SH_R]) * 0.5;
        let up = (sh_mid - hips_mid).normalize_or(Vec3::Y);
        let side = ((p[HIP_L] - p[HIP_R]) + (p[SH_L] - p[SH_R])).normalize_or(Vec3::X);
        let side = (side - up * side.dot(up)).normalize_or(Vec3::X);
        let fwd = side.cross(up);
        let torso = Quat::from_mat3(&Mat3::from_cols(side, up, fwd)).normalize();
        let hips_pos = hips_mid + torso * Vec3::new(0.0, 0.04, 0.0);

        // Going away: sink into the ground.
        let sink = rd.sinking.unwrap_or(0.0) / SINK_TIME * 0.6;

        let mut set = |name: &str, rotation: Quat, translation: Option<Vec3>| {
            if let Some(mut t) = rig.bone(name).and_then(|e| transforms.get_mut(e).ok()) {
                t.rotation = rotation;
                if let Some(tr) = translation {
                    t.translation = tr;
                }
            }
        };
        set("Hips", torso, Some(hips_pos - Vec3::Y * sink));
        for name in [
            "Spine",
            "Spine1",
            "Spine2",
            "LeftShoulder",
            "RightShoulder",
            "Head",
        ] {
            set(name, Quat::IDENTITY, None);
        }

        // A bone whose parent has world rotation `parent` should point along `dir`.
        let aim = |parent: Quat, rest: Vec3, dir: Vec3| -> (Quat, Quat) {
            let world =
                Quat::from_rotation_arc(parent * rest, dir.normalize_or(parent * rest)) * parent;
            (parent.inverse() * world, world)
        };

        let neck_origin = hips_pos + torso * Vec3::new(0.0, 0.54, 0.0);
        let (neck, _) = aim(torso, Vec3::Y, p[HEAD] - neck_origin);
        set("Neck", neck, None);

        for (arm, fore, sh, el, ha, rest) in [
            ("LeftArm", "LeftForeArm", SH_L, ELBOW_L, HAND_L, Vec3::X),
            (
                "RightArm",
                "RightForeArm",
                SH_R,
                ELBOW_R,
                HAND_R,
                Vec3::NEG_X,
            ),
        ] {
            let (upper_local, upper_world) = aim(torso, rest, p[el] - p[sh]);
            let (fore_local, _) = aim(upper_world, rest, p[ha] - p[el]);
            set(arm, upper_local, None);
            set(fore, fore_local, None);
        }
        for (up_leg, leg, hip, knee, foot) in [
            ("LeftUpLeg", "LeftLeg", HIP_L, KNEE_L, FOOT_L),
            ("RightUpLeg", "RightLeg", HIP_R, KNEE_R, FOOT_R),
        ] {
            let (thigh_local, thigh_world) = aim(torso, Vec3::NEG_Y, p[knee] - p[hip]);
            let (shin_local, _) = aim(thigh_world, Vec3::NEG_Y, p[foot] - p[knee]);
            set(up_leg, thigh_local, None);
            set(leg, shin_local, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallen(push: Vec3) -> Ragdoll {
        let colliders = Colliders::default();
        let start = RagdollStart::from_rest(&Transform::default());
        let mut rd = Ragdoll::new(start.joints, start.joints.map(|p| p - push * SUBSTEP), 1, 1);
        for _ in 0..(120 * 4) {
            step(&mut rd, &colliders);
        }
        rd
    }

    #[test]
    fn a_ragdoll_falls_down_and_stays_in_one_piece() {
        let rd = fallen(Vec3::Z * 2.0);
        // Lying down: the head is near the ground, nothing is under it.
        assert!(rd.pos[HEAD].y < 0.5, "head at {}", rd.pos[HEAD].y);
        for (i, p) in rd.pos.iter().enumerate() {
            assert!(p.y >= RADIUS[i] - 1e-3);
            assert!(p.is_finite());
        }
        for &(a, b) in STICKS {
            let len = rd.pos[a].distance(rd.pos[b]);
            assert!((len - rest_length(a, b)).abs() < 0.05, "{a}-{b}: {len}");
        }
    }

    #[test]
    fn limbs_dont_sink_into_each_other() {
        for push in [Vec3::Z * 2.0, Vec3::X * 2.5, Vec3::new(-1.5, 0.0, 1.5)] {
            let rd = fallen(push);
            for i in 0..CAPSULES.len() {
                for j in (i + 1)..CAPSULES.len() {
                    if self_pair(i, j)
                        && let Some((_, _, _, depth)) = contact(&rd.pos, i, &rd.pos, j)
                    {
                        assert!(depth < 0.08, "capsules {i} and {j} overlap by {depth}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_standing_body_has_no_self_contacts() {
        // Otherwise a fresh body would jump apart the moment it falls.
        for i in 0..CAPSULES.len() {
            for j in (i + 1)..CAPSULES.len() {
                if self_pair(i, j) {
                    assert!(
                        contact(&REST, i, &REST, j).is_none(),
                        "{i} and {j} touch at rest"
                    );
                }
            }
        }
    }

    #[test]
    fn a_body_lands_on_another_instead_of_through_it() {
        let colliders = Colliders::default();
        let mut below = fallen(Vec3::Z * 2.0);
        below.asleep = true;
        // Drop a second body onto the first, a little above it.
        let lift = Vec3::new(0.0, 0.6, 0.0);
        let pos = below.pos.map(|p| p + lift);
        let mut above = Ragdoll::new(pos, pos, 2, 1);
        for _ in 0..(120 * 3) {
            step(&mut above, &colliders);
            collide_bodies(&mut below, &mut above);
        }
        for i in 0..CAPSULES.len() {
            for j in 0..CAPSULES.len() {
                if let Some((_, _, _, depth)) = contact(&below.pos, i, &above.pos, j) {
                    assert!(depth < 0.08, "capsules {i} and {j} overlap by {depth}");
                }
            }
        }
    }

    #[test]
    fn a_paintball_hits_and_shoves_a_body() {
        let mut rd = fallen(Vec3::Z * 2.0);
        let c = rd.center();
        let hit = rd
            .ball_hit(c + Vec3::new(-3.0, 0.0, 0.0), c + Vec3::new(3.0, 0.0, 0.0))
            .expect("ball through the body hits it");
        assert!(hit.t > 0.3 && hit.t < 0.7);
        rd.asleep = true;
        rd.shove(&hit, Vec3::X * 60.0, 25.0);
        assert!(!rd.asleep);
        assert!(rd.pos[HIP_L].x - rd.prev[HIP_L].x > 0.0);
        assert!(
            rd.ball_hit(c + Vec3::Y * 3.0, c + Vec3::new(1.0, 3.0, 0.0))
                .is_none()
        );
    }

    #[test]
    fn poses_survive_the_wire() {
        let rd = fallen(Vec3::Z * 2.0);
        let back = decode_joints(&encode_joints(&rd.pos)).unwrap();
        for (a, b) in rd.pos.iter().zip(back) {
            assert!(a.distance(b) < 0.01);
        }
        assert!(decode_joints(&[1, 2, 3]).is_none());
    }
}
