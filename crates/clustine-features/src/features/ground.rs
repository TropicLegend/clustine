//! The kinds of configured features that step F2 of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`): the ground's features, from disks of clay and rocks to
//! lakes, springs, icebergs and monster rooms. Each kind is a file of `ground/`.

use crate::feature::Feature;
use crate::reader::{Read, Reader};

pub mod block_blob;
pub mod block_pile;
pub mod blue_ice;
pub mod disk;
pub mod freeze_top_layer;
pub mod iceberg;
pub mod lake;
pub mod monster_room;
pub mod shared;
pub mod single_block_pillar;
pub mod spike;
pub mod spring;
pub mod underwater_magma;

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(match kind {
        "minecraft:disk" => Box::new(disk::Disk::read(reader)?),
        "minecraft:block_blob" => Box::new(block_blob::BlockBlob::read(reader)?),
        "minecraft:block_pile" => Box::new(block_pile::BlockPile::read(reader)?),
        "minecraft:spring_feature" => Box::new(spring::Spring::read(reader)?),
        "minecraft:lake" => Box::new(lake::Lake::read(reader)?),
        "minecraft:freeze_top_layer" => Box::new(freeze_top_layer::FreezeTopLayer::read(reader)?),
        "minecraft:iceberg" => Box::new(iceberg::Iceberg::read(reader)?),
        "minecraft:blue_ice" => Box::new(blue_ice::BlueIce::read(reader)?),
        "minecraft:spike" => Box::new(spike::Spike::read(reader)?),
        "minecraft:underwater_magma" => Box::new(underwater_magma::UnderwaterMagma::read(reader)?),
        "minecraft:monster_room" => Box::new(monster_room::MonsterRoom::read(reader)?),
        "minecraft:single_block_pillar" => {
            Box::new(single_block_pillar::SingleBlockPillar::read(reader)?)
        }
        other => return Err(reader.error(format!("{other} is not a kind of step F2"))),
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
            if batch != Batch::Ground {
                continue;
            }
            let reader = crate::reader::Reader::new(&EMPTY, kind);
            if let Err(error) = super::read(kind, &reader) {
                assert!(!error.what.contains("is not a kind"), "{kind}: {error}");
            }
        }
    }

    #[test]
    fn every_feature_of_this_step_in_the_data_is_built() {
        use clustine_worldgen_data::feature as data;

        use crate::feature::read_configured;
        use crate::reader::Reader;

        let mut seen = 0;
        for entry in data::configured_features() {
            let Ok(kind) = Reader::new(&entry.data, entry.name).kind() else {
                continue;
            };
            if crate::dispatch::batch(kind) != Some(Batch::Ground) {
                continue;
            }
            let feature = read_configured(&Reader::new(&entry.data, entry.name))
                .unwrap_or_else(|error| panic!("{}: {error}", entry.name));
            assert!(feature.built(), "{}", entry.name);
            seen += 1;
        }
        assert!(seen >= 20, "{seen}");
    }

    #[test]
    fn a_chunk_of_every_overworld_biome_is_decorated_with_this_step_without_a_failure() {
        use clustine_data::blocks;
        use clustine_world::{Biome, ChunkPos};
        use clustine_worldgen_data::feature as data;

        use crate::testing::{World, settings};

        for (number, biome) in data::biome_features().iter().enumerate() {
            if biome.steps.iter().all(|step| step.is_empty()) {
                continue;
            }
            let mut world = World::flat(blocks::STONE, 62);
            world.biome = Biome(number as u16);
            crate::decorate(settings(), &mut world, ChunkPos { x: 0, z: 0 });
            assert!(
                world.writes.iter().all(|(pos, _)| {
                    let chunk = pos.chunk();
                    chunk.x.abs() <= 1 && chunk.z.abs() <= 1
                }),
                "{}",
                biome.biome
            );
        }
    }
}
