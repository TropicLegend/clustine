//! What a block broken by a rule drops: its loot table evaluated as the game's
//! `Block.getDrops(state, level, pos, blockEntity)` evaluates it, with no tool, no
//! entity and no explosion (`docs/groundwork/n4-brief.md`, section 8.3). Read from the
//! 26.3 jar's classes: `LootTable.getRandomItemsRaw` and `createStackSplitter`,
//! `LootPool.addRandomItems` and `addRandomItem`, `LootPoolEntryContainer.expand`,
//! `AlternativesEntry.compose`, `LootItem`, the conditions and item functions the block
//! tables use, and the integer number providers.
//!
//! The evaluator draws from a [`LootDraws`] the caller gives, in the order the game
//! draws from the table's random sequence, and reads nothing else of the world but the
//! blocks a `location_check` names. What it returns is the list `getDrops` returns: the
//! stacks in the game's order, split at the item's stack size, and empty stacks where
//! the game's list has them, since `Block.popResource` draws an empty stack's position
//! before it finds it empty. Placing them (`popResource`) is the caller's.
//!
//! Where a table needs what a block broken by a rule does not have, the game's answer
//! without it is taken: `match_tool` fails without a tool (silk touch and shears never
//! apply), `table_bonus` and `apply_bonus` see an enchantment level of 0,
//! `survives_explosion` holds and `explosion_decay` keeps every item, and
//! `entity_properties` fails without an entity (a chorus flower and a snow layer broken
//! by a rule drop nothing, in the game too). A block broken by an explosion
//! (`BlockBehaviour.onExplosionHit`) is evaluated with what that gives the table
//! ([`Broken`]): the exploding entity as `this_entity`, and the explosion's radius where
//! its drops decay. Where a table needs what Clustine does not
//! keep, the evaluator gives less than the game, and these are named differences:
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
/// nothing ([`Broken::BY_A_RULE`]); by an explosion its direct source, which the table
/// sees as `this_entity`, and, where its drops decay (`DESTROY_WITH_DECAY`), its radius
/// (`EXPLOSION_RADIUS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Broken {
    /// Whether there is a `this_entity`: the entity that exploded.
    pub this_entity: bool,
    /// The explosion's radius, given only where its drops decay.
    pub explosion_radius: Option<f32>,
}

impl Broken {
    /// A block broken by a rule, with no entity and no explosion.
    pub const BY_A_RULE: Self = Self {
        this_entity: false,
        explosion_radius: None,
    };
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
    broken: Broken,
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
    broken: Broken,
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
    broken: Broken,
    draws: &'a mut D,
    block_at: &'a mut W,
    drops: Vec<ItemStack>,
}

impl<D: LootDraws, W: FnMut(BlockPos) -> BlockState> Context<'_, D, W> {
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
            self.drops.push(ItemStack {
                item: stack.item,
                count,
            });
            return;
        }
        let mut left = count;
        while left > 0 {
            let part = max.min(left);
            left -= part;
            self.drops.push(ItemStack {
                item: stack.item,
                count: part,
            });
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
            // `ApplyBonusCount.run` with the empty tool of a rule's break: the formula
            // at level 0, which draws for two of the three.
            LootModifier::ApplyBonus { formula, .. } => {
                let count = stack.count();
                stack.count = match *formula {
                    BonusFormula::OreDrops => count,
                    BonusFormula::UniformBonusCount { .. } => {
                        count.wrapping_add(self.draws.next_int(1))
                    }
                    BonusFormula::BinomialWithBonusCount { extra, probability } => {
                        let mut count = count;
                        for _ in 0..extra {
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

    /// `LootItemCondition.test` of the conditions the block tables use, as they answer
    /// for a block broken by a rule.
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
            // `BonusLevelTableCondition`: the empty tool has level 0.
            LootCondition::TableBonus { chances, .. } => {
                let chance = *chances.first().expect("a table bonus lists its chances");
                self.draws.next_float() < chance
            }
            LootCondition::RandomChance { chance } => self.draws.next_float() < *chance,
            // `EntityPredicate.matches` fails at once without an entity, and an empty
            // predicate holds of any: only `this` is ever given, by an explosion.
            LootCondition::EntityProperties { entity } => {
                *entity == "this" && self.broken.this_entity
            }
            // The empty tool is no item and has no enchantment.
            LootCondition::MatchToolItems { .. } | LootCondition::MatchToolEnchantment { .. } => {
                false
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
