//! Enchanting an item at random as the game's `EnchantmentHelper` does
//! (`docs/groundwork/n10-plan.md`, decision 8; n7 EnchantmentHelper 1119–1440): the
//! enchanting table's rule that a trade's `enchant_with_levels` and, later, raiders' and
//! mobs' equipment use, and the single enchantment of `enchant_randomly`. The
//! enchantments' weights, costs and sets are the generated data
//! (`clustine_data::enchanting`); what an enchantment does is `enchantments.rs`'s.
//!
//! Every draw is the game's, in its order: `selectEnchantment` draws `nextInt` twice for
//! the level's bonus and `nextFloat` twice for its spread, then a weighted pick
//! (`WeightedRandom.getRandomItem`, one `nextInt` of the total weight), then a `nextInt(50)`
//! before each further pick, the level halved after each.
//!
//! What is inferred, not dumped: `Enchantment$Cost.calculate` (`base +
//! per_level_above_first · (level − 1)`), the weight of a candidate being its
//! enchantment's (the lambda `WeightedRandom` is given), and `ItemStack.enchant` as
//! `ItemEnchantments$Mutable.upgrade` (a level above nought kept where it is higher than
//! the one there). Every item has `minecraft:enchantments` by default and the enchanted
//! book `minecraft:stored_enchantments`, so `updateEnchantments` always writes.

use clustine_data::enchanting::{self, Enchantment, are_compatible};
use clustine_data::items;
use clustine_data::trading::NamedSet;
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};

use crate::region::Random;

/// `EnchantmentInstance`: an enchantment at a level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EnchantmentInstance {
    pub(crate) enchantment: &'static Enchantment,
    pub(crate) level: i32,
}

/// The enchantments to choose from, in the game's stream order: `set`'s where there is
/// one (a tag in its order), else the whole registry in its order.
pub(crate) fn options(set: Option<&NamedSet>) -> Vec<&'static Enchantment> {
    match set {
        Some(set) => enchanting::enchantments_in(set),
        None => enchanting::enchantments().iter().collect(),
    }
}

/// `Mth.nextInt(random, min, max)`: `min` without a draw where `min >= max`, else
/// `nextInt(max − min + 1) + min`.
pub(crate) fn next_int_between(random: &mut Random, min: i32, max: i32) -> i32 {
    if min >= max {
        return min;
    }
    let span = u32::try_from(i64::from(max) - i64::from(min) + 1).unwrap_or(u32::MAX);
    min.wrapping_add(random.next_int(span) as i32)
}

/// Java's `Math.round(float)`: `floor(x + 1/2)`, saturating, nought for NaN. In doubles
/// the half is added exactly to any float.
fn round(x: f32) -> i32 {
    (f64::from(x) + 0.5).floor() as i32
}

/// `getAvailableEnchantmentResults(level, stack, options)`: of the options for the item
/// (primary for it, or any where it is a book), each at the highest level whose cost
/// range holds `level`, where one does.
pub(crate) fn available_results(
    level: i32,
    item: i32,
    options: &[&'static Enchantment],
) -> Vec<EnchantmentInstance> {
    let book = item == items::BOOK;
    let mut results = Vec::new();
    for &enchantment in options {
        if !(enchantment.is_primary_item(item) || book) {
            continue;
        }
        let mut at = enchantment.max_level;
        while at >= enchantment.min_level() {
            if level >= enchantment.min_cost(at) && level <= enchantment.max_cost(at) {
                results.push(EnchantmentInstance {
                    enchantment,
                    level: at,
                });
                break;
            }
            at -= 1;
        }
    }
    results
}

/// `WeightedRandom.getRandomItem(random, list, weight)`: one `nextInt` of the total
/// weight walked down the list, none and no draw where the total is nought.
fn weighted_pick(random: &mut Random, list: &[EnchantmentInstance]) -> Option<EnchantmentInstance> {
    // `getTotalWeight` sums in a `long` and refuses more than an `int`; 43 weights of at
    // most 1024 never come near.
    let total: i64 = list
        .iter()
        .map(|instance| i64::from(instance.enchantment.weight))
        .sum();
    let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
    let mut left = random.next_int(total) as i32;
    for instance in list {
        left -= instance.enchantment.weight;
        if left < 0 {
            return Some(*instance);
        }
    }
    None
}

/// `filterCompatibleEnchantments(list, last)`: drops what does not go with `last`, `last`
/// itself among them.
pub(crate) fn filter_compatible(list: &mut Vec<EnchantmentInstance>, last: &EnchantmentInstance) {
    list.retain(|instance| are_compatible(instance.enchantment, last.enchantment));
}

/// `selectEnchantment(random, stack, level, options)`: the enchantments an enchanting
/// table's `level` gives the item, none for an item that cannot be enchanted.
pub(crate) fn select_enchantment(
    random: &mut Random,
    item: i32,
    level: i32,
    options: &[&'static Enchantment],
) -> Vec<EnchantmentInstance> {
    let mut chosen = Vec::new();
    let Some(enchantability) = enchanting::enchantability(item) else {
        return chosen;
    };
    let quarter = u32::try_from(enchantability / 4 + 1).unwrap_or(1);
    // `level + (1 + nextInt(e/4 + 1) + nextInt(e/4 + 1))`, the draws in that order.
    let first = random.next_int(quarter) as i32;
    let second = random.next_int(quarter) as i32;
    let mut level = level.wrapping_add(1i32.wrapping_add(first).wrapping_add(second));
    let spread = (random.next_float() + random.next_float() - 1.0) * 0.15;
    let level_f = level as f32;
    level = round(level_f + level_f * spread).clamp(1, i32::MAX);
    let mut available = available_results(level, item, options);
    if available.is_empty() {
        return chosen;
    }
    chosen.extend(weighted_pick(random, &available));
    while random.next_int(50) as i32 <= level {
        if let Some(last) = chosen.last().copied() {
            filter_compatible(&mut available, &last);
        }
        if available.is_empty() {
            break;
        }
        chosen.extend(weighted_pick(random, &available));
        level /= 2;
    }
    chosen
}

/// `enchantItem(random, stack, level, options)`: the item enchanted as a table at
/// `level` would; a book becomes an enchanted book that stores them.
pub(crate) fn enchant_item(
    random: &mut Random,
    stack: ItemStack,
    level: i32,
    options: &[&'static Enchantment],
) -> ItemStack {
    let chosen = select_enchantment(random, stack.item, level, options);
    let mut stack = if stack.item == items::BOOK {
        ItemStack::new(items::ENCHANTED_BOOK, 1)
    } else {
        stack
    };
    for instance in chosen {
        enchant(&mut stack, instance.enchantment.name, instance.level);
    }
    stack
}

/// The component an item keeps its enchantments in (`getComponentType`): an enchanted
/// book's are stored for an anvil, every other item's its own.
pub(crate) fn enchantments_kind(item: i32) -> Kind {
    if item == items::ENCHANTED_BOOK {
        Kind::StoredEnchantments
    } else {
        Kind::Enchantments
    }
}

/// The stack's enchantments in the component [`enchantments_kind`] names: the patch's,
/// else the item's default, which is none.
pub(crate) fn enchantments_of(stack: &ItemStack) -> &[(String, i32)] {
    match stack.components.get(enchantments_kind(stack.item)) {
        Some(Component::Enchantments(levels) | Component::StoredEnchantments(levels)) => levels,
        _ => &[],
    }
}

/// `ItemStack.enchant(enchantment, level)`: the enchantment at `level` where that is
/// above nought and above what the stack has of it, in the stack's component for them,
/// kept in the registry's order.
pub(crate) fn enchant(stack: &mut ItemStack, name: &str, level: i32) {
    if level <= 0 {
        return;
    }
    let level = level.min(255);
    let mut levels = enchantments_of(stack).to_vec();
    match levels.iter_mut().find(|(known, _)| known == name) {
        Some(entry) => entry.1 = entry.1.max(level),
        None => levels.push((name.to_owned(), level)),
    }
    let levels = clustine_world::components::in_registry_order(levels);
    stack.components.set(match enchantments_kind(stack.item) {
        Kind::StoredEnchantments => Component::StoredEnchantments(levels),
        _ => Component::Enchantments(levels),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_world::EntityId;

    fn random(seed: u64) -> Random {
        Random::of_entity_aside(seed, EntityId(7), 3, 11)
    }

    fn named(name: &str) -> &'static Enchantment {
        enchanting::enchantment(name).expect("an enchantment of the registry")
    }

    /// `Mth.nextInt` draws nothing where the bounds meet, and otherwise stays within
    /// them.
    #[test]
    fn a_range_of_one_value_draws_nothing() {
        let mut drawn = random(1);
        let start = drawn.clone();
        assert_eq!(next_int_between(&mut drawn, 4, 4), 4);
        assert_eq!(drawn.draws_since(&start), 0);
        for _ in 0..200 {
            assert!((5..=19).contains(&next_int_between(&mut drawn, 5, 19)));
        }
    }

    #[test]
    fn java_s_round_of_a_float_rounds_halves_up() {
        assert_eq!(round(2.5), 3);
        assert_eq!(round(-2.5), -2);
        assert_eq!(round(2.4999998), 2);
        assert_eq!(round(f32::NAN), 0);
        assert_eq!(round(1e20), i32::MAX);
    }

    /// At a cost of 30 a diamond sword's sharpness is at level 3: level 3's costs, `1 +
    /// 11·2 = 23` to `21 + 11·2 = 43`, hold 30, and level 4's, 34 to 54, do not.
    #[test]
    fn each_option_is_at_the_highest_level_whose_costs_hold_the_level() {
        let found = available_results(30, items::DIAMOND_SWORD, &[named("minecraft:sharpness")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].level, 3);
        // Nothing for an item it is not primary for; anything for a book.
        assert!(available_results(30, items::STICK, &[named("minecraft:sharpness")]).is_empty());
        assert_eq!(
            available_results(30, items::BOOK, &[named("minecraft:sharpness")]).len(),
            1
        );
        // Below the lowest cost there is none.
        assert!(
            available_results(0, items::DIAMOND_SWORD, &[named("minecraft:sharpness")]).is_empty()
        );
    }

    #[test]
    fn what_goes_with_the_last_pick_is_kept_and_the_pick_itself_is_not() {
        let at = |name| EnchantmentInstance {
            enchantment: named(name),
            level: 1,
        };
        let mut list = vec![
            at("minecraft:sharpness"),
            at("minecraft:smite"),
            at("minecraft:unbreaking"),
            at("minecraft:looting"),
        ];
        filter_compatible(&mut list, &at("minecraft:sharpness"));
        let names: Vec<&str> = list.iter().map(|i| i.enchantment.name).collect();
        assert_eq!(names, ["minecraft:unbreaking", "minecraft:looting"]);
    }

    /// The draws of `selectEnchantment`, counted: two `nextInt`s and two `nextFloat`s,
    /// one for the first pick, then one `nextInt(50)` before each further pick and one
    /// for the pick; an item without enchantability draws nothing.
    #[test]
    fn select_enchantment_draws_as_the_game_does() {
        let options = options(Some(&NamedSet::Tag("#minecraft:on_traded_equipment")));
        let mut drawn = random(2);
        let start = drawn.clone();
        assert!(select_enchantment(&mut drawn, items::STICK, 30, &options).is_empty());
        assert_eq!(drawn.draws_since(&start), 0);
        for seed in 0..300 {
            let mut drawn = random(seed);
            let start = drawn.clone();
            let chosen = select_enchantment(&mut drawn, items::DIAMOND_CHESTPLATE, 15, &options);
            assert!(!chosen.is_empty(), "armour at 15 always has one");
            // 4 for the level, 1 for the first pick, then for each further pick a
            // nextInt(50) and the pick, and one last nextInt(50), which either fails or
            // finds the list run out.
            let extra = chosen.len() as u64 - 1;
            assert_eq!(
                drawn.draws_since(&start),
                6 + 2 * extra,
                "{} picks",
                chosen.len()
            );
            for (at, one) in chosen.iter().enumerate() {
                assert!(one.enchantment.is_primary_item(items::DIAMOND_CHESTPLATE));
                for other in &chosen[at + 1..] {
                    assert!(are_compatible(one.enchantment, other.enchantment));
                }
            }
        }
    }

    /// A book becomes an enchanted book whose enchantments are stored; a sword keeps its
    /// own.
    #[test]
    fn a_book_becomes_an_enchanted_book_that_stores_what_it_was_given() {
        let all = options(None);
        for seed in 0..50 {
            let book = enchant_item(&mut random(seed), ItemStack::new(items::BOOK, 1), 30, &all);
            assert_eq!(book.item, items::ENCHANTED_BOOK);
            assert_eq!(book.count, 1);
            assert!(book.components.get(Kind::Enchantments).is_none());
            assert!(!enchantments_of(&book).is_empty());
            let sword = enchant_item(
                &mut random(seed),
                ItemStack::new(items::DIAMOND_SWORD, 1),
                30,
                &all,
            );
            assert!(sword.components.get(Kind::StoredEnchantments).is_none());
            assert!(!enchantments_of(&sword).is_empty());
        }
    }

    #[test]
    fn enchanting_keeps_the_higher_level_in_the_registry_s_order() {
        let mut sword = ItemStack::new(items::DIAMOND_SWORD, 1);
        enchant(&mut sword, "minecraft:unbreaking", 2);
        enchant(&mut sword, "minecraft:sharpness", 3);
        enchant(&mut sword, "minecraft:unbreaking", 1);
        enchant(&mut sword, "minecraft:looting", 0);
        assert_eq!(
            sword.components.get(Kind::Enchantments),
            Some(&Component::Enchantments(vec![
                ("minecraft:sharpness".into(), 3),
                ("minecraft:unbreaking".into(), 2),
            ]))
        );
    }
}
