//! Everything a map can place: each piece's model and the simple shapes that
//! collide with it, in the model's own space. Map files name pieces by `id`,
//! so ids must never change once maps use them.

/// One collision part of a piece, in the model's own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Part {
    /// Axis-aligned box: min, max.
    Box([f32; 3], [f32; 3]),
    /// Upright cylinder: x, z, radius, bottom, height.
    Cyl(f32, f32, f32, f32, f32),
    /// A slope rising toward the model's -Z: min, max.
    Ramp([f32; 3], [f32; 3]),
}

/// Groups for the editor's palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Bunkers,
    Walls,
    Structures,
    Props,
    Nature,
    Pickups,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Bunkers,
        Category::Walls,
        Category::Structures,
        Category::Props,
        Category::Nature,
        Category::Pickups,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Bunkers => "Bunkers",
            Category::Walls => "Walls",
            Category::Structures => "Structures",
            Category::Props => "Props",
            Category::Nature => "Nature",
            Category::Pickups => "Pickups",
        }
    }
}

/// A placeable piece.
#[derive(Debug, PartialEq)]
pub struct Piece {
    /// What map files call it.
    pub id: &'static str,
    /// What the editor calls it.
    pub label: &'static str,
    pub category: Category,
    /// Path under `assets/`.
    pub model: &'static str,
    /// Its collision shapes (none: you walk and shoot through it).
    pub parts: &'static [Part],
    /// Whether paint splats stick to it (false for odd shapes, where a flat
    /// decal on its bounding box would float in the air).
    pub decals: bool,
}

/// Looks a piece up by id.
pub fn piece(id: &str) -> Option<&'static Piece> {
    PIECES.iter().find(|p| p.id == id)
}

/// Looks a piece up by its model path.
pub fn piece_by_model(model: &str) -> Option<&'static Piece> {
    PIECES.iter().find(|p| p.model == model)
}

use Category::*;
use Part::{Box as B, Cyl as C, Ramp as R};

const CONTAINER: Part = B([-3.04, 0.0, -1.28], [3.1, 2.59, 1.28]);

macro_rules! p {
    ($id:literal, $label:literal, $cat:expr, $model:literal, $decals:expr, [$($part:expr),* $(,)?]) => {
        Piece { id: $id, label: $label, category: $cat, model: $model, parts: &[$($part),*], decals: $decals }
    };
}

/// Every piece, in palette order.
pub static PIECES: &[Piece] = &[
    // Inflatable bunkers.
    p!("can", "Can", Bunkers, "models/bunkers/can.glb", true, [C(0.0, 0.0, 0.537, 0.0, 1.31)]),
    p!("mini_can", "Mini can", Bunkers, "models/bunkers/mini_can.glb", true, [C(0.0, 0.0, 0.44, 0.0, 0.9)]),
    p!("cake", "Cake", Bunkers, "models/bunkers/cake.glb", true, [C(0.0, 0.0, 0.85, 0.0, 0.76)]),
    p!("bunker_tree", "Tree bunker", Bunkers, "models/bunkers/tree.glb", false, [C(0.0, 0.0, 0.5, 0.0, 1.95)]),
    p!("brick", "Brick", Bunkers, "models/bunkers/brick.glb", true, [B([-0.806, 0.0, -0.456], [0.806, 0.9, 0.456])]),
    p!("dorito", "Dorito", Bunkers, "models/bunkers/dorito.glb", false, [B([-0.675, 0.0, -0.39], [0.675, 1.15, 0.78])]),
    p!("snake", "Snake", Bunkers, "models/bunkers/snake.glb", true, [B([-1.8, 0.0, -0.356], [1.8, 0.75, 0.356])]),
    p!("tall_wall", "Tall wall", Bunkers, "models/bunkers/tall_wall.glb", true, [B([-1.006, 0.0, -0.306], [1.006, 2.0, 0.306])]),
    p!("temple", "Temple", Bunkers, "models/bunkers/temple.glb", false, [B([-0.7, 0.0, -0.606], [0.7, 1.6, 0.606])]),
    // Walls.
    p!("wall_2m", "Wall 2 m", Walls, "models/map-kit/walls/wall_2m.glb", true, [B([-1.0, 0.0, -0.08], [1.0, 2.4, 0.08])]),
    p!("wall_1m", "Wall 1 m", Walls, "models/map-kit/walls/wall_1m.glb", true, [B([-0.5, 0.0, -0.08], [0.5, 2.4, 0.08])]),
    p!("wall_low", "Low cover wall", Walls, "models/map-kit/walls/wall_low_cover.glb", true, [B([-1.0, 0.0, -0.08], [1.0, 1.1, 0.08])]),
    p!("wall_window", "Window wall", Walls, "models/map-kit/walls/wall_window.glb", true, [
        B([-1.0, 0.0, -0.08], [1.0, 1.0, 0.08]),
        B([-1.0, 1.6, -0.08], [1.0, 2.4, 0.08]),
        B([-1.0, 1.0, -0.08], [-0.7, 1.6, 0.08]),
        B([0.7, 1.0, -0.08], [1.0, 1.6, 0.08]),
    ]),
    p!("wall_door", "Doorway wall", Walls, "models/map-kit/walls/wall_doorway.glb", true, [
        B([-1.0, 0.0, -0.08], [-0.45, 2.4, 0.08]),
        B([0.45, 0.0, -0.08], [1.0, 2.4, 0.08]),
        B([-0.45, 2.0, -0.08], [0.45, 2.4, 0.08]),
    ]),
    p!("concrete_wall", "Concrete wall", Walls, "models/map-kit/walls/concrete_wall_2m.glb", true, [B([-1.0, 0.0, -0.15], [1.0, 2.4, 0.15])]),
    p!("jersey_barrier", "Jersey barrier", Walls, "models/map-kit/walls/jersey_barrier.glb", true, [B([-1.0, 0.0, -0.3], [1.0, 0.8, 0.3])]),
    p!("sandbags", "Sandbag wall", Walls, "models/props/sandbag_wall.glb", true, [B([-1.244, 0.0, -0.185], [1.244, 0.59, 0.185])]),
    p!("barricade", "Barricade", Walls, "models/weapons/gadgets/deployable_barricade.glb", true, [B([-0.6, 0.0, -0.39], [0.6, 1.0, 0.03])]),
    p!("fence_wood", "Wood fence", Walls, "models/map-kit/structures/fence_wood_2m.glb", false, []),
    p!("gate", "Gate", Walls, "models/map-kit/structures/gate_2m.glb", false, [B([-1.0, 0.0, -0.07], [1.0, 1.5, 0.07])]),
    p!("net_section", "Net section", Walls, "models/props/netting_fence_section.glb", false, []),
    // Structures.
    p!("container_green", "Container (green)", Structures, "models/map-kit/structures/shipping_container_green.glb", true, [CONTAINER]),
    p!("container_rust", "Container (rust)", Structures, "models/map-kit/structures/shipping_container_rust.glb", true, [CONTAINER]),
    p!("container_blue", "Container (blue)", Structures, "models/map-kit/structures/shipping_container_blue.glb", true, [CONTAINER]),
    p!("platform", "Platform 2x2", Structures, "models/map-kit/structures/platform_2x2.glb", true, [B([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0])]),
    p!("ramp", "Ramp 2 m", Structures, "models/map-kit/structures/ramp_2m.glb", false, [R([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0])]),
    p!("stairs", "Stairs 1 m", Structures, "models/map-kit/structures/stairs_1m.glb", false, [R([-0.6, 0.0, -1.0], [0.6, 1.0, 1.0])]),
    p!("bridge", "Bridge 4 m", Structures, "models/map-kit/structures/bridge_4m.glb", true, [B([-2.0, 0.0, -0.61], [2.0, 1.0, 0.61])]),
    p!("shack", "Field shack", Structures, "models/map-kit/structures/field_shack.glb", true, [B([-1.7, 0.0, -1.8], [1.7, 3.0, 1.8])]),
    p!("watch_tower", "Watch tower", Structures, "models/map-kit/structures/watch_tower.glb", true, [
        C(-1.1, -1.1, 0.12, 0.0, 3.0),
        C(1.1, -1.1, 0.12, 0.0, 3.0),
        C(-1.1, 1.1, 0.12, 0.0, 3.0),
        C(1.1, 1.1, 0.12, 0.0, 3.0),
        B([-1.3, 2.9, -1.3], [1.3, 4.1, 1.3]),
    ]),
    p!("scoreboard", "Scoreboard", Structures, "models/map-kit/structures/scoreboard_stand.glb", false, []),
    // Props.
    p!("barrel", "Barrel", Props, "models/props/barrel.glb", true, [C(0.0, 0.0, 0.295, 0.0, 0.885)]),
    p!("crate", "Crate", Props, "models/props/crate.glb", true, [B([-0.4, 0.0, -0.4], [0.4, 0.8, 0.4])]),
    p!("pallet_stack", "Pallet stack", Props, "models/map-kit/props/pallet_stack.glb", true, [B([-0.6, 0.0, -0.5], [0.6, 0.46, 0.5])]),
    p!("pallet", "Pallet", Props, "models/map-kit/props/pallet.glb", false, []),
    p!("tires", "Tire stack", Props, "models/map-kit/props/tire_stack.glb", true, [C(0.0, 0.0, 0.36, 0.0, 0.88)]),
    p!("spool", "Cable spool", Props, "models/map-kit/props/cable_spool.glb", true, [B([-0.68, 0.0, -0.48], [0.68, 1.38, 0.48])]),
    p!("hay", "Hay bale", Props, "models/map-kit/props/hay_bale.glb", true, [B([-0.5, 0.0, -0.24], [0.5, 0.51, 0.24])]),
    p!("log_pile", "Log pile", Props, "models/map-kit/props/log_pile.glb", true, [B([-1.1, 0.0, -0.46], [1.1, 0.83, 0.46])]),
    p!("floodlight", "Floodlight", Props, "models/map-kit/props/floodlight_pole.glb", false, [C(0.0, 0.0, 0.12, 0.0, 6.25)]),
    p!("sign", "Sign post", Props, "models/map-kit/props/sign_post.glb", false, []),
    p!("boundary_post", "Boundary post", Props, "models/map-kit/props/boundary_post.glb", false, []),
    p!("cone", "Field cone", Props, "models/props/field_cone.glb", false, []),
    p!("grenade_crate", "Grenade crate", Props, "models/weapons/gadgets/grenade_crate.glb", false, []),
    // Nature.
    p!("oak", "Oak", Nature, "models/map-kit/nature/tree_oak.glb", false, [C(0.0, 0.0, 0.3, 0.0, 4.0)]),
    p!("pine", "Pine", Nature, "models/map-kit/nature/tree_pine.glb", false, [C(0.0, 0.0, 0.4, 0.0, 3.4)]),
    p!("pine_tall", "Tall pine", Nature, "models/map-kit/nature/tree_pine_tall.glb", false, [C(0.0, 0.0, 0.35, 0.0, 5.2)]),
    p!("bush", "Bush", Nature, "models/map-kit/nature/bush.glb", false, []),
    p!("grass", "Grass clump", Nature, "models/map-kit/nature/grass_clump.glb", false, []),
    p!("rock_large", "Large rock", Nature, "models/map-kit/nature/rock_large.glb", false, [B([-1.6, 0.0, -1.0], [1.8, 1.9, 1.3])]),
    p!("rock_medium", "Medium rock", Nature, "models/map-kit/nature/rock_medium.glb", false, [B([-0.65, 0.0, -0.6], [0.9, 0.85, 0.7])]),
    p!("rock_small", "Small rock", Nature, "models/map-kit/nature/rock_small.glb", false, []),
    p!("log", "Fallen log", Nature, "models/map-kit/nature/fallen_log.glb", false, [B([-1.41, 0.0, -0.2], [1.41, 0.6, 0.43])]),
    p!("stump", "Tree stump", Nature, "models/map-kit/nature/tree_stump.glb", false, [C(0.0, 0.0, 0.3, 0.0, 0.46)]),
    // Pickups (scenery: they don't do anything yet).
    p!("paint_case", "Paint case", Pickups, "models/pickups/paint_case.glb", false, []),
    p!("pod_pickup", "Pod pickup", Pickups, "models/pickups/pod_pickup.glb", false, []),
    p!("air_refill", "Air refill station", Pickups, "models/pickups/air_refill_station.glb", false, []),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_models_are_unique() {
        for (i, a) in PIECES.iter().enumerate() {
            for b in &PIECES[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.model, b.model);
            }
        }
    }
}
