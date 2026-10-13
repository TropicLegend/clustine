// Written for Clustine from the pinned jar's own classes
// (`net.minecraft.world.level.levelgen.carver.CaveWorldCarver`, `CanyonWorldCarver` and
// its `Shape`, `net.minecraft.util.valueproviders`, `UniformHeight`, `VerticalAnchor`).

//! What the emitted carvers are written with.
//!
//! A carver cuts caves or ravines into the ground after the fill and the surface. In
//! 26.3 a carver is one of two records, a cave or a canyon, and everything about it
//! is data (`worldgen/carver`): how often a chunk starts one, at which heights, how
//! thick, how far. `cargo datagen` writes the game's four as statics
//! ([`crate::carvers::CARVERS`]) and which of them each biome has, in the biome's
//! order ([`crate::carvers::OF_BIOME`]).
//!
//! The numbers a carver draws at random are given as providers: a description of how
//! a number is drawn, not the number. Here are the descriptions alone, with no
//! generator of random numbers; `clustine-terrain` draws (its module `carver`), in the
//! game's order, and is compared with the game there.

use crate::material::Anchor;

/// How a `float` is drawn (`FloatProvider`). Only the kinds the game's carvers use.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FloatProvider {
    /// The number itself; nothing is drawn.
    Constant(f32),
    /// Evenly between two ends: one draw.
    Uniform {
        min_inclusive: f32,
        max_exclusive: f32,
    },
    /// A trapezoid: two draws.
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

/// How an `int` is drawn (`IntProvider`). Only the kind the game's carvers use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntProvider {
    /// A draw below a draw below a draw, which is nearly always small: three draws.
    VeryBiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
}

/// A height drawn evenly between two anchors, both included (`UniformHeight`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UniformHeight {
    pub min_inclusive: Anchor,
    pub max_inclusive: Anchor,
}

/// A carver of caves (`CaveWorldCarver`): rooms, and tunnels that branch.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cave {
    /// How likely a chunk is to start caves.
    pub probability: f32,
    pub y: UniformHeight,
    /// How many caves a chunk that starts any starts.
    pub count: IntProvider,
    pub thickness: FloatProvider,
    /// Whether one tunnel in ten is made thicker.
    pub weird_thickness_bias: bool,
    pub room_vertical_radius_multiplier: FloatProvider,
    pub horizontal_radius_multiplier: FloatProvider,
    pub vertical_radius_multiplier: FloatProvider,
    pub start_vertical_radius_multiplier: FloatProvider,
    /// Below this part of a tunnel's height nothing is carved, which leaves a floor.
    pub floor_level: FloatProvider,
}

/// A carver of ravines (`CanyonWorldCarver` with its `Shape`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Canyon {
    /// How likely a chunk is to start a ravine.
    pub probability: f32,
    pub y: UniformHeight,
    pub vertical_rotation: FloatProvider,
    pub distance_factor: FloatProvider,
    pub thickness: FloatProvider,
    /// One height in so many gets a width of its own; the others keep the last.
    pub width_smoothness: i32,
    pub horizontal_radius_factor: FloatProvider,
    pub vertical_radius_default_factor: f32,
    pub vertical_radius_center_factor: f32,
    pub y_scale: FloatProvider,
}

/// One of the two kinds of carver.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Cave(Cave),
    Canyon(Canyon),
}

/// An entry of `worldgen/carver`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Carver {
    /// The carver's name in the game's registry (`minecraft:cave`).
    pub name: &'static str,
    pub kind: Kind,
}

impl Carver {
    /// How likely a chunk is to start this carver.
    #[must_use]
    pub const fn probability(&self) -> f32 {
        match &self.kind {
            Kind::Cave(cave) => cave.probability,
            Kind::Canyon(canyon) => canyon.probability,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carvers::{CARVERS, OF_BIOME};

    #[test]
    fn the_games_four_carvers_are_emitted_by_name() {
        let names: Vec<&str> = CARVERS.iter().map(|carver| carver.name).collect();
        assert_eq!(
            names,
            [
                "minecraft:canyon",
                "minecraft:cave",
                "minecraft:cave_extra_underground",
                "minecraft:nether_cave"
            ]
        );
        assert!(matches!(CARVERS[0].kind, Kind::Canyon(_)));
        let Kind::Cave(nether) = CARVERS[3].kind else {
            panic!("the Nether's carver is a cave");
        };
        // What the Nether's caves have and the overworld's leave at the default.
        assert_eq!(
            nether.start_vertical_radius_multiplier,
            FloatProvider::Constant(5.0)
        );
        assert!(!nether.weird_thickness_bias);
        let Kind::Cave(cave) = CARVERS[1].kind else {
            panic!("a cave");
        };
        assert_eq!(
            cave.start_vertical_radius_multiplier,
            FloatProvider::Constant(1.0)
        );
        assert!(cave.weird_thickness_bias);
        assert_eq!(cave.y.min_inclusive, Anchor::AboveBottom(8));
    }

    #[test]
    fn a_biome_has_its_carvers_in_the_datas_order() {
        let biomes = crate::biomes::CLIMATE;
        assert_eq!(OF_BIOME.len(), biomes.len());
        let of = |name: &str| -> Vec<&str> {
            let number = biomes
                .iter()
                .position(|biome| biome.name == name)
                .expect("a biome of the game");
            OF_BIOME[number]
                .iter()
                .map(|carver| CARVERS[*carver].name)
                .collect()
        };
        // The order is the data's, not the names': it is part of a carver's seed.
        assert_eq!(
            of("minecraft:plains"),
            [
                "minecraft:cave",
                "minecraft:cave_extra_underground",
                "minecraft:canyon"
            ]
        );
        assert_eq!(of("minecraft:nether_wastes"), ["minecraft:nether_cave"]);
        assert!(of("minecraft:the_end").is_empty());
        assert!(of("minecraft:the_void").is_empty());
    }
}
