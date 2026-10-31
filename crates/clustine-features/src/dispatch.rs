//! Which step of the plan builds each kind of configured feature
//! (`docs/groundwork/f-plan.md`, section 3), and from a kind to the code that reads it.
//! Each step has a module of its own under `features`, so steps that run side by side
//! never edit one file; this table is not edited by them.

use crate::feature::Feature;
use crate::features;
use crate::reader::{Read, Reader};

/// The steps that build kinds of features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Batch {
    /// F1b: the combinators, simple blocks, columns and ores.
    Frame,
    /// F2: the ground's features.
    Ground,
    /// F3: trees.
    Trees,
    /// F4: plants.
    Plants,
    /// F5: caves and buried things.
    Caves,
    /// F7: the Nether's and the End's.
    NetherAndEnd,
}

/// Every kind of configured feature in the pinned jar's data, nested ones included,
/// with the step that builds it.
pub const KINDS: [(&str, Batch); 56] = [
    ("minecraft:bamboo", Batch::Plants),
    ("minecraft:block_blob", Batch::Ground),
    ("minecraft:block_column", Batch::Frame),
    ("minecraft:block_pile", Batch::Ground),
    ("minecraft:blue_ice", Batch::Ground),
    ("minecraft:bonus_chest", Batch::NetherAndEnd),
    ("minecraft:chorus_plant", Batch::NetherAndEnd),
    ("minecraft:coral_claw", Batch::Plants),
    ("minecraft:coral_tree", Batch::Plants),
    ("minecraft:delta_feature", Batch::NetherAndEnd),
    ("minecraft:disk", Batch::Ground),
    ("minecraft:end_gateway", Batch::NetherAndEnd),
    ("minecraft:end_island", Batch::NetherAndEnd),
    ("minecraft:end_platform", Batch::NetherAndEnd),
    ("minecraft:end_podium", Batch::NetherAndEnd),
    ("minecraft:end_spike", Batch::NetherAndEnd),
    ("minecraft:fallen_tree", Batch::Trees),
    ("minecraft:fossil", Batch::Caves),
    ("minecraft:freeze_top_layer", Batch::Ground),
    ("minecraft:geode", Batch::Caves),
    ("minecraft:huge_brown_mushroom", Batch::Trees),
    ("minecraft:huge_fungus", Batch::NetherAndEnd),
    ("minecraft:huge_red_mushroom", Batch::Trees),
    ("minecraft:iceberg", Batch::Ground),
    ("minecraft:lake", Batch::Ground),
    ("minecraft:large_dripstone", Batch::Caves),
    ("minecraft:monster_room", Batch::Ground),
    ("minecraft:multiface_growth", Batch::Plants),
    ("minecraft:netherrack_replace_blobs", Batch::NetherAndEnd),
    ("minecraft:no_op", Batch::Frame),
    ("minecraft:ore", Batch::Frame),
    ("minecraft:overlay", Batch::Frame),
    ("minecraft:projected_random_patchy_square", Batch::Plants),
    ("minecraft:random_boolean_selector", Batch::Frame),
    ("minecraft:random_neighbor_spread", Batch::NetherAndEnd),
    ("minecraft:random_selector", Batch::Frame),
    ("minecraft:root_system", Batch::Trees),
    ("minecraft:scattered_ore", Batch::Frame),
    ("minecraft:sculk_patch", Batch::Plants),
    ("minecraft:sequence", Batch::Frame),
    ("minecraft:simple_block", Batch::Frame),
    ("minecraft:simple_random_selector", Batch::Frame),
    ("minecraft:single_block_pillar", Batch::Ground),
    ("minecraft:speleothem", Batch::Caves),
    ("minecraft:speleothem_cluster", Batch::Caves),
    ("minecraft:spike", Batch::Ground),
    ("minecraft:spring_feature", Batch::Ground),
    ("minecraft:stepped_column_cluster", Batch::Caves),
    ("minecraft:template", Batch::Caves),
    ("minecraft:tree", Batch::Trees),
    ("minecraft:underwater_magma", Batch::Ground),
    ("minecraft:vegetation_patch", Batch::Plants),
    ("minecraft:vines", Batch::Plants),
    ("minecraft:void_start_platform", Batch::NetherAndEnd),
    ("minecraft:waterlogged_vegetation_patch", Batch::Plants),
    ("minecraft:weighted_random_selector", Batch::Frame),
];

/// The step that builds `kind`.
pub fn batch(kind: &str) -> Option<Batch> {
    KINDS
        .binary_search_by(|(name, _)| (*name).cmp(kind))
        .ok()
        .map(|index| KINDS[index].1)
}

/// Reads a configured feature of `kind` from the members of `reader` beside `type`.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    let batch =
        batch(kind).ok_or_else(|| reader.error(format!("{kind} is not a kind of feature")))?;
    match batch {
        Batch::Frame => features::frame::read(kind, reader),
        Batch::Ground => features::ground::read(kind, reader),
        Batch::Trees => features::trees::read(kind, reader),
        Batch::Plants => features::plants::read(kind, reader),
        Batch::Caves => features::caves::read(kind, reader),
        Batch::NetherAndEnd if features::end::reads(kind) => features::end::read(kind, reader),
        Batch::NetherAndEnd => features::nether_and_end::read(kind, reader),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kinds_are_sorted_so_that_a_kind_is_found() {
        assert!(KINDS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(batch("minecraft:tree"), Some(Batch::Trees));
        assert_eq!(batch("minecraft:no_such_kind"), None);
    }
}
