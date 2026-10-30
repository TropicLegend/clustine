//! The piglin (`Piglin`, `AbstractPiglin`; `docs/groundwork/p-brief.md`, section 5.7,
//! and `docs/groundwork/p-plan.md`, section 18): family E5 of P12. A monster of the
//! Nether whose mind is a brain (`piglin_ai.rs`, `PiglinAi`) and whose body is here: its
//! state and synced data (`IsImmuneToZombification` at 16, the baby at 17, the charging
//! crossbow at 18, the dance at 19), its inventory of eight, its equipment's rules
//! (`equipItemIfPossible` and the piglin's `canReplaceCurrentItem`), its spawn (a baby
//! one time in five; a crossbow, a golden spear or a golden sword; golden armour each one
//! time in ten), a player's gift of gold, the pick-up of what it wants, its sounds and its
//! zombification (`AbstractPiglin.customServerAiStep`: more than 300 ticks where
//! `piglins_zombify` holds, and it becomes a zombified piglin with its equipment and
//! nausea for 200 ticks).
//!
//! What it is told (`MobType::told`, `region/telling.rs`: a player's anger by a block
//! piglins guard or a container opened, another piglin's target) and what hurt it
//! (`hurtServer`'s `wasHurtBy`) are kept as they come and taken at the start of its next
//! AI step, before its brain's tick, where its world can be read: see `piglin_ai.rs`.
//!
//! The piglin brute (`piglin_brute.rs`) shares what `AbstractPiglin` has, which is here.
//!
//! Named differences: the arm's swing of its off hand as it throws (`swing(OFF_HAND)`)
//! is not shown, the region showing a mob's main hand only; an empty stack left over by
//! a full inventory, which the game throws as an empty item, is not thrown (its random
//! place is still drawn); a golden spear is held as a sword is (the spear's use by a mob
//! is decided out, `docs/groundwork/n9-plan.md`, section 8.6); its inventory falls as it
//! dies before its equipment rather than after; and a baby does not ride a baby hoglin
//! (`Mount`: the region has no mob's mount of another), but follows it.

use clustine_blocks::data::{item_in_tag, same_block};
use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::piglin_ai::{self, PROVIDER};
use super::{Dimensions, Interaction, MobType, SpawnReason, Spawning, Used};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::attributes::ADD_MULTIPLIED_BASE;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{self, Modifier, attribute};
use crate::mobs::brain::{self, MemoryKind, Provider, Value};
use crate::mobs::making::populate_default_equipment_enchantments;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::region::{DEFAULT_PICK_UP_DELAY, Random};
use crate::shown::DataShown;
use crate::spawning::rules::SpawnWorld;
use clustine_data::PathType;
use clustine_world::generation::Draws;

/// Whether the type is whole, so that `built()` gives it.
pub(crate) const BUILT: bool = true;

/// The piglin's and the brute's entity types.
pub(crate) const PIGLIN: u16 = entity_types::PIGLIN as u16;
pub(crate) const PIGLIN_BRUTE: u16 = entity_types::PIGLIN_BRUTE as u16;

/// The version of a piglin's type state.
const VERSION: u8 = 1;

/// The synced data's indices: `AbstractPiglin.DATA_IMMUNE_TO_ZOMBIFICATION` after
/// `Mob`'s 15, then the piglin's `DATA_BABY_ID`, `DATA_IS_CHARGING_CROSSBOW` and
/// `DATA_IS_DANCING`.
pub(crate) const IMMUNE_INDEX: u8 = 16;
pub(crate) const BABY_INDEX: u8 = 17;
pub(crate) const CHARGING_INDEX: u8 = 18;
pub(crate) const DANCING_INDEX: u8 = 19;

/// `AbstractPiglin.CONVERSION_TIME`.
pub(crate) const CONVERSION_TIME: i32 = 300;

/// `INVENTORY_SIZE`.
pub(crate) const INVENTORY_SIZE: usize = 8;

/// `SPEED_MODIFIER_BABY`: a baby a fifth faster (`ADD_MULTIPLIED_BASE` of 0.2, its double
/// widened from the constant's).
const BABY_SPEED: &str = "minecraft:baby";
const BABY_SPEED_AMOUNT: f64 = 0.200_000_002_980_232_24;

/// `PROBABILITY_OF_SPAWNING_AS_BABY`, `CHANCE_OF_WEARING_EACH_ARMOUR_ITEM`.
const BABY_CHANCE: f32 = 0.2;
const ARMOUR_CHANCE: f32 = 0.1;

/// `Piglin`'s `xpReward`.
const EXPERIENCE: i32 = 5;

/// What a piglin was told or hurt by since its last AI step, kept to be taken at its
/// next (see the module's head): the entity, its type, and the player's UUID where it is
/// a player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pending {
    /// `hurtServer`'s `wasHurtBy(level, piglin, attacker)`.
    Hurt(Other),
    /// `angerNearbyPiglins`'s anger at a player, the piglin idle when told.
    Anger(Other),
    /// `broadcastAngerTarget`'s target, another piglin's.
    Target(Other),
}

/// Another entity a [`Pending`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Other {
    pub(crate) key: EntityId,
    pub(crate) kind: u16,
    pub(crate) player: Option<u128>,
}

/// What a piglin or a brute keeps beyond the body: `AbstractPiglin`'s immunity and time
/// in the overworld, the piglin's baby flag, `cannotHunt`, the crossbow's charge and the
/// dance (synced), its inventory, and what it was told since its last AI step.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PiglinState {
    pub(crate) immune: bool,
    pub(crate) time_in_overworld: i32,
    pub(crate) baby: bool,
    pub(crate) cannot_hunt: bool,
    pub(crate) charging: bool,
    pub(crate) dancing: bool,
    pub(crate) inventory: Vec<ItemStack>,
    pub(crate) pending: Vec<Pending>,
}

/// A piglin's state from its type state; one of nothing or that this build cannot read
/// as a fresh piglin's.
pub(crate) fn read(mob: &Mob) -> PiglinState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return PiglinState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

fn put(mob: &mut Mob, own: &PiglinState) {
    mob.state = if *own == PiglinState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// Changes a piglin's state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut PiglinState)) {
    let mut own = read(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put(mob, &own);
    }
}

// ------------------------------------------------------------------------------------
// What the items mean to a piglin (`PiglinAi`'s tests of a stack).

/// `isLovedItem(stack)`: `#piglin_loved`.
pub(crate) fn is_loved(stack: &ItemStack) -> bool {
    item_in_tag(stack.item, "minecraft:piglin_loved")
}

/// `isBarterCurrency(stack)`: a gold ingot (`BARTERING_ITEM`).
pub(crate) fn is_barter_currency(stack: &ItemStack) -> bool {
    stack.item == items::GOLD_INGOT
}

/// `isFood(stack)`: `#piglin_food`.
pub(crate) fn is_food(stack: &ItemStack) -> bool {
    item_in_tag(stack.item, "minecraft:piglin_food")
}

/// `isWearingSafeArmor(entity)` of what it wears, the feet to the head: any of
/// `#piglin_safe_armor`.
pub(crate) fn wears_safe_armour(worn: &[Option<ItemStack>]) -> bool {
    worn.iter()
        .flatten()
        .any(|stack| item_in_tag(stack.item, "minecraft:piglin_safe_armor"))
}

/// The stack in the slot `slot`, none for an empty one.
pub(crate) fn in_slot(mob: &Mob, slot: u8) -> Option<&ItemStack> {
    mob.body.equipment.slots[usize::from(slot)]
        .as_ref()
        .filter(|stack| stack.count > 0)
}

/// `getOffhandItem()`, none for an empty hand.
pub(crate) fn off_hand(mob: &Mob) -> Option<&ItemStack> {
    in_slot(mob, slots::OFF_HAND)
}

/// `isNotHoldingLovedItemInOffHand(piglin)`.
pub(crate) fn not_holding_loved_in_off_hand(mob: &Mob) -> bool {
    off_hand(mob).is_none_or(|stack| !is_loved(stack))
}

/// `isBaby()`: the piglin's flag; a brute is never one.
pub(crate) fn is_baby(mob: &Mob) -> bool {
    read(mob).baby
}

// ------------------------------------------------------------------------------------
// The equipment's rules.

/// `setItemSlotAndDropWhenKilled(slot, stack)`: worn, and dropped for certain at death
/// (`setGuaranteedDrop`, a chance of 2).
pub(crate) fn set_and_drop_when_killed(mob: &mut Mob, slot: u8, stack: Option<ItemStack>) {
    let at = usize::from(slot);
    mob.body.equipment.slots[at] = stack.filter(|stack| stack.count > 0);
    let chances = mob.body.equipment.drop_chances.get_or_insert([0.085; 8]);
    chances[at] = 2.0;
}

/// `Piglin.holdInMainHand(stack)`: kept for certain, and the piglin kept from despawning.
pub(crate) fn hold_in_main_hand(mob: &mut Mob, stack: ItemStack) {
    set_and_drop_when_killed(mob, slots::MAIN_HAND, Some(stack));
    mob.body.flags.persistent = true;
}

/// `Piglin.holdInOffHand(stack)`: kept for certain; anything but a gold ingot keeps the
/// piglin from despawning.
pub(crate) fn hold_in_off_hand(mob: &mut Mob, stack: ItemStack) {
    let currency = is_barter_currency(&stack);
    set_and_drop_when_killed(mob, slots::OFF_HAND, Some(stack));
    if !currency {
        mob.body.flags.persistent = true;
    }
}

/// `getEquipmentSlotForItem(stack)`: its `equippable`'s slot where a piglin may use it
/// (none of an animal's body or a saddle), else the main hand.
fn slot_for_item(stack: &ItemStack) -> u8 {
    clustine_data::ItemComponents::of(stack.item)
        .equippable
        .map(|equippable| equippable.slot)
        .filter(|slot| *slot <= slots::HEAD)
        .unwrap_or(slots::MAIN_HAND)
}

/// `isEquippableInSlot(stack, slot)`: an item without `equippable` in the main hand, or
/// one in its own slot.
fn equippable_in_slot(stack: &ItemStack, slot: u8) -> bool {
    match clustine_data::ItemComponents::of(stack.item).equippable {
        None => slot == slots::MAIN_HAND,
        Some(equippable) => equippable.slot == slot && slot <= slots::HEAD,
    }
}

/// `getPreferredWeaponType()`: `#piglin_preferred_weapons` for an adult piglin, none for
/// a baby and for a brute (`Mob`'s).
fn preferred_weapon(kind: u16, mob: &Mob) -> Option<&'static str> {
    (kind == PIGLIN && !is_baby(mob)).then_some("minecraft:piglin_preferred_weapons")
}

/// `getApproximateAttributeWith(stack, attribute, slot)`: the mob's base of the attribute
/// (none where its type has none) with the stack's own modifiers for the slot, in their
/// order (`ItemAttributeModifiers.compute`).
fn approximate_attribute(kind: u16, mob: &Mob, stack: &ItemStack, id: i32, slot: u8) -> f64 {
    let attribute = attribute(id);
    let base = match mob.body.attributes.made(attribute) {
        Some(instance) => instance.base,
        None => body::attributes::supplied(kind, attribute).unwrap_or(0.0),
    };
    let mut value = base;
    for modifier in clustine_data::ItemComponents::of(stack.item).modifiers {
        let applies = match modifier.slot {
            0 => true,
            1 => slot == slots::MAIN_HAND,
            2 => slot == slots::OFF_HAND,
            3 => slot <= slots::OFF_HAND,
            group @ 4..=7 => slot == group - 2,
            8 => (slots::FEET..=slots::BODY).contains(&slot),
            9 => slot == slots::BODY,
            10 => slot == slots::SADDLE,
            _ => false,
        };
        if !applies || modifier.attribute != attribute {
            continue;
        }
        value += match modifier.operation {
            0 => modifier.amount,
            1 => modifier.amount * base,
            _ => modifier.amount * value,
        };
    }
    value
}

/// The enchantments of a stack (`ENCHANTMENTS`), by their count.
fn enchantment_count(stack: &ItemStack) -> usize {
    use clustine_world::components::{Component, Kind};
    match stack.components.get(Kind::Enchantments) {
        Some(Component::Enchantments(list)) => list.len(),
        _ => 0,
    }
}

/// The damage a stack has taken (`getDamageValue`).
fn damage_value(stack: &ItemStack) -> i32 {
    use clustine_world::components::{Component, Kind};
    match stack.components.get(Kind::Damage) {
        Some(Component::Damage(damage)) => *damage,
        _ => 0,
    }
}

/// `Mob.canReplaceEqualItem(new, current)`: more enchantments, else less damage, else a
/// name where the current has none (no stack of Clustine's carries a custom name).
fn can_replace_equal_item(new: &ItemStack, current: &ItemStack) -> bool {
    let (a, b) = (enchantment_count(new), enchantment_count(current));
    if a != b {
        return a > b;
    }
    let (a, b) = (damage_value(new), damage_value(current));
    if a != b {
        return a < b;
    }
    false
}

/// `Mob.canReplaceCurrentItem(new, current, slot)` (n7 Mob.txt 1338–1530): anything for
/// an empty slot; armour by its armour, then its toughness, then as equal; the main
/// hand's weapon by the preferred type, then its attack damage, then as equal; never in
/// another slot.
fn mob_can_replace(
    kind: u16,
    mob: &Mob,
    new: &ItemStack,
    current: Option<&ItemStack>,
    slot: u8,
) -> bool {
    use clustine_data::attributes as ids;
    let Some(current) = current else {
        return true;
    };
    if (slots::FEET..=slots::HEAD).contains(&slot) {
        if prevents_armour_change(current) {
            return false;
        }
        for id in [ids::ARMOR, ids::ARMOR_TOUGHNESS] {
            let a = approximate_attribute(kind, mob, new, id, slot);
            let b = approximate_attribute(kind, mob, current, id, slot);
            if a != b {
                return a > b;
            }
        }
        return can_replace_equal_item(new, current);
    }
    if slot != slots::MAIN_HAND {
        return false;
    }
    if let Some(tag) = preferred_weapon(kind, mob) {
        let (new_is, current_is) = (item_in_tag(new.item, tag), item_in_tag(current.item, tag));
        if current_is && !new_is {
            return false;
        }
        if !current_is && new_is {
            return true;
        }
    }
    let a = approximate_attribute(kind, mob, new, ids::ATTACK_DAMAGE, slot);
    let b = approximate_attribute(kind, mob, current, ids::ATTACK_DAMAGE, slot);
    if a != b {
        return a > b;
    }
    can_replace_equal_item(new, current)
}

/// `EnchantmentHelper.has(stack, PREVENT_ARMOR_CHANGE)`: Curse of Binding.
fn prevents_armour_change(stack: &ItemStack) -> bool {
    use clustine_world::components::{Component, Kind};
    match stack.components.get(Kind::Enchantments) {
        Some(Component::Enchantments(list)) => list
            .iter()
            .any(|(name, _)| name == "minecraft:binding_curse"),
        _ => false,
    }
}

/// `Piglin.canReplaceCurrentItem(new, current, slot)` (Piglin.txt 474–502): never past
/// Curse of Binding; a loved or preferred item over one that is neither, never the other
/// way; else `Mob`'s. A brute's is `Mob`'s.
pub(crate) fn can_replace(
    kind: u16,
    mob: &Mob,
    new: &ItemStack,
    current: Option<&ItemStack>,
    slot: u8,
) -> bool {
    if kind != PIGLIN {
        return mob_can_replace(kind, mob, new, current, slot);
    }
    if current.is_some_and(prevents_armour_change) {
        return false;
    }
    let preferred = preferred_weapon(kind, mob);
    let liked = |stack: &ItemStack| {
        is_loved(stack) || preferred.is_some_and(|tag| item_in_tag(stack.item, tag))
    };
    let new_liked = liked(new);
    let current_liked = current.is_some_and(liked);
    if new_liked && !current_liked {
        return true;
    }
    if !new_liked && current_liked {
        return false;
    }
    mob_can_replace(kind, mob, new, current, slot)
}

/// `canReplaceCurrentItem(stack)`: of the slot it would go in.
pub(crate) fn can_replace_current(kind: u16, mob: &Mob, stack: &ItemStack) -> bool {
    let slot = slot_for_item(stack);
    can_replace(kind, mob, stack, in_slot(mob, slot), slot)
}

/// What `equipItemIfPossible` did: the stack worn (empty where nothing was), and the
/// stack it took the place of, where a draw let it fall.
pub(crate) struct Equipped {
    pub(crate) worn: Option<ItemStack>,
    pub(crate) dropped: Option<ItemStack>,
}

/// `Mob.equipItemIfPossible(level, stack)` (n7 Mob.txt 1127–1210): in its slot where it
/// may replace what is there (armour that may not goes to an empty main hand instead),
/// what was there falling where `max(nextFloat() − 0.1, 0)` of its own random is below
/// its drop chance; worn for certain, and the mob kept from despawning. Nothing where
/// it may not.
pub(crate) fn equip_if_possible(
    kind: u16,
    mob: &mut Mob,
    stack: &ItemStack,
    own: &mut Random,
) -> Equipped {
    let nothing = Equipped {
        worn: None,
        dropped: None,
    };
    let mut slot = slot_for_item(stack);
    if !equippable_in_slot(stack, slot) {
        return nothing;
    }
    let mut current = in_slot(mob, slot).cloned();
    let mut replace = can_replace(kind, mob, stack, current.as_ref(), slot);
    if (slots::FEET..=slots::HEAD).contains(&slot) && !replace {
        slot = slots::MAIN_HAND;
        current = in_slot(mob, slot).cloned();
        replace = current.is_none();
    }
    // `canHoldItem`: anything.
    if !replace {
        return nothing;
    }
    let chance = f64::from(mob.body.equipment.drop_chance(usize::from(slot)));
    let mut dropped = None;
    if let Some(current) = current
        && f64::from((own.next_float() - 0.1).max(0.0)) < chance
    {
        dropped = Some(current);
    }
    // `EquipmentSlot.limit(stack)`: one for armour, the whole stack in a hand.
    let worn = if (slots::FEET..=slots::HEAD).contains(&slot) {
        stack.with_count(1)
    } else {
        stack.clone()
    };
    set_and_drop_when_killed(mob, slot, Some(worn.clone()));
    mob.body.flags.persistent = true;
    Equipped {
        worn: Some(worn),
        dropped,
    }
}

/// `SimpleContainer.canAddItem(stack)` of the inventory of eight.
pub(crate) fn can_add_to_inventory(own: &PiglinState, stack: &ItemStack) -> bool {
    let max = crate::physics::item::max_stack_size(stack);
    own.inventory.len() < INVENTORY_SIZE
        || own
            .inventory
            .iter()
            .any(|held| held.same_kind(stack) && held.count < max)
}

/// `SimpleContainer.addItem(stack)`: onto the stacks of its kind first, in their order,
/// then into an empty slot; what does not fit is given back.
pub(crate) fn add_to_inventory(own: &mut PiglinState, stack: ItemStack) -> Option<ItemStack> {
    let max = crate::physics::item::max_stack_size(&stack);
    let mut left = stack;
    for held in &mut own.inventory {
        if left.count <= 0 {
            break;
        }
        if held.same_kind(&left) && held.count < max {
            let moved = (max - held.count).min(left.count);
            held.count += moved;
            left.count -= moved;
        }
    }
    if left.count > 0 && own.inventory.len() < INVENTORY_SIZE {
        own.inventory.push(left.clone());
        left.count = 0;
    }
    (left.count > 0).then_some(left)
}

// ------------------------------------------------------------------------------------
// What `AbstractPiglin` shares with the brute.

/// `AbstractPiglin`'s constructor: it picks up loot, opens doors, and fire beside a path
/// costs 16 and fire is never walked.
pub(crate) fn register_abstract(setup: &mut Setup) {
    setup.set_malus(PathType::FireInNeighbor, 16.0);
    setup.set_malus(PathType::Fire, -1.0);
    setup.config.navigation.flags.can_open_doors = true;
}

/// `AbstractPiglin.isConverting()`: not immune, with its AI, where `piglins_zombify`
/// holds (the level's, which holds in the overworld and the End).
pub(crate) fn is_converting(tick: &MobTick<'_>) -> bool {
    !read(&tick.mob).immune
        && !tick.mob.body.flags.no_ai
        && tick.world.dimension().rules().piglins_zombify
}

/// `AbstractPiglin.customServerAiStep`'s own part, after the type's brain: the time in
/// the overworld counted while converting, else back to nothing; above 300 the converted
/// sound (at easy, never peaceful) and the conversion, the type's `finish` first (the
/// piglin's dropping what it admires and its inventory).
pub(crate) fn zombify_step(
    tick: &mut MobTick<'_>,
    converted_sound: i32,
    finish: fn(&mut MobTick<'_>),
) {
    let converting = is_converting(tick);
    update(&mut tick.mob, |own| {
        own.time_in_overworld = if converting {
            own.time_in_overworld.saturating_add(1)
        } else {
            0
        };
    });
    if read(&tick.mob).time_in_overworld <= CONVERSION_TIME {
        return;
    }
    let volume = tick.mob_type.sound_volume_of(&tick.mob);
    body::make_sound(tick, Some(converted_sound), volume);
    finish(tick);
    // `convertTo(ZOMBIFIED_PIGLIN, single(this, true, true), z -> z.addEffect(nausea))`.
    tick.out.local.push(Local::Convert {
        into: entity_types::ZOMBIFIED_PIGLIN as u16,
        keep_equipment: true,
        sound: None,
        after: Some(Box::new(nausea())),
    });
}

/// `MobEffectInstance(NAUSEA, 200, 0)`.
pub(crate) fn nausea() -> clustine_world::components::EffectInstance {
    clustine_world::components::EffectInstance {
        effect: "minecraft:nausea".to_owned(),
        amplifier: 0,
        duration: 200,
        ambient: false,
        visible: true,
        show_icon: true,
    }
}

/// `spawnAtLocation(level, stack)` of the mob in its tick: an item at its feet with the
/// default delay.
pub(crate) fn spawn_at_location(tick: &mut MobTick<'_>, stack: ItemStack) {
    if stack.count <= 0 {
        return;
    }
    let at = tick.living.body.position;
    tick.out.local.push(Local::Drop {
        stack,
        at,
        pick_up_delay: DEFAULT_PICK_UP_DELAY,
    });
}

/// `AbstractPiglin`'s saved and synced data as a template's tag holds it:
/// `IsImmuneToZombification` and `TimeInOverworld`, and its main hand
/// (`equipment.mainhand`, which a bastion's piglins carry).
pub(crate) fn from_template_abstract(mob: &mut Mob, tag: &[u8]) {
    use super::template;
    let immune = template::boolean(tag, "IsImmuneToZombification").unwrap_or(false);
    let time = template::int(tag, "TimeInOverworld").unwrap_or(0);
    let picks_up = template::boolean(tag, "CanPickUpLoot").unwrap_or(true);
    mob.body.flags.picks_up_loot = picks_up;
    update(mob, |own| {
        own.immune = immune;
        own.time_in_overworld = time;
    });
    if let Some(equipment) = template::compound(tag, "equipment") {
        for (name, slot) in [
            ("mainhand", slots::MAIN_HAND),
            ("offhand", slots::OFF_HAND),
            ("feet", slots::FEET),
            ("legs", slots::LEGS),
            ("chest", slots::CHEST),
            ("head", slots::HEAD),
        ] {
            let Some(stack) = template::compound(&equipment, name).and_then(|stack| {
                let id = template::text(&stack, "id")?;
                let item = clustine_data::ITEMS
                    .iter()
                    .position(|item| item.name == id)?;
                let count = template::int(&stack, "count").unwrap_or(1);
                Some(ItemStack::new(i32::try_from(item).ok()?, count))
            }) else {
                continue;
            };
            mob.body.equipment.slots[usize::from(slot)] = Some(stack);
        }
    }
}

/// `AbstractPiglin`'s shown data: its immunity.
pub(crate) fn shown_abstract(own: &PiglinState) -> (u8, DataShown, bool) {
    (IMMUNE_INDEX, DataShown::Boolean(own.immune), !own.immune)
}

/// `Mob.getBaseExperienceReward` of an `xpReward` of `base`: one to three more for each
/// slot but the saddle that holds something likely to fall (`Mob`'s, which the brute
/// keeps; the piglin's own gives its `xpReward` alone).
pub(crate) fn experience_with_equipment(base: i32, mob: &Mob, draws: &mut Random) -> i32 {
    let mut reward = base;
    let equipment = &mob.body.equipment;
    for slot in 0..equipment.slots.len() {
        if slot == usize::from(slots::SADDLE) || equipment.slots[slot].is_none() {
            continue;
        }
        if equipment.drop_chance(slot) <= 1.0 {
            reward += 1 + draws.next_int(3) as i32;
        }
    }
    reward
}

/// A note of the other a hurt or a telling names.
pub(crate) fn other(key: EntityId, kind: u16, player: Option<u128>) -> Other {
    Other { key, kind, player }
}

/// `hurtServer`'s `wasHurtBy` of a piglin or a brute, kept for its next AI step: a hurt
/// that took, by a living thing.
pub(crate) fn note_hurt(struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
    let Some(cause) = source.cause.as_ref().filter(|cause| cause.living) else {
        return;
    };
    if !took {
        return;
    }
    let noted = Pending::Hurt(other(cause.key, cause.kind, cause.player));
    update(struck.mob, |own| own.pending.push(noted));
}

// ------------------------------------------------------------------------------------
// The piglin.

/// `Piglin.setBaby(baby)`: the flag, and the baby's speed taken off and, for a baby,
/// put on.
pub(crate) fn set_baby(mob: &mut Mob, baby: bool) {
    update(mob, |own| own.baby = baby);
    if let Some(speed) = mob
        .body
        .attributes
        .instance(PIGLIN, attribute(clustine_data::attributes::MOVEMENT_SPEED))
    {
        speed.remove_modifier(BABY_SPEED);
        if baby {
            speed.add_modifier(Modifier {
                id: BABY_SPEED.to_owned(),
                amount: BABY_SPEED_AMOUNT,
                operation: ADD_MULTIPLIED_BASE,
            });
        }
    }
}

/// `Piglin.createSpawnWeapon()`, of its own random: a crossbow one time in two, else a
/// golden spear one time in ten, else a golden sword.
fn spawn_weapon(own: &mut Random) -> ItemStack {
    if f64::from(own.next_float()) < 0.5 {
        return ItemStack::new(items::CROSSBOW, 1);
    }
    let item = if own.next_int(10) == 0 {
        items::GOLDEN_SPEAR
    } else {
        items::GOLDEN_SWORD
    };
    ItemStack::new(item, 1)
}

/// `PiglinAi.initMemories(piglin, random)`: it has hunted recently, for 30 to 120
/// seconds (`TIME_BETWEEN_HUNTS`, of the level's draws).
fn init_memories(mob: &mut Mob, level: &mut dyn Draws) {
    let ttl = i64::from(level.next_int_between(piglin_ai::HUNTS.0, piglin_ai::HUNTS.1));
    let mut memories = brain::memories_of(&PROVIDER, mob, 0);
    memories.set_with_expiry(MemoryKind::HuntedRecently, Value::Bool(true), ttl);
    brain::write_memories(&PROVIDER, mob, memories);
}

/// `Piglin.finishConversion`: what it admires falls (`cancelAdmiring`), then each stack
/// of its inventory.
fn finish_conversion(tick: &mut MobTick<'_>) {
    piglin_ai::cancel_admiring(tick);
    let inventory = std::mem::take(&mut read(&tick.mob).inventory);
    update(&mut tick.mob, |own| own.inventory.clear());
    for stack in inventory {
        spawn_at_location(tick, stack);
    }
}

/// `Piglin.checkPiglinSpawnRules` (Piglin.txt 150–160): not above a nether wart block.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    let _ = draws;
    world
        .block(pos.offset(0, -1, 0))
        .is_none_or(|below| !same_block(below, clustine_data::blocks::NETHER_WART_BLOCK))
}

/// The piglin.
pub(crate) struct Piglin;

impl MobType for Piglin {
    /// `AbstractPiglin`'s constructor's settings; its mind is its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        register_abstract(setup);
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `setCanPickUpLoot(true)`.
    fn construct(&self, mob: &mut Mob, _: &mut Random) {
        mob.body.flags.picks_up_loot = true;
    }

    /// `Piglin.customServerAiStep`: what it was told or hurt by, then its brain's tick
    /// and its activity's update, then `AbstractPiglin`'s conversion.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        piglin_ai::take_pending(tick);
        brain::run(tick, piglin_ai::update_activity);
        zombify_step(
            tick,
            sounds::ENTITY_PIGLIN_CONVERTED_TO_ZOMBIFIED,
            finish_conversion,
        );
    }

    /// `Piglin.finalizeSpawn` (Piglin.txt 161–183): not for a structure's, a baby one
    /// time in five of the level's draws, else an adult's weapon of its own; its
    /// hunt's memory; golden armour (each one time in ten); the equipment's
    /// enchantments. `Mob`'s follows.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if spawn.reason != SpawnReason::Structure {
            if spawn.level.next_float() < BABY_CHANCE {
                set_baby(spawn.mob, true);
            } else if !is_baby(spawn.mob) {
                let weapon = spawn_weapon(spawn.own);
                spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = Some(weapon);
            }
        }
        init_memories(spawn.mob, spawn.level);
        if !is_baby(spawn.mob) {
            for (slot, item) in [
                (slots::HEAD, items::GOLDEN_HELMET),
                (slots::CHEST, items::GOLDEN_CHESTPLATE),
                (slots::LEGS, items::GOLDEN_LEGGINGS),
                (slots::FEET, items::GOLDEN_BOOTS),
            ] {
                if spawn.level.next_float() < ARMOUR_CHANCE {
                    spawn.mob.body.equipment.slots[usize::from(slot)] =
                        Some(ItemStack::new(item, 1));
                }
            }
        }
        let special = spawn.difficulty.1;
        populate_default_equipment_enchantments(spawn.mob, spawn.level, special);
    }

    /// `Piglin.readAdditionalSaveData` of a template's tag: `IsBaby`, `CannotHunt`,
    /// its inventory (a bastion's is empty), and `AbstractPiglin`'s.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        from_template_abstract(mob, tag);
        let baby = super::template::boolean(tag, "IsBaby").unwrap_or(false);
        set_baby(mob, baby);
        let cannot_hunt = super::template::boolean(tag, "CannotHunt").unwrap_or(false);
        update(mob, |own| own.cannot_hunt = cannot_hunt);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        super::monster_ai::walk_target_value(ai, pos)
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        piglin_ai::mob_interact(used)
    }

    /// `Mob.aiStep`'s pick-up, after the living one.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        piglin_ai::pick_up(tick);
    }

    /// `Piglin.hurtServer`: `wasHurtBy`, kept for its next AI step.
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        note_hurt(struck, source, took);
    }

    fn told(&self, mob: &mut Mob, told: &clustine_world::Told, seen: bool, tick: u64) -> bool {
        let _ = tick;
        piglin_ai::told(mob, told, seen)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        is_baby(mob)
    }

    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        set_baby(mob, baby);
    }

    /// `BABY_DIMENSIONS`: `scalable(0.49, 0.98)`, its eyes at 0.78.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        is_baby(mob).then_some(Dimensions {
            width: 0.49,
            height: 0.98,
            eye_height: 0.78,
        })
    }

    /// `Piglin.dropCustomDeathLoot`'s inventory.
    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        let inventory = read(mob).inventory;
        update(mob, |own| own.inventory.clear());
        inventory
    }

    fn experience_reward(&self, _: u16, _: &Mob, _: &mut Random) -> i32 {
        EXPERIENCE
    }

    /// `AbstractPiglin.playAmbientSound()`: only while idle.
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        if !piglin_ai::is_idle(&tick.mob) {
            return;
        }
        let sound = piglin_ai::idle_sound(tick);
        let volume = self.sound_volume_of(&tick.mob);
        body::make_sound(tick, Some(sound), volume);
    }

    /// What an egg's mob plays: the idle piglin's.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        vec![
            shown_abstract(&own),
            (BABY_INDEX, DataShown::Boolean(own.baby), !own.baby),
            (
                CHARGING_INDEX,
                DataShown::Boolean(own.charging),
                !own.charging,
            ),
            (DANCING_INDEX, DataShown::Boolean(own.dancing), !own.dancing),
        ]
    }
}

/// Where a mob throws from and how (`BehaviorUtils.throwItem(mob, stack, target)`, n7
/// BehaviorUtils.txt): from 0.3 below its eyes, towards `target` at 0.3 each way, picked
/// up after the default delay.
pub(crate) fn throw_item(tick: &mut MobTick<'_>, eye_y: f64, stack: ItemStack, target: Vec3) {
    if stack.count <= 0 {
        return;
    }
    let position = tick.living.body.position;
    let at = [position[0], eye_y - f64::from(0.3_f32), position[2]];
    let way = crate::physics::normalize(crate::physics::sub(target, position));
    let speed = f64::from(0.3_f32);
    tick.out.local.push(Local::Throw {
        stack,
        at,
        pick_up_delay: DEFAULT_PICK_UP_DELAY,
        motion: [way[0] * speed, way[1] * speed, way[2] * speed],
    });
}
