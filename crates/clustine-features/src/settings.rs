//! What a dimension's features are placed by, read once from the data: every
//! configured and placed feature, the game's order of features by step and what each
//! biome lists.

use std::fmt;
use std::sync::Arc;

use clustine_structures::template_blocks::TemplateLibrary;
use clustine_terrain::biome::Dimension;
use clustine_terrain::biome::zoom::Zoom;
use clustine_worldgen_data::feature as data;

use crate::feature::{Feature, FeatureRef, read_configured};
use crate::features::frame::no_op::NoOp;
use crate::placement::PlacedFeature;
use crate::reader::{Read, Reader};
use crate::structures::StartsOf;
use crate::values::Generation;

/// How many steps of decoration the game has (`GenerationStep.Decoration`).
pub const STEPS: usize = 11;

/// The features of one dimension of a world.
pub struct FeatureSettings {
    seed: i64,
    zoom: Zoom,
    min_y: i32,
    height: i32,
    /// The heights the generator makes and its sea level, which heights are drawn
    /// against.
    generation: Generation,
    /// Whether the dimension's random numbers are `java.util.Random`'s.
    legacy_random_source: bool,
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
    /// The blocks of the structure templates, read by the server from the operator's
    /// own jar (the terrain plan's question 3); without them a feature built of a
    /// template places nothing.
    templates: Option<Arc<TemplateLibrary>>,
    /// Where the starts of structures are found; without them no structure's block
    /// is placed (the plan of phase S, until its switch).
    structures: Option<Arc<dyn StartsOf>>,
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
        let min_y = min_section * 16;
        let height = i32::try_from(sections * 16).expect("a dimension is not that high");
        // The game's `WorldGenerationContext`: the generator's heights within the
        // dimension's. The Nether and the End generate 128 blocks of their 256.
        let noise = dimension.noise_settings();
        let generation = Generation {
            min_y: min_y.max(noise.min_y),
            height: height.min(noise.height),
            sea_level: noise.sea_level,
        };
        Ok(Self {
            seed,
            zoom: Zoom::new(seed),
            min_y,
            height,
            generation,
            legacy_random_source: noise.legacy_random_source,
            configured,
            placed,
            steps,
            biomes,
            templates: None,
            structures: None,
        })
    }

    /// These settings with the blocks of the structure templates.
    #[must_use]
    pub fn with_templates(mut self, templates: Arc<TemplateLibrary>) -> Self {
        self.templates = Some(templates);
        self
    }

    /// These settings with the placed features named turned into the game's `no_op`,
    /// each keeping its place in its step and so every other feature's seed: for the
    /// judge, which turns the same off in the game to find which feature a difference
    /// comes from. A name that is no placed feature is refused.
    pub fn with_placed_off(mut self, names: &[&str]) -> Read<Self> {
        for name in names {
            let (index, _) = data::placed(name).ok_or_else(|| crate::reader::DataError {
                at: (*name).to_owned(),
                what: "is not a placed feature".to_owned(),
            })?;
            self.placed[index] = PlacedFeature {
                feature: FeatureRef::Inline(Box::new(NoOp)),
                modifiers: Vec::new(),
            };
        }
        Ok(self)
    }

    /// These settings placing the structures' blocks, of the starts `starts` gives.
    #[must_use]
    pub fn with_structures(mut self, starts: Arc<dyn StartsOf>) -> Self {
        self.structures = Some(starts);
        self
    }

    /// Where the starts of structures are found, if their blocks are placed.
    pub fn structures(&self) -> Option<&dyn StartsOf> {
        self.structures.as_deref()
    }

    /// The blocks of the structure templates, where the server has them.
    pub fn templates(&self) -> Option<&TemplateLibrary> {
        self.templates.as_deref()
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

    /// The heights the generator makes and its sea level (`WorldGenerationContext`),
    /// which the modifier `height_range` draws against.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Whether the dimension's random numbers are `java.util.Random`'s, which the
    /// region's own random source follows.
    pub fn legacy_random_source(&self) -> bool {
        self.legacy_random_source
    }

    /// The configured feature with this index in the data.
    pub fn configured(&self, index: usize) -> &dyn Feature {
        self.configured[index].as_ref()
    }

    /// The configured feature of this name (`minecraft:oak`), as a sapling's grower or
    /// bone meal names it.
    pub fn configured_named(&self, name: &str) -> Option<&dyn Feature> {
        let index = data::configured_features()
            .iter()
            .position(|entry| entry.name == name)?;
        Some(self.configured(index))
    }

    /// `BiomeGenerationSettings.getBoneMealFeatures` of the biome `biome`
    /// (`minecraft:plains`): the configured features its placed features name, in the
    /// order of its steps and of its file, that the tag
    /// `#minecraft:can_spawn_from_bone_meal` holds; what grass's bone meal places one in
    /// eight times (n8 GrassBlock 117–190). No feature of the tag is nested in another in
    /// the pinned jar's data, so only the configured feature a placed one names is read.
    pub fn bone_meal_features(&self, biome: &str) -> Vec<&'static str> {
        /// `data/minecraft/tags/worldgen/feature/can_spawn_from_bone_meal.json` of the
        /// pinned jar (Mojang's data).
        const CAN_SPAWN_FROM_BONE_MEAL: [&str; 8] = [
            "minecraft:flower_default",
            "minecraft:flower_flower_forest",
            "minecraft:flower_swamp",
            "minecraft:flower_plain",
            "minecraft:flower_meadow",
            "minecraft:flower_cherry",
            "minecraft:wildflower",
            "minecraft:flower_pale_garden",
        ];
        let Some(listed) = data::biome_features()
            .iter()
            .find(|listed| listed.biome == biome)
        else {
            return Vec::new();
        };
        let placed = data::placed_features();
        listed
            .steps
            .iter()
            .flat_map(|step| step.iter())
            .filter_map(
                |index| match placed.get(usize::from(*index))?.data.get("feature") {
                    Some(data::Data::Text(name)) => Some(*name),
                    _ => None,
                },
            )
            .filter(|name| CAN_SPAWN_FROM_BONE_MEAL.contains(name))
            .collect()
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
