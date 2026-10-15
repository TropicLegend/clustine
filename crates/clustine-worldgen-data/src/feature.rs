//! The game's features as data: every configured feature, every placed feature and
//! every named block-state provider of the pinned jar, each as a tree that mirrors its
//! file (`worldgen/feature`, `worldgen/placed_feature`, `worldgen/block_state_provider`);
//! for each dimension, the order in which the game places features, step by step; and
//! for each biome, the placed features it lists in each step.
//!
//! The trees are not typed here. A string in them may name a configured feature, a
//! placed feature, a provider, a block, a tag or a template, and only the place where it
//! stands says which (`minecraft:glow_lichen` is both a configured and a placed
//! feature), so the code that reads a type of feature resolves its own names, with
//! [`configured`], [`placed`] and [`provider`], and reads every member it is given:
//! what it leaves unread is a member it does not know. A number is kept as the file
//! writes it, so that it is parsed as the game's codec reads it, as a 32-bit or a
//! 64-bit float or an integer, and is rounded once.
//!
//! The order is the game's own (`ChunkGenerator.featuresPerStep`, read by
//! `tools/datagen/java/Extract.java`), not worked out here: a feature's place in its
//! step's list is part of its seed.

use crate::generated::features::{
    BIOME_FEATURES, CONFIGURED, ORDER, PLACED, PROCESSOR_LISTS, PROVIDERS,
};

/// A value in a feature's file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Data {
    Bool(bool),
    /// A number as the file writes it.
    Number(&'static str),
    Text(&'static str),
    List(&'static [Data]),
    /// The members in the file's order.
    Object(&'static [(&'static str, Data)]),
}

impl Data {
    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&'static Data> {
        match self {
            Data::Object(members) => members
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// What an object's member `type` names, the kind of a feature, a modifier, a
    /// provider or a placer.
    pub fn kind(&self) -> Option<&'static str> {
        match self.get("type") {
            Some(Data::Text(kind)) => Some(kind),
            _ => None,
        }
    }
}

/// A named entry of one of the registries.
#[derive(Debug)]
pub struct Entry {
    /// The name with its namespace, as other entries name it: `minecraft:oak`.
    pub name: &'static str,
    pub data: Data,
}

/// The order of one dimension's features: for each step of decoration, the placed
/// features in the order the game places them, as indices into [`placed_features`].
#[derive(Debug)]
pub struct Order {
    pub dimension: &'static str,
    pub steps: &'static [&'static [u16]],
}

/// The placed features a biome lists in each step, as indices into
/// [`placed_features`], in the order of the biome's file.
#[derive(Debug)]
pub struct BiomeFeatures {
    pub biome: &'static str,
    pub steps: &'static [&'static [u16]],
}

/// Every configured feature, sorted by name.
pub fn configured_features() -> &'static [Entry] {
    &CONFIGURED
}

/// Every placed feature, sorted by name.
pub fn placed_features() -> &'static [Entry] {
    &PLACED
}

/// Every named block-state provider, sorted by name.
pub fn providers() -> &'static [Entry] {
    &PROVIDERS
}

/// What each biome lists by step, in the order of the biomes' numbers.
pub fn biome_features() -> &'static [BiomeFeatures] {
    &BIOME_FEATURES
}

/// The configured feature called `name`, with its index.
pub fn configured(name: &str) -> Option<(usize, &'static Entry)> {
    find(&CONFIGURED, name)
}

/// The placed feature called `name`, with its index.
pub fn placed(name: &str) -> Option<(usize, &'static Entry)> {
    find(&PLACED, name)
}

/// The named block-state provider called `name`, with its index.
pub fn provider(name: &str) -> Option<(usize, &'static Entry)> {
    find(&PROVIDERS, name)
}

/// The named processor list called `name` (`worldgen/processor_list`), with its index:
/// what structure pools and features name by name.
pub fn processor_list(name: &str) -> Option<(usize, &'static Entry)> {
    find(&PROCESSOR_LISTS, name)
}

/// Every named processor list, sorted by name.
pub fn processor_lists() -> &'static [Entry] {
    &PROCESSOR_LISTS
}

/// The order of the features of `dimension` (`overworld`, `nether` or `end`).
pub fn order(dimension: &str) -> Option<&'static Order> {
    ORDER.iter().find(|order| order.dimension == dimension)
}

fn find(entries: &'static [Entry], name: &str) -> Option<(usize, &'static Entry)> {
    entries
        .binary_search_by(|entry| entry.name.cmp(name))
        .ok()
        .map(|index| (index, &entries[index]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registries_are_sorted_by_name_so_that_a_name_is_found() {
        for entries in [configured_features(), placed_features(), providers()] {
            assert!(entries.windows(2).all(|pair| pair[0].name < pair[1].name));
        }
        let (_, oak) = configured("minecraft:oak").unwrap();
        assert_eq!(oak.data.kind(), Some("minecraft:tree"));
        assert!(placed("minecraft:trees_plains").is_some());
        assert!(provider("minecraft:soil_beneath_tree").is_some());
        assert!(configured("minecraft:no_such_feature").is_none());
    }

    #[test]
    fn a_placed_feature_names_its_configured_feature_and_its_modifiers() {
        let (_, iron) = placed("minecraft:ore_iron_middle").unwrap();
        assert_eq!(
            iron.data.get("feature"),
            Some(&Data::Text("minecraft:ore_iron"))
        );
        let Some(Data::List(modifiers)) = iron.data.get("placement") else {
            panic!("a placed feature has a list of modifiers");
        };
        assert_eq!(modifiers[0].kind(), Some("minecraft:count"));
        assert_eq!(modifiers[0].get("count"), Some(&Data::Number("10")));
    }

    #[test]
    fn every_dimension_orders_eleven_steps_or_fewer_without_a_feature_twice() {
        for dimension in ["overworld", "nether", "end"] {
            let order = order(dimension).unwrap();
            assert!(order.steps.len() <= 11);
            let mut seen: Vec<u16> = order
                .steps
                .iter()
                .flat_map(|step| step.iter().copied())
                .collect();
            let all = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), all, "{dimension} places a feature twice");
        }
    }

    #[test]
    fn every_feature_a_biome_lists_is_placed_in_that_step_where_it_is_placed_at_all() {
        for biome in &BIOME_FEATURES {
            for (step, features) in biome.steps.iter().enumerate() {
                for feature in *features {
                    let placed_in: Vec<usize> = ORDER
                        .iter()
                        .flat_map(|order| order.steps.iter().enumerate())
                        .filter(|(_, ordered)| ordered.contains(feature))
                        .map(|(at, _)| at)
                        .collect();
                    assert!(
                        placed_in.is_empty() || placed_in.contains(&step),
                        "{} lists {} in step {step}, the game places it in {placed_in:?}",
                        biome.biome,
                        PLACED[*feature as usize].name
                    );
                }
            }
        }
    }
}
