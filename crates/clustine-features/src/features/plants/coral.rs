// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/coral.rs`.

//! `coral_claw` and `coral_tree`: the shapes of warm oceans' coral, a claw of
//! branches or a trunk with branches, each of whose blocks is a placed feature of the
//! data (`CoralClawFeature`, `CoralTreeFeature`).
//!
//! In 26.3 the data says what a block of coral is: the placed feature `feature`,
//! which in the pinned jar's data tests for water there and above, places a coral
//! block and decorates it with corals, fans and sea pickles (`coral/*_block`). The
//! shapes only walk; a block that is not placed ends the walk as the game's
//! `placeCoralBlock` returning false did. SteelMC places the coral block and its
//! decoration in code, as the game before 26.3 did, and draws the coral block from
//! the tag; here the selector of the data has drawn it, and the feature places it.

use clustine_data::Face;
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::{anticlockwise, clockwise, shuffle};
use crate::blocks::{HORIZONTAL, relative};
use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::reader::{Read, Reader};

/// Which of the two shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Claw,
    Tree,
}

#[derive(Debug)]
pub struct Coral {
    shape: Shape,
    /// A block of coral with what grows on it.
    feature: PlacedRef,
}

impl Coral {
    pub fn read(reader: &Reader, shape: Shape) -> Read<Self> {
        Ok(Self {
            shape,
            feature: PlacedRef::read(&reader.field("feature")?)?,
        })
    }

    /// `placeCoralBlock`.
    fn block(&self, context: &mut Context<'_, '_>, at: BlockPos) -> bool {
        self.feature.place(context, at)
    }

    fn claw(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !self.block(context, origin) {
            return false;
        }
        let claw = HORIZONTAL[context.random.next_i32_bounded(4) as usize];
        let branches = context.random.next_i32_bounded(2) + 2;
        let mut directions = [claw, clockwise(claw), anticlockwise(claw)];
        shuffle(&mut *context.random, &mut directions);
        for branch in directions.into_iter().take(branches as usize) {
            let mut at = relative(origin, branch);
            let sideways = context.random.next_i32_bounded(2) + 1;
            let (segment, inwards) = if branch == claw {
                (claw, context.random.next_i32_bounded(3) + 2)
            } else {
                at = relative(at, Face::Up);
                // `Util.getRandom` of the branch's direction and up.
                let segment = [branch, Face::Up][context.random.next_i32_bounded(2) as usize];
                (segment, context.random.next_i32_bounded(3) + 3)
            };
            for _ in 0..sideways {
                if !self.block(context, at) {
                    break;
                }
                at = relative(at, segment);
            }
            at = relative(relative(at, segment.opposite()), Face::Up);
            for _ in 0..inwards {
                at = relative(at, claw);
                if !self.block(context, at) {
                    break;
                }
                if context.random.next_f32() < 0.25 {
                    at = relative(at, Face::Up);
                }
            }
        }
        true
    }

    fn tree(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut at = origin;
        let trunk = context.random.next_i32_bounded(3) + 1;
        for _ in 0..trunk {
            if !self.block(context, at) {
                // The game says a tree whose trunk is cut short was placed.
                return true;
            }
            at = relative(at, Face::Up);
        }
        let top = at;
        let branches = context.random.next_i32_bounded(3) + 2;
        let mut directions = HORIZONTAL;
        shuffle(&mut *context.random, &mut directions);
        for branch in directions.into_iter().take(branches as usize) {
            at = relative(top, branch);
            let height = context.random.next_i32_bounded(5) + 2;
            let mut segment = 0;
            for j in 0..height {
                if !self.block(context, at) {
                    break;
                }
                segment += 1;
                at = relative(at, Face::Up);
                if j == 0 || segment >= 2 && context.random.next_f32() < 0.25 {
                    at = relative(at, branch);
                    segment = 0;
                }
            }
        }
        true
    }
}

impl Feature for Coral {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        match self.shape {
            Shape::Claw => self.claw(context, origin),
            Shape::Tree => self.tree(context, origin),
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::blocks::same_block;
    use crate::testing::World;

    /// A coral tree of the data's tube coral, as `warm_ocean_vegetation` names it.
    static TREE: Data = Data::Object(&[
        ("type", Data::Text("minecraft:coral_tree")),
        (
            "feature",
            Data::Object(&[
                ("feature", Data::Text("minecraft:coral/tube_block")),
                (
                    "placement",
                    Data::List(&[Data::Object(&[
                        ("type", Data::Text("minecraft:block_predicate_filter")),
                        (
                            "predicate",
                            Data::Object(&[
                                ("type", Data::Text("minecraft:all_of")),
                                (
                                    "predicates",
                                    Data::List(&[
                                        Data::Object(&[
                                            ("type", Data::Text("minecraft:any_of")),
                                            (
                                                "predicates",
                                                Data::List(&[
                                                    Data::Object(&[
                                                        (
                                                            "type",
                                                            Data::Text("minecraft:matching_blocks"),
                                                        ),
                                                        ("blocks", Data::Text("minecraft:water")),
                                                    ]),
                                                    Data::Object(&[
                                                        (
                                                            "type",
                                                            Data::Text(
                                                                "minecraft:matching_block_tag",
                                                            ),
                                                        ),
                                                        ("tag", Data::Text("minecraft:corals")),
                                                    ]),
                                                ]),
                                            ),
                                        ]),
                                        Data::Object(&[
                                            ("type", Data::Text("minecraft:matching_blocks")),
                                            ("blocks", Data::Text("minecraft:water")),
                                            (
                                                "offset",
                                                Data::List(&[
                                                    Data::Number("0"),
                                                    Data::Number("1"),
                                                    Data::Number("0"),
                                                ]),
                                            ),
                                        ]),
                                    ]),
                                ),
                            ]),
                        ),
                    ])]),
                ),
            ]),
        ),
    ]);

    static CLAW: Data = Data::Object(&[
        ("type", Data::Text("minecraft:coral_claw")),
        (
            "feature",
            Data::Object(&[
                ("feature", Data::Text("minecraft:coral/brain_block")),
                ("placement", Data::List(&[])),
            ]),
        ),
    ]);

    fn ocean() -> World {
        let mut world = World::flat(blocks::WATER, 62);
        for x in -16..32 {
            for z in -16..32 {
                world.set(BlockPos::new(x, 40, z), blocks::SAND);
            }
        }
        world
    }

    fn coral_blocks(world: &World, block: clustine_data::BlockState) -> Vec<BlockPos> {
        world
            .writes
            .iter()
            .filter(|(_, state)| same_block(*state, block))
            .map(|(pos, _)| *pos)
            .collect()
    }

    #[test]
    fn a_tree_stands_on_its_trunk_and_branches_out_in_the_water() {
        let tree = crate::feature::read_configured(&Reader::new(&TREE, "test")).unwrap();
        let mut world = ocean();
        let origin = BlockPos::new(8, 41, 8);
        assert!(world.with_context(11, |context| tree.place(context, origin)));
        let placed = coral_blocks(&world, blocks::TUBE_CORAL_BLOCK);
        // The trunk, one to three high, then two to four branches of at least one.
        assert_eq!(placed[0], origin);
        assert!(placed.len() >= 4, "{placed:?}");
        assert!(placed.iter().all(|pos| pos.y >= 41));
        assert!(
            placed
                .iter()
                .all(|pos| (pos.x - 8).abs() <= 6 && (pos.z - 8).abs() <= 6)
        );
    }

    #[test]
    fn a_tree_out_of_water_places_nothing_and_says_it_was_placed() {
        let tree = crate::feature::read_configured(&Reader::new(&TREE, "test")).unwrap();
        let mut world = World::flat(blocks::SAND, 40);
        assert!(world.with_context(11, |context| tree.place(context, BlockPos::new(8, 41, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_claw_draws_its_direction_after_its_first_block() {
        let claw = crate::feature::read_configured(&Reader::new(&CLAW, "test")).unwrap();
        let mut world = ocean();
        let origin = BlockPos::new(8, 41, 8);
        assert!(world.with_context(4, |context| claw.place(context, origin)));
        let placed = coral_blocks(&world, blocks::BRAIN_CORAL_BLOCK);
        assert_eq!(placed[0], origin);
        assert!(placed.len() >= 3, "{placed:?}");
        // Where the first block is not placed, the claw is not, and draws nothing:
        // here the block lies outside what the run may write.
        let mut world = ocean();
        let mut random = WorldgenRandom::from_seed(4);
        let mut by_hand = WorldgenRandom::from_seed(4);
        let outside = BlockPos::new(60, 41, 8);
        assert!(!world.with_random(&mut random, None, |context| claw.place(context, outside)));
        assert!(world.writes.is_empty());
        assert_eq!(random.next_i64(), by_hand.next_i64());
    }
}
