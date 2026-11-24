//! Ranked Elimination: rank points (RP) saved on this computer.
//!
//! Winning a ranked match gives RP (plus a little for splats), losing or
//! leaving one before it's decided takes some away. Points climb through
//! Bronze, Silver, Gold, Platinum and Diamond (three divisions each) to
//! Champion.

use crate::{
    AppState,
    combat::LocalState,
    loadout::config_file,
    net::NetworkState,
    protocol::{GameMode, MatchState, Phase, PlayerId, Team, WIN_BLUE, WIN_DRAW, WIN_ORANGE},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub const WIN_RP: i32 = 30;
pub const LOSS_RP: i32 = -20;
pub const KILL_RP: i32 = 2;
pub const MAX_KILL_RP: i32 = 10;

pub struct RankedPlugin;

impl Plugin for RankedPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_rank())
            .init_resource::<RankResult>()
            .add_systems(
                Update,
                (share_rank, score_ranked).run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), leave_penalty);
    }
}

/// Our rank points, saved between runs.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rank {
    pub points: u32,
    pub wins: u32,
    pub losses: u32,
    /// The last ranked match already scored (so it's never counted twice).
    #[serde(skip)]
    scored_match: Option<u32>,
    /// The ranked match being played right now, if any.
    #[serde(skip)]
    live_match: Option<u32>,
}

/// The change from the last ranked match, for the results screen.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct RankResult {
    pub match_id: Option<u32>,
    pub delta: i32,
    pub before: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Bronze,
    Silver,
    Gold,
    Platinum,
    Diamond,
    Champion,
}

impl Tier {
    pub const ALL: [Tier; 6] = [
        Tier::Bronze,
        Tier::Silver,
        Tier::Gold,
        Tier::Platinum,
        Tier::Diamond,
        Tier::Champion,
    ];

    /// The fewest points for this tier.
    pub fn floor(self) -> u32 {
        match self {
            Tier::Bronze => 0,
            Tier::Silver => 300,
            Tier::Gold => 600,
            Tier::Platinum => 900,
            Tier::Diamond => 1200,
            Tier::Champion => 1500,
        }
    }

    pub fn of(points: u32) -> Tier {
        Tier::ALL
            .into_iter()
            .rev()
            .find(|t| points >= t.floor())
            .unwrap_or(Tier::Bronze)
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Bronze => "Bronze",
            Tier::Silver => "Silver",
            Tier::Gold => "Gold",
            Tier::Platinum => "Platinum",
            Tier::Diamond => "Diamond",
            Tier::Champion => "Champion",
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Tier::Bronze => "textures/ui-kit/badges/rank_bronze.png",
            Tier::Silver => "textures/ui-kit/badges/rank_silver.png",
            Tier::Gold => "textures/ui-kit/badges/rank_gold.png",
            Tier::Platinum => "textures/ui-kit/badges/rank_platinum.png",
            Tier::Diamond => "textures/ui-kit/badges/rank_diamond.png",
            Tier::Champion => "textures/ui-kit/badges/rank_champion.png",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Tier::Bronze => Color::srgb_u8(0xc8, 0x84, 0x52),
            Tier::Silver => Color::srgb_u8(0xc9, 0xd2, 0xdc),
            Tier::Gold => Color::srgb_u8(0xf5, 0xc4, 0x2c),
            Tier::Platinum => Color::srgb_u8(0x5f, 0xd6, 0xc8),
            Tier::Diamond => Color::srgb_u8(0x7c, 0xb4, 0xff),
            Tier::Champion => Color::srgb_u8(0xff, 0x6a, 0x6a),
        }
    }
}

/// "Gold II", "Champion".
pub fn rank_name(points: u32) -> String {
    let tier = Tier::of(points);
    if tier == Tier::Champion {
        return tier.name().to_string();
    }
    // Three divisions of 100 points: III, II, I.
    let div = ((points - tier.floor()) / 100).min(2);
    let roman = ["III", "II", "I"][div as usize];
    format!("{} {roman}", tier.name())
}

/// Progress through the current division (0..1) and points to the next one.
pub fn division_progress(points: u32) -> (f32, Option<u32>) {
    if Tier::of(points) == Tier::Champion {
        return (1.0, None);
    }
    let into = points % 100;
    (into as f32 / 100.0, Some(100 - into))
}

/// The RP change for a ranked match: `won` is None for a draw.
pub fn rp_change(won: Option<bool>, splats: u32) -> i32 {
    match won {
        Some(true) => WIN_RP + (splats as i32 * KILL_RP).min(MAX_KILL_RP),
        Some(false) => LOSS_RP,
        None => 0,
    }
}

fn apply(points: u32, delta: i32) -> u32 {
    (points as i64 + delta as i64).clamp(0, 9999) as u32
}

fn save_path() -> Option<std::path::PathBuf> {
    config_file("ranked.json")
}

fn load_rank() -> Rank {
    save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Rank>(&b).ok())
        .unwrap_or_default()
}

fn save_rank(rank: &Rank) {
    let Some(path) = save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(rank)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save rank to {}: {err}", path.display());
    }
}

fn is_ranked(ms: &MatchState) -> bool {
    ms.settings.ranked && ms.settings.mode == GameMode::Elimination
}

/// Everyone sees our rank on the scoreboard.
fn share_rank(rank: Res<Rank>, mut local: ResMut<LocalState>) {
    let rp = rank.points.min(u16::MAX as u32) as u16;
    if local.0.rank_points != rp {
        local.0.rank_points = rp;
    }
}

fn won(ms: &MatchState, id: PlayerId) -> Option<bool> {
    let mine = match Team::of(id) {
        Team::Blue => WIN_BLUE,
        Team::Orange => WIN_ORANGE,
    };
    match ms.winner {
        WIN_DRAW => None,
        w if w == WIN_BLUE || w == WIN_ORANGE => Some(w == mine),
        _ => None,
    }
}

fn score_ranked(net: Res<NetworkState>, mut rank: ResMut<Rank>, mut result: ResMut<RankResult>) {
    let ms = &net.match_state;
    let live = (is_ranked(ms)
        && matches!(ms.phase, Phase::Countdown | Phase::Live | Phase::RoundOver))
    .then_some(ms.match_id);
    if rank.live_match != live {
        rank.live_match = live;
    }
    if !is_ranked(ms) || ms.phase != Phase::MatchOver {
        return;
    }
    let Some(id) = net.local_player_id else {
        return;
    };
    if rank.scored_match == Some(ms.match_id) {
        return;
    }
    // Only players who were in the match (on the board) are scored.
    if !ms.board.iter().any(|l| l[0] == id) {
        rank.scored_match = Some(ms.match_id);
        return;
    }
    let outcome = won(ms, id);
    let delta = rp_change(outcome, ms.kills(id));
    let before = rank.points;
    rank.points = apply(rank.points, delta);
    match outcome {
        Some(true) => rank.wins += 1,
        Some(false) => rank.losses += 1,
        None => {}
    }
    rank.scored_match = Some(ms.match_id);
    *result = RankResult {
        match_id: Some(ms.match_id),
        delta,
        before,
    };
    save_rank(&rank);
}

/// Quitting a ranked match that's still being played counts as a loss
/// (being dropped by the host or the network doesn't).
fn leave_penalty(net: Res<NetworkState>, mut rank: ResMut<Rank>) {
    let Some(id) = rank.live_match.take() else {
        return;
    };
    if net.error_text.is_some() || rank.scored_match == Some(id) {
        return;
    }
    rank.points = apply(rank.points, LOSS_RP);
    rank.losses += 1;
    rank.scored_match = Some(id);
    save_rank(&rank);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_and_divisions() {
        assert_eq!(rank_name(0), "Bronze III");
        assert_eq!(rank_name(150), "Bronze II");
        assert_eq!(rank_name(299), "Bronze I");
        assert_eq!(rank_name(300), "Silver III");
        assert_eq!(rank_name(1499), "Diamond I");
        assert_eq!(rank_name(2000), "Champion");
        assert_eq!(Tier::of(650), Tier::Gold);
    }

    #[test]
    fn rp_changes() {
        assert_eq!(rp_change(Some(true), 0), 30);
        assert_eq!(rp_change(Some(true), 3), 36);
        assert_eq!(rp_change(Some(true), 20), 40);
        assert_eq!(rp_change(Some(false), 9), -20);
        assert_eq!(rp_change(None, 4), 0);
        assert_eq!(apply(10, -20), 0);
    }
}
