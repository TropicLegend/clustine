//! What a dimension's features are placed by, read once from the data: every
//! configured and placed feature, the game's order of features by step and what each
//! biome lists.

use std::fmt;

use clustine_terrain::biome::Dimension;
use clustine_terrain::biome::zoom::Zoom;
use clustine_worldgen_data::feature as data;

use crate::feature::{Feature, read_configured};
use crate::placement::PlacedFeature;
use crate::reader::{Read, Reader};

/// How many steps of decoration the game has (`GenerationStep.Decoration`).
pub const STEPS: usize = 11;

/// The features of one dimension of a world.
pub struct FeatureSettings {
    seed: i64,
    zoom: Zoom,
    min_y: i32,
    height: i32,
    /// Every configured feature of the data, by its index there.
    configured: Vec<Box<dyn Feature>>,
    /// Every placed feature of the data, by its index there.
    placed: Vec<PlacedFeature>,
    /// For each step, the placed features in the game's order: a feature's place in
    /// its step's list is part of its seed.
    steps: Vec<Vec<usize>>,
    /// For each biome by its number, for each step, the places in the step's list of
    /// the features the biome lists, sorted.
    biomes: Vec<Vec<Vec<usize>>>,
}

impl fmt::Debug for FeatureSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeatureSettings")
            .field("seed", &self.seed)
            .field("min_y", &self.min_y)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl FeatureSettings {
    /// Reads every feature of the data, failing on one that cannot be read whole.
    pub fn new(seed: i64, dimension: Dimension) -> Read<Self> {
        let configured = data::configured_features()
            .iter()
            .map(|entry| read_configured(&Reader::new(&entry.data, entry.name)))
            .collect::<Read<Vec<_>>>()?;
        let placed = data::placed_features()
            .iter()
            .map(|entry| PlacedFeature::read(&Reader::new(&entry.data, entry.name)))
            .collect::<Read<Vec<_>>>()?;

        let name = match dimension {
            Dimension::Overworld => "overworld",
            Dimension::Nether => "nether",
            Dimension::End => "end",
        };
        let order = data::order(name).ok_or_else(|| crate::reader::DataError {
            at: name.to_string(),
            what: "the game orders no features for this dimension".to_string(),
        })?;
        let steps: Vec<Vec<usize>> = order
            .steps
            .iter()
            .map(|step| step.iter().map(|index| usize::from(*index)).collect())
            .collect();
        let biomes = data::biome_features()
            .iter()
            .map(|biome| {
                biome
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(step, listed)| {
                        let mut places: Vec<usize> = listed
                            .iter()
                            .filter_map(|feature| {
                                steps
                                    .get(step)?
                                    .iter()
                                    .position(|ordered| *ordered == usize::from(*feature))
                            })
                            .collect();
                        places.sort_unstable();
                        places
                    })
                    .collect()
            })
            .collect();

        let (min_section, sections) = dimension.sections();
        Ok(Self {
            seed,
            zoom: Zoom::new(seed),
            min_y: min_section * 16,
            height: i32::try_from(sections * 16).expect("a dimension is not that high"),
            configured,
            placed,
            steps,
            biomes,
        })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// The configured feature with this index in the data.
    pub fn configured(&self, index: usize) -> &dyn Feature {
        self.configured[index].as_ref()
    }

    /// The placed feature with this index in the data.
    pub fn placed(&self, index: usize) -> &PlacedFeature {
        &self.placed[index]
    }

    /// The placed features of `step` in the game's order, by their index in the data.
    pub fn step(&self, step: usize) -> &[usize] {
        self.steps.get(step).map_or(&[], Vec::as_slice)
    }

    /// How many steps the game orders features in for this dimension.
    pub fn steps(&self) -> usize {
        self.steps.len()
    }

    /// The places in `step`'s list of the features `biome` lists there, sorted.
    pub fn listed(&self, biome: u16, step: usize) -> &[usize] {
        self.biomes
            .get(usize::from(biome))
            .and_then(|steps| steps.get(step))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether `biome` lists the placed feature with this index in the data, in any
    /// step: what the modifier `biome` asks (`BiomeGenerationSettings.hasFeature`).
    pub fn lists(&self, biome: u16, placed: usize) -> bool {
        data::biome_features()
            .get(usize::from(biome))
            .is_some_and(|listed| {
                listed
                    .steps
                    .iter()
                    .any(|step| step.iter().any(|feature| usize::from(*feature) == placed))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_of_the_data_is_read_whole_in_every_dimension() {
        for dimension in Dimension::ALL {
            let settings = FeatureSettings::new(13579, dimension)
                .unwrap_or_else(|error| panic!("{dimension:?}: {error}"));
            assert!(settings.steps() <= STEPS);
        }
    }

    #[test]
    fn a_biome_lists_its_ores_in_the_step_of_underground_ores() {
        let settings = FeatureSettings::new(13579, Dimension::Overworld).unwrap();
        let plains = data::biome_features()
            .iter()
            .position(|biome| biome.biome == "minecraft:plains")
            .unwrap();
        let (iron, _) = data::placed("minecraft:ore_iron_middle").unwrap();
        let place = settings
            .step(6)
            .iter()
            .position(|feature| *feature == iron)
            .unwrap();
        assert!(settings.listed(plains as u16, 6).contains(&place));
        assert!(settings.lists(plains as u16, iron));
    }
}
