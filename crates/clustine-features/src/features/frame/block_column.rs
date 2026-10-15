// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/block_column.rs`.

//! `block_column`: layers of blocks in a line from the origin, each of a drawn
//! length, cut short where the way is not free (`BlockColumnFeature`).

use clustine_data::Face;
use clustine_world::BlockPos;

use crate::blocks::{read_face, relative};
use crate::context::Context;
use crate::feature::Feature;
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug)]
pub struct BlockColumn {
    layers: Vec<(IntProvider, Provider)>,
    direction: Face,
    allowed: BlockPredicate,
    prioritize_tip: bool,
}

impl BlockColumn {
    pub fn read(reader: &Reader) -> Read<Self> {
        let mut layers = Vec::new();
        for layer in reader.field("layers")?.list()? {
            layers.push((
                IntProvider::read(&layer.field("height")?)?,
                Provider::read(&layer.field("provider")?)?,
            ));
            layer.finish()?;
        }
        Ok(Self {
            layers,
            direction: read_face(&reader.field("direction")?)?,
            allowed: BlockPredicate::read(&reader.field("allowed_placement")?)?,
            prioritize_tip: reader.field("prioritize_tip")?.boolean()?,
        })
    }
}

impl Feature for BlockColumn {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut heights: Vec<i32> = self
            .layers
            .iter()
            .map(|(height, _)| height.sample(&mut *context.random))
            .collect();
        let total: i32 = heights.iter().sum();
        if total == 0 {
            return false;
        }
        // The way is tested one block ahead of where each block goes: from the block
        // after the origin to the one after the last.
        let mut test = relative(origin, self.direction);
        for placed in 0..total {
            if !self.allowed.test(context, test) {
                truncate(&mut heights, total, placed, self.prioritize_tip);
                break;
            }
            test = relative(test, self.direction);
        }
        let mut at = origin;
        for ((_, provider), height) in self.layers.iter().zip(&heights) {
            for _ in 0..*height {
                let state = provider.state(context, at);
                context.level.set_block(at, state);
                at = relative(at, self.direction);
            }
        }
        true
    }
}

/// Takes `total - kept` blocks off the layers: from the first layer on where the tip
/// is to be kept, else from the last back.
fn truncate(heights: &mut [i32], total: i32, kept: i32, prioritize_tip: bool) {
    let mut to_remove = total - kept;
    let mut take = |height: &mut i32| {
        let removed = (*height).min(to_remove);
        to_remove -= removed;
        *height -= removed;
    };
    if prioritize_tip {
        heights.iter_mut().for_each(&mut take);
    } else {
        heights.iter_mut().rev().for_each(&mut take);
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;

    #[test]
    fn the_layers_are_cut_from_the_tip_or_the_base() {
        let mut heights = [3, 2, 4];
        truncate(&mut heights, 9, 4, true);
        assert_eq!(heights, [0, 0, 4]);
        let mut heights = [3, 2, 4];
        truncate(&mut heights, 9, 4, false);
        assert_eq!(heights, [3, 1, 0]);
    }

    static CACTUS: Data = Data::Object(&[
        ("type", Data::Text("minecraft:block_column")),
        (
            "allowed_placement",
            Data::Object(&[
                ("type", Data::Text("minecraft:matching_block_tag")),
                ("tag", Data::Text("minecraft:air")),
            ]),
        ),
        ("direction", Data::Text("up")),
        (
            "layers",
            Data::List(&[
                Data::Object(&[
                    ("height", Data::Number("3")),
                    (
                        "provider",
                        Data::Object(&[("id", Data::Text("minecraft:cactus"))]),
                    ),
                ]),
                Data::Object(&[
                    ("height", Data::Number("1")),
                    (
                        "provider",
                        Data::Object(&[("id", Data::Text("minecraft:cactus_flower"))]),
                    ),
                ]),
            ]),
        ),
        ("prioritize_tip", Data::Bool(false)),
    ]);

    fn cactus() -> Box<dyn Feature> {
        crate::feature::read_configured(&Reader::new(&CACTUS, "test")).unwrap()
    }

    #[test]
    fn a_column_places_its_layers_up_from_the_origin() {
        let mut world = World::flat(blocks::SAND, 62);
        let at = BlockPos::new(2, 63, 2);
        assert!(world.with_context(0, |context| cactus().place(context, at)));
        for y in 63..66 {
            assert_eq!(world.get(BlockPos::new(2, y, 2)), blocks::CACTUS);
        }
        assert_eq!(world.get(BlockPos::new(2, 66, 2)), blocks::CACTUS_FLOWER);
        assert_eq!(world.get(BlockPos::new(2, 67, 2)), blocks::AIR);
    }

    #[test]
    fn a_column_under_a_ceiling_loses_its_top_layers_first_unless_the_tip_counts() {
        // A ceiling at 66: the way is free at 64 and 65 only, so two blocks are kept.
        let mut world = World::flat(blocks::SAND, 62);
        world.set(BlockPos::new(2, 66, 2), blocks::STONE);
        let at = BlockPos::new(2, 63, 2);
        assert!(world.with_context(0, |context| cactus().place(context, at)));
        assert_eq!(world.get(BlockPos::new(2, 63, 2)), blocks::CACTUS);
        assert_eq!(world.get(BlockPos::new(2, 64, 2)), blocks::CACTUS);
        assert_eq!(world.get(BlockPos::new(2, 65, 2)), blocks::AIR);
    }
}
