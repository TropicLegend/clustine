//! The block loot tables of the game's data, compiled by `cargo datagen` into the form a
//! loot evaluator reads (`docs/groundwork/n4-brief.md`, sections 8.2 and 8.3).
//!
//! A table's pools, entries and modifiers are as in the game's files. The conditions
//! and the item modifiers are numbered once each, in [`condition`] and [`modifier`],
//! and named by number; a condition that the files name by a predicate file (the
//! tool's silk touch, shears) is that file's condition. Items are named by their ids
//! in [`crate::ITEMS`], blocks by theirs in [`crate::BLOCKS`].
//!
//! Only what the game's block tables use is here; `cargo datagen` refuses a table with
//! anything else.

use crate::generated::loot_tables::{BLOCK_TABLES, CONDITIONS, MODIFIERS, TABLES};

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
    /// How many times the pool draws: a constant in every block table.
    pub rolls: i32,
    /// Whether the pool draws at all.
    pub condition: Option<u16>,
    pub entries: &'static [LootEntry],
    /// Applied to every stack of the pool, after the entry's.
    pub modifiers: &'static [u16],
}

#[derive(Debug)]
pub enum LootEntry {
    /// One stack of one of the item, then its modifiers in order.
    Item {
        item: u16,
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
    /// (`this`), and with none fails.
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
}

/// The condition of this number. Panics if there is none.
pub fn condition(number: u16) -> &'static LootCondition {
    &CONDITIONS[usize::from(number)]
}

/// The item modifier of this number. Panics if there is none.
pub fn modifier(number: u16) -> &'static LootModifier {
    &MODIFIERS[usize::from(number)]
}

/// Every block loot table, by name.
pub fn tables() -> &'static [LootTable] {
    &TABLES
}

/// The loot table of the block of this id: its own, or the one of the block it drops
/// like (a wall torch drops what a torch drops). `None` for a block that drops nothing
/// (air, a fluid, a portal). Panics if there is no such block.
pub fn block_table(block_id: usize) -> Option<&'static LootTable> {
    TABLES.get(usize::from(BLOCK_TABLES[block_id]))
}

/// The table of this name, such as `minecraft:blocks/stone`.
pub fn table_by_name(name: &str) -> Option<&'static LootTable> {
    let index = TABLES.binary_search_by(|table| table.name.cmp(name)).ok()?;
    Some(&TABLES[index])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockState, block_id_by_name, items};

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
                        condition: c,
                        modifiers,
                    } => {
                        assert!(usize::from(*item) < crate::ITEMS.len());
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
                    LootEntry::Dynamic { condition: c, .. } => {
                        c.map(condition);
                    }
                }
            }
        }
        assert_eq!(tables().len(), 1201);
        assert!(tables().is_sorted_by(|a, b| a.name < b.name));
        for table in tables() {
            for pool in table.pools {
                assert_eq!(pool.rolls, 1, "{}", table.name);
                check(pool.entries);
            }
        }
    }
}
