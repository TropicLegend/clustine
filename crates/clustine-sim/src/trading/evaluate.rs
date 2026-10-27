//! The item modifiers and number providers of the trades, evaluated as the game's loot
//! functions and `ContextIntProvider`s are (`docs/groundwork/n10-dumps.md`, part B,
//! sections 5 and 8; n7 `EnchantRandomlyFunction`, `EnchantWithLevelsFunction`,
//! `SetRandomDyesFunction`, `SetRandomPotionFunction`, `SetStewEffectFunction`,
//! `FilteredFunction`, `SetPotionFunction`, `SetItemCountFunction`; n10 `DyedItemColor`).
//! Every draw is the game's, in its order, from the random the caller gives.
//!
//! The stack a trade's modifiers work on may carry the game's hidden
//! `minecraft:additional_trade_cost` (an `int`, synchronised but never saved, n9
//! DataComponents `lambda$static$42`), which the enchant functions write and
//! `VillagerTrade.getOffer` takes off before the offer is made. It lives only between the
//! two, so it is kept beside the stack here ([`TradeStack`]) rather than as a kind of
//! the world's components: no offer, container or packet ever holds it.
//!
//! `exploration_map` always fails (the plan's decision 9): Clustine keeps no structure
//! references over the 100 chunks the search reaches, and the game's function leaves the
//! stack as it is where it finds none, so the trade's `filtered` on `map_id` then
//! discards it, as the game does.

use clustine_data::enchanting;
use clustine_data::items;
use clustine_data::potions::POTIONS;
use clustine_data::trading::{
    ContextIntProvider, EnchantmentPredicate, ItemFilter, ItemSubPredicate, NamedSet, StewEffect,
    TradeComponent, TradeModifier,
};
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind, PotionContents};

use crate::items::enchanting::{self as enchant, next_int_between};
use crate::region::Random;

/// A stack as a trade's modifiers have it: the stack (`None` for the game's empty
/// stack) and the hidden additional trade cost on it, if any.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TradeStack {
    pub(crate) stack: Option<ItemStack>,
    pub(crate) additional_trade_cost: Option<i32>,
}

impl TradeStack {
    /// `stack`, with no hidden cost.
    pub(crate) fn of(stack: ItemStack) -> TradeStack {
        TradeStack {
            stack: Some(stack).filter(|stack| stack.count > 0),
            additional_trade_cost: None,
        }
    }
}

/// `ContextIntProvider.getInt`: a provider whose arithmetic overflows gives nought, as
/// the game's catch of `ArithmeticException` does (n10 ContextIntProvider 5–17).
pub(crate) fn int_value(provider: &ContextIntProvider, random: &mut Random) -> i32 {
    exact_int(provider, random).unwrap_or(0)
}

/// The provider's value, or `None` where an `add` overflows (`Math.addExact`).
fn exact_int(provider: &ContextIntProvider, random: &mut Random) -> Option<i32> {
    Some(match *provider {
        ContextIntProvider::Constant(value) => value,
        // `Mth.nextInt(random, min, max)`.
        ContextIntProvider::Uniform { min, max } => next_int_between(random, min, max),
        // `n` draws of `nextFloat`, each counting where it is below `p`.
        ContextIntProvider::Binomial { n, p } => {
            let mut count = 0;
            for _ in 0..n.max(0) {
                if random.next_float() < p {
                    count += 1;
                }
            }
            count
        }
        // The inputs in order (`minecraft:add`); the game's `addExact` throws at the
        // first overflow, so the inputs after it draw nothing. The game's data never
        // overflows.
        ContextIntProvider::Add(inputs) => {
            let mut sum: i32 = 0;
            for input in inputs {
                sum = sum.checked_add(exact_int(input, random)?)?;
            }
            sum
        }
    })
}

/// `DyeColor`'s texture colours (`getTextureDiffuseColor`, n7 DyeColor's static
/// initialiser), in its order of ids, white 0 to black 15. The alpha the game makes
/// opaque is left out: dyeing reads red, green and blue.
const DYE_COLOURS: [i32; 16] = [
    16_383_998, 16_351_261, 13_061_821, 3_847_130, 16_701_501, 8_439_583, 15_961_002, 4_673_362,
    10_329_495, 1_481_884, 8_991_416, 3_949_738, 8_606_770, 6_192_150, 11_546_150, 1_908_001,
];

/// `DyedItemColor.applyDyes(old, dyes)` (n10 DyedItemColor 59–210): the old colour and
/// the dyes' averaged, each channel scaled by the average of the brightest channels over
/// the brightest averaged channel, in `int`s but for the scale's `float`s.
pub(crate) fn mixed_colour(old: Option<i32>, dyes: &[i32]) -> i32 {
    let (mut red, mut green, mut blue, mut brightest, mut count) = (0i32, 0i32, 0i32, 0i32, 0i32);
    for colour in old.into_iter().chain(dyes.iter().copied()) {
        let (r, g, b) = ((colour >> 16) & 0xFF, (colour >> 8) & 0xFF, colour & 0xFF);
        brightest += r.max(g).max(b);
        red += r;
        green += g;
        blue += b;
        count += 1;
    }
    if count == 0 {
        // The game divides by nought here and throws; `set_random_dyes` never gives
        // no dye.
        return 0;
    }
    let (red, green, blue) = (red / count, green / count, blue / count);
    let average_brightest = brightest as f32 / count as f32;
    let top = red.max(green).max(blue) as f32;
    // `f2i` of NaN is nought and of a float beyond an `int` the nearest `int`, as Rust's
    // cast.
    let scaled = |channel: i32| (channel as f32 * average_brightest / top) as i32;
    // `ARGB.color(0, r, g, b)`.
    ((scaled(red) & 0xFF) << 16) | ((scaled(green) & 0xFF) << 8) | (scaled(blue) & 0xFF)
}

/// `MobEffect.isInstantaneous()`: instant health, instant damage and saturation (as
/// `region/thrown.rs` has it).
fn is_instantaneous(effect: &str) -> bool {
    matches!(
        effect,
        "minecraft:instant_health" | "minecraft:instant_damage" | "minecraft:saturation"
    )
}

/// A component a trade names, as the world keeps it.
pub(crate) fn world_component(component: &TradeComponent) -> Component {
    match component {
        TradeComponent::PotionContents { potion } => Component::PotionContents(PotionContents {
            potion: Some((*potion).to_owned()),
            ..PotionContents::default()
        }),
    }
}

/// The stack's `potion_contents`: its patch's, else none (the defaults of the potions
/// and the tipped arrow are empty).
fn potion_contents(stack: &ItemStack) -> PotionContents {
    match stack.components.get(Kind::PotionContents) {
        Some(Component::PotionContents(contents)) => contents.clone(),
        _ => PotionContents::default(),
    }
}

/// `PotionContents.withPotion`: the potion set, the rest kept.
fn set_potion(stack: &mut ItemStack, potion: &str) {
    let mut contents = potion_contents(stack);
    contents.potion = Some(potion.to_owned());
    stack.components.set(Component::PotionContents(contents));
}

/// Applies `modifiers` to `stack` in order (`LootItemFunctions`' sequence). Once the
/// stack is empty the rest are not applied: in the game's data a `discard` ends every
/// sequence it is in, and an empty stack gives no offer whatever follows.
pub(crate) fn apply_all(modifiers: &[TradeModifier], stack: &mut TradeStack, random: &mut Random) {
    for modifier in modifiers {
        if stack.stack.is_none() {
            return;
        }
        apply(modifier, stack, random);
    }
}

/// Applies one modifier as the game's loot function runs it.
pub(crate) fn apply(modifier: &TradeModifier, trade: &mut TradeStack, random: &mut Random) {
    let Some(stack) = trade.stack.as_mut() else {
        return;
    };
    match modifier {
        TradeModifier::EnchantRandomly {
            options,
            only_compatible,
            include_additional_cost_component,
        } => {
            // n7 EnchantRandomlyFunction `run` and `enchantItem`.
            let book = stack.item == items::BOOK;
            let compatible = !book && *only_compatible;
            let item = stack.item;
            let candidates: Vec<_> = enchant::options(options.as_ref())
                .into_iter()
                .filter(|enchantment| !compatible || enchantment.is_supported_item(item))
                .collect();
            if candidates.is_empty() {
                // "Couldn't find a compatible enchantment": the stack as it was.
                return;
            }
            let length = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
            let enchantment = candidates[random.next_int(length) as usize];
            let level = next_int_between(random, enchantment.min_level(), enchantment.max_level);
            if book {
                *trade = TradeStack::of(ItemStack::new(items::ENCHANTED_BOOK, 1));
            }
            let stack = trade
                .stack
                .as_mut()
                .expect("an enchanted book or the stack enchanted");
            enchant::enchant(stack, enchantment.name, level);
            if *include_additional_cost_component {
                // `2 + nextInt(5 + level·10) + 3·level`.
                let bound = 5i32.wrapping_add(level.wrapping_mul(10));
                let drawn = random.next_int(u32::try_from(bound).unwrap_or(1)) as i32;
                trade.additional_trade_cost = Some(
                    2i32.wrapping_add(drawn)
                        .wrapping_add(3i32.wrapping_mul(level)),
                );
            }
        }
        TradeModifier::EnchantWithLevels {
            levels,
            options,
            include_additional_cost_component,
        } => {
            // n7 EnchantWithLevelsFunction `run`.
            let levels = int_value(levels, random);
            let book = stack.item == items::BOOK;
            let options = enchant::options(options.as_ref());
            let enchanted = enchant::enchant_item(random, stack.clone(), levels, &options);
            if book {
                // A new stack: whatever the book carried is gone with it.
                *trade = TradeStack::of(enchanted);
            } else {
                *stack = enchanted;
            }
            if *include_additional_cost_component && trade.stack.is_some() && levels > 0 {
                trade.additional_trade_cost = Some(levels);
            }
        }
        TradeModifier::Filtered {
            item_filter,
            on_pass,
            on_fail,
        } => {
            let then = if passes(item_filter, stack) {
                on_pass
            } else {
                on_fail
            };
            apply_all(then, trade, random);
        }
        TradeModifier::Discard => trade.stack = None,
        TradeModifier::SetRandomDyes { number_of_dyes } => {
            let count = int_value(number_of_dyes, random);
            if count <= 0 {
                return;
            }
            // `Util.getRandom(DyeColor.VALUES, random)` for each.
            let dyes: Vec<i32> = (0..count)
                .map(|_| DYE_COLOURS[random.next_int(16) as usize])
                .collect();
            let old = match stack.components.get(Kind::DyedColor) {
                Some(Component::DyedColor(rgb)) => Some(*rgb),
                _ => None,
            };
            // `applyDyes(stack, dyes)`: on a copy of one.
            stack.count = 1;
            stack
                .components
                .set(Component::DyedColor(mixed_colour(old, &dyes)));
        }
        TradeModifier::SetRandomPotion { options } => {
            let names: Vec<&'static str> = POTIONS.iter().map(|potion| potion.name).collect();
            let candidates = match options {
                // `HolderSet.getRandomElement`: `Util.getRandomSafe` of its contents.
                Some(NamedSet::Tag(tag)) => enchanting::tag_names("minecraft:potion", tag, &names),
                Some(NamedSet::Listed(listed)) => listed.to_vec(),
                // `Registry.getRandom`: of every potion.
                None => names,
            };
            if candidates.is_empty() {
                return;
            }
            let length = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
            let potion = candidates[random.next_int(length) as usize];
            set_potion(stack, potion);
        }
        TradeModifier::SetStewEffect { effects } => {
            if stack.item != items::SUSPICIOUS_STEW || effects.is_empty() {
                return;
            }
            let length = u32::try_from(effects.len()).unwrap_or(u32::MAX);
            let StewEffect { effect, duration } = effects[random.next_int(length) as usize];
            let mut duration = int_value(&duration, random);
            if !is_instantaneous(effect) {
                duration = duration.wrapping_mul(20);
            }
            // `SuspiciousStewEffects.withEffectAdded`.
            let mut entries = match stack.components.get(Kind::SuspiciousStewEffects) {
                Some(Component::SuspiciousStewEffects(entries)) => entries.clone(),
                _ => Vec::new(),
            };
            entries.push((effect.to_owned(), duration));
            stack
                .components
                .set(Component::SuspiciousStewEffects(entries));
        }
        TradeModifier::SetPotion { potion } => set_potion(stack, potion),
        TradeModifier::SetCount { count, add } => {
            let base = if *add { stack.count } else { 0 };
            stack.count = base.wrapping_add(int_value(count, random));
            if stack.count <= 0 {
                trade.stack = None;
            }
        }
        // Decision 9: no structure is ever found, so the stack is as it was.
        TradeModifier::ExplorationMap { .. } => {}
    }
}

/// `ItemPredicate.test` of a `filtered`: the item of the set where there is one, and
/// every component predicate holding.
pub(crate) fn passes(filter: &ItemFilter, stack: &ItemStack) -> bool {
    if let Some(items) = &filter.items
        && !enchanting::item_in(items, stack.item)
    {
        return false;
    }
    filter.predicates.iter().all(|predicate| match predicate {
        // Every item has `enchantments` by default; a patch may remove it.
        ItemSubPredicate::Enchantments(wanted) => {
            !stack.components.removes(Kind::Enchantments)
                && enchantments_hold(wanted, levels_of(stack, Kind::Enchantments))
        }
        ItemSubPredicate::StoredEnchantments(wanted) => {
            // Only an enchanted book has stored enchantments by default; another item
            // has the component only where its patch gives it.
            match stack.components.get(Kind::StoredEnchantments) {
                Some(Component::StoredEnchantments(levels)) => enchantments_hold(wanted, levels),
                _ if stack.item == items::ENCHANTED_BOOK
                    && !stack.components.removes(Kind::StoredEnchantments) =>
                {
                    enchantments_hold(wanted, &[])
                }
                _ => false,
            }
        }
        // `DyedItemColorPredicate` with no colour: the item has one.
        ItemSubPredicate::DyedColor => matches!(
            stack.components.get(Kind::DyedColor),
            Some(Component::DyedColor(_))
        ),
        // No stack in Clustine has a map id.
        ItemSubPredicate::MapId => false,
    })
}

/// The stack's `enchantments`: its patch's, else the default every item has, none.
fn levels_of(stack: &ItemStack, kind: Kind) -> &[(String, i32)] {
    match stack.components.get(kind) {
        Some(Component::Enchantments(levels) | Component::StoredEnchantments(levels)) => levels,
        _ => &[],
    }
}

/// `ItemEnchantmentsPredicate`: each predicate contained in the enchantments.
fn enchantments_hold(wanted: &[EnchantmentPredicate], levels: &[(String, i32)]) -> bool {
    wanted
        .iter()
        .all(|predicate| contained_in(predicate, levels))
}

/// `EnchantmentPredicate.containedIn` (**Inferred**, not dumped): one of the set at a
/// level in range where there is a set; else any at a level in range where there are
/// bounds; else any at all.
fn contained_in(predicate: &EnchantmentPredicate, levels: &[(String, i32)]) -> bool {
    let in_range = |level: i32| {
        predicate.min_level.is_none_or(|min| level >= min)
            && predicate.max_level.is_none_or(|max| level <= max)
    };
    match &predicate.enchantments {
        Some(set) => levels.iter().any(|(name, level)| {
            *level > 0 && enchanting::enchantment_in(set, name) && in_range(*level)
        }),
        None if predicate.min_level.is_some() || predicate.max_level.is_some() => {
            levels.iter().any(|(_, level)| in_range(*level))
        }
        None => !levels.is_empty(),
    }
}
