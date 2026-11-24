//! What a player carries: a modular paintball marker (body + attachments from
//! the LowPoly Armory set) or one of the water guns, plus the stats each part
//! gives. A loadout packs into a `u32` so it rides along in `PlayerState`.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One choosable part: its name, a one-line blurb and its model file.
pub struct Part {
    pub name: &'static str,
    pub blurb: &'static str,
    pub model: &'static str,
}

pub const BODIES: &[Part] = &[
    Part {
        name: "Vanta",
        blurb: "All-rounder. Steady rate of fire, easy to aim.",
        model: "models/guns/bodies/vanta.glb",
    },
    Part {
        name: "Bulldog",
        blurb: "Heavy hitter. Slow, but every ball counts.",
        model: "models/guns/bodies/bulldog.glb",
    },
    Part {
        name: "Raptor",
        blurb: "Speedball marker. Sprays fast, spreads wide.",
        model: "models/guns/bodies/raptor.glb",
    },
    Part {
        name: "Phantom",
        blurb: "Precision marker with a fast, flat shot.",
        model: "models/guns/bodies/phantom.glb",
    },
    Part {
        name: "Titan",
        blurb: "Full-auto workhorse. Heavy to carry.",
        model: "models/guns/bodies/titan.glb",
    },
];

pub const BARRELS: &[Part] = &[
    Part {
        name: "Stub",
        blurb: "Short and light: move faster, less accurate.",
        model: "models/guns/attachments/barrels/barrel_stub.glb",
    },
    Part {
        name: "Long",
        blurb: "Tighter groups and a faster ball.",
        model: "models/guns/attachments/barrels/barrel_long.glb",
    },
    Part {
        name: "Shrouded",
        blurb: "Quieter shots, slightly tighter groups.",
        model: "models/guns/attachments/barrels/barrel_shrouded.glb",
    },
    Part {
        name: "Ported",
        blurb: "Ports tame the kick: best accuracy.",
        model: "models/guns/attachments/barrels/barrel_ported.glb",
    },
    Part {
        name: "Bull",
        blurb: "Hits harder, groups wider.",
        model: "models/guns/attachments/barrels/barrel_bull.glb",
    },
];

pub const HOPPERS: &[Part] = &[
    Part {
        name: "Gravity",
        blurb: "40 balls, average reload.",
        model: "models/guns/attachments/hoppers/hopper_gravity.glb",
    },
    Part {
        name: "Loader",
        blurb: "100 balls, slow reload.",
        model: "models/guns/attachments/hoppers/hopper_loader.glb",
    },
    Part {
        name: "Low Pro",
        blurb: "60 balls, quick reload.",
        model: "models/guns/attachments/hoppers/hopper_low_pro.glb",
    },
    Part {
        name: "Drum",
        blurb: "150 balls, slowest reload, heavy.",
        model: "models/guns/attachments/hoppers/hopper_drum.glb",
    },
    Part {
        name: "Pod",
        blurb: "30 balls, fastest reload.",
        model: "models/guns/attachments/hoppers/hopper_pod.glb",
    },
];

/// Sights, stocks and foregrips can be left off (index 0 is "None").
pub const SIGHTS: &[Part] = &[
    Part {
        name: "None",
        blurb: "No sight: a small zoom when aiming.",
        model: "",
    },
    Part {
        name: "Iron",
        blurb: "Simple irons. 1.2x zoom.",
        model: "models/guns/attachments/sights/sight_iron.glb",
    },
    Part {
        name: "Red Dot",
        blurb: "Clean dot. 1.35x zoom.",
        model: "models/guns/attachments/sights/sight_red_dot.glb",
    },
    Part {
        name: "Holo",
        blurb: "Wide window. 1.5x zoom.",
        model: "models/guns/attachments/sights/sight_holo.glb",
    },
    Part {
        name: "Scope",
        blurb: "Long range. 2.6x zoom, slower to move.",
        model: "models/guns/attachments/sights/sight_scope.glb",
    },
];

pub const STOCKS: &[Part] = &[
    Part {
        name: "None",
        blurb: "Lightest. Less steady when aiming.",
        model: "",
    },
    Part {
        name: "Fixed",
        blurb: "Solid shoulder: steadier aim.",
        model: "models/guns/attachments/stocks/stock_fixed.glb",
    },
    Part {
        name: "Skeleton",
        blurb: "Light frame: a little steadier, still quick.",
        model: "models/guns/attachments/stocks/stock_skeleton.glb",
    },
    Part {
        name: "Collapsible",
        blurb: "Balanced steadiness and speed.",
        model: "models/guns/attachments/stocks/stock_collapsible.glb",
    },
    Part {
        name: "Cheek Rise",
        blurb: "Steadiest aim, heaviest stock.",
        model: "models/guns/attachments/stocks/stock_cheek_rise.glb",
    },
];

pub const TANKS: &[Part] = &[
    Part {
        name: "HPA Small",
        blurb: "Light air tank.",
        model: "models/guns/attachments/tanks/tank_hpa_small.glb",
    },
    Part {
        name: "HPA Large",
        blurb: "More pressure: faster balls, heavier.",
        model: "models/guns/attachments/tanks/tank_hpa_large.glb",
    },
    Part {
        name: "HPA Carbon",
        blurb: "Fast balls without the weight.",
        model: "models/guns/attachments/tanks/tank_hpa_carbon.glb",
    },
    Part {
        name: "CO2",
        blurb: "Cheap and cold: slower balls, quicker cycling.",
        model: "models/guns/attachments/tanks/tank_co2.glb",
    },
];

pub const FOREGRIPS: &[Part] = &[
    Part {
        name: "None",
        blurb: "Nothing under the barrel.",
        model: "",
    },
    Part {
        name: "Vertical",
        blurb: "Tighter groups while moving.",
        model: "models/guns/attachments/foregrips/grip_vertical.glb",
    },
    Part {
        name: "Angled",
        blurb: "Faster to aim, slightly tighter.",
        model: "models/guns/attachments/foregrips/grip_angled.glb",
    },
    Part {
        name: "Bipod",
        blurb: "Very tight when still, heavy.",
        model: "models/guns/attachments/foregrips/grip_bipod.glb",
    },
];

pub const WATER_GUNS: &[Part] = &[
    Part {
        name: "Minnow",
        blurb: "Pocket squirter. Quick and light.",
        model: "models/water-guns/minnow.glb",
    },
    Part {
        name: "Tidal",
        blurb: "Pump-action stream with good reach.",
        model: "models/water-guns/tidal.glb",
    },
    Part {
        name: "Monsoon",
        blurb: "Huge tank, soaks everything in front of it.",
        model: "models/water-guns/monsoon.glb",
    },
];

/// Which crosshair the HUD shows.
pub const CROSSHAIRS: &[(&str, &str)] = &[
    ("Cross", "textures/ui/crosshair_cross.png"),
    ("Dot", "textures/ui/crosshair_dot.png"),
    ("Circle", "textures/ui/crosshair_circle.png"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GunKind {
    #[default]
    Paintball,
    Water,
}

/// The player's chosen gun. Indices point into the tables above.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Loadout {
    pub kind: GunKind,
    pub body: u8,
    pub barrel: u8,
    pub hopper: u8,
    pub sight: u8,
    pub stock: u8,
    pub tank: u8,
    pub foregrip: u8,
    pub water: u8,
    pub crosshair: u8,
}

impl Default for Loadout {
    fn default() -> Self {
        // Close to the Vanta factory build.
        Self {
            kind: GunKind::Paintball,
            body: 0,
            barrel: 1,
            hopper: 0,
            sight: 2,
            stock: 1,
            tank: 0,
            foregrip: 0,
            water: 1,
            crosshair: 0,
        }
    }
}

/// A slot in the loadout screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Body,
    Barrel,
    Hopper,
    Sight,
    Stock,
    Tank,
    Foregrip,
    Water,
}

impl Slot {
    pub const PAINTBALL: [Slot; 7] = [
        Slot::Body,
        Slot::Barrel,
        Slot::Hopper,
        Slot::Sight,
        Slot::Stock,
        Slot::Tank,
        Slot::Foregrip,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Slot::Body => "Body",
            Slot::Barrel => "Barrel",
            Slot::Hopper => "Hopper",
            Slot::Sight => "Sight",
            Slot::Stock => "Stock",
            Slot::Tank => "Air Tank",
            Slot::Foregrip => "Foregrip",
            Slot::Water => "Water Gun",
        }
    }

    pub fn parts(self) -> &'static [Part] {
        match self {
            Slot::Body => BODIES,
            Slot::Barrel => BARRELS,
            Slot::Hopper => HOPPERS,
            Slot::Sight => SIGHTS,
            Slot::Stock => STOCKS,
            Slot::Tank => TANKS,
            Slot::Foregrip => FOREGRIPS,
            Slot::Water => WATER_GUNS,
        }
    }
}

impl Loadout {
    pub fn get(&self, slot: Slot) -> u8 {
        match slot {
            Slot::Body => self.body,
            Slot::Barrel => self.barrel,
            Slot::Hopper => self.hopper,
            Slot::Sight => self.sight,
            Slot::Stock => self.stock,
            Slot::Tank => self.tank,
            Slot::Foregrip => self.foregrip,
            Slot::Water => self.water,
        }
    }

    pub fn set(&mut self, slot: Slot, value: u8) {
        let value = value.min(slot.parts().len() as u8 - 1);
        match slot {
            Slot::Body => self.body = value,
            Slot::Barrel => self.barrel = value,
            Slot::Hopper => self.hopper = value,
            Slot::Sight => self.sight = value,
            Slot::Stock => self.stock = value,
            Slot::Tank => self.tank = value,
            Slot::Foregrip => self.foregrip = value,
            Slot::Water => self.water = value,
        }
    }

    /// Keeps every index in range (a hand-edited or older save).
    pub fn sanitized(mut self) -> Self {
        for slot in Slot::PAINTBALL.into_iter().chain([Slot::Water]) {
            let v = self.get(slot);
            self.set(slot, v);
        }
        self.crosshair = self.crosshair.min(CROSSHAIRS.len() as u8 - 1);
        self
    }

    /// Packs the gun into 4 bits per part (the crosshair stays local).
    pub fn pack(&self) -> u32 {
        match self.kind {
            GunKind::Water => 1 | (self.water as u32) << 4,
            GunKind::Paintball => {
                (self.body as u32) << 4
                    | (self.barrel as u32) << 8
                    | (self.hopper as u32) << 12
                    | (self.sight as u32) << 16
                    | (self.stock as u32) << 20
                    | (self.tank as u32) << 24
                    | (self.foregrip as u32) << 28
            }
        }
    }

    pub fn unpack(v: u32) -> Self {
        let nib = |shift: u32| ((v >> shift) & 0xF) as u8;
        let mut l = Loadout::default();
        if nib(0) == 1 {
            l.kind = GunKind::Water;
            l.water = nib(4);
        } else {
            l.kind = GunKind::Paintball;
            l.body = nib(4);
            l.barrel = nib(8);
            l.hopper = nib(12);
            l.sight = nib(16);
            l.stock = nib(20);
            l.tank = nib(24);
            l.foregrip = nib(28);
        }
        l.sanitized()
    }

    /// The model files that make up this gun, all placed at the same origin.
    pub fn model_paths(&self) -> Vec<&'static str> {
        match self.kind {
            GunKind::Water => vec![WATER_GUNS[self.water as usize].model],
            GunKind::Paintball => [
                BODIES[self.body as usize].model,
                BARRELS[self.barrel as usize].model,
                HOPPERS[self.hopper as usize].model,
                SIGHTS[self.sight as usize].model,
                STOCKS[self.stock as usize].model,
                TANKS[self.tank as usize].model,
                FOREGRIPS[self.foregrip as usize].model,
            ]
            .into_iter()
            .filter(|m| !m.is_empty())
            .collect(),
        }
    }

    pub fn display_name(&self) -> String {
        match self.kind {
            GunKind::Water => format!("{} water gun", WATER_GUNS[self.water as usize].name),
            GunKind::Paintball => format!("{} marker", BODIES[self.body as usize].name),
        }
    }

    pub fn stats(&self) -> WeaponStats {
        match self.kind {
            GunKind::Water => water_stats(self.water),
            GunKind::Paintball => paintball_stats(self),
        }
    }
}

/// How a gun shoots and handles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponStats {
    pub water: bool,
    /// Health taken per hit.
    pub damage: f32,
    /// Seconds between shots (held trigger is automatic).
    pub fire_interval: f32,
    /// Muzzle speed, m/s.
    pub speed: f32,
    /// Cone half-angle (radians) when moving and not aiming.
    pub spread: f32,
    /// Spread multiplier while aiming down sights.
    pub aim_spread: f32,
    pub capacity: u32,
    pub reload: f32,
    /// How much aiming zooms in.
    pub zoom: f32,
    /// Walk/sprint speed multiplier.
    pub mobility: f32,
    /// Gravity on the projectile, m/s^2.
    pub gravity: f32,
    /// Shot sound volume.
    pub loudness: f32,
}

fn paintball_stats(l: &Loadout) -> WeaponStats {
    // Body base stats.
    let (damage, fire_interval, speed, spread, mobility) = match l.body {
        0 => (24.0, 0.12, 80.0, 0.016, 1.0),   // Vanta
        1 => (40.0, 0.38, 88.0, 0.008, 0.97),  // Bulldog
        2 => (17.0, 0.075, 74.0, 0.026, 1.03), // Raptor
        3 => (30.0, 0.2, 95.0, 0.009, 1.0),    // Phantom
        _ => (22.0, 0.09, 78.0, 0.02, 0.9),    // Titan
    };
    let mut s = WeaponStats {
        water: false,
        damage,
        fire_interval,
        speed,
        spread,
        aim_spread: 0.55,
        capacity: 40,
        reload: 2.2,
        zoom: 1.1,
        mobility,
        gravity: 9.81,
        loudness: 1.0,
    };
    match l.barrel {
        0 => {
            s.spread *= 1.35;
            s.speed *= 0.92;
            s.mobility *= 1.04;
        }
        1 => {
            s.spread *= 0.65;
            s.speed *= 1.1;
            s.mobility *= 0.98;
        }
        2 => {
            s.spread *= 0.85;
            s.loudness = 0.45;
        }
        3 => s.spread *= 0.6,
        _ => {
            s.damage += 5.0;
            s.spread *= 1.15;
        }
    }
    (s.capacity, s.reload) = match l.hopper {
        0 => (40, 2.2),
        1 => (100, 2.9),
        2 => (60, 1.8),
        3 => {
            s.mobility *= 0.95;
            (150, 3.4)
        }
        _ => (30, 1.2),
    };
    s.zoom = match l.sight {
        0 => 1.1,
        1 => 1.2,
        2 => 1.35,
        3 => 1.5,
        _ => {
            s.mobility *= 0.97;
            2.6
        }
    };
    match l.stock {
        0 => s.aim_spread = 0.7,
        1 => s.aim_spread = 0.45,
        2 => {
            s.aim_spread = 0.55;
            s.mobility *= 1.02;
        }
        3 => s.aim_spread = 0.5,
        _ => {
            s.aim_spread = 0.35;
            s.mobility *= 0.97;
        }
    }
    match l.tank {
        0 => {}
        1 => {
            s.speed *= 1.06;
            s.mobility *= 0.97;
        }
        2 => s.speed *= 1.05,
        _ => {
            s.speed *= 0.94;
            s.fire_interval *= 0.92;
        }
    }
    match l.foregrip {
        0 => {}
        1 => s.spread *= 0.85,
        2 => {
            s.spread *= 0.92;
            s.aim_spread *= 0.9;
        }
        _ => {
            s.spread *= 0.75;
            s.mobility *= 0.95;
        }
    }
    s
}

fn water_stats(i: u8) -> WeaponStats {
    let base = WeaponStats {
        water: true,
        damage: 6.0,
        fire_interval: 0.05,
        speed: 24.0,
        spread: 0.03,
        aim_spread: 0.7,
        capacity: 60,
        reload: 1.6,
        zoom: 1.1,
        mobility: 1.06,
        gravity: 7.0,
        loudness: 0.6,
    };
    match i {
        0 => base,
        1 => WeaponStats {
            damage: 8.0,
            fire_interval: 0.06,
            speed: 30.0,
            spread: 0.022,
            capacity: 90,
            reload: 2.2,
            zoom: 1.2,
            mobility: 1.0,
            ..base
        },
        _ => WeaponStats {
            damage: 10.0,
            fire_interval: 0.055,
            speed: 28.0,
            spread: 0.04,
            capacity: 140,
            reload: 3.0,
            zoom: 1.1,
            mobility: 0.93,
            ..base
        },
    }
}

/// Every gun model, loaded once at startup so switching parts never hitches.
#[derive(Resource)]
pub struct GunAssets {
    handles: Vec<(&'static str, Handle<WorldAsset>)>,
}

impl GunAssets {
    pub fn get(&self, path: &str) -> Option<Handle<WorldAsset>> {
        self.handles
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, h)| h.clone())
    }
}

pub fn load_gun_assets(mut commands: Commands, assets: Res<AssetServer>) {
    let mut handles = Vec::new();
    for table in [
        BODIES, BARRELS, HOPPERS, SIGHTS, STOCKS, TANKS, FOREGRIPS, WATER_GUNS,
    ] {
        for part in table {
            if !part.model.is_empty() {
                handles.push((
                    part.model,
                    assets.load(GltfAssetLabel::Scene(0).from_asset(part.model)),
                ));
            }
        }
    }
    commands.insert_resource(GunAssets { handles });
}

/// Marks the entity that holds a gun's parts (so it can be rebuilt).
#[derive(Component)]
pub struct GunModel {
    pub packed: u32,
}

/// Spawns the parts of `loadout` as children of `parent` (all at its origin:
/// the LowPoly Armory parts share one coordinate space and snap together).
pub fn spawn_gun_parts(
    commands: &mut Commands,
    guns: &GunAssets,
    parent: Entity,
    loadout: &Loadout,
) {
    commands.entity(parent).despawn_related::<Children>();
    for path in loadout.model_paths() {
        if let Some(handle) = guns.get(path) {
            let child = commands.spawn(WorldAssetRoot(handle)).id();
            commands.entity(parent).add_child(child);
        }
    }
}

/// Where loadouts are saved between runs.
fn save_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| {
            let mut p = PathBuf::from(h);
            if cfg!(target_os = "macos") {
                p.push("Library/Application Support");
            } else {
                p.push(".config");
            }
            p
        })
    })?;
    Some(base.join("Ink-cognito").join("loadout.json"))
}

pub fn load_saved_loadout() -> Loadout {
    save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Loadout>(&bytes).ok())
        .map(Loadout::sanitized)
        .unwrap_or_default()
}

pub fn save_loadout(loadout: &Loadout) {
    let Some(path) = save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(loadout)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save the loadout to {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadouts_survive_packing() {
        let mut l = Loadout {
            kind: GunKind::Paintball,
            body: 4,
            barrel: 3,
            hopper: 4,
            sight: 4,
            stock: 4,
            tank: 3,
            foregrip: 3,
            water: 0,
            crosshair: 0,
        };
        assert_eq!(Loadout::unpack(l.pack()).model_paths(), l.model_paths());
        l.kind = GunKind::Water;
        l.water = 2;
        let back = Loadout::unpack(l.pack());
        assert_eq!(back.kind, GunKind::Water);
        assert_eq!(back.water, 2);
        // Garbage never indexes out of range.
        let junk = Loadout::unpack(u32::MAX);
        let _ = junk.model_paths();
        let _ = junk.stats();
    }
}
