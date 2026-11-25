//! What a player carries: a modular paintball marker (body + attachments from
//! the LowPoly Armory set) or one of the water guns, plus the stats each part
//! gives. A loadout packs into a `u32` so it rides along in `PlayerState`.

use bevy::{
    gltf::GltfMaterialName,
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
};
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
    Part {
        name: "Breacher",
        blurb: "Paint shotgun: 6 balls x 13 damage a pull. Brutal up close.",
        model: "models/guns/bodies/shotgun.glb",
    },
    Part {
        name: "Longshot",
        blurb: "Bolt-action sniper marker. Two hits splat.",
        model: "models/guns/bodies/sniper.glb",
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
    Part {
        name: "Sawed-off",
        blurb: "Short and wide: light, hits a bit harder, sprays.",
        model: "models/guns/attachments/barrels/barrel_sawed_off.glb",
    },
    Part {
        name: "Suppressor",
        blurb: "Near-silent shots, slightly tighter.",
        model: "models/guns/attachments/barrels/barrel_suppressor.glb",
    },
    Part {
        name: "Twin",
        blurb: "Two bores: cycles faster, groups wider.",
        model: "models/guns/attachments/barrels/barrel_twin.glb",
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
    Part {
        name: "Box",
        blurb: "80 balls, steady reload.",
        model: "models/guns/attachments/hoppers/hopper_box.glb",
    },
    Part {
        name: "Speedfeed",
        blurb: "50 balls, quick reload.",
        model: "models/guns/attachments/hoppers/hopper_speedfeed.glb",
    },
    Part {
        name: "Stick Mag",
        blurb: "25 balls, very fast swaps.",
        model: "models/guns/attachments/hoppers/hopper_stick_mag.glb",
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
    Part {
        name: "Laser",
        blurb: "Tighter from the hip. 1.15x zoom.",
        model: "models/guns/attachments/sights/sight_laser.glb",
    },
    Part {
        name: "Prism",
        blurb: "Mid-range glass. 1.8x zoom.",
        model: "models/guns/attachments/sights/sight_prism.glb",
    },
    Part {
        name: "Reflex",
        blurb: "Open and quick. 1.3x zoom.",
        model: "models/guns/attachments/sights/sight_reflex.glb",
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
    Part {
        name: "Minimal",
        blurb: "A bare brace: quick, a little steadier.",
        model: "models/guns/attachments/stocks/stock_minimal.glb",
    },
    Part {
        name: "Thumbhole",
        blurb: "Locked-in grip: very steady aim.",
        model: "models/guns/attachments/stocks/stock_thumbhole.glb",
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
    Part {
        name: "HPA Mini",
        blurb: "Tiny and light: a touch slower balls.",
        model: "models/guns/attachments/tanks/tank_hpa_mini.glb",
    },
    Part {
        name: "Twin Bottle",
        blurb: "Most pressure: fastest balls, heaviest.",
        model: "models/guns/attachments/tanks/tank_twin_bottle.glb",
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
    Part {
        name: "Handstop",
        blurb: "Slightly tighter, no weight.",
        model: "models/guns/attachments/foregrips/grip_handstop.glb",
    },
    Part {
        name: "Stubby",
        blurb: "Short grip: tighter while moving.",
        model: "models/guns/attachments/foregrips/grip_stubby.glb",
    },
];

/// Camo skins for the marker body (index 0 is the factory paint).
pub const SKINS: &[Part] = &[
    Part {
        name: "Factory",
        blurb: "The body's own paint.",
        model: "",
    },
    Part {
        name: "Woodland",
        blurb: "Classic greens and browns.",
        model: "textures/skins/camo_woodland.png",
    },
    Part {
        name: "Desert",
        blurb: "Sand and stone.",
        model: "textures/skins/camo_desert.png",
    },
    Part {
        name: "Arctic",
        blurb: "Snow and ice.",
        model: "textures/skins/camo_arctic.png",
    },
    Part {
        name: "Urban",
        blurb: "Concrete greys.",
        model: "textures/skins/camo_urban.png",
    },
    Part {
        name: "Digital",
        blurb: "Pixel camo.",
        model: "textures/skins/camo_digital.png",
    },
    Part {
        name: "Tiger",
        blurb: "Bold stripes.",
        model: "textures/skins/camo_tiger.png",
    },
    Part {
        name: "Carbon",
        blurb: "Carbon-fibre weave.",
        model: "textures/skins/skin_carbon_fiber.png",
    },
    Part {
        name: "Splatter",
        blurb: "Already been in a few games.",
        model: "textures/skins/skin_paint_splatter.png",
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

/// What the paint bomb key (G) throws. No models: the throwables are drawn
/// as plain spheres. Index order matches `grenades::Kind::for_bomb_slot`.
pub const GRENADES: &[Part] = &[
    Part {
        name: "Paint Bomb",
        blurb: "One big burst of paintballs. 18 s recharge.",
        model: "",
    },
    Part {
        name: "Ultra Bang",
        blurb: "Keeps bursting every 0.5 s for 7 s. 90 s recharge.",
        model: "",
    },
];

/// What the flashbang key (Q) throws.
pub const FLASHES: &[Part] = &[
    Part {
        name: "Flashbang",
        blurb: "One cloud of paint over nearby screens. 22 s recharge.",
        model: "",
    },
    Part {
        name: "9-Banger",
        blurb: "Bursts 9 times in a row. 66 s recharge.",
        model: "",
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
    /// Camo skin (`SKINS`), paintball markers only.
    pub skin: u8,
    /// Which grenade (`GRENADES`) the paint bomb key throws.
    pub grenade: u8,
    /// Which flash (`FLASHES`) the flashbang key throws.
    pub flash: u8,
}

impl Default for Loadout {
    fn default() -> Self {
        // The Vanta with the parts everyone starts with.
        Self {
            kind: GunKind::Paintball,
            body: 0,
            barrel: 0,
            hopper: 0,
            sight: 1,
            stock: 1,
            tank: 0,
            foregrip: 0,
            water: 1,
            crosshair: 0,
            skin: 0,
            grenade: 0,
            flash: 0,
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
    Skin,
    Water,
    Grenade,
    Flash,
}

impl Slot {
    pub const PAINTBALL: [Slot; 8] = [
        Slot::Body,
        Slot::Barrel,
        Slot::Hopper,
        Slot::Sight,
        Slot::Stock,
        Slot::Tank,
        Slot::Foregrip,
        Slot::Skin,
    ];

    /// The throwables, chosen with either gun kind.
    pub const THROWABLES: [Slot; 2] = [Slot::Grenade, Slot::Flash];

    pub fn is_throwable(self) -> bool {
        matches!(self, Slot::Grenade | Slot::Flash)
    }

    pub fn label(self) -> &'static str {
        match self {
            Slot::Body => "Body",
            Slot::Barrel => "Barrel",
            Slot::Hopper => "Hopper",
            Slot::Sight => "Sight",
            Slot::Stock => "Stock",
            Slot::Tank => "Air Tank",
            Slot::Foregrip => "Foregrip",
            Slot::Skin => "Skin",
            Slot::Water => "Water Gun",
            Slot::Grenade => "Grenade",
            Slot::Flash => "Flash",
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
            Slot::Skin => SKINS,
            Slot::Water => WATER_GUNS,
            Slot::Grenade => GRENADES,
            Slot::Flash => FLASHES,
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
            Slot::Skin => self.skin,
            Slot::Water => self.water,
            Slot::Grenade => self.grenade,
            Slot::Flash => self.flash,
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
            Slot::Skin => self.skin = value,
            Slot::Water => self.water = value,
            Slot::Grenade => self.grenade = value,
            Slot::Flash => self.flash = value,
        }
    }

    /// Keeps every index in range (a hand-edited or older save).
    pub fn sanitized(mut self) -> Self {
        for slot in Slot::PAINTBALL
            .into_iter()
            .chain([Slot::Water])
            .chain(Slot::THROWABLES)
        {
            let v = self.get(slot);
            self.set(slot, v);
        }
        self.crosshair = self.crosshair.min(CROSSHAIRS.len() as u8 - 1);
        self
    }

    /// Packs the gun into 4 bits per part (the crosshair stays local). The
    /// skin rides in bits 1-3 plus bit 31 (foregrips only need 3 bits).
    pub fn pack(&self) -> u32 {
        match self.kind {
            GunKind::Water => 1 | (self.water as u32) << 4,
            GunKind::Paintball => {
                let skin = self.skin as u32;
                (skin & 7) << 1
                    | (skin >> 3) << 31
                    | (self.body as u32) << 4
                    | (self.barrel as u32) << 8
                    | (self.hopper as u32) << 12
                    | (self.sight as u32) << 16
                    | (self.stock as u32) << 20
                    | (self.tank as u32) << 24
                    | ((self.foregrip as u32) & 7) << 28
            }
        }
    }

    /// The throwables, packed for `PlayerState::throwables`: grenade in the
    /// low 4 bits, flash in the high 4.
    pub fn pack_throwables(&self) -> u8 {
        (self.grenade & 0xF) | (self.flash & 0xF) << 4
    }

    pub fn unpack(v: u32) -> Self {
        let nib = |shift: u32| ((v >> shift) & 0xF) as u8;
        let mut l = Loadout::default();
        if v & 1 == 1 {
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
            l.foregrip = nib(28) & 7;
            l.skin = (((v >> 1) & 7) | (v >> 31) << 3) as u8;
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
            GunKind::Paintball => match self.body {
                5 => "Breacher shotgun".to_string(),
                6 => "Longshot sniper".to_string(),
                b => format!("{} marker", BODIES[b as usize].name),
            },
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
    /// Balls per trigger pull (the shotgun fires several).
    pub pellets: u32,
}

fn paintball_stats(l: &Loadout) -> WeaponStats {
    // Body base stats.
    let (damage, fire_interval, speed, spread, mobility) = match l.body {
        0 => (24.0, 0.12, 80.0, 0.016, 1.0),   // Vanta
        1 => (40.0, 0.38, 88.0, 0.008, 0.97),  // Bulldog
        2 => (17.0, 0.075, 74.0, 0.026, 1.03), // Raptor
        3 => (30.0, 0.2, 95.0, 0.009, 1.0),    // Phantom
        4 => (22.0, 0.09, 78.0, 0.02, 0.9),    // Titan
        5 => (13.0, 0.5, 68.0, 0.065, 0.98),   // Breacher (per ball)
        _ => (70.0, 1.0, 140.0, 0.002, 0.92),  // Longshot
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
        pellets: 1,
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
        4 => {
            s.damage += 5.0;
            s.spread *= 1.15;
        }
        5 => {
            s.damage += 3.0;
            s.spread *= 1.6;
            s.mobility *= 1.05;
        }
        6 => {
            s.loudness = 0.3;
            s.speed *= 0.97;
            s.spread *= 0.8;
        }
        _ => {
            s.fire_interval *= 0.85;
            s.spread *= 1.25;
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
        4 => (30, 1.2),
        5 => (80, 2.4),
        6 => (50, 1.4),
        _ => (25, 0.9),
    };
    s.zoom = match l.sight {
        0 => 1.1,
        1 => 1.2,
        2 => 1.35,
        3 => 1.5,
        4 => {
            s.mobility *= 0.97;
            2.6
        }
        5 => {
            s.spread *= 0.8;
            1.15
        }
        6 => 1.8,
        _ => 1.3,
    };
    match l.stock {
        0 => s.aim_spread = 0.7,
        1 => s.aim_spread = 0.45,
        2 => {
            s.aim_spread = 0.55;
            s.mobility *= 1.02;
        }
        3 => s.aim_spread = 0.5,
        4 => {
            s.aim_spread = 0.35;
            s.mobility *= 0.97;
        }
        5 => {
            s.aim_spread = 0.6;
            s.mobility *= 1.03;
        }
        _ => {
            s.aim_spread = 0.4;
            s.mobility *= 0.99;
        }
    }
    match l.tank {
        0 => {}
        1 => {
            s.speed *= 1.06;
            s.mobility *= 0.97;
        }
        2 => s.speed *= 1.05,
        3 => {
            s.speed *= 0.94;
            s.fire_interval *= 0.92;
        }
        4 => {
            s.speed *= 0.97;
            s.mobility *= 1.02;
        }
        _ => {
            s.speed *= 1.08;
            s.mobility *= 0.95;
        }
    }
    match l.foregrip {
        0 => {}
        1 => s.spread *= 0.85,
        2 => {
            s.spread *= 0.92;
            s.aim_spread *= 0.9;
        }
        3 => {
            s.spread *= 0.75;
            s.mobility *= 0.95;
        }
        4 => {
            s.spread *= 0.9;
            s.mobility *= 1.01;
        }
        _ => s.spread *= 0.88,
    }
    // The shotgun and sniper load shells / rounds, not a hopper's worth of balls.
    match l.body {
        5 => {
            s.pellets = 6;
            s.capacity = (s.capacity / 5).clamp(5, 24);
            // Spread is the whole pattern: aiming tightens it less.
            s.aim_spread = s.aim_spread.max(0.75);
        }
        6 => {
            s.capacity = (s.capacity / 8).clamp(3, 15);
            s.zoom = s.zoom.max(2.2);
            s.reload *= 1.2;
        }
        _ => {}
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
        pellets: 1,
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
    /// One material per skin (index 0, the factory paint, is unused).
    skins: Vec<Handle<StandardMaterial>>,
}

impl GunAssets {
    pub fn get(&self, path: &str) -> Option<Handle<WorldAsset>> {
        self.handles
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, h)| h.clone())
    }
}

pub fn load_gun_assets(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let repeat = |s: &mut ImageLoaderSettings| {
        s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            ..default()
        });
    };
    let skins = SKINS
        .iter()
        .map(|skin| {
            if skin.model.is_empty() {
                return Handle::default();
            }
            let texture: Handle<Image> =
                assets.load_builder().with_settings(repeat).load(skin.model);
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: Some(texture),
                perceptual_roughness: 0.55,
                ..default()
            })
        })
        .collect();
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
    commands.insert_resource(GunAssets { handles, skins });
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
    let skin = match loadout.kind {
        GunKind::Paintball if loadout.skin > 0 => guns.skins.get(loadout.skin as usize).cloned(),
        _ => None,
    };
    for path in loadout.model_paths() {
        if let Some(handle) = guns.get(path) {
            let mut child = commands.spawn(WorldAssetRoot(handle));
            if let Some(skin) = &skin {
                child.insert(GunSkin(skin.clone()));
            }
            let child = child.id();
            commands.entity(parent).add_child(child);
        }
    }
}

/// A gun part painted with a camo skin (applied once its model is spawned).
#[derive(Component)]
pub struct GunSkin(Handle<StandardMaterial>);

/// Repaints a skinned part's `body_paint` material with its skin.
pub fn apply_gun_skin(
    ready: On<bevy::world_serialization::WorldInstanceReady>,
    skins: Query<&GunSkin>,
    children: Query<&Children>,
    painted: Query<&GltfMaterialName>,
    mut commands: Commands,
) {
    let Ok(skin) = skins.get(ready.entity) else {
        return;
    };
    for e in children.iter_descendants(ready.entity) {
        if painted.get(e).is_ok_and(|n| n.0 == "body_paint") {
            commands.entity(e).insert(MeshMaterial3d(skin.0.clone()));
        }
    }
}

/// Maximum number of custom loadout classes a player can keep.
pub const MAX_LOADOUTS: usize = 10;
/// Maximum character length of a custom loadout name.
pub const MAX_LOADOUT_NAME_LEN: usize = 24;

/// A single named custom loadout class (Call of Duty-style).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamedLoadout {
    pub name: String,
    pub loadout: Loadout,
}

impl Default for NamedLoadout {
    fn default() -> Self {
        Self {
            name: "Loadout 1".to_string(),
            loadout: Loadout::default(),
        }
    }
}

/// The player's collection of named custom loadouts and which one is equipped.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoadoutBook {
    pub active: usize,
    pub presets: Vec<NamedLoadout>,
}

impl Default for LoadoutBook {
    fn default() -> Self {
        Self {
            active: 0,
            presets: vec![
                NamedLoadout {
                    name: "Loadout 1".to_string(),
                    loadout: Loadout::default(),
                },
                NamedLoadout {
                    name: "Loadout 2".to_string(),
                    loadout: Loadout {
                        body: 2, // Raptor
                        ..Loadout::default()
                    },
                },
                NamedLoadout {
                    name: "Loadout 3".to_string(),
                    loadout: Loadout {
                        body: 5, // Breacher shotgun
                        ..Loadout::default()
                    },
                },
                NamedLoadout {
                    name: "Loadout 4".to_string(),
                    loadout: Loadout {
                        body: 6, // Longshot sniper
                        ..Loadout::default()
                    },
                },
                NamedLoadout {
                    name: "Loadout 5".to_string(),
                    loadout: Loadout {
                        kind: GunKind::Water,
                        water: 1, // Tidal
                        ..Loadout::default()
                    },
                },
            ],
        }
    }
}

impl LoadoutBook {
    pub fn sanitized(mut self) -> Self {
        if self.presets.is_empty() {
            return Self::default();
        }
        if self.presets.len() > MAX_LOADOUTS {
            self.presets.truncate(MAX_LOADOUTS);
        }
        for (idx, preset) in self.presets.iter_mut().enumerate() {
            preset.loadout = preset.loadout.sanitized();
            let trimmed: String = preset
                .name
                .trim()
                .chars()
                .take(MAX_LOADOUT_NAME_LEN)
                .collect();
            preset.name = if trimmed.is_empty() {
                format!("Loadout {}", idx + 1)
            } else {
                trimmed
            };
        }
        self.active = self.active.min(self.presets.len() - 1);
        self
    }

    pub fn active_preset(&self) -> &NamedLoadout {
        &self.presets[self.active.min(self.presets.len() - 1)]
    }

    pub fn active_preset_mut(&mut self) -> &mut NamedLoadout {
        let idx = self.active.min(self.presets.len() - 1);
        &mut self.presets[idx]
    }

    pub fn active_loadout(&self) -> Loadout {
        self.active_preset().loadout
    }

    pub fn select(&mut self, index: usize) {
        if index < self.presets.len() {
            self.active = index;
        }
    }

    pub fn add_preset(&mut self) -> Option<usize> {
        if self.presets.len() >= MAX_LOADOUTS {
            return None;
        }
        let next_num = self.presets.len() + 1;
        self.presets.push(NamedLoadout {
            name: format!("Loadout {next_num}"),
            loadout: Loadout::default(),
        });
        let idx = self.presets.len() - 1;
        self.active = idx;
        Some(idx)
    }

    pub fn delete_active(&mut self) -> bool {
        if self.presets.len() <= 1 {
            return false;
        }
        let idx = self.active.min(self.presets.len() - 1);
        self.presets.remove(idx);
        if self.active >= self.presets.len() {
            self.active = self.presets.len() - 1;
        }
        true
    }

    pub fn rename_active(&mut self, name: &str) {
        let idx = self.active.min(self.presets.len() - 1);
        let cleaned: String = name.trim().chars().take(MAX_LOADOUT_NAME_LEN).collect();
        self.presets[idx].name = if cleaned.is_empty() {
            format!("Loadout {}", idx + 1)
        } else {
            cleaned
        };
    }
}

/// Legacy single-loadout file path.
fn save_path() -> Option<PathBuf> {
    config_file("loadout.json")
}

/// Where custom loadout presets are saved between runs.
fn book_save_path() -> Option<PathBuf> {
    config_file("loadouts.json")
}

/// A file in the game's settings folder.
pub fn config_file(name: &str) -> Option<PathBuf> {
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
    Some(base.join("Ink-cognito").join(name))
}

pub fn load_saved_loadout_book() -> LoadoutBook {
    if let Some(book) = book_save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<LoadoutBook>(&bytes).ok())
    {
        return book.sanitized();
    }
    // Migrate from legacy single loadout.json if present.
    let mut book = LoadoutBook::default();
    if let Some(legacy) = save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Loadout>(&bytes).ok())
    {
        book.presets[0].loadout = legacy.sanitized();
    }
    book.sanitized()
}

pub fn save_loadout_book(book: &LoadoutBook) {
    let Some(path) = book_save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(book)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save loadouts to {}: {err}", path.display());
    }
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
            skin: 3,
        };
        assert_eq!(Loadout::unpack(l.pack()).model_paths(), l.model_paths());
        assert_eq!(Loadout::unpack(l.pack()).skin, 3);
        // The last of every table, including the 9th skin (uses bit 31).
        let last = |s: Slot| s.parts().len() as u8 - 1;
        let max = Loadout {
            body: last(Slot::Body),
            barrel: last(Slot::Barrel),
            hopper: last(Slot::Hopper),
            sight: last(Slot::Sight),
            stock: last(Slot::Stock),
            tank: last(Slot::Tank),
            foregrip: last(Slot::Foregrip),
            skin: last(Slot::Skin),
            ..Loadout::default()
        };
        assert_eq!(
            Loadout::unpack(max.pack()),
            Loadout {
                crosshair: 0,
                ..max
            }
        );
        // The shotgun fires a spread of balls.
        assert_eq!(
            Loadout {
                body: 5,
                ..Loadout::default()
            }
            .stats()
            .pellets,
            6
        );
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

    #[test]
    fn loadout_book_manage_presets() {
        let mut book = LoadoutBook::default();
        assert_eq!(book.presets.len(), 5);
        assert_eq!(book.active, 0);

        book.rename_active("  Rush Class  ");
        assert_eq!(book.active_preset().name, "Rush Class");

        let added = book.add_preset().unwrap();
        assert_eq!(book.active, added);
        assert_eq!(book.presets.len(), 6);

        assert!(book.delete_active());
        assert_eq!(book.presets.len(), 5);
        assert!(book.active < book.presets.len());

        // Empty name falls back to default numbered name.
        book.rename_active("   ");
        assert!(book.active_preset().name.starts_with("Loadout "));
    }
}
