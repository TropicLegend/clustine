//! `delta_feature`: a flat pool of one block in the ground, with a rim of another a
//! step aside (`DeltaFeature`, read from
//! `d-javap/net.minecraft.world.level.levelgen.feature.DeltaFeature.txt`; the offsets
//! below are its bytecode's): the basalt deltas' pools of lava rimmed with magma.

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::shared::{manhattan, within_manhattan};
use crate::blocks::{read_state, relative, same_block};
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

/// `CANNOT_REPLACE`: what a pool never replaces, the fortress's blocks among them
/// (the static initialiser, offsets 11-35).
const CANNOT_REPLACE: [BlockState; 7] = [
    blocks::BEDROCK,
    blocks::NETHER_BRICKS,
    blocks::NETHER_BRICK_FENCE,
    blocks::NETHER_BRICK_STAIRS,
    blocks::NETHER_WART,
    blocks::CHEST,
    blocks::SPAWNER,
];

/// The chance that a pool has a rim at all (`RIM_SPAWN_CHANCE`).
const RIM_SPAWN_CHANCE: f64 = 0.9;

#[derive(Debug)]
pub struct Delta {
    contents: BlockState,
    rim: BlockState,
    size: IntProvider,
    rim_size: IntProvider,
}

impl Delta {
    pub fn read(reader: &Reader) -> Read<Self> {
        let delta = Self {
            contents: read_state(&reader.field("contents")?)?,
            rim: read_state(&reader.field("rim")?)?,
            size: IntProvider::read(&reader.field("size")?)?,
            rim_size: IntProvider::read(&reader.field("rim_size")?)?,
        };
        reader.finish()?;
        Ok(delta)
    }

    /// `isClear`: not the contents' block already (0-23), not a block a pool never
    /// replaces (24-38), with air above and none of the five other sides open to air
    /// (39-118).
    fn is_clear(&self, context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
        let state = context.level.block(pos);
        if same_block(state, self.contents)
            || CANNOT_REPLACE.iter().any(|kept| same_block(state, *kept))
        {
            return false;
        }
        // `Direction.values()`; the order decides nothing, as any side that fails
        // fails the whole.
        for face in Face::ALL {
            let air = context.level.block(relative(pos, face)).props().is_air();
            if air != (face == Face::Up) {
                return false;
            }
        }
        true
    }
}

impl Feature for Delta {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        // The draws in the game's order: the rim's chance (4-21), its two offsets only
        // when it may spawn (23-63), then the pool's two radii, x before z (87-109).
        // A rim is laid only where both offsets are not nought (65-85).
        let spawn_rim = context.random.next_f64() < RIM_SPAWN_CHANCE;
        let rim_x = if spawn_rim {
            self.rim_size.sample(context.random)
        } else {
            0
        };
        let rim_z = if spawn_rim {
            self.rim_size.sample(context.random)
        } else {
            0
        };
        let has_rim = spawn_rim && rim_x != 0 && rim_z != 0;
        let radius_x = self.size.sample(context.random);
        let radius_z = self.size.sample(context.random);
        let greatest = radius_x.max(radius_z);
        let mut placed = false;
        // Each block reads what the blocks before it placed, so the walk's order
        // matters here: `withinBoxByManhattanDistance` flat in y (120-127), stopped at
        // the first block beyond the greater radius (159-171). Where a block is clear,
        // the rim there (184-200), then the contents a step aside where that is clear
        // (203-236).
        for pos in within_manhattan(origin, radius_x, 0, radius_z) {
            if manhattan(pos, origin) > greatest {
                break;
            }
            if self.is_clear(context, pos) {
                if has_rim {
                    placed = true;
                    context.level.set_block(pos, self.rim);
                }
                let aside = pos.offset(rim_x, 0, rim_z);
                if self.is_clear(context, aside) {
                    placed = true;
                    context.level.set_block(aside, self.contents);
                }
            }
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn delta() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:delta").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    /// The seed of a context whose first draws give a rim of the two offsets wanted,
    /// and the pool's radii.
    fn seed_with_rim(wanted: bool) -> (u64, i32, i32, i32, i32) {
        (0..1000)
            .find_map(|seed| {
                let mut random = clustine_noise::WorldgenRandom::from_seed(seed);
                let spawn = random.next_f64() < RIM_SPAWN_CHANCE;
                let (x, z) = if spawn {
                    (random.next_i32_bounded(3), random.next_i32_bounded(3))
                } else {
                    (0, 0)
                };
                let (rx, rz) = (
                    random.next_i32_bounded(5) + 3,
                    random.next_i32_bounded(5) + 3,
                );
                let rim = spawn && x != 0 && z != 0;
                (rim == wanted).then_some((seed, x, z, rx, rz))
            })
            .unwrap()
    }

    #[test]
    fn a_pool_without_a_rim_fills_the_ground_s_top_with_lava_a_step_aside() {
        let delta = delta();
        let (seed, x, z, rx, rz) = seed_with_rim(false);
        let mut world = World::flat(blocks::NETHERRACK, 40);
        let origin = BlockPos::new(8, 40, 8);
        assert!(world.with_context(seed, |context| delta.place(context, origin)));
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::LAVA);
            assert_eq!(pos.y, 40);
            let from = pos.offset(-x, 0, -z);
            assert!(manhattan(from, origin) <= rx.max(rz), "{pos:?}");
        }
    }

    #[test]
    fn a_pool_with_a_rim_lays_magma_where_it_starts_and_lava_a_step_aside() {
        let delta = delta();
        let (seed, x, z, _, _) = seed_with_rim(true);
        let mut world = World::flat(blocks::NETHERRACK, 40);
        let origin = BlockPos::new(8, 40, 8);
        assert!(world.with_context(seed, |context| delta.place(context, origin)));
        // The origin is clear: magma, then lava at its step aside, which was clear.
        assert_eq!(world.writes[0], (origin, blocks::MAGMA_BLOCK));
        assert_eq!(world.writes[1], (origin.offset(x, 0, z), blocks::LAVA));
        // Nothing but the two blocks is laid.
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state == blocks::LAVA || *state == blocks::MAGMA_BLOCK)
        );
    }

    #[test]
    fn a_pool_is_not_laid_open_to_air_at_a_side_or_under_a_roof() {
        let delta = delta();
        let (seed, _, _, _, _) = seed_with_rim(false);
        // Ground with air below its top: every block has air beneath or no air above.
        let mut world = World::flat(blocks::NETHERRACK, 40);
        for x in -16..32 {
            for z in -16..32 {
                world.set(BlockPos::new(x, 39, z), blocks::AIR);
            }
        }
        assert!(!world.with_context(seed, |context| {
            delta.place(context, BlockPos::new(8, 40, 8))
        }));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_pool_leaves_every_block_it_may_not_replace_and_its_own_contents() {
        let delta = delta();
        for wanted in [false, true] {
            let (seed, _, _, _, _) = seed_with_rim(wanted);
            // The seven of `CANNOT_REPLACE`, in any state, and lava already there.
            let kept = [
                blocks::BEDROCK,
                blocks::NETHER_BRICKS,
                blocks::NETHER_BRICK_FENCE,
                blocks::NETHER_BRICK_STAIRS,
                blocks::NETHER_WART,
                blocks::CHEST,
                blocks::SPAWNER,
                blocks::LAVA.with_property("level", "3").unwrap(),
            ];
            for ground in kept {
                let mut world = World::flat(ground, 40);
                assert!(
                    !world.with_context(seed, |context| {
                        delta.place(context, BlockPos::new(8, 40, 8))
                    }),
                    "{ground:?}"
                );
                assert!(world.writes.is_empty(), "{ground:?}");
            }
        }
    }
}
