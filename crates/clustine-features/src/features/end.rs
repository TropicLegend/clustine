//! The End's kinds of configured features (`docs/groundwork/p-plan.md`, section 19.2,
//! P10a): the spikes round the arena with their crystals, the small islands, the chorus
//! plants, the return gateways of the highlands, the platform at (100, 49, 0), and the
//! void world's start platform. Each kind is a file of `end/`, read from its class's
//! bytecode in `/home/claude/reference/d-javap` (the void's platform excepted, which no
//! dump holds: see its file).
//!
//! The kinds are of the step `NetherAndEnd` (`crate::dispatch`); the Nether's are in
//! `nether_and_end`, and `dispatch` sends these here.

use crate::feature::Feature;
use crate::features::Unbuilt;
use crate::reader::{Read, Reader};

pub mod chorus_plant;
pub mod end_gateway;
pub mod end_island;
pub mod end_platform;
pub mod end_spike;
pub mod void_start_platform;

/// The End's kinds that are built. The podium (`end_podium`) is the dragon fight's,
/// placed by the fight and by no biome, and the bonus chest is no End's.
pub const END_KINDS: [&str; 6] = [
    "minecraft:chorus_plant",
    "minecraft:end_gateway",
    "minecraft:end_island",
    "minecraft:end_platform",
    "minecraft:end_spike",
    "minecraft:void_start_platform",
];

/// Whether `kind` is one this module reads.
pub fn reads(kind: &str) -> bool {
    END_KINDS.contains(&kind)
}

/// Reads a configured feature of one of the End's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(match kind {
        "minecraft:chorus_plant" => Box::new(chorus_plant::ChorusPlant::read(reader)?),
        "minecraft:end_gateway" => Box::new(end_gateway::EndGateway::read(reader)?),
        "minecraft:end_island" => Box::new(end_island::EndIsland::read(reader)?),
        "minecraft:end_platform" => Box::new(end_platform::EndPlatform::read(reader)?),
        "minecraft:end_spike" => Box::new(end_spike::EndSpike::read(reader)?),
        "minecraft:void_start_platform" => {
            Box::new(void_start_platform::VoidStartPlatform::read(reader)?)
        }
        _ => Unbuilt::read(kind, reader),
    })
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::dispatch::{Batch, KINDS};
    use crate::feature::read_configured;

    const END_BIOMES: [&str; 5] = [
        "minecraft:the_end",
        "minecraft:end_highlands",
        "minecraft:end_midlands",
        "minecraft:small_end_islands",
        "minecraft:end_barrens",
    ];

    #[test]
    fn the_end_s_kinds_are_of_the_nether_and_end_s_step_and_read_here() {
        for kind in END_KINDS {
            assert_eq!(
                crate::dispatch::batch(kind),
                Some(Batch::NetherAndEnd),
                "{kind}"
            );
        }
        // Every other kind of the step is the Nether's, or not built.
        for (kind, batch) in KINDS {
            if batch == Batch::NetherAndEnd && !reads(kind) {
                assert!(
                    crate::features::nether_and_end::NETHER_KINDS.contains(&kind)
                        || ["minecraft:bonus_chest", "minecraft:end_podium"].contains(&kind),
                    "{kind}"
                );
            }
        }
    }

    #[test]
    fn every_feature_of_the_end_s_kinds_in_the_data_is_read_and_built() {
        let mut seen = 0;
        for entry in data::configured_features() {
            let Ok(kind) = Reader::new(&entry.data, entry.name).kind() else {
                continue;
            };
            if !reads(kind) {
                continue;
            }
            let feature = read_configured(&Reader::new(&entry.data, entry.name))
                .unwrap_or_else(|error| panic!("{}: {error}", entry.name));
            assert!(feature.built(), "{}", entry.name);
            seen += 1;
        }
        // The spike, the island, chorus, two gateways, the platform and the void's.
        assert!(seen >= 7, "{seen}");
    }

    #[test]
    fn every_feature_an_end_biome_lists_is_built() {
        let mut seen = 0;
        for biome in data::biome_features() {
            if !END_BIOMES.contains(&biome.biome) {
                continue;
            }
            seen += 1;
            for step in biome.steps {
                for placed in step.iter() {
                    let placed = &data::placed_features()[usize::from(*placed)];
                    let feature = placed
                        .data
                        .get("feature")
                        .expect("a placed feature names its feature");
                    let built = match feature {
                        data::Data::Text(name) => {
                            let (_, entry) =
                                data::configured(name).expect("the configured feature is named");
                            read_configured(&Reader::new(&entry.data, entry.name))
                                .expect("it is read")
                                .built()
                        }
                        inline => read_configured(&Reader::new(inline, placed.name))
                            .expect("it is read")
                            .built(),
                    };
                    assert!(built, "{} of {}", placed.name, biome.biome);
                }
            }
        }
        assert_eq!(seen, END_BIOMES.len());
    }
}
