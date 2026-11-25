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
    /// A box turned in the model's space: center, half its size along its
    /// own axes, rotation (x, y, z, w).
    Turned([f32; 3], [f32; 3], [f32; 4]),
    /// A cylinder along any axis: the middles of its two ends, radius.
    Rod([f32; 3], [f32; 3], f32),
    /// Any convex shape.
    Hull(&'static Hull),
}

/// A convex shape: its corners, the planes of its faces and its edges.
#[derive(Debug, PartialEq)]
pub struct Hull {
    pub points: &'static [[f32; 3]],
    /// Each face as a normal pointing out and `d`: inside is `n . p <= d`.
    pub planes: &'static [[f32; 4]],
    /// Pairs of `points` indices.
    pub edges: &'static [[u16; 2]],
}

/// Groups for the editor's palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Bunkers,
    Walls,
    Structures,
    Building,
    Interior,
    Industrial,
    Urban,
    Field,
    Props,
    Nature,
    Terrain,
    Pickups,
}

impl Category {
    pub const ALL: [Category; 12] = [
        Category::Bunkers,
        Category::Walls,
        Category::Structures,
        Category::Building,
        Category::Interior,
        Category::Industrial,
        Category::Urban,
        Category::Field,
        Category::Props,
        Category::Nature,
        Category::Terrain,
        Category::Pickups,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Bunkers => "Bunkers",
            Category::Walls => "Walls",
            Category::Structures => "Structures",
            Category::Building => "Building",
            Category::Interior => "Interior",
            Category::Industrial => "Industrial",
            Category::Urban => "Urban",
            Category::Field => "Field",
            Category::Props => "Props",
            Category::Nature => "Nature",
            Category::Terrain => "Terrain",
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

pub use crate::pieces::PIECES;

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
