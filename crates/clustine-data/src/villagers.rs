//! Villagers' types and professions: their registry ids, and the type a villager takes
//! from the biome at its feet (`docs/groundwork/n7k-brief.md`, section 2.1).
//!
//! Neither registry is synchronised, so a client knows their ids only as the game's
//! built-in registries number them: in the order of registration. That order is the
//! order of the `register` calls in each class's `bootstrap` (the dumps
//! `n7-javap/net.minecraft.world.entity.npc.villager.VillagerType.txt` and
//! `VillagerProfession.txt`), and the generated `reports/registries.json` gives the
//! same `protocol_id`s. Written by hand from the dumps, as the lists of
//! `random_ticks/fire.rs` are; nothing here is generated.

/// The villager types' registry ids (`VillagerType.bootstrap`, VillagerType.txt 48–78).
pub mod types {
    pub const DESERT: i32 = 0;
    pub const JUNGLE: i32 = 1;
    pub const PLAINS: i32 = 2;
    pub const SAVANNA: i32 = 3;
    pub const SNOW: i32 = 4;
    pub const SWAMP: i32 = 5;
    pub const TAIGA: i32 = 6;
}

/// The villager types' names, by registry id.
pub const TYPE_NAMES: [&str; 7] = [
    "minecraft:desert",
    "minecraft:jungle",
    "minecraft:plains",
    "minecraft:savanna",
    "minecraft:snow",
    "minecraft:swamp",
    "minecraft:taiga",
];

/// The professions' registry ids (`VillagerProfession.bootstrap`, VillagerProfession.txt
/// 165–695: none at 168, armorer 175, butcher 214, cartographer 253, cleric 292, farmer
/// 331, fisherman 377, fletcher 416, leatherworker 455, librarian 494, mason 533, nitwit
/// 572, shepherd 579, toolsmith 618, weaponsmith 657).
pub mod professions {
    pub const NONE: i32 = 0;
    pub const ARMORER: i32 = 1;
    pub const BUTCHER: i32 = 2;
    pub const CARTOGRAPHER: i32 = 3;
    pub const CLERIC: i32 = 4;
    pub const FARMER: i32 = 5;
    pub const FISHERMAN: i32 = 6;
    pub const FLETCHER: i32 = 7;
    pub const LEATHERWORKER: i32 = 8;
    pub const LIBRARIAN: i32 = 9;
    pub const MASON: i32 = 10;
    pub const NITWIT: i32 = 11;
    pub const SHEPHERD: i32 = 12;
    pub const TOOLSMITH: i32 = 13;
    pub const WEAPONSMITH: i32 = 14;
}

/// The professions' names, by registry id.
pub const PROFESSION_NAMES: [&str; 15] = [
    "minecraft:none",
    "minecraft:armorer",
    "minecraft:butcher",
    "minecraft:cartographer",
    "minecraft:cleric",
    "minecraft:farmer",
    "minecraft:fisherman",
    "minecraft:fletcher",
    "minecraft:leatherworker",
    "minecraft:librarian",
    "minecraft:mason",
    "minecraft:nitwit",
    "minecraft:shepherd",
    "minecraft:toolsmith",
    "minecraft:weaponsmith",
];

/// The biomes `VillagerType.BY_BIOME` names and their types, sorted by name for a
/// binary search (the map built in `lambda$static$0`, VillagerType.txt 94–242): 29
/// biomes, as the plan's review found; the brief's count of 33 was wrong.
const BY_BIOME: [(&str, i32); 29] = [
    ("minecraft:badlands", types::DESERT),
    ("minecraft:bamboo_jungle", types::JUNGLE),
    ("minecraft:deep_frozen_ocean", types::SNOW),
    ("minecraft:desert", types::DESERT),
    ("minecraft:eroded_badlands", types::DESERT),
    ("minecraft:frozen_ocean", types::SNOW),
    ("minecraft:frozen_peaks", types::SNOW),
    ("minecraft:frozen_river", types::SNOW),
    ("minecraft:grove", types::SNOW),
    ("minecraft:ice_spikes", types::SNOW),
    ("minecraft:jagged_peaks", types::SNOW),
    ("minecraft:jungle", types::JUNGLE),
    ("minecraft:mangrove_swamp", types::SWAMP),
    ("minecraft:old_growth_pine_taiga", types::TAIGA),
    ("minecraft:old_growth_spruce_taiga", types::TAIGA),
    ("minecraft:savanna", types::SAVANNA),
    ("minecraft:savanna_plateau", types::SAVANNA),
    ("minecraft:snowy_beach", types::SNOW),
    ("minecraft:snowy_plains", types::SNOW),
    ("minecraft:snowy_slopes", types::SNOW),
    ("minecraft:snowy_taiga", types::SNOW),
    ("minecraft:sparse_jungle", types::JUNGLE),
    ("minecraft:swamp", types::SWAMP),
    ("minecraft:taiga", types::TAIGA),
    ("minecraft:windswept_forest", types::TAIGA),
    ("minecraft:windswept_gravelly_hills", types::TAIGA),
    ("minecraft:windswept_hills", types::TAIGA),
    ("minecraft:windswept_savanna", types::SAVANNA),
    ("minecraft:wooded_badlands", types::DESERT),
];

/// The type of a villager whose type is settled at a biome of this name
/// (`VillagerType.byBiome`, VillagerType.txt 79–93): the map's, and plains for every
/// biome it does not name (`VillagerData.DEFAULT_TYPE`).
pub fn type_by_biome_name(biome: &str) -> i32 {
    BY_BIOME
        .binary_search_by(|(name, _)| (*name).cmp(biome))
        .map_or(types::PLAINS, |index| BY_BIOME[index].1)
}

/// The type of a villager whose type is settled at the biome of this id, a biome's id
/// as a chunk's sections hold it (its place in the synchronised registry
/// `minecraft:worldgen/biome`). An id the registry does not hold is plains, as a biome
/// without a key is in the game.
pub fn type_by_biome(biome: u16) -> i32 {
    crate::synced_registry("minecraft:worldgen/biome")
        .and_then(|biomes| biomes.entries.get(usize::from(biome)))
        .map_or(types::PLAINS, |name| type_by_biome_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn biome(name: &str) -> u16 {
        let biomes = crate::synced_registry("minecraft:worldgen/biome").unwrap();
        u16::try_from(biomes.id_of(name).unwrap()).unwrap()
    }

    #[test]
    fn the_biomes_of_the_map_are_sorted_and_each_the_game_s() {
        assert!(BY_BIOME.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let biomes = crate::synced_registry("minecraft:worldgen/biome").unwrap();
        for (name, _) in BY_BIOME {
            assert!(biomes.id_of(name).is_some(), "{name}");
        }
    }

    #[test]
    fn the_map_names_twenty_nine_biomes_by_type_as_the_dump_does() {
        let count = |kind| BY_BIOME.iter().filter(|(_, of)| *of == kind).count();
        assert_eq!(count(types::DESERT), 4);
        assert_eq!(count(types::JUNGLE), 3);
        assert_eq!(count(types::PLAINS), 0);
        assert_eq!(count(types::SAVANNA), 3);
        assert_eq!(count(types::SNOW), 11);
        assert_eq!(count(types::SWAMP), 2);
        assert_eq!(count(types::TAIGA), 6);
    }

    #[test]
    fn a_biome_of_each_type_gives_its_type_and_others_fall_to_plains() {
        for (name, expected) in [
            ("minecraft:eroded_badlands", types::DESERT),
            ("minecraft:sparse_jungle", types::JUNGLE),
            ("minecraft:windswept_savanna", types::SAVANNA),
            ("minecraft:grove", types::SNOW),
            ("minecraft:mangrove_swamp", types::SWAMP),
            ("minecraft:windswept_gravelly_hills", types::TAIGA),
            // Not in the map: plains, the default, as plains itself is.
            ("minecraft:plains", types::PLAINS),
            ("minecraft:mushroom_fields", types::PLAINS),
            ("minecraft:cherry_grove", types::PLAINS),
            ("minecraft:the_void", types::PLAINS),
        ] {
            assert_eq!(type_by_biome(biome(name)), expected, "{name}");
            assert_eq!(type_by_biome_name(name), expected, "{name}");
        }
    }

    #[test]
    fn an_id_beyond_the_registry_is_plains() {
        assert_eq!(type_by_biome(u16::MAX), types::PLAINS);
    }

    #[test]
    fn the_names_stand_at_their_ids() {
        assert_eq!(TYPE_NAMES[types::DESERT as usize], "minecraft:desert");
        assert_eq!(TYPE_NAMES[types::PLAINS as usize], "minecraft:plains");
        assert_eq!(TYPE_NAMES[types::TAIGA as usize], "minecraft:taiga");
        assert_eq!(
            PROFESSION_NAMES[professions::NONE as usize],
            "minecraft:none"
        );
        assert_eq!(
            PROFESSION_NAMES[professions::NITWIT as usize],
            "minecraft:nitwit"
        );
        assert_eq!(
            PROFESSION_NAMES[professions::WEAPONSMITH as usize],
            "minecraft:weaponsmith"
        );
    }
}
