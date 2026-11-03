//! What a broken block drops: its loot table evaluated as the game's `Block.getDrops`
//! evaluates it (`docs/groundwork/n4-brief.md`, section 8.3; `docs/groundwork/v-plan.md`,
//! sections 2.6 and 4.5). Read from the 26.3 jar's classes: `LootTable.getRandomItemsRaw`
//! and `createStackSplitter`, `LootPool.addRandomItems` and `addRandomItem`,
//! `LootPoolEntryContainer.expand`, `AlternativesEntry.compose`, `LootItem`, the
//! conditions and item functions the block tables use, and the integer number providers.
//!
//! The evaluator draws from a [`LootDraws`] the caller gives, in the order the game
//! draws from the table's random sequence, and reads nothing else of the world but the
//! blocks a `location_check` names. What it returns is the list `getDrops` returns: the
//! stacks in the game's order, split at the item's stack size, and empty stacks where
//! the game's list has them, since `Block.popResource` draws an empty stack's position
//! before it finds it empty. Placing them (`popResource`) is the caller's.
//!
//! How the block was broken is a [`Broken`]. The game always gives the table a tool
//! (`LootContextParams.TOOL`): `ItemStack.EMPTY` where a rule or an explosion breaks it
//! (`n6:Block.getDrops(state, level, pos, blockEntity) @18-24`), the player's main hand
//! where a player does (`getDrops(..., entity, tool) @18-23`, with the player as
//! `this_entity` @26-31). With the empty tool `match_tool` fails (silk touch and shears
//! never apply) and `table_bonus` and `apply_bonus` see an enchantment level of 0; with a
//! player's they read its item and its `minecraft:enchantments`. Without an explosion
//! `survives_explosion` holds and `explosion_decay` keeps every item, and
//! `entity_properties` fails without an entity (a chorus flower and a snow layer broken
//! by a rule drop nothing, in the game too, and broken by a player drop). A block broken
//! by an explosion (`BlockBehaviour.onExplosionHit`) is evaluated with what that gives
//! the table: the exploding entity as `this_entity`, and the explosion's radius where its
//! drops decay. Where a table needs what Clustine does not keep, the evaluator gives less
//! than the game, and these are named differences:
//!
//! - `copy_components` (containers, banners, heads, shulker boxes, beehives, the
//!   decorated pot, the copper golem statues): a stack here is an item and a count, so
//!   the plain item drops, without the block entity's name, contents, patterns, bees or
//!   sherds.
//! - `copy_state` (bee nests and beehives with their honey level, the copper golem
//!   statues with their pose): the plain item, without the `block_state` component.
//! - The decorated pot's `dynamic` entry (a cracked pot drops its sherds, which its
//!   block entity names): the evaluator has no block entity and drops nothing for it.
//! - The stack splitter's `isItemEnabled`: every item counts as enabled, since every
//!   block in a Clustine world is of the features it runs with.

use clustine_data::BlockState;
use clustine_data::ItemComponents;
use clustine_data::loot::{
    BonusFormula, IntProvider, LootCondition, LootEntry, LootModifier, LootTable, StatePredicate,
    block_table, condition, modifier,
};
use clustine_world::{BlockPos, ItemStack};

#[cfg(test)]
mod tests;

/// The draws of a loot table's random source, in the order the game takes them. The
/// simulation feeds it from the block's stream (`docs/groundwork/n4-plan.md`,
/// section 7, "Drops").
pub trait LootDraws {
    /// `RandomSource.nextFloat()`: in `[0, 1)`.
    fn next_float(&mut self) -> f32;

    /// `RandomSource.nextInt(bound)`: in `[0, bound)`, `bound` at least 1. The game
    /// draws even for a bound of 1 (`uniform_bonus_count` without an enchantment).
    fn next_int(&mut self, bound: i32) -> i32;
}

/// What the loot of a broken block is given beside the block and its place: by a rule
/// nothing but the empty tool ([`Broken::BY_A_RULE`]); by an explosion its direct
/// source, which the table sees as `this_entity`, and, where its drops decay
/// (`DESTROY_WITH_DECAY`), its radius (`EXPLOSION_RADIUS`); by a player the player as
/// `this_entity` and the tool in their main hand ([`Broken::by_player`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Broken<'a> {
    /// Whether there is a `this_entity`: the entity that exploded, or the player.
    pub this_entity: bool,
    /// The explosion's radius, given only where its drops decay.
    pub explosion_radius: Option<f32>,
    /// The tool, `None` for the game's `ItemStack.EMPTY` (a rule's break, an
    /// explosion's, a player's bare hand). An empty stack given here reads as `None`.
    pub tool: Option<&'a ItemStack>,
}

impl<'a> Broken<'a> {
    /// A block broken by a rule, with no entity, no explosion and the empty tool.
    pub const BY_A_RULE: Broken<'static> = Broken {
        this_entity: false,
        explosion_radius: None,
        tool: None,
    };

    /// A block broken by an explosion of `this_entity` (whether it has a source the
    /// table sees), with its radius where its drops decay, and the empty tool
    /// (`BlockBehaviour.onExplosionHit` gives `ItemStack.EMPTY`).
    pub const fn by_explosion(this_entity: bool, explosion_radius: Option<f32>) -> Self {
        Broken {
            this_entity,
            explosion_radius,
            tool: None,
        }
    }

    /// A block broken by a player holding `tool` in their main hand (an empty stack for
    /// a bare hand): `Block.getDrops(state, level, pos, blockEntity, player, tool)`,
    /// with the player as `this_entity`.
    pub const fn by_player(tool: &'a ItemStack) -> Self {
        Broken {
            this_entity: true,
            explosion_radius: None,
            tool: Some(tool),
        }
    }
}

/// What the block `state` at `at` drops, broken by a rule: by its own table or the
/// table of the block it drops like (a wall torch by the torch's), nothing for a block
/// without one. `block_at` answers for the blocks a `location_check` reads (the other
/// half of tall grass); a caller that cannot see a block answers air, which no such
/// check matches, as the game's fails where the block is not loaded.
pub fn block_drops(
    state: BlockState,
    at: BlockPos,
    draws: &mut impl LootDraws,
    block_at: &mut impl FnMut(BlockPos) -> BlockState,
) -> Vec<ItemStack> {
    block_drops_as(state, at, Broken::BY_A_RULE, draws, block_at)
}

/// [`block_drops`] of a block broken as `broken` says: by an explosion, a chorus flower
/// and a snow layer drop (their `entity_properties` see the exploding entity), and
/// `survives_explosion` and `explosion_decay` draw where the drops decay.
pub fn block_drops_as(
    state: BlockState,
    at: BlockPos,
    broken: Broken<'_>,
    draws: &mut impl LootDraws,
    block_at: &mut impl FnMut(BlockPos) -> BlockState,
) -> Vec<ItemStack> {
    let Some(table) = state.block_id().and_then(block_table) else {
        return Vec::new();
    };
    table_drops_as(table, state, at, broken, draws, block_at)
}

/// What `table` gives for the block `state` at `at` broken by a rule: the game's
/// `LootTable.getRandomItems` with the parameters of `Block.getDrops` without a block
/// entity.
pub fn table_drops(
    table: &'static LootTable,
    state: BlockState,
    at: BlockPos,
    draws: &mut impl LootDraws,
    block_at: &mut impl FnMut(BlockPos) -> BlockState,
) -> Vec<ItemStack> {
    table_drops_as(table, state, at, Broken::BY_A_RULE, draws, block_at)
}

/// [`table_drops`] of a block broken as `broken` says.
pub fn table_drops_as(
    table: &'static LootTable,
    state: BlockState,
    at: BlockPos,
    broken: Broken<'_>,
    draws: &mut impl LootDraws,
    block_at: &mut impl FnMut(BlockPos) -> BlockState,
) -> Vec<ItemStack> {
    let mut context = Context {
        state,
        at,
        broken,
        draws,
        block_at,
        drops: Vec::new(),
    };
    for pool in table.pools {
        // `addRandomItems`: the pool's condition, then its rolls (a constant, and no
        // bonus rolls without luck).
        if pool.condition.is_some_and(|number| !context.test(number)) {
            continue;
        }
        let rolls = context.int(pool.rolls);
        for _ in 0..rolls {
            context.add_random_item(pool.entries, [pool.modifiers, table.modifiers]);
        }
    }
    context.drops
}

/// A stack as the game's `ItemStack` counts it while the functions run: `setCount`
/// takes any number, and a count of nought or less makes the stack empty, whose
/// `getCount` is then nought.
#[derive(Clone, Copy)]
struct Stack {
    item: i32,
    count: i32,
}

impl Stack {
    fn count(self) -> i32 {
        if self.count <= 0 || self.item == clustine_data::items::AIR {
            0
        } else {
            self.count
        }
    }
}

struct Context<'a, D, W> {
    state: BlockState,
    at: BlockPos,
    broken: Broken<'a>,
    draws: &'a mut D,
    block_at: &'a mut W,
    drops: Vec<ItemStack>,
}

impl<D: LootDraws, W: FnMut(BlockPos) -> BlockState> Context<'_, D, W> {
    /// The tool, `None` where it is the empty stack (`ItemStack.isEmpty`: air, or a
    /// count of nought or less).
    fn tool(&self) -> Option<&ItemStack> {
        self.broken
            .tool
            .filter(|tool| tool.count > 0 && tool.item != clustine_data::items::AIR)
    }

    /// `EnchantmentHelper.getItemEnchantmentLevel` of the tool: 0 for the empty one,
    /// whose components are none.
    fn level(&self, enchantment: &str) -> i32 {
        self.tool()
            .map_or(0, |tool| tool.components.enchantment_level(enchantment))
    }

    /// `LootPool.addRandomItem`: every entry expanded in order (their conditions drawn
    /// as they are tested), the choices of no weight left out, then one stack of the
    /// one choice, without a draw, or of one chosen by weight among several (a draw the
    /// block tables never take: each pool has one entry, and an `alternatives` gives
    /// at most one).
    fn add_random_item(&mut self, entries: &'static [LootEntry], outer: [&'static [u16]; 2]) {
        let mut found = Vec::new();
        for entry in entries {
            self.expand(entry, &mut found);
        }
        found.retain(|entry| weight(entry) > 0);
        let chosen = match found.len() {
            0 => return,
            1 => found[0],
            _ => {
                let total: i32 = found.iter().map(|entry| weight(entry)).sum();
                let mut left = self.draws.next_int(total);
                let mut chosen = found[0];
                for entry in &found {
                    left -= weight(entry);
                    if left < 0 {
                        chosen = entry;
                        break;
                    }
                }
                chosen
            }
        };
        self.create(chosen, outer);
    }

    /// `LootPoolEntryContainer.expand`: an entry whose condition fails gives nothing
    /// and fails; an `alternatives` gives what its first child that succeeds gives.
    fn expand(&mut self, entry: &'static LootEntry, found: &mut Vec<&'static LootEntry>) -> bool {
        match entry {
            LootEntry::Item { condition, .. }
            | LootEntry::Dynamic { condition, .. }
            | LootEntry::Empty { condition, .. }
            | LootEntry::Table { condition, .. }
            | LootEntry::Tag { condition, .. } => {
                if condition.is_some_and(|number| !self.test(number)) {
                    return false;
                }
                found.push(entry);
                true
            }
            LootEntry::Alternatives {
                condition,
                children,
            } => {
                if condition.is_some_and(|number| !self.test(number)) {
                    return false;
                }
                for child in *children {
                    if self.expand(child, found) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// `createItemStack`: a stack of one, through the entry's functions, the pool's
    /// and the table's in that order, into the stack splitter.
    fn create(&mut self, entry: &'static LootEntry, outer: [&'static [u16]; 2]) {
        match entry {
            LootEntry::Item {
                item, modifiers, ..
            } => {
                let mut stack = Stack {
                    item: i32::from(*item),
                    count: 1,
                };
                for layer in [*modifiers, outer[0], outer[1]] {
                    for &number in layer {
                        stack = self.apply(number, stack);
                    }
                }
                self.split(stack);
            }
            // The decorated pot's sherds come from its block entity, which a rule's
            // break does not give here (the module's documentation). No block's table
            // nests another or names a tag, and nothing is nothing.
            LootEntry::Dynamic { .. }
            | LootEntry::Empty { .. }
            | LootEntry::Table { .. }
            | LootEntry::Tag { .. } => {}
            LootEntry::Alternatives { .. } => {
                unreachable!("an alternatives expands to its children, never to itself")
            }
        }
    }

    /// `LootTable.createStackSplitter`: a stack below its item's stack size as it is
    /// (an empty one too), a larger one in full stacks and the rest.
    fn split(&mut self, stack: Stack) {
        let count = stack.count();
        let max = i32::from(ItemComponents::of(stack.item).max_stack_size);
        if count < max {
            self.drops.push(ItemStack::new(stack.item, count));
            return;
        }
        let mut left = count;
        while left > 0 {
            let part = max.min(left);
            left -= part;
            self.drops.push(ItemStack::new(stack.item, part));
        }
    }

    /// `LootItemConditionalFunction.apply`: the function's condition first, then the
    /// function where it holds.
    fn apply(&mut self, number: u16, mut stack: Stack) -> Stack {
        let run = modifier(number);
        let when = match run {
            LootModifier::SetCount { condition, .. }
            | LootModifier::ExplosionDecay { condition }
            | LootModifier::ApplyBonus { condition, .. }
            | LootModifier::LimitCount { condition, .. }
            | LootModifier::CopyComponents { condition }
            | LootModifier::CopyState { condition, .. }
            | LootModifier::EnchantedCountIncrease { condition, .. }
            | LootModifier::FurnaceSmelt { condition }
            | LootModifier::SetPotion { condition, .. }
            | LootModifier::SetOminousBottleAmplifier { condition, .. } => *condition,
        };
        if when.is_some_and(|number| !self.test(number)) {
            return stack;
        }
        match run {
            // `SetItemCountFunction.run`.
            LootModifier::SetCount { count, add, .. } => {
                let base = if *add { stack.count() } else { 0 };
                stack.count = base.wrapping_add(self.int(*count));
            }
            // `ApplyBonusCount.run @0-49`: the formula at the tool's level of the
            // enchantment (0 for the empty tool, which the game always gives a block's
            // table, so the formula runs at level 0 rather than not at all). The
            // formulas are `ApplyBonusCount$OreDrops`, `$UniformBonusCount` and
            // `$BinomialWithBonusCount.calculateNewCount`, which are not in the dumps:
            // **Inferred** from the game's source as published and SteelMC's 26.3 port,
            // save that the uniform bonus draws at level 0 too, as the game's has no
            // test of the level there (SteelMC's skips the draw).
            LootModifier::ApplyBonus {
                enchantment,
                formula,
                ..
            } => {
                let level = self.level(enchantment);
                let count = stack.count();
                stack.count = match *formula {
                    // `count · (max(nextInt(level + 2) − 1, 0) + 1)`, drawn only above
                    // level 0.
                    BonusFormula::OreDrops => {
                        if level > 0 {
                            let bonus = self.draws.next_int(level.wrapping_add(2)) - 1;
                            count.wrapping_mul(bonus.max(0) + 1)
                        } else {
                            count
                        }
                    }
                    // `count + nextInt(bonusMultiplier · level + 1)`.
                    BonusFormula::UniformBonusCount { bonus_multiplier } => {
                        let bound = bonus_multiplier.wrapping_mul(level).wrapping_add(1);
                        count.wrapping_add(self.draws.next_int(bound))
                    }
                    // One float a round, `level + extraRounds` rounds, each below the
                    // probability adding one.
                    BonusFormula::BinomialWithBonusCount { extra, probability } => {
                        let mut count = count;
                        for _ in 0..level.wrapping_add(extra) {
                            if self.draws.next_float() < probability {
                                count = count.wrapping_add(1);
                            }
                        }
                        count
                    }
                };
            }
            // `LimitCount.run`: `Mth.clamp`, or one bound.
            LootModifier::LimitCount { min, max, .. } => {
                let mut count = stack.count();
                if let Some(min) = min {
                    count = count.max(*min);
                }
                if let Some(max) = max {
                    count = count.min(*max);
                }
                stack.count = count;
            }
            // `ApplyExplosionDecay.run`: each item kept by a draw at or below one in the
            // radius; without a radius, nothing. **Inferred**: the class is not in the
            // dumps; this is the game's rule as SteelMC's 26.3 port has it.
            LootModifier::ExplosionDecay { .. } => {
                if let Some(radius) = self.broken.explosion_radius {
                    let chance = 1.0 / radius;
                    let mut kept = 0_i32;
                    for _ in 0..stack.count() {
                        if self.draws.next_float() <= chance {
                            kept += 1;
                        }
                    }
                    stack.count = kept;
                }
            }
            // Components a stack here does not carry (the module's documentation).
            LootModifier::CopyComponents { .. }
            | LootModifier::CopyState { .. }
            | LootModifier::SetPotion { .. } => {}
            // The amplifier is drawn, and is a component the stack does not carry.
            LootModifier::SetOminousBottleAmplifier { amplifier, .. } => {
                self.int(*amplifier);
            }
            // Without an attacker there is no weapon, nor any level of its enchantment.
            LootModifier::EnchantedCountIncrease { .. } => {}
            LootModifier::FurnaceSmelt { .. } => {
                if let Some(cooked) = clustine_data::loot::smelted(stack.item) {
                    stack.item = cooked;
                }
            }
        }
        stack
    }

    /// `ContextIntProvider.getInt` of the providers the block tables use.
    fn int(&mut self, provider: IntProvider) -> i32 {
        match provider {
            IntProvider::Constant(value) => value,
            // `Mth.nextInt(random, min, max)`.
            IntProvider::Uniform { min, max } => {
                if min >= max {
                    min
                } else {
                    self.draws
                        .next_int(max.wrapping_sub(min).wrapping_add(1))
                        .wrapping_add(min)
                }
            }
            IntProvider::Binomial { n, p } => {
                let mut count = 0;
                for _ in 0..n {
                    if self.draws.next_float() < p {
                        count += 1;
                    }
                }
                count
            }
        }
    }

    /// `LootItemCondition.test` of the conditions the block tables use.
    fn test(&mut self, number: u16) -> bool {
        match condition(number) {
            // `ExplosionCondition.test`: without a radius true without a draw, with one a
            // draw at or below one in the radius (**Inferred**, as the decay above).
            LootCondition::SurvivesExplosion => match self.broken.explosion_radius {
                None => true,
                Some(radius) => self.draws.next_float() <= 1.0 / radius,
            },
            LootCondition::MatchBlock(predicate) => matches(self.state, predicate),
            // `LocationCheck`: from the centre of the block plus the offset, floored,
            // which is the block's position plus the offset.
            LootCondition::LocationCheck { offset, block } => {
                let at = BlockPos::new(
                    self.at.x.wrapping_add(offset[0]),
                    self.at.y.wrapping_add(offset[1]),
                    self.at.z.wrapping_add(offset[2]),
                );
                matches((self.block_at)(at), block)
            }
            // `CompositeLootItemCondition`: in order, stopping at the first that decides.
            LootCondition::AnyOf(terms) => {
                for &term in *terms {
                    if self.test(term) {
                        return true;
                    }
                }
                false
            }
            LootCondition::AllOf(terms) => {
                for &term in *terms {
                    if !self.test(term) {
                        return false;
                    }
                }
                true
            }
            LootCondition::Inverted(term) => !self.test(*term),
            // `BonusLevelTableCondition.test @0-80`: the chance at the tool's level,
            // the last for a level beyond the list, and a draw below it.
            LootCondition::TableBonus {
                enchantment,
                chances,
            } => {
                let level = usize::try_from(self.level(enchantment)).unwrap_or(0);
                let last = chances.len().saturating_sub(1);
                let chance = *chances
                    .get(level.min(last))
                    .expect("a table bonus lists its chances");
                self.draws.next_float() < chance
            }
            LootCondition::RandomChance { chance } => self.draws.next_float() < *chance,
            // `EntityPredicate.matches` fails at once without an entity, and an empty
            // predicate holds of any: only `this` is ever given, by an explosion.
            LootCondition::EntityProperties { entity } => {
                *entity == "this" && self.broken.this_entity
            }
            // `MatchTool.test @0-47`: `ItemPredicate.test` of the tool. The block
            // tables' predicates name only items, an item or a tag of them, and the
            // empty tool is air, which none names.
            LootCondition::MatchToolItems { items } => {
                self.tool().is_some_and(|tool| tool_is(tool.item, items))
            }
            // An `ItemPredicate` with only `minecraft:enchantments`
            // (`ItemEnchantmentsPredicate.Enchantments`): the tool's level of the
            // enchantment, read from `ENCHANTMENTS` alone, within `levels`, which the
            // block tables give as a minimum. A level of 0 is no enchantment and fails.
            // `ItemPredicate` and `EnchantmentPredicate` are not in the dumps: their
            // tests (the items, then the sub-predicates; a level of 0 failing before
            // `levels` is asked) are **Inferred** from the game's source as published.
            LootCondition::MatchToolEnchantment {
                enchantment,
                min_level,
            } => {
                let level = self.level(enchantment);
                level > 0 && level >= *min_level
            }
            // No player kills a block, and no block's table asks what an entity is or
            // what damaged it.
            LootCondition::KilledByPlayer
            | LootCondition::EntityMatches { .. }
            | LootCondition::DamageSource(_) => false,
            // No attacker has a weapon whose enchantment counts.
            LootCondition::RandomChanceWithEnchantedBonus { unenchanted, .. } => {
                self.draws.next_float() < *unenchanted
            }
        }
    }
}

/// Whether the item `item` is `items`: an item by name, or in an item tag with `#`
/// (`ItemPredicate`'s `items`, a `HolderSet`). A name the data has not got holds
/// nothing.
fn tool_is(item: i32, items: &str) -> bool {
    match items.strip_prefix('#') {
        Some(tag) => crate::data::item_in_tag(item, tag),
        None => clustine_data::item_id_by_name(items) == Some(item),
    }
}

/// The weight of a choice: a singleton's own, 1 for the decorated pot's `dynamic`, the
/// one other entry an expanded pool can hold.
fn weight(entry: &LootEntry) -> i32 {
    match entry {
        LootEntry::Item { weight, .. }
        | LootEntry::Empty { weight, .. }
        | LootEntry::Table { weight, .. }
        | LootEntry::Tag { weight, .. } => *weight,
        LootEntry::Alternatives { .. } | LootEntry::Dynamic { .. } => 1,
    }
}

/// `BlockPredicate.matchesState`: the block, and each named property having the value
/// (a property the block does not have fails).
fn matches(state: BlockState, predicate: &StatePredicate) -> bool {
    state.block_id() == Some(usize::from(predicate.block))
        && predicate
            .properties
            .iter()
            .all(|&(name, value)| state.property(name) == Some(value))
}
