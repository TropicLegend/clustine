//! The kinds of configured features that step F5 of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`): what lies in caves and under the ground. Geodes, large
//! dripstones, the speleothems of 26.x (pointed dripstone and sulfur spikes, alone and
//! in clusters), stepped columns of basalt, fossils, and the template feature that
//! places the desert well and the sulfur springs. Each kind is a file of `caves/`.

use crate::feature::Feature;
use crate::reader::{Read, Reader};

pub mod fossil;
pub mod geode;
pub mod large_dripstone;
pub mod shared;
pub mod speleothem;
pub mod speleothem_cluster;
pub mod stepped_column_cluster;
pub mod template;
pub mod templates;

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    Ok(match kind {
        "minecraft:geode" => Box::new(geode::Geode::read(reader)?),
        "minecraft:large_dripstone" => Box::new(large_dripstone::LargeDripstone::read(reader)?),
        "minecraft:speleothem" => Box::new(speleothem::Speleothem::read(reader)?),
        "minecraft:speleothem_cluster" => {
            Box::new(speleothem_cluster::SpeleothemCluster::read(reader)?)
        }
        "minecraft:stepped_column_cluster" => {
            Box::new(stepped_column_cluster::SteppedColumnCluster::read(reader)?)
        }
        "minecraft:fossil" => Box::new(fossil::Fossil::read(reader)?),
        "minecraft:template" => Box::new(template::Template::read(reader)?),
        other => return Err(reader.error(format!("{other} is not a kind of step F5"))),
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
            if batch != Batch::Caves {
                continue;
            }
            let reader = crate::reader::Reader::new(&EMPTY, kind);
            // Each kind is known here: it fails for a missing member, never for its kind.
            if let Err(error) = super::read(kind, &reader) {
                assert!(!error.what.contains("is not a kind"), "{kind}: {error}");
            }
        }
    }

    #[test]
    fn every_feature_of_this_step_in_the_data_is_built() {
        use clustine_worldgen_data::feature as data;

        let settings = crate::testing::settings();
        for (index, entry) in data::configured_features().iter().enumerate() {
            let Some(kind) = entry.data.kind() else {
                continue;
            };
            if crate::dispatch::batch(kind) == Some(Batch::Caves) {
                assert!(settings.configured(index).built(), "{}", entry.name);
            }
        }
    }
}
