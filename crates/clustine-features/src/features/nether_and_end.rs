//! The kinds of configured features that step F7 of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`), the Nether's in step P5 of `docs/groundwork/p-plan.md`:
//! blobs of basalt and blackstone, the deltas' pools, huge fungi and the spreading of
//! glowstone and wart. Each kind is a file of `nether_and_end/`. The End's kinds are
//! not built yet.

use crate::feature::Feature;
use crate::features::Unbuilt;
use crate::reader::{Read, Reader};

pub mod delta;
pub mod huge_fungus;
pub mod netherrack_replace_blobs;
pub mod random_neighbor_spread;
pub mod shared;

/// The kinds of this step that are built: every kind a Nether biome's features reach
/// that no other step builds.
pub const NETHER_KINDS: [&str; 4] = [
    "minecraft:delta_feature",
    "minecraft:huge_fungus",
    "minecraft:netherrack_replace_blobs",
    "minecraft:random_neighbor_spread",
];

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(match kind {
        "minecraft:delta_feature" => Box::new(delta::Delta::read(reader)?),
        "minecraft:huge_fungus" => Box::new(huge_fungus::HugeFungus::read(reader)?),
        "minecraft:netherrack_replace_blobs" => Box::new(
            netherrack_replace_blobs::NetherrackReplaceBlobs::read(reader)?,
        ),
        "minecraft:random_neighbor_spread" => {
            Box::new(random_neighbor_spread::RandomNeighborSpread::read(reader)?)
        }
        _ => Unbuilt::read(kind, reader),
    })
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::{Biome, ChunkPos};
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::dispatch::{Batch, KINDS};
    use crate::feature::read_configured;
    use crate::testing::World;

    const NETHER_BIOMES: [&str; 5] = [
        "minecraft:nether_wastes",
        "minecraft:soul_sand_valley",
        "minecraft:crimson_forest",
        "minecraft:warped_forest",
        "minecraft:basalt_deltas",
    ];

    #[test]
    fn the_nether_s_kinds_are_this_step_s() {
        for kind in NETHER_KINDS {
            assert_eq!(
                crate::dispatch::batch(kind),
                Some(Batch::NetherAndEnd),
                "{kind}"
            );
        }
    }

    #[test]
    fn every_feature_of_the_nether_s_kinds_in_the_data_is_built() {
        let mut seen = 0;
        for entry in data::configured_features() {
            let Ok(kind) = Reader::new(&entry.data, entry.name).kind() else {
                continue;
            };
            if !NETHER_KINDS.contains(&kind) {
                continue;
            }
            let feature = read_configured(&Reader::new(&entry.data, entry.name))
                .unwrap_or_else(|error| panic!("{}: {error}", entry.name));
            assert!(feature.built(), "{}", entry.name);
            seen += 1;
        }
        // The delta, two blobs, four fungi and glowstone; weeping vines' wart is
        // written out inside its overlay.
        assert!(seen >= 8, "{seen}");
    }

    #[test]
    fn every_feature_a_nether_biome_lists_is_built() {
        let mut seen = 0;
        for biome in data::biome_features() {
            if !NETHER_BIOMES.contains(&biome.biome) {
                continue;
            }
            seen += 1;
            for step in biome.steps {
                for placed in step.iter() {
                    let placed = &data::placed_features()[usize::from(*placed)];
                    let unbuilt = unbuilt_within(&placed.data);
                    assert!(
                        unbuilt.is_none(),
                        "{} of {} reaches {unbuilt:?}",
                        placed.name,
                        biome.biome
                    );
                }
            }
        }
        assert_eq!(seen, NETHER_BIOMES.len());
    }

    /// The first kind of feature not built that `data` names or writes out, following
    /// the names of configured and placed features where a feature is expected.
    fn unbuilt_within(data: &'static data::Data) -> Option<&'static str> {
        match data {
            data::Data::Object(members) => {
                if let Some(kind) = data.kind()
                    && crate::dispatch::batch(kind) == Some(Batch::NetherAndEnd)
                    && !NETHER_KINDS.contains(&kind)
                {
                    return Some(kind);
                }
                members.iter().find_map(|(key, value)| match (*key, value) {
                    ("feature" | "features", data::Data::Text(name)) => named(name),
                    ("features", data::Data::List(items)) => items.iter().find_map(|item| {
                        if let data::Data::Text(name) = item {
                            named(name)
                        } else {
                            unbuilt_within(item)
                        }
                    }),
                    _ => unbuilt_within(value),
                })
            }
            data::Data::List(items) => items.iter().find_map(unbuilt_within),
            _ => None,
        }
    }

    /// [`unbuilt_within`] of the configured or placed feature called `name`.
    fn named(name: &str) -> Option<&'static str> {
        let entry = data::configured(name)
            .or_else(|| data::placed(name))
            .map(|(_, entry)| entry)?;
        unbuilt_within(&entry.data)
    }

    #[test]
    fn a_chunk_of_every_nether_biome_is_decorated_without_a_failure() {
        let settings = shared::nether_settings();
        for (number, biome) in data::biome_features().iter().enumerate() {
            if !NETHER_BIOMES.contains(&biome.biome) {
                continue;
            }
            let mut world = World::flat(blocks::NETHERRACK, 60);
            world.biome = Biome(number as u16);
            crate::decorate(settings, &mut world, ChunkPos { x: 0, z: 0 });
            assert!(!world.writes.is_empty(), "{}", biome.biome);
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

    #[test]
    fn every_kind_of_this_step_is_read_by_it() {
        use clustine_worldgen_data::feature::Data;
        static EMPTY: Data = Data::Object(&[]);
        for (kind, batch) in KINDS {
            if batch != Batch::NetherAndEnd {
                continue;
            }
            let reader = crate::reader::Reader::new(&EMPTY, kind);
            if let Err(error) = super::read(kind, &reader) {
                assert!(!error.what.contains("is not a kind"), "{kind}: {error}");
            }
        }
    }
}
