//! What a biome makes when mobs are made (`generated/spawns.rs`): its spawners by
//! category, its spawn costs, and the probability of the creatures made with a chunk
//! (`docs/groundwork/n7-brief.md`, sections 8.0 and 8.9).

/// `MobSpawnSettings$SpawnerData`: an entity type, by its number among
/// [`crate::ENTITY_TYPES`], its weight, and how many of a group, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spawner {
    pub kind: usize,
    pub weight: i32,
    pub min_count: i32,
    pub max_count: i32,
}

/// `MobSpawnSettings$MobSpawnCost` of an entity type: its energy budget and charge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnCost {
    pub kind: usize,
    pub energy_budget: f64,
    pub charge: f64,
}

/// One biome's: by category name (`creature`, `monster` and the rest), the spawners of
/// those it defines; its costs; and `creature_world_gen_spawn_probability`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiomeSpawns {
    pub name: &'static str,
    pub creature_probability: f32,
    pub categories: &'static [(&'static str, &'static [Spawner])],
    pub costs: &'static [SpawnCost],
}

impl BiomeSpawns {
    /// The spawners of `category`, `None` where the biome does not define it.
    pub fn of(&self, category: &str) -> Option<&'static [Spawner]> {
        self.categories
            .iter()
            .find(|(name, _)| *name == category)
            .map(|(_, spawners)| *spawners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BIOME_SPAWNS, entity_types};

    fn biome(name: &str) -> &'static BiomeSpawns {
        BIOME_SPAWNS
            .iter()
            .find(|biome| biome.name == name)
            .expect("a biome")
    }

    fn spawner(kind: i32, weight: i32, min_count: i32, max_count: i32) -> Spawner {
        Spawner {
            kind: kind as usize,
            weight,
            min_count,
            max_count,
        }
    }

    /// The brief's table 8.9: the plains' creatures in the data's order, with their
    /// weights and counts.
    #[test]
    fn the_plains_make_sheep_pigs_chickens_cows_horses_and_donkeys_as_the_brief_reads_them() {
        use entity_types::*;
        assert_eq!(
            biome("minecraft:plains").of("creature"),
            Some(
                &[
                    spawner(SHEEP, 12, 4, 4),
                    spawner(PIG, 10, 4, 4),
                    spawner(CHICKEN, 10, 4, 4),
                    spawner(COW, 8, 4, 4),
                    spawner(HORSE, 5, 2, 6),
                    spawner(DONKEY, 1, 1, 3),
                ][..]
            )
        );
    }

    /// Five biomes lower the probability of the creatures made with a chunk; every other
    /// leaves the game's 0.1 (the brief, section 8.0).
    #[test]
    fn five_biomes_lower_the_probability_and_the_rest_keep_a_tenth() {
        let lowered: Vec<(&str, f32)> = BIOME_SPAWNS
            .iter()
            .filter(|biome| biome.creature_probability != 0.1)
            .map(|biome| (biome.name, biome.creature_probability))
            .collect();
        assert_eq!(
            lowered,
            [
                ("minecraft:badlands", 0.03),
                ("minecraft:eroded_badlands", 0.03),
                ("minecraft:ice_spikes", 0.07),
                ("minecraft:snowy_plains", 0.07),
                ("minecraft:wooded_badlands", 0.04),
            ]
        );
    }

    /// A biome that defines no category has none, and the soul sand valley's costs are
    /// read with their budgets and charges.
    #[test]
    fn categories_not_defined_are_absent_and_costs_are_read() {
        assert_eq!(biome("minecraft:deep_ocean").of("creature"), None);
        assert_eq!(biome("minecraft:the_void").of("creature"), Some(&[][..]));
        let valley = biome("minecraft:soul_sand_valley");
        assert!(
            valley
                .costs
                .iter()
                .any(|cost| cost.kind == entity_types::GHAST as usize
                    && cost.energy_budget == 0.15
                    && cost.charge == 0.7)
        );
    }
}
