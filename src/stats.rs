//! Career stats: splats, deaths, accuracy, wins and a record per gun, saved on
//! this computer, and the STATS page in the main menu that shows them.
//!
//! Only live play counts (not warmup, the countdown or the results), the same
//! as the unlocks in `progression.rs`. Accuracy counts balls: a Breacher pull
//! is six balls, and each one that lands on an enemy is a hit. Paint bomb
//! shrapnel isn't counted either way.

use crate::{
    AppState,
    combat::LocalState,
    loadout::{Loadout, config_file},
    menu::{BG_CARD, MenuScreen, MenuState, TEXT_MUTED, TEXT_PRIMARY, TOP_BAR_HEIGHT},
    net::NetworkState,
    progression::LocalSplat,
    protocol::{GameMode, MatchState, Phase, PlayerId, WIN_DRAW, is_bot},
    ui_kit::{KIT_YELLOW, UiKit, kit},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub struct StatsPlugin;

impl Plugin for StatsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_stats())
            .init_resource::<ShotTally>()
            .init_resource::<Tracker>()
            .add_systems(OnEnter(AppState::InGame), start_tracking)
            .add_systems(
                Update,
                track_stats
                    .after(crate::world::WorldSystems::Net)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), |stats: Res<PlayerStats>| {
                save_stats(&stats)
            })
            .add_systems(Update, show_stats_screen);
    }
}

/// Balls the local player fired and landed since `track_stats` last looked
/// (combat adds to it, `track_stats` empties it).
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ShotTally {
    pub fired: u32,
    pub hit: u32,
    pub headshots: u32,
}

/// One gun's record. Paintball markers go by body, water guns by name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GunStats {
    pub splats: u32,
    /// Times we were splatted while carrying it.
    pub deaths: u32,
    pub balls_fired: u64,
    pub balls_hit: u64,
    pub headshots: u32,
    /// Metres.
    pub longest_splat: f32,
}

impl GunStats {
    pub fn accuracy(&self) -> Option<f32> {
        accuracy(self.balls_hit, self.balls_fired)
    }
}

/// Matches played and won in one mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeRecord {
    pub matches: u32,
    pub wins: u32,
}

/// How a match ended for us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Loss,
    Draw,
}

/// Everything we keep, saved as `stats.json` next to the loadouts.
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerStats {
    pub splats: u32,
    pub deaths: u32,
    pub balls_fired: u64,
    pub balls_hit: u64,
    pub headshots: u32,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    /// Metres.
    pub longest_splat: f32,
    /// Most splats in one life.
    pub best_streak: u32,
    pub bot_splats: u32,
    pub splatted_by_bots: u32,
    /// Seconds of live play.
    pub play_seconds: f64,
    /// Each gun we've used, by its display name ("Vanta marker").
    pub guns: BTreeMap<String, GunStats>,
    /// The guns that splatted us, by display name.
    pub splatted_by_gun: BTreeMap<String, u32>,
    /// By mode name ("Team Splat", "Ranked", ...).
    pub modes: BTreeMap<String, ModeRecord>,
    /// Splats since we were last splatted.
    #[serde(skip)]
    pub streak: u32,
}

impl PlayerStats {
    pub fn accuracy(&self) -> Option<f32> {
        accuracy(self.balls_hit, self.balls_fired)
    }

    /// Splats per death (just the splats with no deaths yet).
    pub fn splat_ratio(&self) -> f32 {
        self.splats as f32 / self.deaths.max(1) as f32
    }

    /// Wins out of matches finished, 0..=1.
    pub fn win_rate(&self) -> Option<f32> {
        (self.matches > 0).then(|| self.wins as f32 / self.matches as f32)
    }

    fn gun(&mut self, gun: &str) -> &mut GunStats {
        self.guns.entry(gun.to_string()).or_default()
    }

    pub fn record_splat(&mut self, gun: &str, distance: f32, on_bot: bool) {
        self.splats += 1;
        self.streak += 1;
        self.best_streak = self.best_streak.max(self.streak);
        self.longest_splat = self.longest_splat.max(distance);
        if on_bot {
            self.bot_splats += 1;
        }
        let g = self.gun(gun);
        g.splats += 1;
        g.longest_splat = g.longest_splat.max(distance);
    }

    /// We were splatted carrying `gun`, by someone carrying `by_gun` (None
    /// when we splatted ourselves).
    pub fn record_death(&mut self, gun: &str, by_gun: Option<&str>, by_bot: bool) {
        self.deaths += 1;
        self.streak = 0;
        self.gun(gun).deaths += 1;
        if let Some(by) = by_gun {
            *self.splatted_by_gun.entry(by.to_string()).or_default() += 1;
        }
        if by_bot {
            self.splatted_by_bots += 1;
        }
    }

    pub fn record_shots(&mut self, gun: &str, tally: ShotTally) {
        if tally.fired == 0 && tally.hit == 0 {
            return;
        }
        self.balls_fired += tally.fired as u64;
        self.balls_hit += tally.hit as u64;
        self.headshots += tally.headshots;
        let g = self.gun(gun);
        g.balls_fired += tally.fired as u64;
        g.balls_hit += tally.hit as u64;
        g.headshots += tally.headshots;
    }

    pub fn record_match(&mut self, mode: &str, outcome: Outcome) {
        self.matches += 1;
        let m = self.modes.entry(mode.to_string()).or_default();
        m.matches += 1;
        match outcome {
            Outcome::Win => {
                self.wins += 1;
                m.wins += 1;
            }
            Outcome::Loss => self.losses += 1,
            Outcome::Draw => self.draws += 1,
        }
    }

    /// Guns with any record, most splats first (then most balls fired).
    pub fn guns_by_splats(&self) -> Vec<(&str, &GunStats)> {
        let mut v: Vec<_> = self
            .guns
            .iter()
            .filter(|(_, g)| g.splats > 0 || g.deaths > 0 || g.balls_fired > 0)
            .map(|(n, g)| (n.as_str(), g))
            .collect();
        v.sort_by(|a, b| {
            b.1.splats
                .cmp(&a.1.splats)
                .then(b.1.balls_fired.cmp(&a.1.balls_fired))
                .then(a.0.cmp(b.0))
        });
        v
    }

    /// The gun that has splatted us most, and how many times.
    pub fn nemesis_gun(&self) -> Option<(&str, u32)> {
        self.splatted_by_gun
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(n, c)| (n.as_str(), *c))
    }
}

pub fn accuracy(hit: u64, fired: u64) -> Option<f32> {
    (fired > 0).then(|| (hit as f32 / fired as f32).min(1.0))
}

/// What a match counts as in the per-mode records.
pub fn mode_label(ms: &MatchState) -> &'static str {
    if ms.settings.ranked {
        "Ranked"
    } else {
        ms.settings.mode.name()
    }
}

/// How the match that just ended went for `id`.
pub fn outcome(ms: &MatchState, id: PlayerId) -> Outcome {
    if ms.winner == WIN_DRAW {
        Outcome::Draw
    } else if ms.is_winner(id) {
        Outcome::Win
    } else {
        Outcome::Loss
    }
}

/// "2h 05m", "14m", "40s".
pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

fn percent(v: Option<f32>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{:.1}%", v * 100.0))
}

fn save_path() -> Option<std::path::PathBuf> {
    config_file("stats.json")
}

fn load_stats() -> PlayerStats {
    save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<PlayerStats>(&b).ok())
        .unwrap_or_default()
}

fn save_stats(stats: &PlayerStats) {
    let Some(path) = save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(stats)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save stats to {}: {err}", path.display());
    }
}

/// What `track_stats` has already counted this session in the game.
#[derive(Resource, Default)]
struct Tracker {
    /// Our deaths counter last frame (None until the first look).
    seen_deaths: Option<u32>,
    /// The last match already counted as played.
    scored_match: Option<u32>,
    /// The match the current streak belongs to.
    streak_match: Option<u32>,
}

fn start_tracking(
    mut tracker: ResMut<Tracker>,
    mut tally: ResMut<ShotTally>,
    mut stats: ResMut<PlayerStats>,
) {
    *tracker = Tracker::default();
    *tally = ShotTally::default();
    stats.streak = 0;
}

fn gun_name(packed: u32) -> String {
    Loadout::unpack(packed).display_name()
}

#[allow(clippy::too_many_arguments)]
fn track_stats(
    time: Res<Time>,
    net: Res<NetworkState>,
    local: Res<LocalState>,
    mut splats: MessageReader<LocalSplat>,
    mut tally: ResMut<ShotTally>,
    mut tracker: ResMut<Tracker>,
    mut stats: ResMut<PlayerStats>,
) {
    let ms = &net.match_state;
    let live = ms.counting();
    let me = net.local_player_id;
    let my_gun = gun_name(local.0.loadout);
    let mut save = false;

    // A new match starts a new streak.
    if tracker.streak_match != Some(ms.match_id) {
        tracker.streak_match = Some(ms.match_id);
        stats.bypass_change_detection().streak = 0;
    }

    let shots = std::mem::take(&mut *tally);
    if live {
        if shots.fired > 0 || shots.hit > 0 {
            stats.record_shots(&my_gun, shots);
        }
        stats.bypass_change_detection().play_seconds += time.delta_secs_f64();
    }

    for splat in splats.read() {
        if live {
            stats.record_splat(&my_gun, splat.distance, is_bot(splat.victim));
            save = true;
        }
    }

    let deaths = local.0.deaths;
    let fresh_death = tracker
        .seen_deaths
        .is_some_and(|seen| deaths != seen && deaths.wrapping_sub(seen) < 10);
    tracker.seen_deaths = Some(deaths);
    if fresh_death && live {
        let killer = local.0.killer;
        let by_gun = (Some(killer) != me)
            .then(|| net.players.get(&killer))
            .flatten()
            .map(|p| gun_name(p.state.loadout));
        stats.record_death(&my_gun, by_gun.as_deref(), is_bot(killer));
        save = true;
    }

    if ms.phase == Phase::MatchOver
        && tracker.scored_match != Some(ms.match_id)
        && let Some(id) = me
    {
        tracker.scored_match = Some(ms.match_id);
        // Only a match we played in (we're on the board) counts.
        if ms.board.iter().any(|l| l[0] == id) {
            stats.record_match(mode_label(ms), outcome(ms, id));
            save = true;
        }
    }

    if save {
        save_stats(&stats);
    }
}

// --- The STATS page -----------------------------------------------------------

#[derive(Component)]
struct StatsRoot;

const ROW_BG: Color = Color::srgba(0.09, 0.12, 0.17, 0.85);
const ROW_BG_ALT: Color = Color::srgba(0.06, 0.08, 0.12, 0.85);

fn show_stats_screen(
    mut commands: Commands,
    state: Res<State<AppState>>,
    menu: Res<MenuState>,
    stats: Res<PlayerStats>,
    assets: Res<AssetServer>,
    fonts: Res<UiKit>,
    root_q: Query<Entity, With<StatsRoot>>,
) {
    let open = *state.get() == AppState::MainMenu && menu.screen == MenuScreen::Stats;
    if !open {
        for root in &root_q {
            commands.entity(root).despawn();
        }
        return;
    }
    if !root_q.is_empty() && !stats.is_changed() {
        return;
    }
    for root in &root_q {
        commands.entity(root).despawn();
    }
    spawn_stats_screen(&mut commands, &stats, &assets, &fonts);
}

fn spawn_stats_screen(
    commands: &mut Commands,
    stats: &PlayerStats,
    assets: &AssetServer,
    fonts: &UiKit,
) {
    let tiles_top: [(&str, &str, String); 6] = [
        ("icons/splat.png", "SPLATS", stats.splats.to_string()),
        ("icons/close.png", "SPLATTED", stats.deaths.to_string()),
        (
            "icons/star.png",
            "SPLAT RATIO",
            format!("{:.2}", stats.splat_ratio()),
        ),
        ("icons/target.png", "ACCURACY", percent(stats.accuracy())),
        ("icons/trophy.png", "WINS", stats.wins.to_string()),
        ("icons/crown.png", "WIN RATE", percent(stats.win_rate())),
    ];
    let nemesis = stats
        .nemesis_gun()
        .map_or_else(|| "-".to_string(), |(n, c)| format!("{n} ({c})"));
    let tiles_more: [(&str, String); 8] = [
        (
            "MATCHES",
            format!(
                "{}  ({}W {}L {}D)",
                stats.matches, stats.wins, stats.losses, stats.draws
            ),
        ),
        ("HEADSHOTS", stats.headshots.to_string()),
        ("BEST STREAK", stats.best_streak.to_string()),
        ("LONGEST SPLAT", format!("{:.0} m", stats.longest_splat)),
        ("BALLS FIRED", stats.balls_fired.to_string()),
        (
            "BOTS SPLATTED",
            format!("{}  (by bots {})", stats.bot_splats, stats.splatted_by_bots),
        ),
        ("TIME PLAYED", format_duration(stats.play_seconds)),
        ("NEMESIS GUN", nemesis),
    ];

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(TOP_BAR_HEIGHT),
                bottom: Val::Px(0.0),
                padding: UiRect::axes(Val::Px(32.0), Val::Px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                ..default()
            },
            GlobalZIndex(1),
            Pickable::IGNORE,
            StatsRoot,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("CAREER STATS"),
                fonts.heading(34.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));

            // Headline numbers.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(12.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                for (icon, label, value) in tiles_top {
                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(12.0)),
                            column_gap: Val::Px(12.0),
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(BG_CARD),
                        Pickable::IGNORE,
                    ))
                    .with_children(|tile| {
                        tile.spawn((
                            ImageNode::new(kit(assets, icon)),
                            Node {
                                width: Val::Px(34.0),
                                height: Val::Px(34.0),
                                flex_shrink: 0.0,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                        tile.spawn((Node {
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },))
                            .with_children(|c| {
                                c.spawn((
                                    Text::new(value),
                                    fonts.heading(30.0),
                                    TextColor(TEXT_PRIMARY),
                                    Pickable::IGNORE,
                                ));
                                c.spawn((
                                    Text::new(label),
                                    TextFont::from_font_size(11.0),
                                    TextColor(KIT_YELLOW),
                                    Pickable::IGNORE,
                                ));
                            });
                    });
                }
            });

            // Body: per-gun table | more numbers and modes.
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(12.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|body| {
                body.spawn((
                    Node {
                        flex_grow: 1.6,
                        flex_basis: Val::Px(0.0),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(Val::Px(16.0)),
                        row_gap: Val::Px(4.0),
                        overflow: Overflow::clip_y(),
                        ..default()
                    },
                    BackgroundColor(BG_CARD),
                    Pickable::IGNORE,
                ))
                .with_children(|card| {
                    card.spawn((
                        Text::new("BY GUN"),
                        fonts.heading(26.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    gun_row(
                        card,
                        ["GUN", "SPLATS", "SPLATTED", "ACCURACY", "HEADSHOTS", "LONGEST"]
                            .map(String::from),
                        None,
                        true,
                    );
                    let guns = stats.guns_by_splats();
                    if guns.is_empty() {
                        card.spawn((
                            Text::new(
                                "Play a match to start your record. Every gun you use gets its own line here.",
                            ),
                            TextFont::from_font_size(14.0),
                            TextColor(TEXT_MUTED),
                            Node {
                                margin: UiRect::top(Val::Px(8.0)),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    }
                    for (i, (name, g)) in guns.into_iter().enumerate() {
                        gun_row(
                            card,
                            [
                                name.to_string(),
                                g.splats.to_string(),
                                g.deaths.to_string(),
                                percent(g.accuracy()),
                                g.headshots.to_string(),
                                format!("{:.0} m", g.longest_splat),
                            ],
                            Some(if i % 2 == 0 { ROW_BG } else { ROW_BG_ALT }),
                            false,
                        );
                    }
                });

                body.spawn((
                    Node {
                        flex_grow: 1.0,
                        flex_basis: Val::Px(0.0),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(Val::Px(16.0)),
                        row_gap: Val::Px(6.0),
                        overflow: Overflow::clip_y(),
                        ..default()
                    },
                    BackgroundColor(BG_CARD),
                    Pickable::IGNORE,
                ))
                .with_children(|card| {
                    card.spawn((
                        Text::new("RECORD"),
                        fonts.heading(26.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    for (label, value) in tiles_more {
                        pair_row(card, label, &value);
                    }
                    card.spawn((
                        Text::new("BY MODE"),
                        fonts.heading(26.0),
                        TextColor(TEXT_PRIMARY),
                        Node {
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                    for mode in [
                        GameMode::TeamSplat.name(),
                        GameMode::FreeForAll.name(),
                        GameMode::Elimination.name(),
                        "Ranked",
                    ] {
                        let r = stats.modes.get(mode).copied().unwrap_or_default();
                        pair_row(
                            card,
                            &mode.to_uppercase(),
                            &format!("{} wins / {} played", r.wins, r.matches),
                        );
                    }
                });
            });
        });
}

/// Width share of each column in the gun table.
const GUN_COLS: [f32; 6] = [2.4, 1.0, 1.1, 1.2, 1.2, 1.1];

fn gun_row(parent: &mut ChildSpawnerCommands, cells: [String; 6], bg: Option<Color>, header: bool) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(bg.unwrap_or(Color::NONE)),
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            for (i, cell) in cells.into_iter().enumerate() {
                let color = if header {
                    KIT_YELLOW
                } else if i == 0 {
                    TEXT_PRIMARY
                } else {
                    TEXT_MUTED
                };
                row.spawn((
                    Text::new(cell),
                    TextFont::from_font_size(if header { 11.0 } else { 14.0 }),
                    TextColor(color),
                    Node {
                        flex_grow: GUN_COLS[i],
                        flex_basis: Val::Px(0.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
        });
}

fn pair_row(parent: &mut ChildSpawnerCommands, label: &str, value: &str) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(12.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(ROW_BG),
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(label),
                TextFont::from_font_size(12.0),
                TextColor(KIT_YELLOW),
                Pickable::IGNORE,
            ));
            row.spawn((
                Text::new(value),
                TextFont::from_font_size(14.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{WIN_BLUE, WIN_ORANGE, WIN_PLAYER};

    #[test]
    fn splats_deaths_and_streaks() {
        let mut s = PlayerStats::default();
        s.record_splat("Vanta marker", 12.0, false);
        s.record_splat("Vanta marker", 31.0, true);
        s.record_splat("Bulldog marker", 5.0, false);
        assert_eq!(s.splats, 3);
        assert_eq!(s.best_streak, 3);
        assert_eq!(s.bot_splats, 1);
        assert_eq!(s.longest_splat, 31.0);
        assert_eq!(s.guns["Vanta marker"].splats, 2);
        assert_eq!(s.guns["Vanta marker"].longest_splat, 31.0);

        s.record_death("Bulldog marker", Some("Longshot sniper"), true);
        assert_eq!(s.streak, 0);
        s.record_splat("Bulldog marker", 8.0, false);
        assert_eq!(s.best_streak, 3);
        assert_eq!(s.deaths, 1);
        assert_eq!(s.splatted_by_bots, 1);
        assert_eq!(s.guns["Bulldog marker"].deaths, 1);
        assert_eq!(s.nemesis_gun(), Some(("Longshot sniper", 1)));
        assert_eq!(s.splat_ratio(), 4.0);
    }

    #[test]
    fn accuracy_counts_balls() {
        let mut s = PlayerStats::default();
        assert_eq!(s.accuracy(), None);
        s.record_shots(
            "Breacher shotgun",
            ShotTally {
                fired: 12,
                hit: 3,
                headshots: 1,
            },
        );
        s.record_shots(
            "Vanta marker",
            ShotTally {
                fired: 4,
                hit: 1,
                headshots: 0,
            },
        );
        assert_eq!(s.accuracy(), Some(0.25));
        assert_eq!(s.headshots, 1);
        assert_eq!(s.guns["Breacher shotgun"].accuracy(), Some(0.25));
        // Nothing fired: no empty line for a gun.
        s.record_shots("Titan marker", ShotTally::default());
        assert!(!s.guns.contains_key("Titan marker"));
    }

    #[test]
    fn guns_sort_by_splats() {
        let mut s = PlayerStats::default();
        s.record_splat("Raptor marker", 1.0, false);
        for _ in 0..3 {
            s.record_splat("Phantom marker", 1.0, false);
        }
        s.record_death("Minnow water gun", None, false);
        let order: Vec<_> = s.guns_by_splats().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            order,
            ["Phantom marker", "Raptor marker", "Minnow water gun"]
        );
    }

    #[test]
    fn matches_and_modes() {
        let mut s = PlayerStats::default();
        s.record_match("Team Splat", Outcome::Win);
        s.record_match("Team Splat", Outcome::Loss);
        s.record_match("Ranked", Outcome::Draw);
        assert_eq!((s.matches, s.wins, s.losses, s.draws), (3, 1, 1, 1));
        assert_eq!(
            s.modes["Team Splat"],
            ModeRecord {
                matches: 2,
                wins: 1
            }
        );
        assert_eq!(s.win_rate(), Some(1.0 / 3.0));
    }

    #[test]
    fn outcomes_from_the_match() {
        let mut ms = MatchState {
            phase: Phase::MatchOver,
            winner: WIN_BLUE,
            ..default()
        };
        assert_eq!(outcome(&ms, 0), Outcome::Win);
        assert_eq!(outcome(&ms, 1), Outcome::Loss);
        ms.winner = WIN_ORANGE;
        assert_eq!(outcome(&ms, 1), Outcome::Win);
        ms.winner = WIN_DRAW;
        assert_eq!(outcome(&ms, 0), Outcome::Draw);
        ms.settings.mode = GameMode::FreeForAll;
        ms.winner = WIN_PLAYER + 3;
        assert_eq!(outcome(&ms, 3), Outcome::Win);
        assert_eq!(outcome(&ms, 2), Outcome::Loss);
        assert_eq!(mode_label(&ms), "Free-for-All");
        ms.settings.ranked = true;
        assert_eq!(mode_label(&ms), "Ranked");
    }

    #[test]
    fn saves_round_trip_and_old_files_load() {
        let mut s = PlayerStats::default();
        s.record_splat("Vanta marker", 20.0, false);
        s.record_match("Elimination", Outcome::Win);
        let json = serde_json::to_vec(&s).unwrap();
        let back: PlayerStats = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.splats, 1);
        assert_eq!(back.guns, s.guns);
        assert_eq!(back.modes, s.modes);
        // A file with fewer fields (an older version) still loads.
        let old: PlayerStats = serde_json::from_str(r#"{"splats": 7}"#).unwrap();
        assert_eq!(old.splats, 7);
        assert_eq!(old.deaths, 0);
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(40.0), "40s");
        assert_eq!(format_duration(14.0 * 60.0 + 5.0), "14m");
        assert_eq!(format_duration(2.0 * 3600.0 + 5.0 * 60.0), "2h 05m");
    }
}
