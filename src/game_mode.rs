//! Match rules for the three modes.
//!
//! - **Team Splat**: blue vs orange for 10 minutes, the team with more
//!   splats wins.
//! - **Free-for-All**: everyone for themselves for 10 minutes, the player
//!   with the most splats wins.
//! - **Elimination**: one life per round, the first team to win 5 rounds
//!   wins (also played ranked).
//!
//! The host runs the rules and tells everyone the [`MatchState`]; everyone
//! counts the clock down between updates, goes back to spawn when a
//! countdown starts, and holds still during countdowns and results.

use crate::{
    AppState,
    combat::{Combat, LocalState},
    loadout::Loadout,
    maps::{CurrentMap, MapInfo},
    net::{NetRole, NetworkState},
    protocol::{
        GameMode, MATCH_SECONDS, MAX_HEALTH, MatchSettings, MatchState, Phase, PlayerId,
        ROUNDS_TO_WIN, Team, WIN_BLUE, WIN_DRAW, WIN_NONE, WIN_ORANGE, WIN_PLAYER, enemies,
        set_free_for_all,
    },
    world::{CameraRig, LocalPlayer, MainCamera, WorldSystems},
};
use bevy::prelude::*;
use std::collections::HashMap;

/// Everyone back at spawn, held still, before play starts.
pub const COUNTDOWN_SECONDS: f32 = 5.0;
/// Elimination: the longest a round can last.
pub const ROUND_SECONDS: f32 = 120.0;
/// Elimination: the pause after a round is decided.
pub const ROUND_OVER_SECONDS: f32 = 5.0;
/// Results stay up this long before the next match.
pub const MATCH_OVER_SECONDS: f32 = 15.0;
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

fn end_match(ms: &mut MatchState) {
    ms.winner = match ms.settings.mode {
        GameMode::FreeForAll => ffa_winner(&ms.board),
        _ => team_winner(ms.score),
    };
    ms.phase = Phase::MatchOver;
    ms.time_left = MATCH_OVER_SECONDS;
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

/// Steps the match: counts splats, ends rounds and matches, starts new ones.
fn step_rules(ms: &mut MatchState, seen_deaths: &mut HashMap<PlayerId, u32>, players: &[Seen]) {
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
                    ROUND_SECONDS
                } else {
                    MATCH_SECONDS
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
                    if ms.score.iter().any(|&s| s >= ROUNDS_TO_WIN) {
                        end_match(ms);
                    } else {
                        ms.phase = Phase::RoundOver;
                        ms.time_left = ROUND_OVER_SECONDS;
                    }
                }
            } else if ms.time_left <= 0.0 {
                end_match(ms);
            }
        }
        Phase::RoundOver => {
            if ms.time_left <= 0.0 {
                ms.round += 1;
                start_countdown(ms);
            }
        }
        Phase::MatchOver => {
            if ms.time_left <= 0.0 {
                if ready {
                    new_match(ms);
                    start_countdown(ms);
                } else {
                    new_match(ms);
                    to_warmup(ms);
                }
            }
        }
    }
}

/// Host only: runs the rules and tells everyone when something changed.
fn run_host_rules(time: Res<Time>, mut net: ResMut<NetworkState>, mut host: ResMut<HostMatch>) {
    if net.active_role() != Some(NetRole::Host) || net.local_player_id.is_none() {
        return;
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
    step_rules(&mut ms, &mut host.seen_deaths, &players);

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
    use crate::protocol::MapId;

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
                map: MapId::Dockyard,
                ranked: false,
            },
            ..default()
        }
    }

    #[test]
    fn team_splat_counts_splats_and_ends_on_time() {
        let mut ms = state(GameMode::TeamSplat);
        let mut seen_deaths = HashMap::new();
        // Alone: warmup.
        step_rules(&mut ms, &mut seen_deaths, &[seen(0, true, 0, 0)]);
        assert_eq!(ms.phase, Phase::Warmup);
        // Someone on the other team: countdown, then live.
        let both = [seen(0, true, 0, 0), seen(1, true, 0, 0)];
        step_rules(&mut ms, &mut seen_deaths, &both);
        assert_eq!(ms.phase, Phase::Countdown);
        let resets = ms.resets;
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &both);
        assert_eq!(ms.phase, Phase::Live);
        assert_eq!(ms.time_left, MATCH_SECONDS);
        assert_eq!(ms.resets, resets);
        // Blue (0) splats orange (1) twice.
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &[seen(0, true, 0, 0), seen(1, false, 2, 0)],
        );
        assert_eq!(ms.score, [2, 0]);
        assert_eq!(ms.kills(0), 2);
        assert_eq!(ms.deaths(1), 2);
        ms.time_left = 0.0;
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &[seen(0, true, 0, 0), seen(1, true, 2, 0)],
        );
        assert_eq!(ms.phase, Phase::MatchOver);
        assert_eq!(ms.winner, WIN_BLUE);
        // Then a fresh match.
        ms.time_left = 0.0;
        let id = ms.match_id;
        step_rules(&mut ms, &mut seen_deaths, &both);
        assert_eq!(ms.phase, Phase::Countdown);
        assert_eq!(ms.match_id, id + 1);
        assert_eq!(ms.score, [0, 0]);
        assert_eq!(ms.kills(0), 0);
    }

    #[test]
    fn free_for_all_has_one_winner() {
        let mut ms = state(GameMode::FreeForAll);
        let mut seen_deaths = HashMap::new();
        // Players 0 and 2 are both "blue" but enemies in free-for-all.
        let start = [
            seen(0, true, 0, 0),
            seen(2, true, 0, 0),
            seen(4, true, 0, 0),
        ];
        step_rules(&mut ms, &mut seen_deaths, &start);
        ms.time_left = 0.0;
        step_rules(&mut ms, &mut seen_deaths, &start);
        assert_eq!(ms.phase, Phase::Live);
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &[
                seen(0, true, 0, 0),
                seen(2, true, 1, 4),
                seen(4, true, 1, 2),
            ],
        );
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &[
                seen(0, true, 0, 0),
                seen(2, true, 2, 4),
                seen(4, true, 1, 2),
            ],
        );
        assert_eq!(ms.kills(4), 2);
        assert_eq!(ms.score, [0, 0]);
        ms.time_left = 0.0;
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &[
                seen(0, true, 0, 0),
                seen(2, true, 2, 4),
                seen(4, true, 1, 2),
            ],
        );
        assert_eq!(ms.winner, WIN_PLAYER + 4);
    }

    #[test]
    fn elimination_is_first_to_five_rounds() {
        let mut ms = state(GameMode::Elimination);
        let mut seen_deaths = HashMap::new();
        // Orange's deaths counter only goes up.
        let both = |d: u32| [seen(0, true, 0, 0), seen(1, true, d, 0)];
        step_rules(&mut ms, &mut seen_deaths, &both(0));
        for round in 1..=ROUNDS_TO_WIN {
            assert_eq!(ms.phase, Phase::Countdown);
            assert_eq!(ms.round, round);
            ms.time_left = 0.0;
            step_rules(&mut ms, &mut seen_deaths, &both(round - 1));
            assert_eq!(ms.phase, Phase::Live);
            assert_eq!(ms.time_left, ROUND_SECONDS);
            // Orange goes down: blue takes the round.
            step_rules(
                &mut ms,
                &mut seen_deaths,
                &[seen(0, true, 0, 0), seen(1, false, round, 0)],
            );
            assert_eq!(ms.round_winner, WIN_BLUE);
            assert_eq!(ms.score[0], round);
            if round < ROUNDS_TO_WIN {
                assert_eq!(ms.phase, Phase::RoundOver);
                ms.time_left = 0.0;
                step_rules(&mut ms, &mut seen_deaths, &both(round));
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
        step_rules(
            &mut ms,
            &mut seen_deaths,
            &[seen(0, true, 0, 0), seen(1, true, 0, 0)],
        );
        assert_eq!(ms.phase, Phase::Countdown);
        step_rules(&mut ms, &mut seen_deaths, &[seen(0, true, 0, 0)]);
        assert_eq!(ms.phase, Phase::Warmup);
        assert_eq!(ms.board.len(), 1);
    }
}
