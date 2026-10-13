// Written for Clustine from the pinned jar's own classes
// (`net.minecraft.world.level.levelgen.material`: `MaterialRuleContext`, the classes of
// its packages `condition` and `rule`, `VerticalAnchor`).

//! What the emitted material rules are written against.
//!
//! A material rule says what a block of ground becomes after the game's fill: grass,
//! sand, bedrock, deepslate, an ore of a vein. In the game's data it is a tree of
//! conditions and rules (`worldgen/material_rule`, `worldgen/material_condition`);
//! `cargo datagen` writes each dimension's tree as a function over a [`Context`], which
//! answers the conditions for the block that is being decided. The game compiles the
//! same tree into objects that remember an answer for a column or a block; an answer
//! depends only on the block, so a rule here asks again, and whoever implements
//! [`Context`] remembers what is costly.
//!
//! `clustine-terrain` implements the context (its module `surface`) and is compared
//! with the game there.

use crate::density::NoiseParameters;

/// A height that is given against the heights of the terrain (`VerticalAnchor`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchor {
    /// The height itself.
    Absolute(i32),
    /// So many blocks above the lowest block of the terrain.
    AboveBottom(i32),
    /// So many blocks below the highest block of the terrain.
    BelowTop(i32),
}

impl Anchor {
    /// The height, in terrain whose lowest block is at `min_y` and which is `height`
    /// blocks high (the game's `WorldGenerationContext`).
    #[must_use]
    pub const fn resolve(self, min_y: i32, height: i32) -> i32 {
        match self {
            Anchor::Absolute(y) => y,
            Anchor::AboveBottom(offset) => min_y + offset,
            Anchor::BelowTop(offset) => height - 1 + min_y - offset,
        }
    }
}

/// An ore vein (`OreVeinRule`): three blocks by their state ids, and three density
/// functions by their numbers among the functions the dimension's rule names
/// ([`MaterialRule::FUNCTIONS`], [`crate::NoiseRouter::material_density`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Vein {
    pub ore: u16,
    pub raw_ore: u16,
    pub filler: u16,
    /// How often a block of ore is a block of the raw ore.
    pub raw_ore_chance: f32,
    /// Above zero where the vein is, and then how likely a block is part of it.
    pub density: usize,
    /// How likely a block of the vein is ore.
    pub richness: usize,
    /// Ore only where this is below zero.
    pub filler_gap: usize,
}

/// What the conditions of a material rule ask about the block that is being decided.
/// Each method is one kind of condition of the game's data, with the condition's
/// numbers.
pub trait Context {
    /// `stone_depth`: whether the block is within so many blocks of the nearest air
    /// or fluid above it (`ceiling` false) or below it (true).
    fn stone_depth(
        &mut self,
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    ) -> bool;

    /// `biome`: whether the biome at the block is one of these, by their numbers in
    /// the registry.
    fn biome(&mut self, biomes: &[u16]) -> bool;

    /// `y_above`.
    fn y_above(
        &mut self,
        anchor: Anchor,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    ) -> bool;

    /// `water`: whether the block is not too far below the surface of a fluid.
    fn water(&mut self, offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool) -> bool;

    /// `hole`: whether the surface is no blocks deep here.
    fn hole(&mut self) -> bool;

    /// `steep`.
    fn steep(&mut self) -> bool;

    /// `temperature`: whether it is cold enough to snow at the block.
    fn temperature(&mut self) -> bool;

    /// `above_preliminary_surface`.
    fn above_preliminary_surface(&mut self) -> bool;

    /// `noise_threshold` of a noise that is read flat: whether the noise with this
    /// number among [`MaterialRule::NOISES`] is within the two bounds at the column.
    fn noise(&mut self, number: usize, min: f64, max: f64) -> bool;

    /// `noise_threshold` of a noise that is read at the block.
    fn noise_3d(&mut self, number: usize, min: f64, max: f64) -> bool;

    /// `vertical_gradient`: always at and below the first height, never at and above
    /// the second, and between them by the random numbers of the name with this
    /// number among [`MaterialRule::RANDOMS`].
    fn vertical_gradient(
        &mut self,
        random: usize,
        true_at_and_below: Anchor,
        false_at_and_above: Anchor,
    ) -> bool;

    /// `bandlands`: the band of terracotta at the block.
    fn band(&mut self) -> u16;

    /// `ore_vein`: the block of the vein at the block, if the vein is there.
    fn ore_vein(&mut self, vein: &Vein) -> Option<u16>;
}

/// The material rule of one dimension's noise settings, as emitted.
pub trait MaterialRule {
    /// The rule's name in the game's registry.
    const NAME: &'static str;

    /// The noises the rule's conditions ask, by the numbers the conditions give.
    const NOISES: &'static [&'static NoiseParameters];

    /// The names by which the rule's vertical gradients take their random numbers.
    const RANDOMS: &'static [&'static str];

    /// The density functions the rule's ore veins name, in the order of the numbers
    /// [`crate::NoiseRouter::material_density`] takes.
    const FUNCTIONS: &'static [&'static str];

    /// The state id the rule gives the block the context stands at, or `None` where
    /// the block stays what the fill made it.
    fn apply<C: Context>(context: &mut C) -> Option<u16>;
}

/// What the material rules read of a biome (`Biome.ClimateSettings`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BiomeClimate {
    /// The biome's name (`minecraft:frozen_ocean`).
    pub name: &'static str,
    pub temperature: f32,
    /// Whether the temperature is that of a frozen ocean, which is warmer in patches
    /// (`temperature_modifier` is `frozen`).
    pub frozen: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_is_counted_from_the_ends_of_the_terrain() {
        // The Nether: 128 blocks from 0.
        assert_eq!(Anchor::Absolute(31).resolve(0, 128), 31);
        assert_eq!(Anchor::AboveBottom(5).resolve(0, 128), 5);
        assert_eq!(Anchor::BelowTop(0).resolve(0, 128), 127);
        assert_eq!(Anchor::BelowTop(5).resolve(0, 128), 122);
        // The overworld: 384 from -64.
        assert_eq!(Anchor::AboveBottom(5).resolve(-64, 384), -59);
        assert_eq!(Anchor::BelowTop(0).resolve(-64, 384), 319);
    }
}
