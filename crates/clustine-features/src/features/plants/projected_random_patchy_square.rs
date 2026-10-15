// Adapted from SteelMC's
// `steel-core/src/worldgen/feature/features/projected_random_patchy_square.rs`.

//! `projected_random_patchy_square`: a square around the origin, its corners thinned
//! by chance, each of its blocks dropped through what the data lets it fall through
//! for a few blocks and placed there (`ProjectedRandomPatchySquareFeature`). The
//! basalt pillars of the Nether cap themselves with it.

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug)]
pub struct ProjectedRandomPatchySquare {
    block: Provider,
    max_projection_height: i32,
    project_through: BlockPredicate,
    size: IntProvider,
}

impl ProjectedRandomPatchySquare {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            block: Provider::read(&reader.field("block")?)?,
            max_projection_height: reader.field("max_projection_height")?.int()?,
            project_through: BlockPredicate::read(&reader.field("project_through")?)?,
            size: IntProvider::read(&reader.field("size")?)?,
        })
    }
}

impl Feature for ProjectedRandomPatchySquare {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let size = self.size.sample(&mut *context.random);
        let bound = size * size + 1;
        for dx in -size..=size {
            for dz in -size..=size {
                // The further out on both axes, the likelier a block is left out.
                let leave_out = dx.abs() * dz.abs();
                if context.random.next_i32_bounded(bound) >= bound - leave_out {
                    continue;
                }
                let mut at = origin.offset(dx, 0, dz);
                let mut drop = self.max_projection_height;
                while self.project_through.test(context, at.offset(0, -1, 0)) {
                    at = at.offset(0, -1, 0);
                    drop -= 1;
                    if drop <= 0 {
                        break;
                    }
                }
                if let Some(state) = self.block.optional_state(context, at) {
                    context.level.set_block(at, state);
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;

    static SQUARE: Data = Data::Object(&[
        (
            "type",
            Data::Text("minecraft:projected_random_patchy_square"),
        ),
        ("block", Data::Text("minecraft:basalt")),
        ("max_projection_height", Data::Number("3")),
        (
            "project_through",
            Data::Object(&[
                ("type", Data::Text("minecraft:matching_block_tag")),
                ("tag", Data::Text("minecraft:air")),
            ]),
        ),
        ("size", Data::Number("2")),
    ]);

    #[test]
    fn the_centre_cross_always_drops_to_the_ground_within_reach() {
        let square = crate::feature::read_configured(&Reader::new(&SQUARE, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 60);
        // Ground two below at the centre, ten below elsewhere on the cross: the block
        // drops two there and three, its reach, elsewhere.
        world.set(BlockPos::new(8, 68, 8), blocks::STONE);
        assert!(world.with_context(3, |context| square.place(context, BlockPos::new(8, 71, 8))));
        assert_eq!(world.get(BlockPos::new(8, 69, 8)), blocks::BASALT);
        for (x, z) in [
            (6, 8),
            (7, 8),
            (9, 8),
            (10, 8),
            (8, 6),
            (8, 7),
            (8, 9),
            (8, 10),
        ] {
            assert_eq!(
                world.get(BlockPos::new(x, 68, z)),
                blocks::BASALT,
                "{x} {z}"
            );
        }
    }
}
