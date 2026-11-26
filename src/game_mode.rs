//! Match rules for the three modes, plus end-of-match 3-map voting.
//!
//! - **Team Splat**: blue vs orange; configurable time and splat limits.
//! - **Free-for-All**: everyone for themselves; configurable time and splat limits.
//! - **Elimination**: one life per round, configurable rounds-to-win and round
//!   timer (also played Ranked with locked competitive rules).
//!
//! When a match finishes, the host picks 3 random candidate maps and runs a
//! live vote on the podium. Any tie (including 0-0-0 if nobody voted) is broken
//! uniformly at random among the tied maps, then the winning map loads
//! automatically for the next match.

use crate::{
    AppState,
    combat::{Combat, LocalState},
    loadout::Loadout,
    maps::{CurrentMap, MapInfo, MapLibrary},
    net::{NetRole, NetworkState},
    protocol::{
        GameMode, MAX_HEALTH, MapId, MapVote, MatchSettings, MatchState, Phase, PlayerId, Team,
        WIN_BLUE, WIN_DRAW, WIN_NONE, WIN_ORANGE, WIN_PLAYER, enemies, set_free_for_all,
    },
    world::{CameraRig, LocalPlayer, MainCamera, WorldSystems},
};
use bevy::prelude::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

/// Everyone back at spawn, held still, before play starts.
pub const COUNTDOWN_SECONDS: f32 = 5.0;
/// Elimination: default round length in seconds.
#[allow(dead_code)]
pub const ROUND_SECONDS: f32 = crate::protocol::DEFAULT_ROUND_SECONDS;
/// Elimination: the pause after a round is decided.
pub const ROUND_OVER_SECONDS: f32 = 5.0;
/// Seconds players get on the podium to vote among 3 random next maps.
pub const MAP_VOTE_SECONDS: f32 = 20.0;
/// Once every connected player has voted, trim remaining vote time to this cap.
pub const MAP_VOTE_ALL_VOTED_CAP: f32 = 4.0;
/// Seconds the winning map is highlighted before loading into the next match.
pub const MAP_VOTE_REVEAL_SECONDS: f32 = 3.5;
/// How many random map choices are presented after each match.
pub const MAP_VOTE_CHOICES: usize = 3;
/// How often the host resends the clock when nothing else changes.
const RESEND_SECONDS: f32 = 1.0;
/// Most lines on the scoreboard (keeps the update inside one datagram).
const BOARD_MAX: usize = 24;

pub struct GameModePlugin;

impl Plugin for GameModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchSetup>()
            .init_resource::<HostMatch>()
            .init_resource::<MatchTracker>()
            .add_systems(OnEnter(AppState::InGame), reset_tracking)
            .add_systems(OnExit(AppState::InGame), leave_match)
            .add_systems(
                Update,
                (tick_clock, run_host_rules, follow_match)
                    .chain()
                    .after(WorldSystems::Net)
                    .before(WorldSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// What the player picked on the play screen (used when hosting).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct MatchSetup(pub MatchSettings);

/// The host's bookkeeping behind the match state.
#[derive(Resource, Default)]
struct HostMatch {
    /// Each player's `deaths` counter already counted.
    seen_deaths: HashMap<PlayerId, u32>,
    /// Current end-of-match map votes (`player_id -> option index 0..3`).
    votes: HashMap<PlayerId, u8>,
    since_sent: f32,
    last_sent: Option<MatchState>,
}

/// What this player has already acted on.
#[derive(Resource, Default)]
pub struct MatchTracker {
    /// The last `resets` value we went back to spawn for.
    resets: Option<u32>,
}

fn reset_tracking(mut tracker: ResMut<MatchTracker>, mut host: ResMut<HostMatch>) {
    *tracker = MatchTracker::default();
    *host = HostMatch::default();
}

fn leave_match(mut map: ResMut<CurrentMap>, setup: Res<MatchSetup>) {
    set_free_for_all(false);
    // Back on the menu, show the map that's picked there.
    if map.0 != setup.0.map {
        map.0 = setup.0.map;
    }
}

/// SplitMix64 step for crisp, unbiased pseudo-random draws.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Fresh non-deterministic entropy seed from system time + monotonic counter.
pub fn random_seed() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x517C_C1B7_2722_0A95);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    splitmix64(nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// Picks one random map from `pool`, avoiding `exclude` when more than one
/// distinct map is available.
pub fn pick_random_map(pool: &[MapId], exclude: Option<MapId>, seed: u64) -> MapId {
    let mut unique: Vec<MapId> = Vec::with_capacity(pool.len());
    for &id in pool {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return MapId::default();
    }
    if unique.len() > 1
        && let Some(ex) = exclude
    {
        unique.retain(|&id| id != ex);
    }
    let idx = (splitmix64(seed) as usize) % unique.len();
    unique[idx]
}

/// Picks up to 3 distinct random maps from `pool` for end-of-match map voting.
/// When at least 4 distinct maps exist, the just-played map (`exclude`) is
/// omitted so all 3 vote options are fresh maps.
pub fn pick_vote_maps(pool: &[MapId], exclude: Option<MapId>, mut seed: u64) -> Vec<MapId> {
    let mut unique: Vec<MapId> = Vec::with_capacity(pool.len());
    for &id in pool {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return vec![MapId::default()];
    }
    if unique.len() > MAP_VOTE_CHOICES
        && let Some(ex) = exclude
    {
        unique.retain(|&id| id != ex);
    }
    // Fisher-Yates shuffle.
    for i in (1..unique.len()).rev() {
        seed = splitmix64(seed);
        let j = (seed as usize) % (i + 1);
        unique.swap(i, j);
    }
    unique.truncate(MAP_VOTE_CHOICES);
    unique
}

/// Resolves map vote counts into `(winning_index, was_tiebreak)`.
/// If two or more options tie for the highest vote count (including 0-0-0),
/// chooses uniformly at random among ONLY the tied top options.
pub fn resolve_map_vote(counts: &[u16], seed: u64) -> (u8, bool) {
    if counts.is_empty() {
        return (0, false);
    }
    let max_votes = counts.iter().copied().max().unwrap_or(0);
    let tied: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| (c == max_votes).then_some(i))
        .collect();
    if tied.len() <= 1 {
        return (tied.first().copied().unwrap_or(0) as u8, false);
    }
    let pick = (splitmix64(seed) as usize) % tied.len();
    (tied[pick] as u8, true)
}

/// The match clock runs on every screen between the host's updates.
fn tick_clock(time: Res<Time>, mut net: ResMut<NetworkState>) {
    let dt = time.delta_secs();
    let ms = &mut net.match_state;
    if ms.time_left > 0.0 {
        ms.time_left = (ms.time_left - dt).max(0.0);
    }
    set_free_for_all(ms.settings.mode == GameMode::FreeForAll);
}

/// Whether there's someone to play against.
fn enough_players(mode: GameMode, ids: &[PlayerId]) -> bool {
    if mode.teams() {
        ids.iter().any(|&id| Team::of(id) == Team::Blue)
            && ids.iter().any(|&id| Team::of(id) == Team::Orange)
    } else {
        ids.len() >= 2
    }
}

fn board_line(board: &mut Vec<[u32; 3]>, id: PlayerId) -> &mut [u32; 3] {
    if let Some(i) = board.iter().position(|l| l[0] == id) {
        return &mut board[i];
    }
    board.push([id, 0, 0]);
    board.last_mut().unwrap()
}

fn start_countdown(ms: &mut MatchState) {
    ms.phase = Phase::Countdown;
    ms.time_left = COUNTDOWN_SECONDS;
    ms.resets = ms.resets.wrapping_add(1);
}

fn new_match(ms: &mut MatchState) {
    ms.match_id = ms.match_id.wrapping_add(1);
    ms.round = 1;
    ms.score = [0, 0];
    ms.round_winner = WIN_NONE;
    ms.winner = WIN_NONE;
    ms.vote = MapVote::default();
    for line in &mut ms.board {
        line[1] = 0;
        line[2] = 0;
    }
}

fn to_warmup(ms: &mut MatchState) {
    ms.phase = Phase::Warmup;
    ms.time_left = 0.0;
    ms.round = 1;
    ms.score = [0, 0];
    ms.round_winner = WIN_NONE;
    ms.winner = WIN_NONE;
    ms.vote = MapVote::default();
}

fn team_winner(score: [u32; 2]) -> u8 {
    match score[0].cmp(&score[1]) {
        std::cmp::Ordering::Greater => WIN_BLUE,
        std::cmp::Ordering::Less => WIN_ORANGE,
        std::cmp::Ordering::Equal => WIN_DRAW,
    }
}

fn ffa_winner(board: &[[u32; 3]]) -> u8 {
    let Some(best) = board.iter().map(|l| l[1]).max() else {
        return WIN_DRAW;
    };
    let mut top = board.iter().filter(|l| l[1] == best);
    match (top.next(), top.next()) {
        (Some(l), None) => WIN_PLAYER.saturating_add(l[0].min(200) as u8),
        _ => WIN_DRAW,
    }
}

fn end_match(
    ms: &mut MatchState,
    votes: &mut HashMap<PlayerId, u8>,
    map_pool: &[MapId],
    seed: u64,
) {
    ms.winner = match ms.settings.mode {
        GameMode::FreeForAll => ffa_winner(&ms.board),
        _ => team_winner(ms.score),
    };
    ms.phase = Phase::MatchOver;
    ms.time_left = MAP_VOTE_SECONDS;
    votes.clear();
    let options = pick_vote_maps(map_pool, Some(ms.settings.map), seed);
    let counts = vec![0; options.len()];
    ms.vote = MapVote {
        options,
        counts,
        winner: None,
        tiebreak: false,
    };
}

/// One player as the rules see them.
#[derive(Clone, Copy)]
struct Seen {
    id: PlayerId,
    alive: bool,
    health: f32,
    deaths: u32,
    killer: PlayerId,
}

/// Steps the match: counts splats, ends rounds and matches, runs map voting,
/// and starts the next match on the winning map.
fn step_rules(
    ms: &mut MatchState,
    seen_deaths: &mut HashMap<PlayerId, u32>,
    votes: &mut HashMap<PlayerId, u8>,
    players: &[Seen],
    map_pool: &[MapId],
    seed: u64,
) {
    let ids: Vec<PlayerId> = players.iter().map(|p| p.id).collect();
    let mode = ms.settings.mode;
    let foes = |a: PlayerId, b: PlayerId| a != b && (!mode.teams() || Team::of(a) != Team::of(b));

    // Splats: anyone whose deaths counter moved was splatted by `killer`.
    for p in players {
        let seen = seen_deaths.entry(p.id).or_insert(p.deaths);
        let fresh = p.deaths.wrapping_sub(*seen);
        *seen = p.deaths;
        if fresh == 0 || fresh > 10 || !ms.counting() {
            continue;
        }
        for _ in 0..fresh {
            board_line(&mut ms.board, p.id)[2] += 1;
            if foes(p.killer, p.id) {
                board_line(&mut ms.board, p.killer)[1] += 1;
                if mode == GameMode::TeamSplat {
                    ms.score[Team::of(p.killer).index()] += 1;
                }
            }
        }
    }
    seen_deaths.retain(|id, _| ids.contains(id));
    // Everyone playing has a line; those who left don't.
    for &id in &ids {
        board_line(&mut ms.board, id);
    }
    ms.board.retain(|l| ids.contains(&l[0]));
    ms.board
        .sort_by(|a, b| b[1].cmp(&a[1]).then(a[2].cmp(&b[2])).then(a[0].cmp(&b[0])));
    ms.board.truncate(BOARD_MAX);

    let ready = enough_players(mode, &ids);
    let rounds_to_win = ms.settings.effective_rounds_to_win();
    match ms.phase {
        Phase::Warmup => {
            if ready {
                new_match(ms);
                start_countdown(ms);
            }
        }
        Phase::Countdown | Phase::Live | Phase::RoundOver if !ready => to_warmup(ms),
        Phase::Countdown => {
            if ms.time_left <= 0.0 {
                ms.phase = Phase::Live;
                ms.time_left = if mode == GameMode::Elimination {
                    ms.settings.effective_round_seconds()
                } else {
                    ms.settings.effective_match_seconds()
                };
            }
        }
        Phase::Live => {
            if mode == GameMode::Elimination {
                let alive = |team: Team| {
                    players
                        .iter()
                        .filter(|p| p.alive && Team::of(p.id) == team)
                        .count()
                };
                let (blue, orange) = (alive(Team::Blue), alive(Team::Orange));
                let decided = blue == 0 || orange == 0 || ms.time_left <= 0.0;
                if decided {
                    let winner = if blue != orange {
                        if blue > orange { WIN_BLUE } else { WIN_ORANGE }
                    } else {
                        // Time ran out with the same number standing: most paint left wins.
                        let health = |team: Team| -> f32 {
                            players
                                .iter()
                                .filter(|p| p.alive && Team::of(p.id) == team)
                                .map(|p| p.health)
                                .sum()
                        };
                        let (hb, ho) = (health(Team::Blue), health(Team::Orange));
                        if blue == 0 || (hb - ho).abs() < 1.0 {
                            WIN_DRAW
                        } else if hb > ho {
                            WIN_BLUE
                        } else {
                            WIN_ORANGE
                        }
                    };
                    ms.round_winner = winner;
                    match winner {
                        WIN_BLUE => ms.score[0] += 1,
                        WIN_ORANGE => ms.score[1] += 1,
                        _ => {}
                    }
                    if ms.score.iter().any(|&s| s >= rounds_to_win) {
                        end_match(ms, votes, map_pool, seed);
                    } else {
                        ms.phase = Phase::RoundOver;
                        ms.time_left = ROUND_OVER_SECONDS;
                    }
                }
            } else {
                let score_reached = ms.settings.effective_score_limit().is_some_and(|limit| {
                    if mode == GameMode::TeamSplat {
                        ms.score[0] >= limit || ms.score[1] >= limit
                    } else {
                        ms.board.iter().any(|l| l[1] >= limit)
                    }
                });
                if score_reached || ms.time_left <= 0.0 {
                    end_match(ms, votes, map_pool, seed);
                }
            }
        }
        Phase::RoundOver => {
            if ms.time_left <= 0.0 {
                ms.round += 1;
                start_countdown(ms);
            }
        }
        Phase::MatchOver => {
            // Ensure vote options exist (defensive in case MatchOver was entered externally).
            if ms.vote.options.is_empty() {
                let options = pick_vote_maps(map_pool, Some(ms.settings.map), seed);
                ms.vote.counts = vec![0; options.len()];
                ms.vote.options = options;
            }
            votes.retain(|id, choice| {
                ids.contains(id) && (*choice as usize) < ms.vote.options.len()
            });

            if ms.vote.winner.is_none() {
                // Tally live votes.
                let mut counts = vec![0u16; ms.vote.options.len()];
                for &choice in votes.values() {
                    if let Some(slot) = counts.get_mut(choice as usize) {
                        *slot = slot.saturating_add(1);
                    }
                }
                ms.vote.counts = counts;

                // Speed up the countdown slightly once everyone has voted.
                if !ids.is_empty() && votes.len() >= ids.len() && ms.time_left > MAP_VOTE_ALL_VOTED_CAP
                {
                    ms.time_left = MAP_VOTE_ALL_VOTED_CAP;
                }

                if ms.time_left <= 0.0 {
                    let (winner_idx, tiebreak) = resolve_map_vote(&ms.vote.counts, seed);
                    ms.vote.winner = Some(winner_idx);
                    ms.vote.tiebreak = tiebreak;
                    ms.time_left = MAP_VOTE_REVEAL_SECONDS;
                }
            } else if ms.time_left <= 0.0 {
                // Reveal finished: load the winning map and start the next match!
                if let Some(next_map) = ms.vote.winning_map() {
                    ms.settings.map = next_map;
                }
                votes.clear();
                new_match(ms);
                if ready {
                    start_countdown(ms);
                } else {
                    to_warmup(ms);
                    // Bump resets so everyone respawns on the new map's pads.
                    ms.resets = ms.resets.wrapping_add(1);
                }
            }
        }
    }
}

/// Host only: runs the rules, tallies end-of-match map votes, and tells
/// everyone when something changed.
fn run_host_rules(
    time: Res<Time>,
    library: Res<MapLibrary>,
    mut net: ResMut<NetworkState>,
    mut host: ResMut<HostMatch>,
) {
    if net.active_role() != Some(NetRole::Host) || net.local_player_id.is_none() {
        return;
    }

    // Drain any incoming map votes from connected players (and the host).
    let pending_votes = std::mem::take(&mut net.pending_votes);
    for (from, match_id, choice) in pending_votes {
        if net.match_state.phase == Phase::MatchOver
            && net.match_state.vote.winner.is_none()
            && match_id == net.match_state.match_id
            && (choice as usize) < net.match_state.vote.options.len()
            && net.players.contains_key(&from)
        {
            host.votes.insert(from, choice);
        }
    }

    let players: Vec<Seen> = net
        .players
        .values()
        .map(|p| Seen {
            id: p.id,
            alive: p.state.health > 0.0,
            health: p.state.health,
            deaths: p.state.deaths,
            killer: p.state.killer,
        })
        .collect();

    let mut ms = net.match_state.clone();
    // Ranked always draws from the official built-in map pool; private matches
    // include custom maps the host has loaded too.
    let map_pool: Vec<MapId> = if ms.settings.ranked {
        MapId::BUILT_IN.to_vec()
    } else {
        library.ids().collect()
    };
    let seed = random_seed();
    let host = &mut *host;
    step_rules(
        &mut ms,
        &mut host.seen_deaths,
        &mut host.votes,
        &players,
        &map_pool,
        seed,
    );

    host.since_sent += time.delta_secs();
    let changed = host.last_sent.as_ref().is_none_or(|last| {
        let mut a = last.clone();
        a.time_left = ms.time_left;
        a != ms
    });
    if changed || host.since_sent >= RESEND_SECONDS {
        host.since_sent = 0.0;
        net.send_match(&ms);
        host.last_sent = Some(ms.clone());
    }
    net.match_state = ms;
}

/// Where `id` should spawn now: their team's spawns (or anywhere in
/// free-for-all), away from living enemies unless it's a fresh start.
pub fn pick_spawn(map: &MapInfo, net: &NetworkState, id: PlayerId, salt: u32) -> (Vec3, f32) {
    let ms = &net.match_state;
    let ffa = ms.settings.mode == GameMode::FreeForAll;
    let threats: Vec<Vec3> = if ms.phase == Phase::Countdown {
        Vec::new()
    } else {
        net.players
            .values()
            .filter(|p| p.id != id && p.state.health > 0.0 && enemies(p.id, id))
            .map(|p| Vec3::from_array(p.position))
            .collect()
    };
    map.spawn_for(id, ffa, &threats, salt)
}

/// Puts the local player back at a spawn with full health.
#[allow(clippy::too_many_arguments)]
pub fn respawn_local(
    map: &MapInfo,
    net: &NetworkState,
    loadout: &Loadout,
    combat: &mut Combat,
    local_state: &mut LocalState,
    transform: &mut Transform,
    player: &mut LocalPlayer,
    rig: Option<&mut CameraRig>,
) {
    let salt = local_state.0.deaths.wrapping_add(net.match_state.resets);
    let (pos, yaw) = pick_spawn(map, net, player.id, salt);
    transform.translation = pos;
    transform.rotation = Quat::from_rotation_y(yaw);
    player.velocity_y = 0.0;
    player.grounded = true;
    player.yaw = yaw;
    if let Some(rig) = rig {
        rig.yaw = yaw;
        rig.pitch = 0.0;
    }
    local_state.0.health = MAX_HEALTH;
    combat.reset(&loadout.stats());
}

/// Everyone: builds the host's map, goes back to spawn when a countdown
/// starts, and sits out an Elimination round joined halfway.
#[allow(clippy::too_many_arguments)]
fn follow_match(
    net: Res<NetworkState>,
    map: Res<MapInfo>,
    loadout: Res<Loadout>,
    mut current: ResMut<CurrentMap>,
    mut tracker: ResMut<MatchTracker>,
    mut combat: ResMut<Combat>,
    mut local_state: ResMut<LocalState>,
    mut player_q: Query<(&mut Transform, &mut LocalPlayer)>,
    mut rig_q: Query<&mut CameraRig, With<MainCamera>>,
) {
    let ms = &net.match_state;
    if current.0 != ms.settings.map {
        current.0 = ms.settings.map;
    }
    // Wait until the map we're told to play is the one built.
    if map.id != ms.settings.map {
        return;
    }
    let Ok((mut transform, mut player)) = player_q.single_mut() else {
        return;
    };
    let first = tracker.resets.is_none();
    if tracker.resets == Some(ms.resets) {
        return;
    }
    tracker.resets = Some(ms.resets);

    let mid_round = ms.settings.mode == GameMode::Elimination
        && matches!(ms.phase, Phase::Live | Phase::RoundOver);
    if first && mid_round {
        // Joined during a round: watch until the next one.
        let (pos, yaw) = pick_spawn(&map, &net, player.id, 0);
        transform.translation = pos;
        player.yaw = yaw;
        local_state.0.health = 0.0;
        combat.death_cam = Some(pos - Vec3::Y * crate::world::PLAYER_GROUND_Y);
        combat.dead_for = 0.0;
        return;
    }
    let mut rig = rig_q.single_mut().ok();
    respawn_local(
        &map,
        &net,
        &loadout,
        &mut combat,
        &mut local_state,
        &mut transform,
        &mut player,
        rig.as_deref_mut(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MATCH_SECONDS, ROUNDS_TO_WIN};
    use std::collections::HashSet;

    fn seen(id: PlayerId, alive: bool, deaths: u32, killer: PlayerId) -> Seen {
        Seen {
            id,
            alive,
            health: if alive { MAX_HEALTH } else { 0.0 },
            deaths,
            killer,
        }
    }

    fn state(mode: GameMode) -> MatchState {
        MatchState {
            settings: MatchSettings {
                mode,
                map: MapId::DOCKYARD,
                ranked: false,
                ..default()
            },
            ..default()
        }
    }

    #[test]
    fn team_splat_counts_splats_and_ends_with_map_vote_into_next_match() {
        let mut ms = state(GameMode::TeamSplat);
        let mut seen_deaths = HashMap::new();
        let mut votes = HashMap::new();
        let pool = MapId::BUILT_IN;

        // Alone: warmup.
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[seen(0, true, 0, 0)],
            &pool,
            1,
        );
        assert_eq!(ms.phase, Phase::Warmup);

        // Someone on the other team: countdown, then live.
        let both = [seen(0, true, 0, 0), seen(1, true, 0, 0)];
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 2);
        assert_eq!(ms.phase, Phase::Countdown);
        let resets = ms.resets;
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 3);
        assert_eq!(ms.phase, Phase::Live);
        assert_eq!(ms.time_left, MATCH_SECONDS);
        assert_eq!(ms.resets, resets);

        // Blue (0) splats orange (1) twice.
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[seen(0, true, 0, 0), seen(1, false, 2, 0)],
            &pool,
            4,
        );
        assert_eq!(ms.score, [2, 0]);
        assert_eq!(ms.kills(0), 2);
        assert_eq!(ms.deaths(1), 2);

        // Time up: MatchOver with 3 distinct random vote maps!
        ms.time_left = 0.0;
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[seen(0, true, 0, 0), seen(1, true, 2, 0)],
            &pool,
            5,
        );
        assert_eq!(ms.phase, Phase::MatchOver);
        assert_eq!(ms.winner, WIN_BLUE);
        assert_eq!(ms.vote.options.len(), 3);
        let unique: HashSet<MapId> = ms.vote.options.iter().copied().collect();
        assert_eq!(unique.len(), 3);
        assert!(!unique.contains(&MapId::DOCKYARD));

        // Both players vote for option 1.
        let expected_next = ms.vote.options[1];
        votes.insert(0, 1);
        votes.insert(1, 1);
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 6);
        assert_eq!(ms.vote.counts, vec![0, 2, 0]);
        assert!(ms.time_left <= MAP_VOTE_ALL_VOTED_CAP);

        // Vote timer expires -> winner locks for reveal.
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 7);
        assert_eq!(ms.phase, Phase::MatchOver);
        assert_eq!(ms.vote.winner, Some(1));
        assert!(!ms.vote.tiebreak);
        assert_eq!(ms.vote.winning_map(), Some(expected_next));

        // Reveal timer expires -> new match starts on winning map!
        let prev_id = ms.match_id;
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 8);
        assert_eq!(ms.phase, Phase::Countdown);
        assert_eq!(ms.settings.map, expected_next);
        assert_eq!(ms.match_id, prev_id + 1);
        assert_eq!(ms.score, [0, 0]);
        assert!(ms.vote.options.is_empty());
    }

    #[test]
    fn score_limit_ends_team_splat_early() {
        let mut ms = state(GameMode::TeamSplat);
        ms.settings.score_limit = 3;
        let mut seen_deaths = HashMap::new();
        let mut votes = HashMap::new();
        let pool = MapId::BUILT_IN;
        let both = [seen(0, true, 0, 0), seen(1, true, 0, 0)];

        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 1);
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both, &pool, 2);
        assert_eq!(ms.phase, Phase::Live);

        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[seen(0, true, 0, 0), seen(1, false, 3, 0)],
            &pool,
            3,
        );
        assert_eq!(ms.phase, Phase::MatchOver);
        assert_eq!(ms.winner, WIN_BLUE);
    }

    #[test]
    fn free_for_all_has_one_winner() {
        let mut ms = state(GameMode::FreeForAll);
        let mut seen_deaths = HashMap::new();
        let mut votes = HashMap::new();
        let pool = MapId::BUILT_IN;
        // Players 0 and 2 are both "blue" but enemies in free-for-all.
        let start = [
            seen(0, true, 0, 0),
            seen(2, true, 0, 0),
            seen(4, true, 0, 0),
        ];
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &start, &pool, 1);
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &start, &pool, 2);
        assert_eq!(ms.phase, Phase::Live);
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[
                seen(0, true, 0, 0),
                seen(2, true, 1, 4),
                seen(4, true, 1, 2),
            ],
            &pool,
            3,
        );
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[
                seen(0, true, 0, 0),
                seen(2, true, 2, 4),
                seen(4, true, 1, 2),
            ],
            &pool,
            4,
        );
        assert_eq!(ms.kills(4), 2);
        assert_eq!(ms.score, [0, 0]);
        ms.time_left = 0.0;
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[
                seen(0, true, 0, 0),
                seen(2, true, 2, 4),
                seen(4, true, 1, 2),
            ],
            &pool,
            5,
        );
        assert_eq!(ms.winner, WIN_PLAYER + 4);
    }

    #[test]
    fn elimination_is_first_to_five_rounds() {
        let mut ms = state(GameMode::Elimination);
        let mut seen_deaths = HashMap::new();
        let mut votes = HashMap::new();
        let pool = MapId::BUILT_IN;
        // Orange's deaths counter only goes up.
        let both = |d: u32| [seen(0, true, 0, 0), seen(1, true, d, 0)];
        step_rules(&mut ms, &mut seen_deaths, &mut votes, &both(0), &pool, 1);
        for round in 1..=ROUNDS_TO_WIN {
            assert_eq!(ms.phase, Phase::Countdown);
            assert_eq!(ms.round, round);
            ms.time_left = 0.0;
            step_rules(
                &mut ms,
                &mut seen_deaths,
                &mut votes,
                &both(round - 1),
                &pool,
                round as u64,
            );
            assert_eq!(ms.phase, Phase::Live);
            assert_eq!(ms.time_left, ROUND_SECONDS);
            // Orange goes down: blue takes the round.
            step_rules(
                &mut ms,
                &mut seen_deaths,
                &mut votes,
                &[seen(0, true, 0, 0), seen(1, false, round, 0)],
                &pool,
                round as u64 + 100,
            );
            assert_eq!(ms.round_winner, WIN_BLUE);
            assert_eq!(ms.score[0], round);
            if round < ROUNDS_TO_WIN {
                assert_eq!(ms.phase, Phase::RoundOver);
                ms.time_left = 0.0;
                step_rules(
                    &mut ms,
                    &mut seen_deaths,
                    &mut votes,
                    &both(round),
                    &pool,
                    round as u64 + 200,
                );
            }
        }
        assert_eq!(ms.phase, Phase::MatchOver);
        assert_eq!(ms.winner, WIN_BLUE);
        assert_eq!(ms.kills(0), ROUNDS_TO_WIN);
    }

    #[test]
    fn losing_the_other_team_goes_back_to_warmup() {
        let mut ms = state(GameMode::Elimination);
        let mut seen_deaths = HashMap::new();
        let mut votes = HashMap::new();
        let pool = MapId::BUILT_IN;
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[seen(0, true, 0, 0), seen(1, true, 0, 0)],
            &pool,
            1,
        );
        assert_eq!(ms.phase, Phase::Countdown);
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &mut votes,
            &[seen(0, true, 0, 0)],
            &pool,
            2,
        );
        assert_eq!(ms.phase, Phase::Warmup);
        assert_eq!(ms.board.len(), 1);
    }

    #[test]
    fn map_vote_tiebreak_picks_randomly_only_among_tied_top_options() {
        // Clear winner: never a tiebreak.
        for seed in 0..50 {
            assert_eq!(resolve_map_vote(&[1, 3, 2], seed), (1, false));
        }

        // 2-way tie between index 0 and index 2 (index 1 has fewer votes and must NEVER win).
        let mut seen_two = HashSet::new();
        for seed in 0..200 {
            let (idx, tie) = resolve_map_vote(&[2, 1, 2], seed);
            assert!(tie, "2-way tie must report tiebreak=true");
            assert!(idx == 0 || idx == 2, "non-tied option 1 must never win");
            seen_two.insert(idx);
        }
        assert_eq!(seen_two, HashSet::from([0u8, 2u8]));

        // 3-way tie (including 0-0-0 when nobody voted).
        let mut seen_three = HashSet::new();
        for seed in 0..200 {
            let (idx, tie) = resolve_map_vote(&[0, 0, 0], seed);
            assert!(tie, "0-0-0 tie must report tiebreak=true");
            assert!(idx < 3);
            seen_three.insert(idx);
        }
        assert_eq!(seen_three, HashSet::from([0u8, 1u8, 2u8]));
    }

    #[test]
    fn pick_vote_maps_returns_three_distinct_fresh_maps() {
        let pool = MapId::BUILT_IN;
        for seed in 0..50 {
            let picked = pick_vote_maps(&pool, Some(MapId::CITY), seed);
            assert_eq!(picked.len(), 3);
            let uniq: HashSet<MapId> = picked.iter().copied().collect();
            assert_eq!(uniq.len(), 3);
            assert!(!uniq.contains(&MapId::CITY));
        }
    }
}
