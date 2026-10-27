//! What a mob drops by its loot table as it dies (`Mob.dropFromLootTable`,
//! `LootTable.getRandomItems` with the parameters of an entity's death; the brief's
//! section 1.7): the table of its type (`entities/<type>`), the tables it names, the
//! entity's own conditions and modifiers.
//!
//! The draws are the mob's own stream's, in the game's order: each pool's condition,
//! its rolls, each entry's conditions as it is expanded, a choice by weight only among
//! several, and each stack's modifiers (`LootContext$Builder` is not in the dump, and
//! which random the game's seed of 0 gives is not known: ADR-0028's stream stands in,
//! the brief's 12.1). The stacks come out as the stack splitter passes them on, empty
//! ones among them, which the caller drops nothing for.
//!
//! What the context has, and **Inferred** from the files and the known behaviour of the
//! classes the dump lacks: `this` is the mob; `attacker` and `direct_attacker` what
//! caused and dealt the damage; `attacking_player` and `killed_by_player` hold where a
//! player hurt it within the memory of it (taken as found in the level, whatever region
//! holds the player). No weapon in Clustine is enchanted, so looting's level is 0:
//! `enchanted_count_increase` adds nothing and draws nothing, the enchanted chance is
//! never taken, and no weapon smelts what it kills.

use clustine_blocks::data::entity_in_tag;
use clustine_data::loot::{
    EntityPredicate, IntProvider, LootCondition, LootEntry, LootModifier, LootTable, condition,
    entity_table, modifier, smelted, table,
};
use clustine_data::{ENTITY_TYPES, ITEMS, ItemComponents, TAGS};
use clustine_world::ItemStack;

use super::hurt::{Causer, DamageSource};
use crate::region::Random;

/// What a type's own loot conditions ask of a mob of it (the `type_specific` and
/// component predicates); `None` for what its type does not have, which no predicate
/// holds of.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Facts {
    /// A sheep's colour, as a dye's name.
    pub(crate) sheep_colour: Option<&'static str>,
    /// Whether a sheep is sheared.
    pub(crate) sheared: Option<bool>,
    /// A frog's variant.
    pub(crate) frog_variant: Option<&'static str>,
    /// A mooshroom's colour, `red` or `brown`.
    pub(crate) mooshroom_variant: Option<&'static str>,
    /// A chicken's variant, by its name.
    pub(crate) chicken_variant: Option<&'static str>,
    /// A slime's or magma cube's size.
    pub(crate) cube_size: Option<i32>,
    /// Whether a raider is a patrol's captain.
    pub(crate) captain: Option<bool>,
}

/// The mob whose loot is drawn, as its table's conditions read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Subject {
    /// Its entity type, by id.
    pub(crate) kind: u16,
    pub(crate) on_fire: bool,
    pub(crate) baby: bool,
    /// The entity type of what it rides.
    pub(crate) vehicle: Option<u16>,
    pub(crate) facts: Facts,
}

/// What the mob of `subject` drops by the table of its type, killed by `source`.
pub(crate) fn entity_drops(
    subject: &Subject,
    source: &DamageSource,
    killed_by_player: bool,
    draws: &mut Random,
) -> Vec<ItemStack> {
    let Some(table) = entity_table(i32::from(subject.kind)) else {
        return Vec::new();
    };
    table_drops(table, subject, source, killed_by_player, draws)
}

/// What the table named `name` gives a mob of the type `kind` (a shearing's, a
/// chicken's laying: `dropFromShearingLootTable`, `dropFromGiftLootTable`), which only
/// its own facts decide: drawn from `draws`, as an entity's loot is. Nothing where the
/// table is not compiled.
pub(crate) fn named_drops(
    name: &str,
    kind: u16,
    baby: bool,
    facts: Facts,
    draws: &mut Random,
) -> Vec<ItemStack> {
    let Some(table) = clustine_data::loot::table_by_name(name) else {
        return Vec::new();
    };
    let subject = Subject {
        kind,
        on_fire: false,
        baby,
        vehicle: None,
        facts,
    };
    // No damage source and no player: the tables read only the mob itself.
    let source = DamageSource::of(super::hurt::DamageType("minecraft:generic"));
    table_drops(table, &subject, &source, false, draws)
}

/// What `table` gives for `subject` killed by `source`.
pub(crate) fn table_drops(
    table: &'static LootTable,
    subject: &Subject,
    source: &DamageSource,
    killed_by_player: bool,
    draws: &mut Random,
) -> Vec<ItemStack> {
    let mut context = Context {
        subject,
        source,
        killed_by_player,
        draws,
        visited: Vec::new(),
        drops: Vec::new(),
    };
    let mut stacks = Vec::new();
    context.raw(table, &mut stacks);
    for stack in stacks {
        context.split(stack);
    }
    context.drops
}

/// A stack as the game's `ItemStack` counts it while the functions run: any count, an
/// empty stack where it is not above nought.
#[derive(Debug, Clone, Copy)]
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

struct Context<'a> {
    subject: &'a Subject,
    source: &'a DamageSource,
    killed_by_player: bool,
    draws: &'a mut Random,
    /// The tables being drawn, outermost first (the game's visited set, which stops a
    /// table that names itself).
    visited: Vec<&'static str>,
    drops: Vec<ItemStack>,
}

impl Context<'_> {
    /// `LootTable.getRandomItemsRaw`: each pool in order, through the table's own
    /// modifiers, into `out`.
    fn raw(&mut self, table: &'static LootTable, out: &mut Vec<Stack>) {
        if self.visited.contains(&table.name) {
            return;
        }
        self.visited.push(table.name);
        for pool in table.pools {
            // `addRandomItems`: the condition, then the rolls (no luck, so no bonus).
            if pool.condition.is_some_and(|number| !self.test(number)) {
                continue;
            }
            let rolls = self.int(pool.rolls);
            for _ in 0..rolls {
                let mut made = Vec::new();
                self.add_random_item(pool.entries, &mut made);
                for stack in made {
                    let stack = self.apply_all(pool.modifiers, stack);
                    out.push(self.apply_all(table.modifiers, stack));
                }
            }
        }
        self.visited.pop();
    }

    fn apply_all(&mut self, modifiers: &'static [u16], mut stack: Stack) -> Stack {
        for &number in modifiers {
            stack = self.apply(number, stack);
        }
        stack
    }

    /// `LootPool.addRandomItem`: the entries expanded in order, the choices of no weight
    /// left out, one choice made without a draw, or one of several by weight.
    fn add_random_item(&mut self, entries: &'static [LootEntry], out: &mut Vec<Stack>) {
        let mut found = Vec::new();
        for entry in entries {
            self.expand(entry, &mut found);
        }
        found.retain(|choice| choice.weight > 0);
        let chosen = match found.len() {
            0 => return,
            1 => found[0],
            _ => {
                let total: i32 = found.iter().map(|choice| choice.weight).sum();
                let mut left = self.draws.next_int(total.max(1) as u32) as i32;
                let mut chosen = found[0];
                for choice in &found {
                    left -= choice.weight;
                    if left < 0 {
                        chosen = *choice;
                        break;
                    }
                }
                chosen
            }
        };
        self.create(chosen, out);
    }

    /// `LootPoolEntryContainer.expand`: an entry whose condition fails gives nothing; an
    /// `alternatives` what its first child that succeeds gives; a tag that expands one
    /// choice for each of its items.
    fn expand(&mut self, entry: &'static LootEntry, found: &mut Vec<Choice>) -> bool {
        let (condition, weight) = match entry {
            LootEntry::Item {
                condition, weight, ..
            }
            | LootEntry::Empty { condition, weight }
            | LootEntry::Table {
                condition, weight, ..
            }
            | LootEntry::Tag {
                condition, weight, ..
            } => (*condition, *weight),
            LootEntry::Dynamic { condition, .. } => (*condition, 1),
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
                return false;
            }
        };
        if condition.is_some_and(|number| !self.test(number)) {
            return false;
        }
        match entry {
            LootEntry::Tag {
                tag, expand: true, ..
            } => {
                let items = tag_items(tag);
                let any = !items.is_empty();
                for item in items {
                    found.push(Choice {
                        entry,
                        weight,
                        item: Some(item),
                    });
                }
                any
            }
            _ => {
                found.push(Choice {
                    entry,
                    weight,
                    item: None,
                });
                true
            }
        }
    }

    /// `createItemStack` of the choice: a stack of one through the entry's modifiers, the
    /// nested table's stacks through them, every item of a tag, or nothing.
    fn create(&mut self, choice: Choice, out: &mut Vec<Stack>) {
        match choice.entry {
            LootEntry::Item {
                item, modifiers, ..
            } => {
                let stack = Stack {
                    item: i32::from(*item),
                    count: 1,
                };
                out.push(self.apply_all(modifiers, stack));
            }
            LootEntry::Table {
                table: number,
                modifiers,
                ..
            } => {
                let mut nested = Vec::new();
                self.raw(table(*number), &mut nested);
                for stack in nested {
                    out.push(self.apply_all(modifiers, stack));
                }
            }
            LootEntry::Tag { tag, modifiers, .. } => {
                let items = match choice.item {
                    Some(item) => vec![item],
                    None => tag_items(tag),
                };
                for item in items {
                    out.push(self.apply_all(modifiers, Stack { item, count: 1 }));
                }
            }
            // A block entity's (the decorated pot's), which no entity's table names.
            LootEntry::Empty { .. } | LootEntry::Dynamic { .. } => {}
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

    /// `LootItemConditionalFunction.apply`: the modifier's condition, then the modifier.
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
            LootModifier::SetCount { count, add, .. } => {
                let base = if *add { stack.count() } else { 0 };
                stack.count = base.wrapping_add(self.int(*count));
            }
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
            // An entity's death is no explosion and no block's break.
            LootModifier::ExplosionDecay { .. }
            | LootModifier::ApplyBonus { .. }
            | LootModifier::CopyComponents { .. }
            | LootModifier::CopyState { .. } => {}
            // No weapon has looting: no level, no more, and no draw.
            LootModifier::EnchantedCountIncrease { .. } => {}
            // `SmeltItemFunction`: what the smelting recipe makes of it, its count kept.
            LootModifier::FurnaceSmelt { .. } => {
                if let Some(cooked) = smelted(stack.item) {
                    stack.item = cooked;
                }
            }
            // Components Clustine's stacks do not carry; the amplifier is drawn.
            LootModifier::SetPotion { .. } => {}
            LootModifier::SetOminousBottleAmplifier { amplifier, .. } => {
                self.int(*amplifier);
            }
        }
        stack
    }

    /// The integer providers: a constant, `Mth.nextInt(random, min, max)` (one draw unless
    /// `min >= max`), or `n` draws below `p`.
    fn int(&mut self, provider: IntProvider) -> i32 {
        match provider {
            IntProvider::Constant(value) => value,
            IntProvider::Uniform { min, max } => {
                if min >= max {
                    min
                } else {
                    let bound = max.wrapping_sub(min).wrapping_add(1);
                    (self.draws.next_int(bound as u32) as i32).wrapping_add(min)
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

    /// `LootItemCondition.test` of what an entity's table asks.
    fn test(&mut self, number: u16) -> bool {
        match condition(number) {
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
            LootCondition::RandomChance { chance } => self.draws.next_float() < *chance,
            // Looting's level is 0: the chance without it.
            LootCondition::RandomChanceWithEnchantedBonus { unenchanted, .. } => {
                self.draws.next_float() < *unenchanted
            }
            LootCondition::TableBonus { chances, .. } => {
                let chance = *chances.first().expect("a table bonus lists its chances");
                self.draws.next_float() < chance
            }
            LootCondition::KilledByPlayer => self.killed_by_player,
            LootCondition::EntityProperties { entity } => self.present(entity),
            LootCondition::EntityMatches { entity, predicate } => match *entity {
                "this" => subject_matches(self.subject, predicate),
                "attacker" => self
                    .source
                    .cause
                    .as_ref()
                    .is_some_and(|cause| causer_matches(cause, predicate)),
                "direct_attacker" => self
                    .source
                    .direct
                    .as_ref()
                    .is_some_and(|direct| causer_matches(direct, predicate)),
                // The player is not read further than that they are.
                _ => false,
            },
            LootCondition::DamageSource(predicate) => {
                predicate.tags.iter().all(|&(tag, expected)| {
                    self.source.kind.is(tag.trim_start_matches('#')) == expected
                }) && predicate.source_entity.as_ref().is_none_or(|wanted| {
                    self.source
                        .cause
                        .as_ref()
                        .is_some_and(|cause| causer_matches(cause, wanted))
                }) && predicate.direct_entity.as_ref().is_none_or(|wanted| {
                    self.source
                        .direct
                        .as_ref()
                        .is_some_and(|direct| causer_matches(direct, wanted))
                })
            }
            // An entity's death is no explosion, has no block and no tool.
            LootCondition::SurvivesExplosion => true,
            LootCondition::MatchBlock(_)
            | LootCondition::LocationCheck { .. }
            | LootCondition::MatchToolItems { .. }
            | LootCondition::MatchToolEnchantment { .. } => false,
        }
    }

    /// Whether the context has the entity `entity` names.
    fn present(&self, entity: &str) -> bool {
        match entity {
            "this" => true,
            "attacker" => self.source.cause.is_some(),
            "direct_attacker" => self.source.direct.is_some(),
            "attacking_player" => self.killed_by_player,
            _ => false,
        }
    }
}

/// One choice of a pool: its entry, its weight, and for a tag that expands the item.
#[derive(Debug, Clone, Copy)]
struct Choice {
    entry: &'static LootEntry,
    weight: i32,
    item: Option<i32>,
}

/// The items of the item tag `tag` (with `#`), in the tag's order.
fn tag_items(tag: &str) -> Vec<i32> {
    let name = tag.trim_start_matches('#');
    TAGS.iter()
        .find(|tags| tags.registry == "minecraft:item")
        .and_then(|tags| tags.tags.iter().find(|known| known.name == name))
        .map_or_else(Vec::new, |known| {
            known
                .entries
                .iter()
                .copied()
                .filter(|&item| usize::try_from(item).is_ok_and(|item| item < ITEMS.len()))
                .collect()
        })
}

/// Whether the entity type `kind` is the one `wanted` names, or in the tag it names
/// with `#`.
fn kind_is(kind: u16, wanted: &str) -> bool {
    match wanted.strip_prefix('#') {
        Some(tag) => entity_in_tag(i32::from(kind), tag),
        None => ENTITY_TYPES.get(usize::from(kind)) == Some(&wanted),
    }
}

/// `EntityPredicate.matches` of the mob itself.
fn subject_matches(subject: &Subject, predicate: &EntityPredicate) -> bool {
    let facts = subject.facts;
    predicate.kind.is_none_or(|wanted| kind_is(subject.kind, wanted))
        && predicate.on_fire.is_none_or(|wanted| subject.on_fire == wanted)
        && predicate.baby.is_none_or(|wanted| subject.baby == wanted)
        && predicate.vehicle.is_none_or(|wanted| {
            subject.vehicle.is_some_and(|vehicle| kind_is(vehicle, wanted))
        })
        && predicate
            .sheep_colour
            .is_none_or(|wanted| facts.sheep_colour == Some(wanted))
        && predicate.sheared.is_none_or(|wanted| facts.sheared == Some(wanted))
        && predicate
            .frog_variant
            .is_none_or(|wanted| facts.frog_variant == Some(wanted))
        && predicate
            .mooshroom_variant
            .is_none_or(|wanted| facts.mooshroom_variant == Some(wanted))
        && predicate
            .chicken_variant
            .is_none_or(|wanted| facts.chicken_variant == Some(wanted))
        && predicate.cube_size.is_none_or(|(min, max)| {
            facts.cube_size.is_some_and(|size| {
                min.is_none_or(|min| size >= min) && max.is_none_or(|max| size <= max)
            })
        })
        && predicate.captain.is_none_or(|wanted| facts.captain == Some(wanted))
        // Nothing a mob holds in Clustine is enchanted.
        && predicate.main_hand_enchantment.is_none()
}

/// `EntityPredicate.matches` of what caused or dealt the damage, as far as a source
/// knows it: its type; it neither burns nor is a baby as far as the source says, rides
/// nothing, has no type's facts, and holds nothing enchanted.
fn causer_matches(causer: &Causer, predicate: &EntityPredicate) -> bool {
    predicate
        .kind
        .is_none_or(|wanted| kind_is(causer.kind, wanted))
        && predicate.on_fire.is_none_or(|wanted| !wanted)
        && predicate.baby.is_none_or(|wanted| !wanted)
        && predicate.vehicle.is_none()
        && predicate.sheep_colour.is_none()
        && predicate.sheared.is_none()
        && predicate.frog_variant.is_none()
        && predicate.mooshroom_variant.is_none()
        && predicate.chicken_variant.is_none()
        && predicate.cube_size.is_none()
        && predicate.captain.is_none()
        && predicate.main_hand_enchantment.is_none()
}
