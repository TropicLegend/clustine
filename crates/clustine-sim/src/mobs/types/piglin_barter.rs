//! What a piglin gives for a gold ingot (`PiglinAi.getBarterResponseItems`, PiglinAi.txt
//! 769–791): the loot table `gameplay/piglin_bartering`, of the game's data
//! (`data/minecraft/loot_table/gameplay/piglin_bartering.json`, 26.3).
//!
//! `cargo datagen` does not compile the table (its loot compiler takes the tables of
//! entities, blocks and gifts, and none of the enchanting functions two of its entries
//! use), so it is written out here from the game's file, entry by entry: one roll of one
//! pool of nineteen weighted entries, with their counts (`set_count` of a uniform
//! number), potions (`set_potion`) and the enchantment `enchant_randomly` gives a book
//! and a pair of iron boots (its one option, Soul Speed, at a level of one to three).
//!
//! The draws are the piglin's own stream's, as a mob's loot is (`mobs/body/loot.rs`):
//! the game's loot context draws from a random of its own, which no comparison can
//! follow, so a barter is judged by its outcomes, not its draws. Their order is the
//! game's: the weighted choice (`nextInt` of the total), then the entry's function
//! (`Mth.nextInt(min, max)` of a count; `getRandomSafe`'s `nextInt(1)` of the one option
//! and the level's `Mth.nextInt(1, 3)` of an enchantment).

use clustine_data::items;
use clustine_world::ItemStack;
use clustine_world::components::{Component, PotionContents};

use crate::region::Random;

/// What an entry's function does to the stack it makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Function {
    /// None: one of the item.
    One,
    /// `set_count` of a uniform number from `min` to `max`, inclusive.
    Count { min: i32, max: i32 },
    /// `set_potion` of the potion named.
    Potion(&'static str),
    /// `enchant_randomly` of Soul Speed (levels one to three): a book becomes an
    /// enchanted book that stores it.
    SoulSpeed,
}

/// One entry of the table: its item, its weight and its function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) item: i32,
    pub(crate) weight: u32,
    pub(crate) function: Function,
}

/// The table's entries, in the file's order.
pub(crate) const ENTRIES: [Entry; 19] = [
    entry(items::BOOK, 5, Function::SoulSpeed),
    entry(items::IRON_BOOTS, 8, Function::SoulSpeed),
    entry(
        items::POTION,
        8,
        Function::Potion("minecraft:fire_resistance"),
    ),
    entry(
        items::SPLASH_POTION,
        8,
        Function::Potion("minecraft:fire_resistance"),
    ),
    entry(items::POTION, 10, Function::Potion("minecraft:water")),
    entry(items::IRON_NUGGET, 10, Function::Count { min: 10, max: 36 }),
    entry(items::ENDER_PEARL, 10, Function::Count { min: 2, max: 4 }),
    entry(items::DRIED_GHAST, 10, Function::Count { min: 1, max: 1 }),
    entry(items::STRING, 20, Function::Count { min: 3, max: 9 }),
    entry(items::QUARTZ, 20, Function::Count { min: 5, max: 12 }),
    entry(items::OBSIDIAN, 40, Function::One),
    entry(
        items::CRYING_OBSIDIAN,
        40,
        Function::Count { min: 1, max: 3 },
    ),
    entry(items::FIRE_CHARGE, 40, Function::One),
    entry(items::LEATHER, 40, Function::Count { min: 2, max: 4 }),
    entry(items::SOUL_SAND, 40, Function::Count { min: 2, max: 8 }),
    entry(items::NETHER_BRICK, 40, Function::Count { min: 2, max: 8 }),
    entry(
        items::SPECTRAL_ARROW,
        40,
        Function::Count { min: 6, max: 12 },
    ),
    entry(items::GRAVEL, 40, Function::Count { min: 8, max: 16 }),
    entry(items::BLACKSTONE, 40, Function::Count { min: 8, max: 16 }),
];

const fn entry(item: i32, weight: u32, function: Function) -> Entry {
    Entry {
        item,
        weight,
        function,
    }
}

/// The sum of the weights: 469.
pub(crate) fn total_weight() -> u32 {
    ENTRIES.iter().map(|entry| entry.weight).sum()
}

/// `Mth.nextInt(random, min, max)`: `min` without a draw where `min >= max`.
fn uniform(draws: &mut Random, min: i32, max: i32) -> i32 {
    if min >= max {
        return min;
    }
    let span = u32::try_from(max - min + 1).unwrap_or(1);
    draws.next_int(span) as i32 + min
}

/// The stack the entry `entry` makes, with the draws of its function.
pub(crate) fn make(entry: &Entry, draws: &mut Random) -> ItemStack {
    match entry.function {
        Function::One => ItemStack::new(entry.item, 1),
        Function::Count { min, max } => ItemStack::new(entry.item, uniform(draws, min, max)),
        Function::Potion(potion) => {
            let mut stack = ItemStack::new(entry.item, 1);
            stack
                .components
                .set(Component::PotionContents(PotionContents {
                    potion: Some(potion.to_owned()),
                    ..PotionContents::default()
                }));
            stack
        }
        Function::SoulSpeed => {
            // `getRandomSafe` of the one option, then its level.
            let _option = draws.next_int(1);
            let level = uniform(draws, 1, 3);
            let enchantment = vec![("minecraft:soul_speed".to_owned(), level)];
            if entry.item == items::BOOK {
                let mut stack = ItemStack::new(items::ENCHANTED_BOOK, 1);
                stack
                    .components
                    .set(Component::StoredEnchantments(enchantment));
                stack
            } else {
                let mut stack = ItemStack::new(entry.item, 1);
                stack.components.set(Component::Enchantments(enchantment));
                stack
            }
        }
    }
}

/// The entry a draw of `nextInt(total)` lands on.
pub(crate) fn chosen(roll: u32) -> &'static Entry {
    let mut left = roll;
    for entry in &ENTRIES {
        if left < entry.weight {
            return entry;
        }
        left -= entry.weight;
    }
    &ENTRIES[ENTRIES.len() - 1]
}

/// `getBarterResponseItems(piglin)`: one roll of the table, from the piglin's stream.
pub(crate) fn barter(draws: &mut Random) -> Vec<ItemStack> {
    let roll = draws.next_int(total_weight());
    let stack = make(chosen(roll), draws);
    if stack.count > 0 {
        vec![stack]
    } else {
        Vec::new()
    }
}
