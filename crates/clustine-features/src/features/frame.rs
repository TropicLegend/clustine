//! The kinds of configured features that step F1b of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`): the combinators every batch places others with, and
//! simple blocks, columns and ores. Each kind is a file of `frame/`.

use crate::feature::Feature;
use crate::reader::{Read, Reader};

pub mod block_column;
pub mod no_op;
pub mod ore;
pub mod overlay;
pub mod random_boolean_selector;
pub mod random_selector;
pub mod scattered_ore;
pub mod sequence;
pub mod simple_block;
pub mod simple_random_selector;
pub mod weighted_random_selector;

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(match kind {
        "minecraft:no_op" => Box::new(no_op::NoOp),
        "minecraft:simple_block" => Box::new(simple_block::SimpleBlock::read(reader)?),
        "minecraft:block_column" => Box::new(block_column::BlockColumn::read(reader)?),
        "minecraft:ore" => Box::new(ore::Ore::read(reader)?),
        "minecraft:scattered_ore" => Box::new(scattered_ore::ScatteredOre::read(reader)?),
        "minecraft:random_selector" => Box::new(random_selector::RandomSelector::read(reader)?),
        "minecraft:weighted_random_selector" => Box::new(
            weighted_random_selector::WeightedRandomSelector::read(reader)?,
        ),
        "minecraft:simple_random_selector" => {
            Box::new(simple_random_selector::SimpleRandomSelector::read(reader)?)
        }
        "minecraft:random_boolean_selector" => Box::new(
            random_boolean_selector::RandomBooleanSelector::read(reader)?,
        ),
        "minecraft:sequence" => Box::new(sequence::Sequence::read(reader)?),
        "minecraft:overlay" => Box::new(overlay::Overlay::read(reader)?),
        other => return Err(reader.error(format!("{other} is not a kind of step F1b"))),
    })
}

#[cfg(test)]
mod tests {
    use crate::dispatch::{Batch, KINDS};

    #[test]
    fn every_kind_of_this_step_is_read_by_it() {
        use clustine_worldgen_data::feature::Data;
        static EMPTY: Data = Data::Object(&[]);
        for (kind, batch) in KINDS {
            if batch != Batch::Frame {
                continue;
            }
            let reader = crate::reader::Reader::new(&EMPTY, kind);
            // Each kind is known here: it fails for a missing member, never for its
            // kind (no_op has none to miss).
            if let Err(error) = super::read(kind, &reader) {
                assert!(!error.what.contains("is not a kind"), "{kind}: {error}");
            }
        }
    }

    #[test]
    fn a_chunk_of_every_overworld_biome_is_decorated_without_a_failure() {
        use clustine_data::blocks;
        use clustine_world::{Biome, ChunkPos};
        use clustine_worldgen_data::feature as data;

        use crate::testing::{World, settings};

        let mut ores = 0;
        for (number, biome) in data::biome_features().iter().enumerate() {
            if biome.steps.iter().all(|step| step.is_empty()) {
                continue;
            }
            let mut world = World::flat(blocks::STONE, 62);
            world.biome = Biome(number as u16);
            // The toy view's `attempt` lets a panic through: a feature that fails
            // fails the test.
            crate::decorate(settings(), &mut world, ChunkPos { x: 0, z: 0 });
            ores += world
                .writes
                .iter()
                .filter(|(_, state)| {
                    state
                        .block()
                        .is_some_and(|block| block.name.ends_with("_ore"))
                })
                .count();
            assert!(
                world.writes.iter().all(|(pos, _)| {
                    let chunk = pos.chunk();
                    chunk.x.abs() <= 1 && chunk.z.abs() <= 1
                }),
                "{}",
                biome.biome
            );
        }
        assert!(ores > 0);
    }
}
