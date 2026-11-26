//! The grind: every marker body (receiver) keeps its own splat record, and
//! its attachments and camos unlock from it.
//!
//! - **Camos** unlock with splats: 25, 50, 100, 150, 200, 300, 400, 500.
//! - **Attachments** unlock with splats, or with challenges: splats while
//!   crouching, from 25 m or more, from the hip, or aiming down sights.
//!
//! Only splats in a live match count. Progress is saved on this computer.

use crate::{
    AppState,
    loadout::{
        BODIES, GunKind, Loadout, LoadoutBook, SKINS, Slot, config_file, save_loadout,
        save_loadout_book,
    },
    net::NetworkState,
    ui_kit::{KIT_YELLOW, UiKit, kit, sliced},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A splat from this far away (metres) counts as long range.
pub const LONG_RANGE: f32 = 25.0;
/// Splats with a body that unlock each camo after Factory.
pub const CAMO_SPLATS: [u32; 8] = [25, 50, 100, 150, 200, 300, 400, 500];
const TOAST_SECONDS: f32 = 4.0;

pub struct ProgressionPlugin;

impl Plugin for ProgressionPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_progress())
            .init_resource::<Toasts>()
            .add_message::<LocalSplat>()
            .add_systems(Startup, lock_saved_loadout)
            .add_systems(
                Update,
                (count_splats, show_toasts).run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), |mut t: ResMut<Toasts>| {
                *t = Toasts::default()
            });
    }
}

/// We splatted someone (sent by combat).
#[derive(Message, Debug, Clone, Copy)]
pub struct LocalSplat {
    pub distance: f32,
    pub crouched: bool,
    pub aiming: bool,
}

/// What an unlock asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Challenge {
    Splats,
    Crouched,
    LongRange,
    HipFire,
    Aimed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Req {
    pub challenge: Challenge,
    pub need: u32,
}

const fn req(challenge: Challenge, need: u32) -> Option<Req> {
    Some(Req { challenge, need })
}

use Challenge::*;

/// Barrels: Stub, Long, Shrouded, Ported, Bull, Sawed-off, Suppressor, Twin.
const BARREL_REQS: [Option<Req>; 8] = [
    None,
    req(Splats, 10),
    req(Splats, 25),
    req(LongRange, 10),
    req(Splats, 50),
    req(HipFire, 15),
    req(Crouched, 20),
    req(Splats, 100),
];
/// Hoppers: Gravity, Loader, Low Pro, Drum, Pod, Box, Speedfeed, Stick Mag.
const HOPPER_REQS: [Option<Req>; 8] = [
    None,
    req(Splats, 40),
    req(Splats, 15),
    req(Splats, 120),
    req(Splats, 5),
    req(Splats, 60),
    req(HipFire, 25),
    req(Crouched, 30),
];
/// Sights: None, Iron, Red Dot, Holo, Scope, Laser, Prism, Reflex, then the
/// canted versions of Iron..Reflex (a little more of the same challenge).
const SIGHT_REQS: [Option<Req>; 15] = [
    None,
    None,
    req(Aimed, 15),
    req(Aimed, 30),
    req(LongRange, 20),
    req(HipFire, 20),
    req(Aimed, 40),
    req(Splats, 10),
    req(Aimed, 10),
    req(Aimed, 25),
    req(Aimed, 45),
    req(LongRange, 30),
    req(HipFire, 30),
    req(Aimed, 55),
    req(Splats, 20),
];
/// Stocks: None, Fixed, Skeleton, Collapsible, Cheek Rise, Minimal, Thumbhole.
const STOCK_REQS: [Option<Req>; 7] = [
    None,
    None,
    req(Splats, 30),
    req(Crouched, 20),
    req(LongRange, 15),
    req(Splats, 45),
    req(Aimed, 35),
];
/// Tanks: HPA Small, HPA Large, HPA Carbon, CO2, HPA Mini, Twin Bottle.
const TANK_REQS: [Option<Req>; 6] = [
    None,
    req(Splats, 20),
    req(Splats, 75),
    req(Splats, 12),
    req(HipFire, 35),
    req(Splats, 150),
];
/// Foregrips: None, Vertical, Angled, Bipod, Handstop, Stubby.
const GRIP_REQS: [Option<Req>; 6] = [
    None,
    req(Splats, 12),
    req(Aimed, 25),
    req(Crouched, 25),
    req(Splats, 50),
    req(HipFire, 40),
];

/// What part `i` of `slot` needs before it can be used (None: free).
pub fn requirement(slot: Slot, i: u8) -> Option<Req> {
    let i = i as usize;
    let table: &[Option<Req>] = match slot {
        Slot::Barrel => &BARREL_REQS,
        Slot::Hopper => &HOPPER_REQS,
        Slot::Sight => &SIGHT_REQS,
        Slot::Stock => &STOCK_REQS,
        Slot::Tank => &TANK_REQS,
        Slot::Foregrip => &GRIP_REQS,
        Slot::Skin => {
            return (i > 0)
                .then(|| CAMO_SPLATS.get(i - 1).copied())
                .flatten()
                .and_then(|n| req(Splats, n));
        }
        Slot::Body | Slot::Water | Slot::Grenade | Slot::Flash => return None,
    };
    table.get(i).copied().flatten()
}

impl Req {
    /// "25 splats", "10 splats from 25 m+".
    pub fn describe(&self) -> String {
        let n = self.need;
        let s = if n == 1 { "" } else { "s" };
        match self.challenge {
            Splats => format!("{n} splat{s}"),
            Crouched => format!("{n} crouching splat{s}"),
            LongRange => format!("{n} splat{s} from {LONG_RANGE:.0} m+"),
            HipFire => format!("{n} hip-fire splat{s}"),
            Aimed => format!("{n} aimed splat{s}"),
        }
    }
}

/// One body's record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GunRecord {
    pub splats: u32,
    pub crouched: u32,
    pub long_range: u32,
    pub hip_fire: u32,
    pub aimed: u32,
}

impl GunRecord {
    pub fn count(&self, c: Challenge) -> u32 {
        match c {
            Splats => self.splats,
            Crouched => self.crouched,
            LongRange => self.long_range,
            HipFire => self.hip_fire,
            Aimed => self.aimed,
        }
    }

    fn add(&mut self, splat: &LocalSplat) {
        self.splats += 1;
        if splat.crouched {
            self.crouched += 1;
        }
        if splat.distance >= LONG_RANGE {
            self.long_range += 1;
        }
        if splat.aiming {
            self.aimed += 1;
        } else {
            self.hip_fire += 1;
        }
    }
}

/// Every body's record, by name, saved between runs.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub guns: HashMap<String, GunRecord>,
}

impl Progress {
    pub fn record(&self, body: u8) -> GunRecord {
        BODIES
            .get(body as usize)
            .and_then(|b| self.guns.get(b.name))
            .copied()
            .unwrap_or_default()
    }

    /// The requirement for a part on `body` and how far along it is.
    pub fn status(&self, body: u8, slot: Slot, i: u8) -> Option<(Req, u32)> {
        let r = requirement(slot, i)?;
        Some((r, self.record(body).count(r.challenge).min(r.need)))
    }

    pub fn unlocked(&self, body: u8, slot: Slot, i: u8) -> bool {
        self.status(body, slot, i)
            .is_none_or(|(r, have)| have >= r.need)
    }

    /// How many of `body`'s parts and camos are unlocked, and how many there are.
    pub fn unlocked_count(&self, body: u8) -> (usize, usize) {
        let mut have = 0;
        let mut all = 0;
        for slot in Slot::PAINTBALL {
            if slot == Slot::Body {
                continue;
            }
            for i in 0..slot.parts().len() as u8 {
                if requirement(slot, i).is_some() {
                    all += 1;
                    if self.unlocked(body, slot, i) {
                        have += 1;
                    }
                }
            }
        }
        (have, all)
    }
}

/// Swaps any part this body hasn't unlocked for its free one. True if
/// anything changed.
pub fn lock_loadout(loadout: &mut Loadout, progress: &Progress) -> bool {
    let mut changed = false;
    for slot in Slot::PAINTBALL {
        let i = loadout.get(slot);
        if !progress.unlocked(loadout.body, slot, i) {
            // The best free part: the highest index with no requirement.
            let free = (0..=i)
                .rev()
                .find(|&j| requirement(slot, j).is_none())
                .unwrap_or(0);
            loadout.set(slot, free);
            changed = true;
        }
    }
    changed
}

fn save_path() -> Option<std::path::PathBuf> {
    config_file("progression.json")
}

fn load_progress() -> Progress {
    save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Progress>(&b).ok())
        .unwrap_or_default()
}

fn save_progress(progress: &Progress) {
    let Some(path) = save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(progress)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save progress to {}: {err}", path.display());
    }
}

/// A saved loadout can't use parts that aren't unlocked.
fn lock_saved_loadout(
    mut loadout: ResMut<Loadout>,
    mut book: ResMut<LoadoutBook>,
    progress: Res<Progress>,
) {
    let mut book_changed = false;
    for preset in &mut book.presets {
        if lock_loadout(&mut preset.loadout, &progress) {
            book_changed = true;
        }
    }
    if book_changed {
        save_loadout_book(&book);
    }
    *loadout = book.active_loadout();
    if lock_loadout(&mut loadout, &progress) {
        save_loadout(&loadout);
    }
}

/// Unlock pop-ups waiting to show: (title, line).
#[derive(Resource, Default)]
struct Toasts {
    queue: Vec<(String, String)>,
    showing: Option<(Entity, f32)>,
}

fn count_splats(
    mut splats: MessageReader<LocalSplat>,
    net: Res<NetworkState>,
    loadout: Res<Loadout>,
    mut progress: ResMut<Progress>,
    mut toasts: ResMut<Toasts>,
) {
    let live = net.match_state.counting();
    let mut any = false;
    for splat in splats.read() {
        if !live || loadout.kind != GunKind::Paintball {
            continue;
        }
        let body = loadout.body;
        let Some(part) = BODIES.get(body as usize) else {
            continue;
        };
        let before = progress.clone();
        progress
            .guns
            .entry(part.name.to_string())
            .or_default()
            .add(splat);
        any = true;
        // Anything that just unlocked gets a pop-up.
        for slot in Slot::PAINTBALL {
            if slot == Slot::Body {
                continue;
            }
            for (i, p) in slot.parts().iter().enumerate() {
                let i = i as u8;
                if !before.unlocked(body, slot, i) && progress.unlocked(body, slot, i) {
                    let what = if slot == Slot::Skin {
                        format!("{} camo", p.name)
                    } else {
                        format!("{} {}", p.name, slot.label().to_lowercase())
                    };
                    toasts
                        .queue
                        .push((format!("{what} unlocked"), format!("for the {}", part.name)));
                }
            }
        }
    }
    if any {
        save_progress(&progress);
    }
}

fn show_toasts(
    mut commands: Commands,
    time: Res<Time>,
    mut toasts: ResMut<Toasts>,
    assets: Res<AssetServer>,
    fonts: Res<UiKit>,
) {
    if let Some((e, left)) = &mut toasts.showing {
        *left -= time.delta_secs();
        if *left > 0.0 {
            return;
        }
        commands.entity(*e).despawn();
        toasts.showing = None;
    }
    if toasts.queue.is_empty() {
        return;
    }
    let (title, line) = toasts.queue.remove(0);
    let e = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(150.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Pickable::IGNORE,
            GlobalZIndex(8),
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(14.0),
                    padding: UiRect::axes(Val::Px(24.0), Val::Px(12.0)),
                    ..default()
                },
                sliced(kit(&assets, "panels/panel_accent.png"), 20.0),
                Pickable::IGNORE,
            ))
            .with_children(|t| {
                t.spawn((
                    ImageNode::new(kit(&assets, "icons/star.png")),
                    Node {
                        width: Val::Px(36.0),
                        height: Val::Px(36.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                t.spawn((Node {
                    flex_direction: FlexDirection::Column,
                    ..default()
                },))
                    .with_children(|c| {
                        c.spawn((
                            Text::new(title.to_uppercase()),
                            fonts.heading(26.0),
                            TextColor(KIT_YELLOW),
                            Pickable::IGNORE,
                        ));
                        c.spawn((
                            Text::new(line),
                            TextFont::from_font_size(13.0),
                            TextColor(Color::WHITE),
                            Pickable::IGNORE,
                        ));
                    });
            });
        })
        .id();
    toasts.showing = Some((e, TOAST_SECONDS));
}

/// Camo names line up with the unlock table.
const _: () = assert!(SKINS.len() == CAMO_SPLATS.len() + 1);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loadout::{BARRELS, FOREGRIPS, HOPPERS, SIGHTS, STOCKS, TANKS};

    #[test]
    fn tables_cover_every_part() {
        assert_eq!(BARREL_REQS.len(), BARRELS.len());
        assert_eq!(HOPPER_REQS.len(), HOPPERS.len());
        assert_eq!(SIGHT_REQS.len(), SIGHTS.len());
        assert_eq!(STOCK_REQS.len(), STOCKS.len());
        assert_eq!(TANK_REQS.len(), TANKS.len());
        assert_eq!(GRIP_REQS.len(), FOREGRIPS.len());
        // The default build is all free parts.
        let mut l = Loadout::default();
        assert!(!lock_loadout(&mut l, &Progress::default()));
    }

    #[test]
    fn camos_unlock_per_body() {
        let mut p = Progress::default();
        assert!(!p.unlocked(0, Slot::Skin, 1));
        p.guns.insert(
            BODIES[0].name.to_string(),
            GunRecord {
                splats: 60,
                ..default()
            },
        );
        assert!(p.unlocked(0, Slot::Skin, 1));
        assert!(p.unlocked(0, Slot::Skin, 2));
        assert!(!p.unlocked(0, Slot::Skin, 3));
        // Another body starts from nothing.
        assert!(!p.unlocked(1, Slot::Skin, 1));
        assert_eq!(
            p.status(0, Slot::Skin, 3),
            Some((req(Splats, 100).unwrap(), 60))
        );
    }

    #[test]
    fn challenges_and_locking() {
        let mut r = GunRecord::default();
        r.add(&LocalSplat {
            distance: 30.0,
            crouched: true,
            aiming: true,
        });
        r.add(&LocalSplat {
            distance: 5.0,
            crouched: false,
            aiming: false,
        });
        assert_eq!(
            (r.splats, r.long_range, r.crouched, r.aimed, r.hip_fire),
            (2, 1, 1, 1, 1)
        );

        let p = Progress::default();
        let mut l = Loadout {
            barrel: 7,
            sight: 4,
            skin: 3,
            ..default()
        };
        assert!(lock_loadout(&mut l, &p));
        assert_eq!((l.barrel, l.sight, l.skin), (0, 1, 0));
    }
}
