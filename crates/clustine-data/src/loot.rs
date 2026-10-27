//! The loot tables of the game's data, compiled by `cargo datagen` into the form a loot
//! evaluator reads: the blocks' (`docs/groundwork/n4-brief.md`, sections 8.2 and 8.3),
//! the entities' and the tables they name (`docs/groundwork/n7-brief.md`, section 1.7).
//!
//! A table's pools, entries and modifiers are as in the game's files. The conditions
//! and the item modifiers are numbered once each, in [`condition`] and [`modifier`],
//! and named by number; a condition that the files name by a predicate file (the
//! tool's silk touch, shears) is that file's condition. Items are named by their ids
//! in [`crate::ITEMS`], blocks by theirs in [`crate::BLOCKS`], a nested table by its
//! number among [`tables`]; entity types and tags by their names.
//!
//! Only what the game's tables use is here; `cargo datagen` refuses a table with
//! anything else.

use crate::generated::loot_tables::{
    BLOCK_TABLES, CONDITIONS, ENTITY_TABLES, MODIFIERS, SMELTING, TABLES,
};

#[derive(Debug)]
pub struct LootTable {
    /// The table's name, such as `minecraft:blocks/oak_leaves`.
    pub name: &'static str,
    pub pools: &'static [LootPool],
    /// Applied to every stack of the table, after the pool's and the entry's.
    pub modifiers: &'static [u16],
}

#[derive(Debug)]
pub struct LootPool {
    /// How many times the pool draws: a constant, or a witch's uniform.
    pub rolls: IntProvider,
    /// Whether the pool draws at all.
    pub condition: Option<u16>,
    pub entries: &'static [LootEntry],
    /// Applied to every stack of the pool, after the entry's.
    pub modifiers: &'static [u16],
}

#[derive(Debug)]
pub enum LootEntry {
    /// One stack of one of the item, then its modifiers in order; chosen by its weight
    /// among a pool's choices.
    Item {
        item: u16,
        weight: i32,
        condition: Option<u16>,
        modifiers: &'static [u16],
    },
    /// Nothing, chosen by its weight (a guardian's pool of fish, crystals or nothing).
    Empty { weight: i32, condition: Option<u16> },
    /// What the table of this number in [`tables`] gives, through these modifiers (a
    /// sheep's wool of its colour, a guardian's fish).
    Table {
        table: u16,
        weight: i32,
        condition: Option<u16>,
        modifiers: &'static [u16],
    },
    /// The items of an item tag, by its name with `#`: each a choice of its own where
    /// `expand` (a creeper's music discs), else all of them.
    Tag {
        tag: &'static str,
        expand: bool,
        weight: i32,
        condition: Option<u16>,
        modifiers: &'static [u16],
    },
    /// The first child whose condition holds (the game's `alternatives`).
    Alternatives {
        condition: Option<u16>,
        children: &'static [LootEntry],
    },
    /// What the block entity gives, by name: the decorated pot's sherds.
    Dynamic {
        name: &'static str,
        condition: Option<u16>,
    },
}

/// A block and the values some of its properties have to have.
#[derive(Debug, PartialEq)]
pub struct StatePredicate {
    /// The block's id.
    pub block: u16,
    /// Property names and their values, as the game writes them.
    pub properties: &'static [(&'static str, &'static str)],
}

/// What an entity has to be (`EntityPredicate`), as far as the game's tables ask: each
/// part that is there has to hold, and an entity that is not there holds none.
#[derive(Debug, Default, PartialEq)]
pub struct EntityPredicate {
    /// Its type, by name, or a tag of types with `#`.
    pub kind: Option<&'static str>,
    /// Whether it burns (`minecraft:flags`).
    pub on_fire: Option<bool>,
    /// Whether it is a baby (`minecraft:flags`).
    pub baby: Option<bool>,
    /// The type of what it rides, by name.
    pub vehicle: Option<&'static str>,
    /// A sheep's colour, a dye's name (`minecraft:sheep/color`).
    pub sheep_colour: Option<&'static str>,
    /// A frog's variant (`minecraft:frog/variant`).
    pub frog_variant: Option<&'static str>,
    /// A mooshroom's colour, `red` or `brown` (`minecraft:mooshroom/variant`).
    pub mooshroom_variant: Option<&'static str>,
    /// A chicken's variant (`minecraft:chicken/variant`).
    pub chicken_variant: Option<&'static str>,
    /// Whether a sheep is sheared.
    pub sheared: Option<bool>,
    /// A slime's or magma cube's size, its least and greatest.
    pub cube_size: Option<(Option<i32>, Option<i32>)>,
    /// Whether a raider is its patrol's captain.
    pub captain: Option<bool>,
    /// An enchantment, or a tag of them with `#`, that what it holds in its main hand
    /// has.
    pub main_hand_enchantment: Option<&'static str>,
}

/// What a damage source has to be (`DamageSourcePredicate`).
#[derive(Debug, PartialEq)]
pub struct DamageSourcePredicate {
    /// Damage type tags, with `#`, each with whether its type is in it.
    pub tags: &'static [(&'static str, bool)],
    /// What caused it.
    pub source_entity: Option<EntityPredicate>,
    /// What dealt it.
    pub direct_entity: Option<EntityPredicate>,
}

#[derive(Debug, PartialEq)]
pub enum LootCondition {
    /// Without an explosion true, and without a draw.
    SurvivesExplosion,
    /// The block broken is this one with these values.
    MatchBlock(StatePredicate),
    /// The block at the broken one's position plus `offset` is this one.
    LocationCheck {
        offset: [i32; 3],
        block: StatePredicate,
    },
    AnyOf(&'static [u16]),
    AllOf(&'static [u16]),
    Inverted(u16),
    /// A draw below the chance of the tool's level of the enchantment, the last
    /// chance for a level beyond the list.
    TableBonus {
        enchantment: &'static str,
        chances: &'static [f32],
    },
    /// A draw below the chance.
    RandomChance {
        chance: f32,
    },
    /// `entity_properties` with an empty predicate: holds when there is such an entity
    /// (`this`, `attacker`, `direct_attacker`, `attacking_player`), and with none fails.
    EntityProperties {
        entity: &'static str,
    },
    /// The tool is one of these items: an item's name, or a tag's with `#`. Fails
    /// without a tool.
    MatchToolItems {
        items: &'static str,
    },
    /// The tool has the enchantment at this level or above. Fails without a tool.
    MatchToolEnchantment {
        enchantment: &'static str,
        min_level: i32,
    },
    /// A player dealt the damage within the memory of it (`killed_by_player`: the
    /// context has a `last_damage_player`).
    KilledByPlayer,
    /// A draw below the chance of the attacker's weapon's level of the enchantment:
    /// `unenchanted` at level 0, else `base + per_level · (level − 1)`.
    RandomChanceWithEnchantedBonus {
        enchantment: &'static str,
        unenchanted: f32,
        base: f32,
        per_level: f32,
    },
    /// `entity_properties`: the entity of the context named (as for
    /// [`LootCondition::EntityProperties`]) is there and is as the predicate says.
    EntityMatches {
        entity: &'static str,
        predicate: EntityPredicate,
    },
    /// `damage_source_properties`: there is a damage source, and it is as the
    /// predicate says.
    DamageSource(DamageSourcePredicate),
}

/// A count as the game's integer number providers give it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntProvider {
    Constant(i32),
    /// `Mth.nextInt(random, min, max)`: one draw unless `min >= max`.
    Uniform {
        min: i32,
        max: i32,
    },
    /// `n` draws, each counting where it is below `p`.
    Binomial {
        n: i32,
        p: f32,
    },
}

/// The formula of an `apply_bonus`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BonusFormula {
    OreDrops,
    UniformBonusCount { bonus_multiplier: i32 },
    BinomialWithBonusCount { extra: i32, probability: f32 },
}

/// An item modifier (the game's loot item function), applied where its condition
/// holds.
#[derive(Debug, PartialEq)]
pub enum LootModifier {
    /// Sets the count, or adds to it.
    SetCount {
        count: IntProvider,
        add: bool,
        condition: Option<u16>,
    },
    /// With an explosion, keeps each item with a chance; without one, nothing.
    ExplosionDecay { condition: Option<u16> },
    ApplyBonus {
        enchantment: &'static str,
        formula: BonusFormula,
        condition: Option<u16>,
    },
    /// Clamps the count.
    LimitCount {
        min: Option<i32>,
        max: Option<i32>,
        condition: Option<u16>,
    },
    /// Copies components of the block entity onto the stack (a container's contents,
    /// a banner's patterns, a custom name), which Clustine's stacks do not carry.
    CopyComponents { condition: Option<u16> },
    /// Copies the block's values of these properties onto the stack.
    CopyState {
        block: u16,
        properties: &'static [&'static str],
        condition: Option<u16>,
    },
    /// Looting's more (`enchanted_count_increase`): with the attacker's weapon's level
    /// of the enchantment above 0, the count rises by the level times a uniform float
    /// from `min` to `max`, rounded, held to `limit` where that is above 0.
    EnchantedCountIncrease {
        enchantment: &'static str,
        min: f32,
        max: f32,
        limit: i32,
        condition: Option<u16>,
    },
    /// The item becomes what smelting it makes ([`smelted`]), the count kept.
    FurnaceSmelt { condition: Option<u16> },
    /// The stack's potion, a component Clustine's stacks do not carry.
    SetPotion {
        potion: &'static str,
        condition: Option<u16>,
    },
    /// An ominous bottle's amplifier, a component Clustine's stacks do not carry; the
    /// count is drawn all the same.
    SetOminousBottleAmplifier {
        amplifier: IntProvider,
        condition: Option<u16>,
    },
}

/// The condition of this number. Panics if there is none.
pub fn condition(number: u16) -> &'static LootCondition {
    &CONDITIONS[usize::from(number)]
}

/// The item modifier of this number. Panics if there is none.
pub fn modifier(number: u16) -> &'static LootModifier {
    &MODIFIERS[usize::from(number)]
}

/// Every loot table, by name.
pub fn tables() -> &'static [LootTable] {
    &TABLES
}

/// The table of this number in [`tables`], as a `loot_table` entry names it. Panics if
/// there is none.
pub fn table(number: u16) -> &'static LootTable {
    &TABLES[usize::from(number)]
}

/// The loot table of the block of this id: its own, or the one of the block it drops
/// like (a wall torch drops what a torch drops). `None` for a block that drops nothing
/// (air, a fluid, a portal). Panics if there is no such block.
pub fn block_table(block_id: usize) -> Option<&'static LootTable> {
    TABLES.get(usize::from(BLOCK_TABLES[block_id]))
}

/// The table an entity of the type of this id drops when it dies
/// (`EntityType.getDefaultLootTable`), `None` for a type without one. Panics if there
/// is no such type.
pub fn entity_table(entity_type: i32) -> Option<&'static LootTable> {
    let index = usize::try_from(entity_type).expect("an entity type id is not negative");
    TABLES.get(usize::from(ENTITY_TABLES[index]))
}

/// What smelting the item of this id makes (`furnace_smelt`, the game's smelting
/// recipes), `None` for an item that does not smelt.
pub fn smelted(item: i32) -> Option<i32> {
    let item = u16::try_from(item).ok()?;
    let at = SMELTING.binary_search_by_key(&item, |&(raw, _)| raw).ok()?;
    Some(i32::from(SMELTING[at].1))
}

/// The table of this name, such as `minecraft:blocks/stone`.
pub fn table_by_name(name: &str) -> Option<&'static LootTable> {
    let index = TABLES.binary_search_by(|table| table.name.cmp(name)).ok()?;
    Some(&TABLES[index])
}

/// The professions with a gift table of their own for the Hero of the Village
/// (`GiveGiftToHero`, `docs/groundwork/n10-dumps.md`, part B, section 3).
const GIFT_PROFESSIONS: [&str; 13] = [
    "armorer",
    "butcher",
    "cartographer",
    "cleric",
    "farmer",
    "fisherman",
    "fletcher",
    "leatherworker",
    "librarian",
    "mason",
    "shepherd",
    "toolsmith",
    "weaponsmith",
];

/// The table a villager throws its gift to a Hero of the Village from
/// (`GiveGiftToHero.getLootTableToThrow`): a baby's, else its profession's (by registry
/// name, such as `minecraft:armorer`), else, for `minecraft:none`, `minecraft:nitwit` or
/// any other, the unemployed's.
pub fn hero_gift_table(profession: &str, baby: bool) -> &'static LootTable {
    let local = profession.strip_prefix("minecraft:").unwrap_or(profession);
    let name = if baby {
        "minecraft:gameplay/hero_of_the_village/baby_gift".to_owned()
    } else if GIFT_PROFESSIONS.contains(&local) {
        format!("minecraft:gameplay/hero_of_the_village/{local}_gift")
    } else {
        "minecraft:gameplay/hero_of_the_village/unemployed_gift".to_owned()
    };
    table_by_name(&name).expect("cargo datagen compiles every gift table")
}

/// The table of what a cat brings its owner in the morning
/// (`gameplay/cat_morning_gift`).
pub fn cat_morning_gift_table() -> &'static LootTable {
    table_by_name("minecraft:gameplay/cat_morning_gift")
        .expect("cargo datagen compiles the cat's morning gift")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockState, block_id_by_name, items};

    /// The gift tables (D10 B.3): the thirteen professions', the baby's and the
    /// unemployed's, all of type `gift`, and the cat's morning gift.
    #[test]
    fn the_hero_s_gifts_are_by_profession_and_the_cat_s_by_weight() {
        for profession in GIFT_PROFESSIONS {
            assert_eq!(
                hero_gift_table(&format!("minecraft:{profession}"), false).name,
                format!("minecraft:gameplay/hero_of_the_village/{profession}_gift")
            );
        }
        assert_eq!(
            hero_gift_table("minecraft:armorer", true).name,
            "minecraft:gameplay/hero_of_the_village/baby_gift"
        );
        for profession in ["minecraft:none", "minecraft:nitwit"] {
            assert_eq!(
                hero_gift_table(profession, false).name,
                "minecraft:gameplay/hero_of_the_village/unemployed_gift"
            );
        }
        // The armorer's: one roll of a chainmail piece, each of weight 1.
        let armorer = hero_gift_table("minecraft:armorer", false);
        let [pool] = armorer.pools else {
            panic!("one pool: {:?}", armorer.pools);
        };
        assert_eq!(pool.rolls, IntProvider::Constant(1));
        let gifts: Vec<i32> = pool
            .entries
            .iter()
            .map(|entry| match entry {
                LootEntry::Item {
                    item, weight: 1, ..
                } => i32::from(*item),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            gifts,
            [
                items::CHAINMAIL_HELMET,
                items::CHAINMAIL_CHESTPLATE,
                items::CHAINMAIL_LEGGINGS,
                items::CHAINMAIL_BOOTS
            ]
        );
        // The cat's: seven items, the phantom membrane of weight 2 and the rest 10.
        let cat = cat_morning_gift_table();
        let [pool] = cat.pools else {
            panic!("one pool: {:?}", cat.pools);
        };
        let weights: Vec<i32> = pool
            .entries
            .iter()
            .map(|entry| match entry {
                LootEntry::Item { weight, .. } => *weight,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(weights, [10, 10, 10, 10, 10, 10, 2]);
        // The thirteen, the baby's, the unemployed's and the cat's are tables beside
        // the others (`every_number_a_table_names_is_there` counts them too).
        assert_eq!(tables().len(), 1311 + 21 + 2 + 16);
    }

    fn table(block: &str) -> Option<&'static LootTable> {
        block_table(block_id_by_name(block).unwrap())
    }

    /// Oak leaves (the brief's section 8.2): the leaves themselves with shears or silk
    /// touch, else a sapling by the fortune chances; sticks and apples where neither.
    #[test]
    fn oak_leaves_drop_by_the_tool_and_by_chance() {
        let leaves = table("minecraft:oak_leaves").unwrap();
        assert_eq!(leaves.name, "minecraft:blocks/oak_leaves");
        assert_eq!(leaves.pools.len(), 3);
        let LootEntry::Alternatives { children, .. } = &leaves.pools[0].entries[0] else {
            panic!("the first pool is alternatives");
        };
        let LootEntry::Item {
            item,
            condition: by_tool,
            ..
        } = &children[0]
        else {
            panic!("the leaves themselves first");
        };
        assert_eq!(i32::from(*item), items::OAK_LEAVES);
        let LootCondition::AnyOf(terms) = condition(by_tool.unwrap()) else {
            panic!("shears or silk touch");
        };
        assert_eq!(
            condition(terms[0]),
            &LootCondition::MatchToolItems {
                items: "minecraft:shears"
            }
        );
        assert_eq!(
            condition(terms[1]),
            &LootCondition::MatchToolEnchantment {
                enchantment: "minecraft:silk_touch",
                min_level: 1
            }
        );
        let LootEntry::Item {
            item,
            condition: sapling,
            ..
        } = &children[1]
        else {
            panic!("a sapling second");
        };
        assert_eq!(i32::from(*item), items::OAK_SAPLING);
        let LootCondition::AllOf(terms) = condition(sapling.unwrap()) else {
            panic!("survives and a table bonus");
        };
        assert_eq!(condition(terms[0]), &LootCondition::SurvivesExplosion);
        assert_eq!(
            condition(terms[1]),
            &LootCondition::TableBonus {
                enchantment: "minecraft:fortune",
                chances: &[0.05, 0.0625, 0.083333336, 0.1]
            }
        );

        // The sticks: under neither shears nor silk touch, one or two.
        assert_eq!(
            condition(leaves.pools[1].condition.unwrap()),
            &LootCondition::Inverted(by_tool.unwrap())
        );
        let LootEntry::Item {
            item, modifiers, ..
        } = &leaves.pools[1].entries[0]
        else {
            panic!("sticks");
        };
        assert_eq!(i32::from(*item), items::STICK);
        assert_eq!(
            modifier(modifiers[0]),
            &LootModifier::SetCount {
                count: IntProvider::Uniform { min: 1, max: 2 },
                add: false,
                condition: None
            }
        );
        assert_eq!(
            modifier(modifiers[1]),
            &LootModifier::ExplosionDecay { condition: None }
        );
    }

    #[test]
    fn blocks_drop_like_others_or_not_at_all() {
        assert_eq!(
            table("minecraft:wall_torch").unwrap().name,
            "minecraft:blocks/torch"
        );
        assert!(table("minecraft:air").is_none());
        assert!(table("minecraft:water").is_none());
        // A table without pools drops nothing, and is there.
        assert!(table("minecraft:frosted_ice").unwrap().pools.is_empty());
        assert_eq!(
            table_by_name("minecraft:blocks/stone").map(|table| table.name),
            Some("minecraft:blocks/stone")
        );
    }

    /// A double slab drops two: a count set under a condition on the block's state.
    #[test]
    fn a_double_slab_sets_its_count_by_its_state() {
        let slab = table("minecraft:oak_slab").unwrap();
        let LootEntry::Item { modifiers, .. } = &slab.pools[0].entries[0] else {
            panic!("an item");
        };
        let LootModifier::SetCount {
            count,
            condition: when,
            ..
        } = modifier(modifiers[0])
        else {
            panic!("a count first");
        };
        assert_eq!(*count, IntProvider::Constant(2));
        let LootCondition::MatchBlock(predicate) = condition(when.unwrap()) else {
            panic!("on the block's state");
        };
        let block = BlockState::parse("minecraft:oak_slab")
            .unwrap()
            .block_id()
            .unwrap();
        assert_eq!(usize::from(predicate.block), block);
        assert_eq!(predicate.properties, &[("type", "double")]);
    }

    #[test]
    fn every_number_a_table_names_is_there() {
        fn check(entries: &[LootEntry]) {
            for entry in entries {
                match entry {
                    LootEntry::Item {
                        item,
                        weight,
                        condition: c,
                        modifiers,
                    } => {
                        assert!(usize::from(*item) < crate::ITEMS.len());
                        assert!(*weight > 0);
                        c.map(condition);
                        modifiers.iter().for_each(|&m| {
                            modifier(m);
                        });
                    }
                    LootEntry::Table {
                        table: number,
                        condition: c,
                        modifiers,
                        ..
                    } => {
                        crate::loot::table(*number);
                        c.map(condition);
                        modifiers.iter().for_each(|&m| {
                            modifier(m);
                        });
                    }
                    LootEntry::Tag {
                        condition: c,
                        modifiers,
                        ..
                    } => {
                        c.map(condition);
                        modifiers.iter().for_each(|&m| {
                            modifier(m);
                        });
                    }
                    LootEntry::Alternatives {
                        condition: c,
                        children,
                    } => {
                        c.map(condition);
                        check(children);
                    }
                    LootEntry::Empty { condition: c, .. }
                    | LootEntry::Dynamic { condition: c, .. } => {
                        c.map(condition);
                    }
                }
            }
        }
        // The blocks' 1201, the entities' 109 and the fish of a guardian's; a chicken's
        // laying, and the mooshroom's and the sheep's shearing with their colours' (21);
        // the bogged's and the snow golem's shearing (2).
        assert_eq!(tables().len(), 1311 + 21 + 2 + 16);
        for name in [
            "minecraft:gameplay/chicken_lay",
            "minecraft:shearing/bogged",
            "minecraft:shearing/mooshroom",
            "minecraft:shearing/sheep",
            "minecraft:shearing/snow_golem",
        ] {
            assert!(table_by_name(name).is_some(), "{name}");
        }
        assert!(tables().is_sorted_by(|a, b| a.name < b.name));
        for table in tables() {
            for pool in table.pools {
                if table.name.starts_with("minecraft:blocks/") {
                    assert_eq!(pool.rolls, IntProvider::Constant(1), "{}", table.name);
                }
                check(pool.entries);
            }
        }
    }

    fn entity(name: &str) -> Option<&'static LootTable> {
        let kind = crate::ENTITY_TYPES
            .iter()
            .position(|known| *known == name)
            .unwrap();
        entity_table(i32::try_from(kind).unwrap())
    }

    /// A cow (the brief's section 1.7): leather 0 to 2, raised by looting; beef 1 to 3,
    /// cooked where it burns, raised by looting.
    #[test]
    fn a_cow_drops_leather_and_beef_cooked_where_it_burns() {
        let cow = entity("minecraft:cow").unwrap();
        assert_eq!(cow.name, "minecraft:entities/cow");
        assert_eq!(cow.pools.len(), 2);
        let LootEntry::Item {
            item, modifiers, ..
        } = &cow.pools[0].entries[0]
        else {
            panic!("leather");
        };
        assert_eq!(i32::from(*item), items::LEATHER);
        assert_eq!(
            modifier(modifiers[0]),
            &LootModifier::SetCount {
                count: IntProvider::Uniform { min: 0, max: 2 },
                add: false,
                condition: None
            }
        );
        assert_eq!(
            modifier(modifiers[1]),
            &LootModifier::EnchantedCountIncrease {
                enchantment: "minecraft:looting",
                min: 0.0,
                max: 1.0,
                limit: 0,
                condition: None
            }
        );
        let LootEntry::Item {
            item, modifiers, ..
        } = &cow.pools[1].entries[0]
        else {
            panic!("beef");
        };
        assert_eq!(i32::from(*item), items::BEEF);
        let LootModifier::FurnaceSmelt { condition: when } = modifier(modifiers[1]) else {
            panic!("the smelt second: {:?}", modifier(modifiers[1]));
        };
        let LootCondition::AnyOf(terms) = condition(when.unwrap()) else {
            panic!("burning, or the weapon's");
        };
        assert_eq!(
            condition(terms[0]),
            &LootCondition::EntityMatches {
                entity: "this",
                predicate: EntityPredicate {
                    on_fire: Some(true),
                    ..EntityPredicate::default()
                }
            }
        );
        assert_eq!(smelted(items::BEEF), Some(items::COOKED_BEEF));
        assert_eq!(smelted(items::LEATHER), None);
    }

    /// A bogged's shearing (`loot_table/shearing/bogged.json`): one pool of two rolls,
    /// each a brown or a red mushroom of weight 1, set to a count of 1.
    #[test]
    fn a_bogged_s_shearing_rolls_twice_for_a_brown_or_a_red_mushroom() {
        let bogged = table_by_name("minecraft:shearing/bogged").unwrap();
        assert!(bogged.modifiers.is_empty());
        let [pool] = bogged.pools else {
            panic!("one pool: {:?}", bogged.pools);
        };
        assert_eq!(pool.rolls, IntProvider::Constant(2));
        assert_eq!(pool.condition, None);
        assert!(pool.modifiers.is_empty());
        assert_eq!(pool.entries.len(), 2);
        for (entry, expected) in pool
            .entries
            .iter()
            .zip([items::BROWN_MUSHROOM, items::RED_MUSHROOM])
        {
            let LootEntry::Item {
                item,
                weight: 1,
                condition: None,
                modifiers: [count],
            } = entry
            else {
                panic!("a mushroom of weight 1 with its count: {entry:?}");
            };
            assert_eq!(i32::from(*item), expected);
            assert_eq!(
                modifier(*count),
                &LootModifier::SetCount {
                    count: IntProvider::Constant(1),
                    add: false,
                    condition: None
                }
            );
        }
    }

    /// A snow golem's shearing (`loot_table/shearing/snow_golem.json`): one roll of the
    /// carved pumpkin, with nothing more.
    #[test]
    fn a_snow_golem_s_shearing_gives_its_carved_pumpkin() {
        let golem = table_by_name("minecraft:shearing/snow_golem").unwrap();
        assert!(golem.modifiers.is_empty());
        let [pool] = golem.pools else {
            panic!("one pool: {:?}", golem.pools);
        };
        assert_eq!(pool.rolls, IntProvider::Constant(1));
        assert_eq!(pool.condition, None);
        assert!(pool.modifiers.is_empty());
        let [
            LootEntry::Item {
                item,
                weight: 1,
                condition: None,
                modifiers: [],
            },
        ] = pool.entries
        else {
            panic!("the pumpkin alone: {:?}", pool.entries);
        };
        assert_eq!(i32::from(*item), items::CARVED_PUMPKIN);
    }

    /// A sheep's wool of its colour where it is not sheared, by a nested table; a
    /// witch's rolls of one to three; types without a table drop nothing.
    #[test]
    fn a_sheep_names_its_wool_s_table_and_a_witch_rolls_one_to_three() {
        let sheep = entity("minecraft:sheep").unwrap();
        let LootEntry::Alternatives { children, .. } = &sheep.pools[1].entries[0] else {
            panic!("the wool's alternatives");
        };
        assert_eq!(children.len(), 16);
        let LootEntry::Table {
            table: number,
            condition: when,
            ..
        } = &children[0]
        else {
            panic!("a table");
        };
        assert_eq!(
            crate::loot::table(*number).name,
            "minecraft:entities/sheep/white"
        );
        assert_eq!(
            condition(when.unwrap()),
            &LootCondition::EntityMatches {
                entity: "this",
                predicate: EntityPredicate {
                    sheep_colour: Some("white"),
                    sheared: Some(false),
                    ..EntityPredicate::default()
                }
            }
        );
        let witch = entity("minecraft:witch").unwrap();
        assert_eq!(
            witch.pools[0].rolls,
            IntProvider::Uniform { min: 1, max: 3 }
        );
        assert!(matches!(
            witch.pools[0].entries[5],
            LootEntry::Item { weight: 2, .. }
        ));
        assert!(entity("minecraft:item").is_none());
        assert!(entity("minecraft:allay").unwrap().pools.is_empty());
    }
}
