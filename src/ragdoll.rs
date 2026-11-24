//! Ragdolls for eliminated players: a small Verlet simulation of the
//! skeleton's joints (with the field's bunkers and ground to land on) that
//! drives the rigged model's bones. Each ragdoll disappears after 15 seconds.

use crate::{
    AppState,
    arena::Colliders,
    player_model::{PlayerModelAssets, Rig, RiggedModel},
    protocol::Team,
};
use bevy::{app::AnimationSystems, prelude::*, transform::TransformSystems};

pub struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, simulate_ragdolls.run_if(in_state(AppState::InGame)))
            .add_systems(
                PostUpdate,
                apply_ragdoll_pose
                    .after(AnimationSystems)
                    .before(TransformSystems::Propagate),
            );
    }
}

/// How long a ragdoll stays before it's removed.
pub const RAGDOLL_LIFETIME: f32 = 15.0;
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

#[derive(Component)]
pub struct Ragdoll {
    pos: [Vec3; JOINTS],
    prev: [Vec3; JOINTS],
    age: f32,
    acc: f32,
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

/// Spawns a ragdoll of `team`'s model, knocked along `push`.
pub fn spawn_ragdoll(
    commands: &mut Commands,
    assets: &PlayerModelAssets,
    team: Team,
    start: RagdollStart,
    push: Vec3,
    seed: u32,
) {
    let mut rng = seed.wrapping_mul(0x9E37_79B9) | 1;
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
    commands.spawn((
        WorldAssetRoot(assets.scene(team)),
        RiggedModel,
        Ragdoll {
            pos: start.joints,
            prev,
            age: 0.0,
            acc: 0.0,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

fn simulate_ragdolls(
    mut commands: Commands,
    time: Res<Time>,
    colliders: Res<Colliders>,
    mut ragdolls: Query<(Entity, &mut Ragdoll)>,
) {
    let dt = time.delta_secs().min(0.1);
    for (entity, mut rd) in &mut ragdolls {
        rd.age += dt;
        if rd.age >= RAGDOLL_LIFETIME {
            commands.entity(entity).despawn();
            continue;
        }
        // Settled bodies sleep after a few seconds (cheap and stable).
        if rd.age > 6.0 {
            continue;
        }
        rd.acc += dt;
        let mut steps = 0;
        while rd.acc >= SUBSTEP && steps < 12 {
            rd.acc -= SUBSTEP;
            steps += 1;
            step(&mut rd, &colliders);
        }
    }
}

fn step(rd: &mut Ragdoll, colliders: &Colliders) {
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

        // Sink into the ground over the last second.
        let sink = (rd.age - (RAGDOLL_LIFETIME - 1.0)).max(0.0) * 0.6;

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

    #[test]
    fn a_ragdoll_falls_down_and_stays_in_one_piece() {
        let colliders = Colliders::default();
        let start = RagdollStart::from_rest(&Transform::default());
        let mut rd = Ragdoll {
            pos: start.joints,
            prev: start.joints.map(|p| p - Vec3::Z * 2.0 * SUBSTEP),
            age: 0.0,
            acc: 0.0,
        };
        for _ in 0..(120 * 4) {
            step(&mut rd, &colliders);
        }
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
}
