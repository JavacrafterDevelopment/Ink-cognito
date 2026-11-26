//! Bots: computer players the host adds so a match has someone to play
//! against, up to `MAX_PLAYERS` (5 a team in team modes). They leave as
//! people join, and never play ranked.
//!
//! Bots live only in the host's game. Everyone else sees them as ordinary
//! players: the host sends their position and state like its own, their shots
//! fly on every screen from their `shots` counter, and the host confirms the
//! hits their paint makes (`combat::move_projectiles`). Hits on a bot come back
//! to the host as `NetEvent::BotHit`.
//!
//! Each bot walks a coarse grid of the open ground (`Nav`), built once per map,
//! so it finds its way around bunkers. It roams toward the other team, fights
//! what it sees (turning and aiming at human speeds, with an aim error that
//! settles while it tracks), hides behind cover when it's hurt or reloading and
//! peeks back out to shoot.

use crate::{
    AppState,
    arena::Colliders,
    combat::{REGEN_DELAY, REGEN_RATE, remote_shot_origin},
    game_mode::{pick_spawn, random_seed},
    loadout::{GunKind, Loadout, Slot, WeaponStats},
    maps::MapInfo,
    net::{NetRole, NetworkState},
    protocol::{
        BOT_ID_END, BotLevel, FIRST_BOT_ID, FLAG_AIMING, FLAG_AIRBORNE, FLAG_CROUCH,
        FLAG_RELOADING, GameMode, MAX_HEALTH, MAX_PLAYERS, MapId, Phase, PlayerId, PlayerSnapshot,
        PlayerState, Team, enemies, is_bot, player_color,
    },
    world::{
        CROUCH_DROP, EYE_OFFSET, MovementTuning, PLAYER_GROUND_Y, PLAYER_RADIUS, WorldSystems,
    },
};
use bevy::prelude::*;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    f32::consts::{PI, TAU},
};

pub struct BotsPlugin;

impl Plugin for BotsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BotIds>()
            .init_resource::<Bots>()
            .add_systems(OnExit(AppState::InGame), clear_bots)
            .add_systems(
                Update,
                (build_nav, manage_roster, apply_bot_hits, run_bots)
                    .chain()
                    .after(WorldSystems::Net)
                    .before(WorldSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// The ids of the host's bots (empty when not hosting), for combat to know
/// whose paint the host confirms.
#[derive(Resource, Default)]
pub struct BotIds(pub HashSet<PlayerId>);

/// Names bots play under (shown as "BOT <name>").
const NAMES: &[&str] = &[
    "Viper", "Moss", "Juniper", "Dash", "Pixel", "Rocco", "Blitz", "Nova", "Tango", "Pepper",
    "Wren", "Ziggy", "Ember", "Gizmo", "Sable", "Tilly", "Brick", "Kiwi", "Rook", "Pogo",
];

/// Size of a navigation grid cell, meters.
const NAV_CELL: f32 = 1.0;
/// Seconds between a bot's looks around (staggered between bots).
const THINK_EVERY: f32 = 0.1;
/// How close counts as having reached a path point.
const WAYPOINT_REACHED: f32 = 0.55;
/// Stride length: one footstep every this many meters.
const STRIDE: f32 = 1.7;

/// How well a level of bot plays.
#[derive(Clone, Copy, Debug)]
struct Skill {
    /// Seconds from first seeing someone to the first shot.
    reaction: f32,
    /// Typical aim error when a target is first seen, radians.
    aim_error: f32,
    /// How fast that error settles while tracking (per second).
    settle: f32,
    /// Turn speed, radians per second.
    turn: f32,
    /// How much of a target's movement it leads (0..=1).
    lead: f32,
    /// Shots in a burst, and the pause between bursts.
    burst: u32,
    burst_pause: f32,
    /// Trigger pulls are this much slower than the gun allows.
    fire_slowdown: f32,
    /// How far it sees, meters.
    sight: f32,
    /// Health below which getting hit sends it to cover.
    cover_health: f32,
}

impl Skill {
    fn of(level: BotLevel) -> Self {
        match level {
            BotLevel::Easy | BotLevel::Off => Skill {
                reaction: 0.7,
                aim_error: 0.11,
                settle: 1.2,
                turn: 3.2,
                lead: 0.25,
                burst: 3,
                burst_pause: 0.7,
                fire_slowdown: 1.7,
                sight: 38.0,
                cover_health: 35.0,
            },
            BotLevel::Normal => Skill {
                reaction: 0.42,
                aim_error: 0.065,
                settle: 2.2,
                turn: 5.0,
                lead: 0.65,
                burst: 5,
                burst_pause: 0.4,
                fire_slowdown: 1.25,
                sight: 52.0,
                cover_health: 55.0,
            },
            BotLevel::Hard => Skill {
                reaction: 0.24,
                aim_error: 0.035,
                settle: 3.4,
                turn: 8.0,
                lead: 0.95,
                burst: 9,
                burst_pause: 0.18,
                fire_slowdown: 1.05,
                sight: 70.0,
                cover_health: 65.0,
            },
        }
    }
}

/// What a bot is up to when it isn't simply shooting at what it sees.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cover {
    /// Not using cover.
    None,
    /// Heading to (then crouched behind) cover at this spot until the time.
    Hide { spot: Vec2, until: f32 },
    /// Standing up at the cover spot to shoot, until the time.
    Peek { spot: Vec2, until: f32 },
}

struct Bot {
    id: PlayerId,
    skill: Skill,
    stats: WeaponStats,
    /// Body center, like a player's transform.
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    velocity_y: f32,
    grounded: bool,
    ground_velocity: Vec3,
    /// 0 standing to 1 crouched, eased.
    crouch: f32,
    state: PlayerState,
    ammo: u32,
    reloading: Option<f32>,
    cooldown: f32,
    burst_left: u32,
    last_damage: f32,
    dead_for: f32,
    blind_until: f32,

    // Brain.
    think_in: f32,
    target: Option<PlayerId>,
    /// When the current target was first seen (this sighting), and last seen.
    seen_since: f32,
    seen_at: f32,
    /// Where an enemy was last seen or heard from.
    last_known: Option<(Vec3, f32)>,
    /// Aim offset (yaw, pitch) from a perfect shot; settles while tracking.
    aim_error: Vec2,
    wobble_phase: f32,
    cover: Cover,
    cover_check_in: f32,
    /// Roaming goal and the path to it (or to whatever it's heading to).
    goal: Option<Vec2>,
    goal_until: f32,
    path: Vec<Vec2>,
    path_to: Option<Vec2>,
    repath_in: f32,
    strafe: f32,
    strafe_in: f32,
    /// Where it was a moment ago, to notice being stuck.
    stuck_from: Vec3,
    stuck_for: f32,
    stride: f32,
}

#[derive(Resource)]
struct Bots {
    list: Vec<Bot>,
    nav: Nav,
    /// The match `resets` the bots last went back to spawn for.
    resets: Option<u32>,
    rng: u64,
    /// Each bot id's eliminations so far, kept when it leaves so a bot that
    /// comes back under the same id doesn't reuse a body's number.
    deaths: HashMap<PlayerId, u32>,
    /// Everyone's smoothed velocity, for leading shots.
    tracks: HashMap<PlayerId, (Vec3, Vec3)>,
    next_name: usize,
}

impl Default for Bots {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            nav: Nav::default(),
            resets: None,
            rng: random_seed() | 1,
            deaths: HashMap::new(),
            tracks: HashMap::new(),
            next_name: (random_seed() % NAMES.len() as u64) as usize,
        }
    }
}

impl Bots {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rand()
    }

    fn pick(&mut self, n: usize) -> usize {
        ((self.rand() * n as f32) as usize).min(n.saturating_sub(1))
    }
}

fn clear_bots(mut bots: ResMut<Bots>, mut ids: ResMut<BotIds>) {
    bots.list.clear();
    bots.resets = None;
    bots.tracks.clear();
    ids.0.clear();
}

// ---------------------------------------------------------------------------
// Navigation grid
// ---------------------------------------------------------------------------

/// Which ground cells a player fits in, for finding paths around bunkers.
#[derive(Default)]
struct Nav {
    map: Option<MapId>,
    min: Vec2,
    w: usize,
    h: usize,
    open: Vec<bool>,
}

impl Nav {
    fn build(map: &MapInfo, colliders: &Colliders) -> Self {
        let limit = map.half - Vec2::splat(PLAYER_RADIUS + 0.15);
        Self::build_with(map.id, limit, |p| {
            let mut q = Vec3::new(p.x, PLAYER_GROUND_Y, p.y);
            // A little wider than a player so paths keep off the walls.
            let floor = colliders.resolve_player(&mut q, PLAYER_RADIUS + 0.12, 0.0, 1.8);
            (q.xz() - p).length_squared() < 1e-4 && floor <= 0.3
        })
    }

    fn build_with(id: MapId, limit: Vec2, fits: impl Fn(Vec2) -> bool) -> Self {
        let w = ((limit.x * 2.0) / NAV_CELL).floor().max(1.0) as usize;
        let h = ((limit.y * 2.0) / NAV_CELL).floor().max(1.0) as usize;
        let min = -Vec2::new(w as f32, h as f32) * NAV_CELL * 0.5;
        let mut nav = Self {
            map: Some(id),
            min,
            w,
            h,
            open: vec![false; w * h],
        };
        for y in 0..h {
            for x in 0..w {
                let c = nav.center(x, y);
                nav.open[y * w + x] = fits(c);
            }
        }
        nav
    }

    fn center(&self, x: usize, y: usize) -> Vec2 {
        self.min + (Vec2::new(x as f32, y as f32) + 0.5) * NAV_CELL
    }

    fn cell_of(&self, p: Vec2) -> Option<(usize, usize)> {
        let c = ((p - self.min) / NAV_CELL).floor();
        (c.x >= 0.0 && c.y >= 0.0 && (c.x as usize) < self.w && (c.y as usize) < self.h)
            .then_some((c.x as usize, c.y as usize))
    }

    fn is_open(&self, x: usize, y: usize) -> bool {
        x < self.w && y < self.h && self.open[y * self.w + x]
    }

    fn open_at(&self, p: Vec2) -> bool {
        self.cell_of(p).is_some_and(|(x, y)| self.is_open(x, y))
    }

    /// The open cell nearest `p` (within a few meters).
    fn nearest_open(&self, p: Vec2) -> Option<(usize, usize)> {
        let clamped = p.clamp(self.min, -self.min - Vec2::splat(1e-3));
        let (cx, cy) = self.cell_of(clamped)?;
        if self.is_open(cx, cy) {
            return Some((cx, cy));
        }
        let mut best: Option<((usize, usize), f32)> = None;
        for r in 1..=8i32 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if x < 0 || y < 0 || !self.is_open(x as usize, y as usize) {
                        continue;
                    }
                    let d = self.center(x as usize, y as usize).distance_squared(p);
                    if best.is_none_or(|(_, b)| d < b) {
                        best = Some(((x as usize, y as usize), d));
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|(c, _)| c)
    }

    /// Whether a player can walk straight from `a` to `b`.
    fn clear_line(&self, a: Vec2, b: Vec2) -> bool {
        let d = b - a;
        let steps = (d.length() / (NAV_CELL * 0.4)).ceil().max(1.0) as usize;
        (0..=steps).all(|i| self.open_at(a + d * (i as f32 / steps as f32)))
    }

    /// A walkable path from `from` to `to` (the points to head for in turn,
    /// ending at `to`'s cell), or None when there's no way there.
    fn path(&self, from: Vec2, to: Vec2) -> Option<Vec<Vec2>> {
        let start = self.nearest_open(from)?;
        let goal = self.nearest_open(to)?;
        let idx = |(x, y): (usize, usize)| y * self.w + x;
        let (s, g) = (idx(start), idx(goal));
        let heuristic = |i: usize| {
            let (x, y) = ((i % self.w) as i32, (i / self.w) as i32);
            let (dx, dy) = ((x - goal.0 as i32).abs(), (y - goal.1 as i32).abs());
            (10 * (dx + dy) - 6 * dx.min(dy)) as u32
        };
        let mut cost = vec![u32::MAX; self.open.len()];
        let mut came = vec![usize::MAX; self.open.len()];
        let mut heap = BinaryHeap::new();
        cost[s] = 0;
        heap.push(Reverse((heuristic(s), s)));
        let mut expanded = 0;
        while let Some(Reverse((_, i))) = heap.pop() {
            if i == g {
                break;
            }
            expanded += 1;
            if expanded > 40_000 {
                return None;
            }
            let (x, y) = ((i % self.w) as i32, (i / self.w) as i32);
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || !self.is_open(nx as usize, ny as usize) {
                    continue;
                }
                let diagonal = dx != 0 && dy != 0;
                // No cutting corners past a wall.
                if diagonal
                    && (!self.is_open((x + dx) as usize, y as usize)
                        || !self.is_open(x as usize, (y + dy) as usize))
                {
                    continue;
                }
                let n = ny as usize * self.w + nx as usize;
                let c = cost[i] + if diagonal { 14 } else { 10 };
                if c < cost[n] {
                    cost[n] = c;
                    came[n] = i;
                    heap.push(Reverse((c + heuristic(n), n)));
                }
            }
        }
        if cost[g] == u32::MAX {
            return None;
        }
        let mut cells = vec![g];
        let mut at = g;
        while at != s {
            at = came[at];
            cells.push(at);
        }
        cells.reverse();
        let points: Vec<Vec2> = cells
            .iter()
            .map(|&i| self.center(i % self.w, i / self.w))
            .collect();
        // Skip the corners a straight walk can cut.
        let mut out = Vec::new();
        let mut i = 0;
        let mut here = from;
        while i < points.len() {
            let mut j = (i + 16).min(points.len() - 1);
            while j > i && !self.clear_line(here, points[j]) {
                j -= 1;
            }
            out.push(points[j]);
            here = points[j];
            i = j + 1;
        }
        Some(out)
    }

    fn random_open(&self, rng: &mut Bots) -> Option<Vec2> {
        for _ in 0..40 {
            let x = rng.pick(self.w);
            let y = rng.pick(self.h);
            if self.is_open(x, y) {
                return Some(self.center(x, y));
            }
        }
        None
    }
}

fn build_nav(
    map: Res<MapInfo>,
    colliders: Res<Colliders>,
    net: Res<NetworkState>,
    mut bots: ResMut<Bots>,
) {
    if net.active_role() != Some(NetRole::Host) {
        return;
    }
    if bots.nav.map != Some(map.id) || colliders.is_changed() {
        bots.nav = Nav::build(&map, &colliders);
        for bot in &mut bots.list {
            bot.path.clear();
            bot.path_to = None;
            bot.goal = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Who plays
// ---------------------------------------------------------------------------

/// How many bots each team (blue, orange) wants, given the people on each:
/// up to `MAX_PLAYERS` in all, filling the smaller team first, at most half
/// of `MAX_PLAYERS` a team. In free-for-all, teams only pick colors.
fn wanted_bots(teams: bool, people: [usize; 2]) -> [usize; 2] {
    let mut slots = MAX_PLAYERS.saturating_sub(people[0] + people[1]);
    if !teams {
        return [slots.div_ceil(2), slots / 2];
    }
    let per_team = MAX_PLAYERS / 2;
    let mut have = people;
    let mut bots = [0, 0];
    while slots > 0 {
        let t = if have[0] <= have[1] { 0 } else { 1 };
        if have[t] >= per_team {
            break;
        }
        have[t] += 1;
        bots[t] += 1;
        slots -= 1;
    }
    bots
}

fn random_loadout(bots: &mut Bots) -> Loadout {
    let mut l = Loadout::default();
    if bots.rand() < 0.12 {
        l.kind = GunKind::Water;
        let n = Slot::Water.parts().len();
        l.water = bots.pick(n) as u8;
        return l.sanitized();
    }
    for slot in Slot::PAINTBALL {
        let n = slot.parts().len();
        let v = bots.pick(n) as u8;
        l.set(slot, v);
    }
    l.sanitized()
}

#[allow(clippy::too_many_arguments)]
fn manage_roster(
    time: Res<Time>,
    map: Res<MapInfo>,
    mut net: ResMut<NetworkState>,
    mut bots: ResMut<Bots>,
    mut ids: ResMut<BotIds>,
) {
    let hosting = net.active_role() == Some(NetRole::Host) && net.local_player_id.is_some();
    let settings = net.match_state.settings;
    let level = settings.effective_bots();
    if !hosting || level == BotLevel::Off {
        if !bots.list.is_empty() {
            let gone: Vec<PlayerId> = bots.list.drain(..).map(|b| b.id).collect();
            for id in gone {
                net.remove_bot(id);
            }
            ids.0.clear();
        }
        return;
    }
    // Wait for the match's map (spawns and paths come from it).
    if map.id != settings.map || bots.nav.map != Some(map.id) {
        return;
    }

    let teams = settings.mode.teams();
    let mut people = [0usize; 2];
    for &id in net.players.keys() {
        if !is_bot(id) {
            people[Team::of(id).index()] += 1;
        }
    }
    let wanted = wanted_bots(teams, people);

    // Too many: the dead go first, then the newest.
    for team in [Team::Blue, Team::Orange] {
        let want = wanted[team.index()];
        loop {
            let on_team: Vec<usize> = (0..bots.list.len())
                .filter(|&i| Team::of(bots.list[i].id) == team)
                .collect();
            if on_team.len() <= want {
                break;
            }
            let Some(&i) = on_team
                .iter()
                .max_by_key(|&&i| (bots.list[i].state.health <= 0.0, bots.list[i].id))
            else {
                break;
            };
            let bot = bots.list.remove(i);
            bots.deaths.insert(bot.id, bot.state.deaths);
            ids.0.remove(&bot.id);
            net.remove_bot(bot.id);
        }
    }

    // Too few: one more each frame until there are enough.
    let now = time.elapsed_secs();
    for team in [Team::Blue, Team::Orange] {
        let have = bots.list.iter().filter(|b| Team::of(b.id) == team).count();
        if have >= wanted[team.index()] {
            continue;
        }
        let parity = team.index() as PlayerId;
        let Some(id) = (FIRST_BOT_ID..BOT_ID_END)
            .filter(|id| id % 2 == parity)
            .find(|id| !net.players.contains_key(id))
        else {
            continue;
        };
        let loadout = random_loadout(&mut bots);
        let stats = loadout.stats();
        let name = format!("BOT {}", NAMES[bots.next_name % NAMES.len()]);
        bots.next_name += 1;
        let (pos, yaw) = pick_spawn(&map, &net, id, now as u32);
        // Joining an Elimination round already under way: out until the next one.
        let ms = &net.match_state;
        let sit_out = ms.settings.mode == GameMode::Elimination
            && matches!(ms.phase, Phase::Live | Phase::RoundOver);
        let state = PlayerState {
            health: if sit_out { 0.0 } else { MAX_HEALTH },
            loadout: loadout.pack(),
            deaths: bots.deaths.get(&id).copied().unwrap_or(0),
            killer: id,
            ..default()
        };
        let wobble_phase = bots.range(0.0, TAU);
        let think_in = bots.range(0.0, THINK_EVERY);
        bots.list.push(new_bot(
            id,
            level,
            stats,
            pos,
            yaw,
            state,
            wobble_phase,
            think_in,
        ));
        ids.0.insert(id);
        let snapshot = PlayerSnapshot {
            id,
            position: pos.to_array(),
            yaw,
            color: player_color(id),
            state,
        };
        net.add_bot(snapshot, &name);
    }
}

#[allow(clippy::too_many_arguments)]
fn new_bot(
    id: PlayerId,
    level: BotLevel,
    stats: WeaponStats,
    pos: Vec3,
    yaw: f32,
    state: PlayerState,
    wobble_phase: f32,
    think_in: f32,
) -> Bot {
    let skill = Skill::of(level);
    Bot {
        id,
        skill,
        stats,
        pos,
        yaw,
        pitch: 0.0,
        velocity_y: 0.0,
        grounded: true,
        ground_velocity: Vec3::ZERO,
        crouch: 0.0,
        state,
        ammo: stats.capacity,
        reloading: None,
        cooldown: 0.0,
        burst_left: skill.burst,
        last_damage: -100.0,
        dead_for: 0.0,
        blind_until: 0.0,
        think_in,
        target: None,
        seen_since: 0.0,
        seen_at: -100.0,
        last_known: None,
        aim_error: Vec2::ZERO,
        wobble_phase,
        cover: Cover::None,
        cover_check_in: 0.0,
        goal: None,
        goal_until: 0.0,
        path: Vec::new(),
        path_to: None,
        repath_in: 0.0,
        strafe: 1.0,
        strafe_in: 0.0,
        stuck_from: pos,
        stuck_for: 0.0,
        stride: 0.0,
    }
}

// ---------------------------------------------------------------------------
// Getting hit
// ---------------------------------------------------------------------------

fn apply_bot_hits(time: Res<Time>, mut net: ResMut<NetworkState>, mut bots: ResMut<Bots>) {
    if net.pending_bot_hits.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let hits = std::mem::take(&mut net.pending_bot_hits);
    for hit in hits {
        let attacker_pos = net
            .players
            .get(&hit.attacker)
            .map(|p| Vec3::from_array(p.position));
        let flinch = Vec2::new(bots.range(-1.0, 1.0), bots.range(-0.6, 0.6));
        let Some(bot) = bots.list.iter_mut().find(|b| b.id == hit.victim) else {
            continue;
        };
        if bot.state.health <= 0.0 || !enemies(hit.attacker, bot.id) {
            continue;
        }
        if let Some(p) = attacker_pos {
            bot.last_known = Some((p, now));
        }
        if hit.flash {
            bot.blind_until = now + 2.2;
            continue;
        }
        if !hit.damage.is_finite() || hit.damage <= 0.0 {
            continue;
        }
        bot.state.health = (bot.state.health - hit.damage.min(150.0)).max(0.0);
        bot.last_damage = now;
        bot.aim_error += flinch * bot.skill.aim_error * 0.8;
        if bot.state.health > 0.0 {
            continue;
        }
        // Eliminated: everyone's game drops a body when `deaths` goes up.
        bot.state.deaths = bot.state.deaths.wrapping_add(1);
        bot.state.killer = hit.attacker;
        bot.reloading = None;
        bot.dead_for = 0.0;
        bot.target = None;
        bot.cover = Cover::None;
        bot.path.clear();
        bot.path_to = None;
    }
}

// ---------------------------------------------------------------------------
// Thinking, moving and shooting
// ---------------------------------------------------------------------------

/// A player as the bots see them this frame.
#[derive(Clone, Copy)]
struct Seen {
    id: PlayerId,
    center: Vec3,
    crouched: bool,
    alive: bool,
    velocity: Vec3,
}

fn eye_of(center: Vec3, crouched: bool) -> Vec3 {
    center + Vec3::Y * (EYE_OFFSET - if crouched { CROUCH_DROP } else { 0.0 })
}

/// The middle of the body, where a bot aims.
fn chest_of(center: Vec3, crouched: bool) -> Vec3 {
    if crouched {
        center - Vec3::Y * 0.2
    } else {
        center + Vec3::Y * 0.2
    }
}

/// Whether nothing solid is between `from` and `to`.
fn clear_shot(colliders: &Colliders, from: Vec3, to: Vec3) -> bool {
    colliders.segment_hit(from, to).is_none()
}

/// The direction to fire so a ball (muzzle speed `speed`, falling at
/// `gravity`) from `origin` meets a target at `target` moving at `velocity`
/// (`lead` 0..=1 of it).
fn aim_direction(
    origin: Vec3,
    target: Vec3,
    velocity: Vec3,
    speed: f32,
    gravity: f32,
    lead: f32,
) -> Vec3 {
    let mut aim = target;
    for _ in 0..4 {
        let t = (aim - origin).length() / speed.max(1.0);
        aim = target + velocity * lead * t + Vec3::Y * (0.5 * gravity * t * t);
    }
    (aim - origin).normalize_or(Vec3::NEG_Z)
}

/// Yaw and pitch that look along `dir` (yaw 0 looks down -Z).
fn look_angles(dir: Vec3) -> (f32, f32) {
    let yaw = (-dir.x).atan2(-dir.z);
    let pitch = dir.y.clamp(-1.0, 1.0).asin();
    (yaw, pitch)
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (b - a) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

fn turn_toward(from: f32, to: f32, max: f32) -> f32 {
    let d = angle_diff(from, to);
    from + d.clamp(-max, max)
}

/// Somewhere to head when there's nobody to fight: toward the other team's
/// side in team modes, anywhere in free-for-all.
fn roam_goal(bots: &mut Bots, map: &MapInfo, id: PlayerId, teams: bool) -> Option<Vec2> {
    if !teams {
        let nav = std::mem::take(&mut bots.nav);
        let p = nav.random_open(bots);
        bots.nav = nav;
        return p;
    }
    let centroid = |list: &[Vec3]| {
        (!list.is_empty()).then(|| list.iter().map(|p| p.xz()).sum::<Vec2>() / list.len() as f32)
    };
    let team = Team::of(id).index();
    let own = centroid(&map.spawns[team]).unwrap_or(Vec2::ZERO);
    let enemy = centroid(&map.spawns[1 - team]).unwrap_or(-own);
    let along = enemy - own;
    let side = along.perp().normalize_or(Vec2::X);
    let u = bots.range(0.3, 1.0);
    let spread = map.half.max_element() * 0.55;
    let off = bots.range(-1.0, 1.0) * spread;
    let p = own + along * u + side * off;
    let (x, y) = bots.nav.nearest_open(p)?;
    Some(bots.nav.center(x, y))
}

/// A spot near `bot_at` that's hidden from `threat` (crouched), ideally one
/// to stand up and shoot from.
fn find_cover(nav: &Nav, colliders: &Colliders, bot_at: Vec2, threat_eye: Vec3) -> Option<Vec2> {
    let (cx, cy) = nav.cell_of(bot_at)?;
    let reach = 9i32;
    let mut best: Option<(Vec2, f32)> = None;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (x, y) = (cx as i32 + dx, cy as i32 + dy);
            if x < 0 || y < 0 || !nav.is_open(x as usize, y as usize) {
                continue;
            }
            let c = nav.center(x as usize, y as usize);
            let dist = c.distance(bot_at);
            if dist > reach as f32 || c.distance(threat_eye.xz()) < 5.0 {
                continue;
            }
            let low = Vec3::new(c.x, 0.5, c.y);
            let high = Vec3::new(c.x, 0.95, c.y);
            if clear_shot(colliders, threat_eye, low) || clear_shot(colliders, threat_eye, high) {
                continue;
            }
            let peek = Vec3::new(c.x, 1.55, c.y);
            let score = dist
                + if clear_shot(colliders, threat_eye, peek) {
                    0.0
                } else {
                    4.0
                };
            if best.is_none_or(|(_, b)| score < b) {
                best = Some((c, score));
            }
        }
    }
    best.map(|(c, _)| c)
}

fn respawn(bot: &mut Bot, pos: Vec3, yaw: f32) {
    bot.pos = pos;
    bot.yaw = yaw;
    bot.pitch = 0.0;
    bot.velocity_y = 0.0;
    bot.grounded = true;
    bot.ground_velocity = Vec3::ZERO;
    bot.crouch = 0.0;
    bot.state.health = MAX_HEALTH;
    bot.ammo = bot.stats.capacity;
    bot.reloading = None;
    bot.cooldown = 0.0;
    bot.burst_left = bot.skill.burst;
    bot.last_damage = -100.0;
    bot.dead_for = 0.0;
    bot.target = None;
    bot.last_known = None;
    bot.cover = Cover::None;
    bot.goal = None;
    bot.path.clear();
    bot.path_to = None;
    bot.stuck_from = pos;
    bot.stuck_for = 0.0;
}

#[allow(clippy::too_many_arguments)]
fn run_bots(
    time: Res<Time>,
    map: Res<MapInfo>,
    colliders: Res<Colliders>,
    mut net: ResMut<NetworkState>,
    mut bots: ResMut<Bots>,
) {
    if bots.list.is_empty() {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    let ms = net.match_state.clone();
    if map.id != ms.settings.map {
        return;
    }
    let bots = &mut *bots;

    // Back to spawn when a countdown starts (and sit out a round joined late).
    if bots.resets != Some(ms.resets) {
        let first = bots.resets.is_none();
        bots.resets = Some(ms.resets);
        let mid_round = ms.settings.mode == GameMode::Elimination
            && matches!(ms.phase, Phase::Live | Phase::RoundOver);
        for i in 0..bots.list.len() {
            let id = bots.list[i].id;
            let salt = bots.list[i].state.deaths.wrapping_add(ms.resets);
            let (pos, yaw) = pick_spawn(&map, &net, id, salt);
            let bot = &mut bots.list[i];
            if first && mid_round {
                bot.state.health = 0.0;
                bot.pos = pos;
            } else {
                respawn(bot, pos, yaw);
            }
        }
    }

    // Everyone's whereabouts (and how fast they're going, for leading shots).
    let mut seen: Vec<Seen> = Vec::with_capacity(net.players.len());
    for p in net.players.values() {
        let center = Vec3::from_array(p.position);
        let track = bots.tracks.entry(p.id).or_insert((center, Vec3::ZERO));
        if dt > 0.0 {
            let raw = ((center - track.0) / dt).clamp_length_max(14.0);
            let raw = if raw.length() > 13.0 { Vec3::ZERO } else { raw };
            track.1 = track.1.lerp(raw, 1.0 - (-10.0 * dt).exp());
        }
        track.0 = center;
        seen.push(Seen {
            id: p.id,
            center,
            crouched: p.state.flags & FLAG_CROUCH != 0,
            alive: p.state.health > 0.0,
            velocity: track.1,
        });
    }
    let present: HashSet<PlayerId> = seen.iter().map(|s| s.id).collect();
    bots.tracks.retain(|id, _| present.contains(id));

    let tuning = MovementTuning::of(ms.settings.movement);
    let frozen = ms.frozen();
    let teams = ms.settings.mode.teams();
    let respawn_after = ms.settings.effective_respawn_seconds();
    let mut out = Vec::with_capacity(bots.list.len());

    for i in 0..bots.list.len() {
        // Dead: wait to come back.
        if bots.list[i].state.health <= 0.0 {
            let bot = &mut bots.list[i];
            bot.dead_for += dt;
            bot.state.flags = 0;
            if ms.respawns() && bot.dead_for >= respawn_after && !frozen {
                let id = bot.id;
                let salt = bot.state.deaths.wrapping_add(ms.resets);
                let (pos, yaw) = pick_spawn(&map, &net, id, salt);
                respawn(&mut bots.list[i], pos, yaw);
            }
            out.push(snapshot_of(&bots.list[i]));
            continue;
        }

        // Health comes back like a player's.
        {
            let bot = &mut bots.list[i];
            if now - bot.last_damage >= REGEN_DELAY && bot.state.health < MAX_HEALTH {
                bot.state.health = (bot.state.health + REGEN_RATE * dt).min(MAX_HEALTH);
            }
            bot.cooldown = (bot.cooldown - dt).max(-0.05);
            if let Some(left) = bot.reloading {
                let left = left - dt;
                if left <= 0.0 {
                    bot.reloading = None;
                    bot.ammo = bot.stats.capacity;
                } else {
                    bot.reloading = Some(left);
                }
            }
        }

        if frozen {
            let bot = &mut bots.list[i];
            bot.ground_velocity = Vec3::ZERO;
            bot.crouch = 0.0;
            bot.state.flags = 0;
            out.push(snapshot_of(bot));
            continue;
        }

        think(bots, i, &seen, &colliders, now, dt, &map, teams);
        steer(bots, i, &seen, &colliders, now, dt, &map, &tuning);
        shoot(bots, i, &seen, &colliders, now, dt);
        out.push(snapshot_of(&bots.list[i]));
    }

    for snapshot in out {
        net.update_bot(snapshot);
    }
}

fn snapshot_of(bot: &Bot) -> PlayerSnapshot {
    PlayerSnapshot {
        id: bot.id,
        position: bot.pos.to_array(),
        yaw: bot.yaw,
        color: player_color(bot.id),
        state: bot.state,
    }
}

/// Looks around (a few times a second): who to fight, and whether to take cover.
#[allow(clippy::too_many_arguments)]
fn think(
    bots: &mut Bots,
    i: usize,
    seen: &[Seen],
    colliders: &Colliders,
    now: f32,
    dt: f32,
    map: &MapInfo,
    teams: bool,
) {
    let roll = bots.rand();
    let flip = bots.range(0.6, 1.5);
    let hide_for = bots.range(1.4, 2.6);
    let error_dir = Vec2::from_angle(bots.range(0.0, TAU));
    let error_size = bots.range(1.0, 1.9);
    let bot = &mut bots.list[i];
    bot.strafe_in -= dt;
    if bot.strafe_in <= 0.0 {
        bot.strafe_in = flip;
        if roll < 0.6 {
            bot.strafe = -bot.strafe;
        }
    }
    bot.think_in -= dt;
    if bot.think_in > 0.0 {
        return;
    }
    bot.think_in = THINK_EVERY;

    let crouched = bot.crouch > 0.5;
    let eye = eye_of(bot.pos, crouched);
    let facing = Vec3::new(-bot.yaw.sin(), 0.0, -bot.yaw.cos());
    let blind = now < bot.blind_until;
    let alerted = now - bot.last_damage < 2.0;

    // Who can it see?
    let mut best: Option<(PlayerId, f32)> = None;
    if !blind {
        for s in seen {
            if !s.alive || !enemies(bot.id, s.id) {
                continue;
            }
            let to = s.center - bot.pos;
            let dist = to.length();
            if dist > bot.skill.sight {
                continue;
            }
            let in_view = dist < 5.0
                || alerted
                || bot.target == Some(s.id)
                || facing.dot(to.with_y(0.0).normalize_or(facing)) > 0.26;
            if !in_view {
                continue;
            }
            let visible = clear_shot(colliders, eye, eye_of(s.center, s.crouched))
                || clear_shot(colliders, eye, chest_of(s.center, s.crouched));
            if !visible {
                continue;
            }
            let score = dist - if bot.target == Some(s.id) { 8.0 } else { 0.0 };
            if best.is_none_or(|(_, b)| score < b) {
                best = Some((s.id, score));
            }
        }
    }
    match best {
        Some((id, _)) => {
            if bot.target != Some(id) || now - bot.seen_at > 1.0 {
                // A new sighting: it takes a moment to react, and aim starts rough.
                bot.seen_since = now;
                bot.aim_error = error_dir * bot.skill.aim_error * error_size;
            }
            bot.target = Some(id);
            bot.seen_at = now;
            if let Some(s) = seen.iter().find(|s| s.id == id) {
                bot.last_known = Some((s.center, now));
            }
        }
        None => {
            if now - bot.seen_at > 0.6 {
                bot.target = None;
            }
        }
    }

    // Cover: hurt and under fire, or reloading in a fight.
    bot.cover_check_in -= THINK_EVERY;
    let threat = bot
        .last_known
        .filter(|(_, at)| now - at < 4.0)
        .map(|(p, _)| p);
    let hurt = bot.state.health < bot.skill.cover_health && now - bot.last_damage < 1.5;
    let reloading_in_fight = bot.reloading.is_some() && bot.target.is_some();
    match bot.cover {
        Cover::None => {
            if (hurt || reloading_in_fight)
                && bot.cover_check_in <= 0.0
                && let Some(t) = threat
            {
                bot.cover_check_in = 1.0;
                if let Some(spot) = find_cover(&bots.nav, colliders, bot.pos.xz(), eye_of(t, false))
                {
                    let bot = &mut bots.list[i];
                    bot.cover = Cover::Hide {
                        spot,
                        until: now + hide_for,
                    };
                    bot.path_to = None;
                }
            }
        }
        Cover::Hide { spot, until } => {
            let healthy = bot.state.health >= 90.0;
            if now >= until && bot.reloading.is_none() {
                bot.cover = if healthy || threat.is_none() {
                    Cover::None
                } else {
                    Cover::Peek {
                        spot,
                        until: now + hide_for,
                    }
                };
            }
        }
        Cover::Peek { spot, until } => {
            if hurt && now - bot.last_damage < 0.3 || bot.reloading.is_some() {
                bot.cover = Cover::Hide {
                    spot,
                    until: now + hide_for,
                };
            } else if now >= until {
                bot.cover = if bot.state.health >= 90.0 || threat.is_none() {
                    Cover::None
                } else {
                    Cover::Hide {
                        spot,
                        until: now + hide_for * 0.7,
                    }
                };
            }
        }
    }

    // Reload when empty, or between fights.
    let bot = &mut bots.list[i];
    if bot.reloading.is_none()
        && (bot.ammo == 0
            || (bot.ammo < bot.stats.capacity / 3
                && bot.target.is_none()
                && now - bot.seen_at > 1.5))
    {
        bot.reloading = Some(bot.stats.reload);
    }

    // A new place to roam to now and then.
    if bot.target.is_none()
        && bot.cover == Cover::None
        && (bot.goal.is_none() || now > bot.goal_until)
    {
        let id = bot.id;
        let goal = roam_goal(bots, map, id, teams);
        let until = now + bots.range(8.0, 16.0);
        let bot = &mut bots.list[i];
        bot.goal = goal;
        bot.goal_until = until;
    }
}

/// Picks where to go this frame and moves there like a player would.
#[allow(clippy::too_many_arguments)]
fn steer(
    bots: &mut Bots,
    i: usize,
    seen: &[Seen],
    colliders: &Colliders,
    now: f32,
    dt: f32,
    map: &MapInfo,
    tuning: &MovementTuning,
) {
    let jump_roll = bots.rand();
    let bot = &mut bots.list[i];
    let target = bot
        .target
        .and_then(|id| seen.iter().find(|s| s.id == id))
        .copied();
    let preferred = if bot.stats.water || bot.stats.pellets > 1 {
        9.0
    } else if bot.stats.speed > 120.0 {
        40.0
    } else {
        22.0
    };

    // Where to, and how.
    let mut destination: Option<Vec2> = None;
    let mut wish = Vec2::ZERO;
    let mut crouch = false;
    let mut sprint = false;
    match bot.cover {
        Cover::Hide { spot, .. } => {
            if bot.pos.xz().distance(spot) > 0.4 {
                destination = Some(spot);
                sprint = true;
            } else {
                crouch = true;
            }
        }
        Cover::Peek { spot, .. } => {
            if bot.pos.xz().distance(spot) > 0.6 {
                destination = Some(spot);
            }
        }
        Cover::None => {
            if let Some(t) = target {
                let to = (t.center - bot.pos).xz();
                let dist = to.length();
                let dir = to.normalize_or(Vec2::Y);
                if dist > preferred * 1.5 {
                    destination = Some(t.center.xz());
                } else {
                    // Fight: strafe, and keep about the right distance.
                    let side = dir.perp() * bot.strafe;
                    let range = if dist > preferred * 1.1 {
                        0.6
                    } else if dist < preferred * 0.5 {
                        -0.6
                    } else {
                        0.0
                    };
                    wish = (side + dir * range).normalize_or_zero();
                    let ahead = bot.pos.xz() + wish * 1.0;
                    if !bots.nav.open_at(ahead) {
                        bot.strafe = -bot.strafe;
                        wish = (dir.perp() * bot.strafe + dir * range).normalize_or_zero();
                        if !bots.nav.open_at(bot.pos.xz() + wish * 1.0) {
                            wish = Vec2::ZERO;
                        }
                    }
                }
            } else if let Some((p, at)) = bot.last_known.filter(|(_, at)| now - at < 7.0) {
                // Go look where they were.
                if bot.pos.xz().distance(p.xz()) > 1.5 {
                    destination = Some(p.xz());
                } else if now - at > 1.0 {
                    bot.last_known = None;
                }
            } else if let Some(goal) = bot.goal {
                if bot.pos.xz().distance(goal) < 1.5 {
                    bot.goal = None;
                } else {
                    destination = Some(goal);
                    sprint = true;
                }
            }
        }
    }

    // Follow a path there.
    bot.repath_in -= dt;
    if let Some(dest) = destination {
        let stale = bot.path_to.is_none_or(|p| p.distance(dest) > 2.0) || bot.path.is_empty();
        if stale && bot.repath_in <= 0.0 {
            bot.repath_in = 0.4;
            bot.path = bots.nav.path(bot.pos.xz(), dest).unwrap_or_default();
            bot.path_to = Some(dest);
            if bot.path.is_empty() {
                bot.goal = None;
            }
        }
        while let Some(&next) = bot.path.first() {
            if bot.pos.xz().distance(next) < WAYPOINT_REACHED && bot.path.len() > 1 {
                bot.path.remove(0);
            } else {
                break;
            }
        }
        if let Some(&next) = bot.path.first() {
            let d = next - bot.pos.xz();
            if d.length() > 0.15 {
                wish = d.normalize();
            } else {
                bot.path.clear();
            }
        }
    } else {
        bot.path.clear();
        bot.path_to = None;
    }

    // Stuck on something: hop, then try another way.
    let trying = wish.length_squared() > 0.0;
    if trying {
        bot.stuck_for += dt;
        if bot.stuck_for > 0.7 {
            let moved = (bot.pos - bot.stuck_from).xz().length();
            if moved < 0.35 {
                if bot.grounded && jump_roll < 0.6 {
                    bot.velocity_y = tuning.jump;
                    bot.grounded = false;
                }
                bot.path.clear();
                bot.path_to = None;
                bot.repath_in = 0.0;
                if bot.stuck_for > 2.0 {
                    bot.goal = None;
                    bot.strafe = -bot.strafe;
                }
            } else {
                bot.stuck_for = 0.0;
                bot.stuck_from = bot.pos;
            }
        }
    } else {
        bot.stuck_for = 0.0;
        bot.stuck_from = bot.pos;
    }

    // Facing: at the target while fighting, else where it's going.
    let aiming = target.is_some() && !matches!(bot.cover, Cover::Hide { .. });
    if !aiming {
        let look = if wish.length_squared() > 0.0 {
            Some(wish)
        } else {
            bot.last_known
                .map(|(p, _)| (p - bot.pos).xz())
                .filter(|v| v.length() > 0.5)
        };
        if let Some(look) = look {
            let want = (-look.x).atan2(-look.y);
            bot.yaw = turn_toward(bot.yaw, want, bot.skill.turn * 0.7 * dt);
        }
        bot.pitch += (0.0 - bot.pitch) * (1.0 - (-4.0 * dt).exp());
    }

    // Move like a player: speeds, gravity and pushing out of bunkers.
    let crouch_target = if crouch { 1.0 } else { 0.0 };
    bot.crouch += (crouch_target - bot.crouch) * (1.0 - (-14.0 * dt).exp());
    let mut speed = if sprint && !aiming && !crouch {
        tuning.sprint
    } else {
        tuning.walk
    };
    speed *= bot.stats.mobility;
    if aiming {
        speed *= 0.6;
    }
    if bot.crouch > 0.5 {
        speed *= 0.5;
    }
    let wish_velocity = Vec3::new(wish.x, 0.0, wish.y) * speed;
    if bot.grounded || tuning.air_control {
        bot.ground_velocity = wish_velocity;
    }
    let before = bot.pos;
    let steps = (dt / (1.0 / 120.0)).ceil().max(1.0);
    let step = dt / steps;
    let limit = map.half - Vec2::splat(PLAYER_RADIUS + 0.15);
    for _ in 0..steps as u32 {
        bot.pos += bot.ground_velocity * step;
        bot.pos.y += bot.velocity_y * step - 0.5 * tuning.gravity * step * step;
        bot.velocity_y -= tuning.gravity * step;
        let mut pos = bot.pos;
        let feet = pos.y - PLAYER_GROUND_Y;
        let top = pos.y + PLAYER_GROUND_Y;
        let floor = colliders.resolve_player(&mut pos, PLAYER_RADIUS, feet, top);
        bot.pos = pos;
        let ground = floor + PLAYER_GROUND_Y;
        if bot.pos.y <= ground {
            bot.pos.y = ground;
            bot.velocity_y = 0.0;
            bot.grounded = true;
        } else if bot.pos.y > ground + 0.05 {
            bot.grounded = false;
        }
        bot.pos.x = bot.pos.x.clamp(-limit.x, limit.x);
        bot.pos.z = bot.pos.z.clamp(-limit.y, limit.y);
    }

    // Footsteps others hear.
    let moved = (bot.pos - before).with_y(0.0).length();
    if bot.grounded {
        bot.stride += moved;
        if bot.stride >= STRIDE {
            bot.stride -= STRIDE;
            bot.state.steps = bot.state.steps.wrapping_add(1);
        }
    }

    let mut flags = 0;
    if bot.crouch > 0.5 {
        flags |= FLAG_CROUCH;
    }
    if !bot.grounded {
        flags |= FLAG_AIRBORNE;
    }
    if aiming && target.is_some_and(|t| t.center.distance(bot.pos) > 12.0) {
        flags |= FLAG_AIMING;
    }
    if bot.reloading.is_some() {
        flags |= FLAG_RELOADING;
    }
    bot.state.flags = flags;
}

/// Turns toward the target with a little human error, and pulls the trigger.
fn shoot(bots: &mut Bots, i: usize, seen: &[Seen], colliders: &Colliders, now: f32, dt: f32) {
    let pause_roll = bots.range(0.7, 1.3);
    let bot = &mut bots.list[i];
    let Some(target) = bot
        .target
        .and_then(|id| seen.iter().find(|s| s.id == id))
        .copied()
    else {
        return;
    };
    if matches!(bot.cover, Cover::Hide { .. }) || !target.alive {
        return;
    }
    let crouched = bot.crouch > 0.5;
    let (origin, _) = remote_shot_origin(bot.pos, bot.yaw, bot.pitch, crouched);
    let aim_at = chest_of(target.center, target.crouched);
    let dir = aim_direction(
        origin,
        aim_at,
        target.velocity,
        bot.stats.speed,
        bot.stats.gravity,
        bot.skill.lead,
    );
    let (want_yaw, want_pitch) = look_angles(dir);

    // The error settles while it tracks, and never quite goes away.
    bot.aim_error *= (-bot.skill.settle * dt).exp();
    let t = now + bot.wobble_phase;
    let wobble = Vec2::new((t * 1.7).sin(), (t * 2.3).cos() * 0.6) * bot.skill.aim_error * 0.22;
    let error = bot.aim_error + wobble;
    let want_yaw = want_yaw + error.x;
    let want_pitch = (want_pitch + error.y).clamp(-1.4, 1.4);
    let turn = bot.skill.turn * dt;
    bot.yaw = turn_toward(bot.yaw, want_yaw, turn);
    bot.pitch = turn_toward(bot.pitch, want_pitch, turn);
    bot.yaw = bot.yaw.rem_euclid(TAU);

    // Fire once it has reacted, is on target and the shot isn't into a wall.
    let on_target = angle_diff(bot.yaw, want_yaw).abs() < 0.06
        && angle_diff(bot.pitch, want_pitch).abs() < 0.06;
    let reacted = now - bot.seen_since >= bot.skill.reaction;
    let fresh = now - bot.seen_at < 0.25;
    let range_ok = target.center.distance(bot.pos)
        < if bot.stats.water {
            16.0
        } else {
            bot.skill.sight
        };
    if !(on_target && reacted && fresh && range_ok)
        || bot.reloading.is_some()
        || bot.ammo == 0
        || bot.cooldown > 0.0
    {
        return;
    }
    if !clear_shot(colliders, origin, aim_at) {
        return;
    }
    bot.ammo -= 1;
    bot.state.shots = bot.state.shots.wrapping_add(1);
    bot.cooldown += bot.stats.fire_interval * bot.skill.fire_slowdown;
    bot.burst_left = bot.burst_left.saturating_sub(1);
    if bot.burst_left == 0 {
        bot.burst_left = bot.skill.burst;
        bot.cooldown += bot.skill.burst_pause * pause_roll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bots_fill_to_ten_players() {
        // Alone: four bots with the host, five against.
        assert_eq!(wanted_bots(true, [1, 0]), [4, 5]);
        // Full of people: no bots.
        assert_eq!(wanted_bots(true, [5, 5]), [0, 0]);
        // Lopsided teams never go past 10 in all.
        assert_eq!(wanted_bots(true, [7, 3]), [0, 0]);
        assert_eq!(wanted_bots(true, [4, 2]), [1, 3]);
        // Free-for-all: everyone else up to 10.
        let ffa = wanted_bots(false, [1, 1]);
        assert_eq!(ffa[0] + ffa[1], MAX_PLAYERS - 2);
    }

    #[test]
    fn paths_go_around_walls() {
        // A 20 x 20 field with a wall across the middle, open at one end.
        let nav = Nav::build_with(MapId::default(), Vec2::splat(10.0), |p| {
            !(p.y.abs() < 1.0 && p.x < 6.0)
        });
        let from = Vec2::new(0.0, -6.0);
        let to = Vec2::new(0.0, 6.0);
        let path = nav.path(from, to).expect("a way around");
        let mut here = from;
        for &p in &path {
            assert!(nav.clear_line(here, p), "{here} -> {p} crosses the wall");
            here = p;
        }
        assert!(here.distance(to) < 1.0);
        assert!(path.iter().any(|p| p.x > 6.0), "goes round the open end");
        // Walled in completely: no way.
        let shut = Nav::build_with(MapId::default(), Vec2::splat(10.0), |p| p.y.abs() >= 1.0);
        assert!(shut.path(from, to).is_none());
    }

    #[test]
    fn aim_allows_for_drop() {
        // Fire along the solved direction and see the ball land on target.
        let origin = Vec3::new(0.0, 1.5, 0.0);
        let target = Vec3::new(0.0, 1.1, -40.0);
        let (speed, gravity) = (80.0, 9.81);
        let dir = aim_direction(origin, target, Vec3::ZERO, speed, gravity, 1.0);
        assert!(dir.y > 0.0, "aims above a far target");
        let mut p = origin;
        let mut v = dir * speed;
        let dt = 1.0 / 240.0;
        let mut closest = f32::MAX;
        for _ in 0..240 {
            p += v * dt - Vec3::Y * (0.5 * gravity * dt * dt);
            v.y -= gravity * dt;
            closest = closest.min(p.distance(target));
        }
        assert!(closest < 0.2, "missed by {closest}");
    }

    #[test]
    fn bots_can_walk_between_the_spawns_of_every_map() {
        let library = crate::maps::MapLibrary::built_in();
        for id in MapId::BUILT_IN {
            let (colliders, info) = library.layout(id).collision();
            let nav = Nav::build(&info, &colliders);
            let open = nav.open.iter().filter(|&&o| o).count();
            assert!(open * 3 > nav.open.len(), "{id:?}: only {open} open cells");
            for blue in &info.spawns[0] {
                for orange in info.spawns[1].iter().take(2) {
                    let path = nav.path(blue.xz(), orange.xz());
                    assert!(path.is_some(), "{id:?}: no way from {blue} to {orange}");
                }
            }
            // Somewhere on the field hides a bot from the other team's spawn.
            let threat = info.spawns[1][0] + Vec3::Y * EYE_OFFSET;
            let hidden = (0..info.spawns[0].len()).any(|i| {
                find_cover(&nav, &colliders, info.spawns[0][i].xz() * 0.5, threat).is_some()
            });
            assert!(hidden, "{id:?}: no cover anywhere");
        }
    }

    /// Runs two bots, one each side of Splat Park, for `secs` of game time.
    fn duel(secs: f32) -> Bots {
        let library = crate::maps::MapLibrary::built_in();
        let (colliders, info) = library.layout(MapId::SPLAT_PARK).collision();
        let mut bots = Bots {
            nav: Nav::build(&info, &colliders),
            ..default()
        };
        let stats = Loadout::default().stats();
        for (id, spawn) in [
            (FIRST_BOT_ID, info.spawns[0][0]),
            (FIRST_BOT_ID + 1, info.spawns[1][0]),
        ] {
            let pos = Vec3::new(spawn.x, PLAYER_GROUND_Y, spawn.z);
            let state = PlayerState {
                health: MAX_HEALTH,
                ..default()
            };
            bots.list.push(new_bot(
                id,
                BotLevel::Normal,
                stats,
                pos,
                0.0,
                state,
                0.0,
                0.0,
            ));
        }
        let tuning = MovementTuning::of(crate::protocol::MovementSpeed::Normal);
        let dt = 1.0 / 60.0;
        let mut now = 0.0;
        while now < secs {
            let seen: Vec<Seen> = bots
                .list
                .iter()
                .map(|b| Seen {
                    id: b.id,
                    center: b.pos,
                    crouched: b.crouch > 0.5,
                    alive: true,
                    velocity: b.ground_velocity,
                })
                .collect();
            for i in 0..bots.list.len() {
                think(&mut bots, i, &seen, &colliders, now, dt, &info, true);
                steer(&mut bots, i, &seen, &colliders, now, dt, &info, &tuning);
                shoot(&mut bots, i, &seen, &colliders, now, dt);
                let bot = &mut bots.list[i];
                bot.cooldown -= dt;
                if let Some(left) = bot.reloading {
                    bot.reloading = (left > dt).then_some(left - dt);
                    if bot.reloading.is_none() {
                        bot.ammo = bot.stats.capacity;
                    }
                }
            }
            now += dt;
        }
        bots
    }

    #[test]
    fn bots_find_each_other_and_shoot() {
        let bots = duel(40.0);
        for bot in &bots.list {
            assert!(bot.state.steps > 5, "bot {} barely moved", bot.id);
            assert!(bot.state.shots > 0, "bot {} never fired", bot.id);
        }
    }

    #[test]
    fn look_angles_match_the_game() {
        for &(yaw, pitch) in &[(0.0f32, 0.0f32), (1.0, 0.3), (-2.5, -0.4), (3.0, 0.9)] {
            let dir = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Vec3::NEG_Z;
            let (y, p) = look_angles(dir);
            assert!(angle_diff(y, yaw).abs() < 1e-4);
            assert!((p - pitch).abs() < 1e-4);
        }
    }
}
