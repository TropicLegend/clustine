//! Saplings and the trees they grow (`docs/groundwork/n8-plan.md`, section 7.1):
//! `SaplingBlock.advanceTree` and the game's `TreeGrower`, which places its tree's
//! feature through the region's view (`feature_view.rs`). Bone meal (`bone_meal.rs`)
//! calls [`Region::advance_tree`] for a sapling's and a propagule's success.
//!
//! The random (the plan's 7.3). The game hands the level's random to the grower and to
//! the feature alike. Here the grower's own draws, the weighted lists', are drawn from the
//! stream it is given, as the game draws them from the level's random, so that a sapling
//! that grows nothing (a dark oak alone, a world without features) takes from the stream
//! exactly what the game's takes from the level's random. The feature's draws, of which
//! the game makes a number that depends on the tree, are drawn instead from a random of
//! the game's kind seeded with one long of the stream, drawn where the game's feature
//! would make its first draw: after the lists and after the saplings are taken away, and
//! only where there is a feature to place. So the stream's count is the game's up to the
//! feature, and one draw for all of the feature's.

use clustine_blocks::data::{in_tag, is_class, number, set};
use clustine_data::{BlockClass, BlockState, blocks};
use clustine_world::BlockPos;

use super::updates::{UPDATE_LIMIT, fluid_block};
use super::{Random, Region};
use crate::api::TickOutput;

/// The flags a tree's feature sets its logs, leaves and decorations with
/// (`TreeFeature`'s setter, n8 TreeFeature: 19).
const TREE_FLAGS: u32 = 19;

/// `UPDATE_INVISIBLE | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`: a sapling's new stage, and
/// the saplings put back where the tree did not grow.
const SAPLING_FLAGS: u32 = 260;

/// The flags the saplings are taken away with before the tree (`removeSapling`): every
/// update but the neighbours', the shapes', the drops' and the block entities'.
const REMOVE_FLAGS: u32 = 818;

/// A list of configured features by name with their weights (`WeightedList`), in the
/// game's order.
type Weighted = &'static [(&'static str, u32)];

/// The game's `TreeGrower`: the trees a sapling of its kind grows, alone (`trees`, or
/// `flowers` where a flower stands near), and from four in a square (`mega`), and the
/// tree whose trunk is shortest, by which bone meal judges the room above.
#[derive(Debug)]
pub(crate) struct TreeGrower {
    trees: Weighted,
    mega: Weighted,
    flowers: Weighted,
    shortest: Option<&'static str>,
}

/// The growers of n8 TreeGrower 514–743, each with its three lists as the class's static
/// initialiser builds them.
pub(crate) const OAK: TreeGrower = TreeGrower {
    trees: &[("minecraft:oak", 9), ("minecraft:fancy_oak", 1)],
    mega: &[],
    flowers: &[
        ("minecraft:oak_bees_005", 9),
        ("minecraft:fancy_oak_bees_005", 1),
    ],
    shortest: Some("minecraft:oak"),
};
pub(crate) const SPRUCE: TreeGrower = TreeGrower {
    trees: &[("minecraft:spruce", 1)],
    mega: &[("minecraft:mega_spruce", 1), ("minecraft:mega_pine", 1)],
    flowers: &[],
    shortest: Some("minecraft:spruce"),
};
pub(crate) const MANGROVE: TreeGrower = TreeGrower {
    trees: &[("minecraft:mangrove", 15), ("minecraft:tall_mangrove", 85)],
    mega: &[],
    flowers: &[],
    shortest: Some("minecraft:mangrove"),
};
pub(crate) const AZALEA: TreeGrower = TreeGrower {
    trees: &[("minecraft:azalea_tree", 1)],
    mega: &[],
    flowers: &[],
    shortest: Some("minecraft:azalea_tree"),
};
pub(crate) const BIRCH: TreeGrower = TreeGrower {
    trees: &[("minecraft:birch", 1)],
    mega: &[],
    flowers: &[("minecraft:birch_bees_005", 1)],
    shortest: Some("minecraft:birch"),
};
pub(crate) const JUNGLE: TreeGrower = TreeGrower {
    trees: &[("minecraft:jungle_tree_no_vine", 1)],
    mega: &[("minecraft:mega_jungle_tree", 1)],
    flowers: &[],
    shortest: Some("minecraft:jungle_tree_no_vine"),
};
pub(crate) const ACACIA: TreeGrower = TreeGrower {
    trees: &[("minecraft:acacia", 1)],
    mega: &[],
    flowers: &[],
    shortest: Some("minecraft:acacia"),
};
pub(crate) const CHERRY: TreeGrower = TreeGrower {
    trees: &[("minecraft:cherry", 1)],
    mega: &[],
    flowers: &[("minecraft:cherry_bees_005", 1)],
    shortest: Some("minecraft:cherry"),
};
pub(crate) const DARK_OAK: TreeGrower = TreeGrower {
    trees: &[],
    mega: &[("minecraft:dark_oak", 1)],
    flowers: &[],
    shortest: None,
};
pub(crate) const PALE_OAK: TreeGrower = TreeGrower {
    trees: &[],
    mega: &[("minecraft:pale_oak_bonemeal", 1)],
    flowers: &[],
    shortest: None,
};
pub(crate) const POPLAR: TreeGrower = TreeGrower {
    trees: &[
        ("minecraft:red_poplar", 1),
        ("minecraft:orange_poplar", 1),
        ("minecraft:yellow_poplar", 1),
    ],
    mega: &[],
    flowers: &[],
    shortest: Some("minecraft:red_poplar"),
};

impl TreeGrower {
    /// The grower of the sapling, propagule or azalea `state` (n6 Blocks: each
    /// `SaplingBlock`'s grower, the propagule's `MANGROVE`, the azaleas' `AZALEA`).
    pub(crate) fn of(state: BlockState) -> Option<&'static TreeGrower> {
        Some(match state.block()?.name {
            "minecraft:oak_sapling" => &OAK,
            "minecraft:spruce_sapling" => &SPRUCE,
            "minecraft:birch_sapling" => &BIRCH,
            "minecraft:jungle_sapling" => &JUNGLE,
            "minecraft:acacia_sapling" => &ACACIA,
            "minecraft:cherry_sapling" => &CHERRY,
            "minecraft:dark_oak_sapling" => &DARK_OAK,
            "minecraft:pale_oak_sapling" => &PALE_OAK,
            "minecraft:poplar_sapling" => &POPLAR,
            "minecraft:mangrove_propagule" => &MANGROVE,
            "minecraft:azalea" | "minecraft:flowering_azalea" => &AZALEA,
            _ => return None,
        })
    }

    /// `getMinimumHeight`: the base height of the trunk of the grower's shortest tree,
    /// none for a grower without one (dark oak, pale oak) or whose shortest is not a
    /// tree.
    #[cfg_attr(not(test), expect(dead_code, reason = "bone meal's (N8 D4b)"))]
    pub(crate) fn minimum_height(&self) -> Option<i32> {
        let (_, entry) = clustine_worldgen_data::feature::configured(self.shortest?)?;
        if entry.data.kind() != Some("minecraft:tree") {
            return None;
        }
        match entry.data.get("trunk_placer")?.get("base_height")? {
            clustine_worldgen_data::feature::Data::Number(text) => text.parse().ok(),
            _ => None,
        }
    }

    /// `getConfiguredFeature`: the flower list where a flower stands near and the list
    /// is not empty, else the plain one; each drawn whenever it is not empty.
    fn configured(&self, random: &mut Random, flowers: bool) -> Option<&'static str> {
        if flowers && !self.flowers.is_empty() {
            return weighted(self.flowers, random);
        }
        weighted(self.trees, random)
    }
}

/// `WeightedList.getRandom` (n8 WeightedList 149–166): nothing and no draw for an empty
/// list; otherwise `nextInt(totalWeight)`, a list of one too, and the entry whose share of
/// the weights, in the list's order, holds the number drawn.
pub(crate) fn weighted(list: Weighted, random: &mut Random) -> Option<&'static str> {
    if list.is_empty() {
        return None;
    }
    let total: u32 = list.iter().map(|(_, weight)| weight).sum();
    let mut drawn = random.next_int(total);
    for (name, weight) in list {
        if drawn < *weight {
            return Some(name);
        }
        drawn -= weight;
    }
    None
}

/// Saplings a tree takes away, each with the state it is put back as and where.
pub(crate) type Saplings = Vec<(BlockState, BlockPos)>;

/// A tree the grower has chosen: its feature, where it is placed, and the saplings it
/// takes away first and puts back if it fails, with the states they are put back as.
#[derive(Debug, PartialEq)]
pub(crate) struct Chosen {
    pub(crate) feature: &'static str,
    pub(crate) at: BlockPos,
    pub(crate) saplings: Saplings,
}

impl Region {
    /// `SaplingBlock.advanceTree` of the sapling or propagule `state` at `pos`: a sapling
    /// of stage 0 takes stage 1 (flags 260); one of stage 1 grows its grower's tree,
    /// drawing the grower's lists from `random` and the tree's from a random seeded with
    /// `random`'s next long (the plan's 7.3; this module's comment).
    pub(crate) fn advance_tree(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if number(state, "stage") == Some(0) {
            // `cycle(STAGE)`: stage has two values, so 0 becomes 1.
            let grown = set(state, "stage", "1");
            self.set_block_updated(pos, grown, SAPLING_FLAGS, UPDATE_LIMIT, output);
            return;
        }
        if let Some(grower) = TreeGrower::of(state) {
            self.grow_tree(grower, pos, state, random, output);
        }
    }

    /// `TreeGrower.growTree` (n8 TreeGrower 101–219): the tree chosen, its saplings taken
    /// away to the block of their fluid (flags 818), the feature placed, and the saplings
    /// put back as they were (flags 260) where it did not grow. Whether it grew.
    pub(crate) fn grow_tree(
        &mut self,
        grower: &TreeGrower,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let Some(chosen) = self.choose_tree(grower, pos, state, random) else {
            return false;
        };
        for (_, at) in &chosen.saplings {
            // `removeSapling`: the fluid's legacy block, water for a waterlogged
            // propagule.
            let left = fluid_block(self.block(*at).unwrap_or(blocks::AIR));
            self.set_block_updated(*at, left, REMOVE_FLAGS, UPDATE_LIMIT, output);
        }
        let seed = random.next_u64();
        if self.place_feature(chosen.feature, chosen.at, TREE_FLAGS, seed, output) {
            return true;
        }
        for (sapling, at) in chosen.saplings {
            self.set_block_updated(at, sapling, SAPLING_FLAGS, UPDATE_LIMIT, output);
        }
        false
    }

    /// What `growTree` places, with its lists drawn from `random` in the game's order:
    /// the mega list first, always; where the region has that feature and four saplings
    /// of the block stand in a square with this one, the first square of the four in
    /// the game's order, else the flower or the plain list. None where the list chosen
    /// is empty or the region lacks its feature (the game's registry without it).
    pub(crate) fn choose_tree(
        &self,
        grower: &TreeGrower,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
    ) -> Option<Chosen> {
        if let Some(mega) = weighted(grower.mega, random)
            && self.has_feature(mega)
            && let Some((dx, dz, saplings)) = self.two_by_two(state, pos)
        {
            return Some(Chosen {
                feature: mega,
                at: pos.offset(dx, 0, dz),
                saplings,
            });
        }
        let flowers = self.has_flowers(pos);
        let feature = grower.configured(random, flowers)?;
        if !self.has_feature(feature) {
            return None;
        }
        Some(Chosen {
            feature,
            at: pos,
            saplings: vec![(state, pos)],
        })
    }

    /// `TreeGrower.canGrow` (n8 TreeGrower 441–468), for bone meal's
    /// `isValidBonemealTarget`: the plain or flower list drawn first, then the mega list,
    /// both from `random`; a grower whose plain list gives nothing and whose mega list
    /// gives a tree can grow only where a square of four stands. It asks no registry.
    #[cfg_attr(not(test), expect(dead_code, reason = "bone meal's (N8 D4b)"))]
    pub(crate) fn can_grow(
        &self,
        grower: &TreeGrower,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
    ) -> bool {
        let flowers = self.has_flowers(pos);
        let plain = grower.configured(random, flowers);
        let mega = weighted(grower.mega, random);
        if plain.is_none() && mega.is_some() {
            return self.two_by_two(state, pos).is_some();
        }
        true
    }

    /// Whether the region was given the configured feature `name`: the game's registry
    /// lookup, which in a world without features (a flat world, a test) finds nothing.
    fn has_feature(&self, name: &str) -> bool {
        self.config
            .features
            .0
            .as_ref()
            .is_some_and(|settings| settings.configured_named(name).is_some())
    }

    /// `hasFlowers`: a block of `#flowers` in the box from (−2, −1, −2) to (2, 1, 2)
    /// about `pos`.
    fn has_flowers(&self, pos: BlockPos) -> bool {
        (-2..=2).any(|dx| {
            (-1..=1).any(|dy| {
                (-2..=2).any(|dz| {
                    self.block(pos.offset(dx, dy, dz))
                        .is_some_and(|there| in_tag(there, "minecraft:flowers"))
                })
            })
        })
    }

    /// `findTwoByTwoSaplingPos` (n8 TreeGrower 470–506): of the squares with `pos` at
    /// offsets (0, 0), (0, −1), (−1, 0) and (−1, −1) in that order, the first whose four
    /// blocks are all `state`'s block; its corner's offset and its four saplings in the
    /// game's order, as read.
    fn two_by_two(&self, state: BlockState, pos: BlockPos) -> Option<(i32, i32, Saplings)> {
        for dx in [0, -1] {
            for dz in [0, -1] {
                let square: Saplings = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .into_iter()
                    .map(|(x, z)| {
                        let at = pos.offset(dx + x, 0, dz + z);
                        (self.block(at).unwrap_or(blocks::AIR), at)
                    })
                    .collect();
                if square
                    .iter()
                    .all(|(there, _)| clustine_blocks::data::is_block(*there, state))
                {
                    return Some((dx, dz, square));
                }
            }
        }
        None
    }
}

/// Whether `state` is a sapling that grows by the light above it: `SaplingBlock`'s own
/// random tick, which the ground propagule overrides.
pub(crate) fn is_sapling(state: BlockState) -> bool {
    is_class(state, BlockClass::SaplingBlock)
        && !is_class(state, BlockClass::MangrovePropaguleBlock)
}
