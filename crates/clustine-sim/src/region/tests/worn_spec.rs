//! What a player's equipment adds to their attributes (V2, `docs/groundwork/v-plan.md`,
//! section 4.5), tested from the game's rules by someone who did not write `worn.rs`:
//! `LivingEntity.collectEquipmentChanges` takes each slot's stack (the main hand being the
//! selected hotbar slot) unless it is empty or broken, and `ItemStack.forEachModifier`
//! adds the item's `attribute_modifiers` whose slot group takes in the slot and, through
//! `EnchantmentHelper.forEachModifier`, each `minecraft:attributes` effect of each
//! enchantment whose `slots` take it in, named `<effect id>/<slot>`
//! (`EnchantmentAttributeEffect.idForSlot`) with the amount of its level. The client is
//! told each syncable attribute whose modifiers changed, whole
//! (`ServerEntity.sendDirtyEntityData`), and nothing when nothing changed; an
//! explosion's push takes off `explosion_knockback_resistance`
//! (`ServerExplosion.hurtEntities`).
//!
//! The amounts come from the 26.3 data: `efficiency.json` (`levels_squared`, added 1:
//! 10 at level III), `aqua_affinity.json` (linear 4: 4 at I, `add_multiplied_total`),
//! `blast_protection.json` (linear 0.15, `add_value`, slots `armor`),
//! `sweeping_edge.json` (a fraction, (level) / (level + 1), slot `mainhand`); the items'
//! own from `item_components.rs` (an iron chestplate's 6 armour and 0 toughness, an iron
//! helmet's 2, an iron sword's -2.4f attack speed and 5 attack damage); Haste's attack
//! speed from `MobEffects.<clinit>` @88-102 (0.10000000149011612 a level,
//! `ADD_MULTIPLIED_TOTAL`).

use clustine_data::{attributes as ids, items};
use clustine_world::components::{Component, EffectInstance};
use clustine_world::{ARMOUR_START, OFF_HAND};

use super::n9_world::{act, alone, clock_at, enter_with, entering_place};
use super::*;
use crate::api::AttributeShown;
use crate::mobs::body::effects::{Effect, Layer};
use crate::region::explosions::Explosion;

const ADD_VALUE: u8 = 0;
const ADD_MULTIPLIED_TOTAL: u8 = 2;

const FEET: usize = ARMOUR_START;
const CHEST: usize = ARMOUR_START + 2;
const HEAD: usize = ARMOUR_START + 3;

/// `item` with the enchantments `levels`, as an enchanting table leaves it.
fn enchanted(item: i32, levels: &[(&str, i32)]) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack.components.set(Component::Enchantments(
        levels
            .iter()
            .map(|(name, level)| ((*name).to_owned(), *level))
            .collect(),
    ));
    stack
}

fn efficiency_pickaxe() -> ItemStack {
    enchanted(items::DIAMOND_PICKAXE, &[("minecraft:efficiency", 3)])
}

/// Puts `stack` into player 1's inventory slot `slot` between two ticks.
fn put(region: &mut Region, slot: usize, stack: Option<ItemStack>) {
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.inventory.set(slot, stack);
}

/// The region of `on_ticking_floor_with(&[])` with player 1 holding and wearing nothing,
/// slot 0 selected, and a tick run so that what they had before is looked at.
fn bare() -> Region {
    let mut region = on_ticking_floor_with(&[]);
    {
        let me = region
            .players
            .get_mut(&player(1))
            .expect("player 1 is here");
        me.inventory = Inventory::default();
        me.selected_slot = 0;
    }
    region.tick(&TickInputs::default());
    region
}

/// Every attribute player 1 was told in `output`, in the order told.
fn told(output: &TickOutput) -> Vec<AttributeShown> {
    output
        .player_events
        .iter()
        .filter(|(id, _)| *id == player(1))
        .filter_map(|(_, event)| match event {
            PlayerEvent::Attributes(attributes) => Some(attributes.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The attribute `attribute` as player 1 was last told it in `output`, if they were.
fn told_of(output: &TickOutput, attribute: i32) -> Option<AttributeShown> {
    told(output)
        .into_iter()
        .rev()
        .find(|shown| i32::from(shown.attribute) == attribute)
}

/// The modifiers of `shown`, sorted by id: the game's map keeps no order a client reads.
fn modifiers(shown: &AttributeShown) -> Vec<(String, f64, u8)> {
    let mut modifiers = shown.modifiers.clone();
    modifiers.sort_by(|a, b| a.0.cmp(&b.0));
    modifiers
}

fn modifier(id: &str, amount: f64, operation: u8) -> (String, f64, u8) {
    (id.to_owned(), amount, operation)
}

fn default_of(attribute: i32) -> f64 {
    clustine_data::ATTRIBUTES[usize::try_from(attribute).unwrap()].default
}

/// The attribute ids told in `output`, sorted.
fn told_ids(output: &TickOutput) -> Vec<i32> {
    let mut ids: Vec<i32> = told(output)
        .iter()
        .map(|shown| i32::from(shown.attribute))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[test]
fn an_efficiency_three_pickaxe_selected_tells_its_mining_efficiency_once_and_its_loss_when_put_away()
 {
    let mut region = bare();
    put(&mut region, 0, Some(efficiency_pickaxe()));
    let output = region.tick(&TickInputs::default());
    let shown = told_of(&output, ids::MINING_EFFICIENCY).expect("mining efficiency told");
    assert_eq!(shown.base, default_of(ids::MINING_EFFICIENCY));
    assert_eq!(
        modifiers(&shown),
        [modifier(
            "minecraft:enchantment.efficiency/mainhand",
            10.0,
            ADD_VALUE
        )]
    );
    // The pickaxe's own main-hand attack speed changed too, and is syncable; its attack
    // damage is not syncable and is never told.
    assert_eq!(
        told_ids(&output),
        [ids::ATTACK_SPEED, ids::MINING_EFFICIENCY]
    );

    // Nothing changed: nothing told.
    let output = region.tick(&TickInputs::default());
    assert!(told(&output).is_empty(), "{:?}", told(&output));

    // An empty slot selected: the efficiency goes, told with no modifiers.
    let output = region.tick(&moves(vec![act(
        &region,
        1,
        PlayerInput::SelectSlot { slot: 1 },
    )]));
    let shown = told_of(&output, ids::MINING_EFFICIENCY).expect("mining efficiency told");
    assert!(shown.modifiers.is_empty(), "{shown:?}");
    assert_eq!(shown.base, default_of(ids::MINING_EFFICIENCY));
    let speed = told_of(&output, ids::ATTACK_SPEED).expect("attack speed told");
    assert!(speed.modifiers.is_empty(), "{speed:?}");
}

#[test]
fn an_efficiency_pickaxe_in_the_off_hand_gives_no_mining_efficiency() {
    let mut region = bare();
    put(&mut region, OFF_HAND, Some(efficiency_pickaxe()));
    let output = region.tick(&TickInputs::default());
    // Efficiency's slots are the main hand's only, and the pickaxe's own modifiers are
    // the main hand's: nothing changes, nothing is told.
    assert!(told(&output).is_empty(), "{:?}", told(&output));
    assert_eq!(
        region.player_attribute(player(1), ids::MINING_EFFICIENCY),
        0.0
    );
}

#[test]
fn an_aqua_affinity_helmet_gives_submerged_mining_speed_and_its_own_armour() {
    let mut region = bare();
    put(
        &mut region,
        HEAD,
        Some(enchanted(
            items::IRON_HELMET,
            &[("minecraft:aqua_affinity", 1)],
        )),
    );
    let output = region.tick(&TickInputs::default());
    let submerged = told_of(&output, ids::SUBMERGED_MINING_SPEED).expect("told");
    assert_eq!(submerged.base, default_of(ids::SUBMERGED_MINING_SPEED));
    assert_eq!(
        modifiers(&submerged),
        [modifier(
            "minecraft:enchantment.aqua_affinity/head",
            4.0,
            ADD_MULTIPLIED_TOTAL
        )]
    );
    let armour = told_of(&output, ids::ARMOR).expect("armour told");
    assert_eq!(
        modifiers(&armour),
        [modifier("minecraft:armor.helmet", 2.0, ADD_VALUE)]
    );
    let toughness = told_of(&output, ids::ARMOR_TOUGHNESS).expect("toughness told");
    assert_eq!(
        modifiers(&toughness),
        [modifier("minecraft:armor.helmet", 0.0, ADD_VALUE)]
    );
    // 0.2 × (1 + 4): `add_multiplied_total` multiplies the whole.
    let value = region.player_attribute(player(1), ids::SUBMERGED_MINING_SPEED);
    assert!((value - 0.2 * 5.0).abs() < 1e-9, "{value}");
}

#[test]
fn an_iron_chestplate_tells_its_armour_and_one_worn_to_its_most_gives_none() {
    let mut region = bare();
    put(
        &mut region,
        CHEST,
        Some(ItemStack::new(items::IRON_CHESTPLATE, 1)),
    );
    let output = region.tick(&TickInputs::default());
    let armour = told_of(&output, ids::ARMOR).expect("armour told");
    assert_eq!(armour.base, 0.0);
    assert_eq!(
        modifiers(&armour),
        [modifier("minecraft:armor.chestplate", 6.0, ADD_VALUE)]
    );
    assert_eq!(region.player_attribute(player(1), ids::ARMOR), 6.0);

    // Worn to its most (240, the item's `max_damage`): broken, so it gives nothing, and
    // the armour is told without it.
    let mut broken = ItemStack::new(items::IRON_CHESTPLATE, 1);
    broken.components.set(Component::Damage(240));
    put(&mut region, CHEST, Some(broken.clone()));
    let output = region.tick(&TickInputs::default());
    let armour = told_of(&output, ids::ARMOR).expect("armour told");
    assert!(armour.modifiers.is_empty(), "{armour:?}");
    assert_eq!(region.player_attribute(player(1), ids::ARMOR), 0.0);

    // One short of its most still gives its armour.
    let mut worn = broken;
    worn.components.set(Component::Damage(239));
    put(&mut region, CHEST, Some(worn));
    region.tick(&TickInputs::default());
    assert_eq!(region.player_attribute(player(1), ids::ARMOR), 6.0);
}

#[test]
fn a_broken_chestplate_put_on_tells_nothing() {
    let mut region = bare();
    let mut broken = ItemStack::new(items::IRON_CHESTPLATE, 1);
    broken.components.set(Component::Damage(240));
    put(&mut region, CHEST, Some(broken));
    let output = region.tick(&TickInputs::default());
    assert!(told(&output).is_empty(), "{:?}", told(&output));
}

#[test]
fn a_sword_selected_tells_its_attack_speed_and_never_its_attack_damage() {
    let mut region = bare();
    put(&mut region, 0, Some(ItemStack::new(items::IRON_SWORD, 1)));
    let output = region.tick(&TickInputs::default());
    let speed = told_of(&output, ids::ATTACK_SPEED).expect("attack speed told");
    assert_eq!(speed.base, default_of(ids::ATTACK_SPEED));
    assert_eq!(
        modifiers(&speed),
        [modifier(
            "minecraft:base_attack_speed",
            f64::from(-2.4f32),
            ADD_VALUE
        )]
    );
    assert_eq!(told_ids(&output), [ids::ATTACK_SPEED]);
    // The attack damage is not syncable but is the player's all the same: a player's
    // base of 1 (`Player.createAttributes` @3-6, not the attribute's default of 2) and
    // the sword's 5.
    assert_eq!(region.player_attribute(player(1), ids::ATTACK_DAMAGE), 6.0);
}

fn haste() -> Effect {
    Effect::named("minecraft:haste").expect("haste is an effect")
}

const HASTE_SPEED: f64 = 0.100_000_001_490_116_12;

#[test]
fn a_sword_taken_up_with_haste_tells_both_modifiers_of_attack_speed() {
    let mut region = bare();
    let mut output = TickOutput::default();
    let layer = Layer::new(0, 200, region.tick_number());
    assert!(region.add_player_effect(player(1), haste(), layer, &mut output));
    put(&mut region, 0, Some(ItemStack::new(items::IRON_SWORD, 1)));
    let output = region.tick(&TickInputs::default());
    let speed = told_of(&output, ids::ATTACK_SPEED).expect("attack speed told");
    assert_eq!(
        modifiers(&speed),
        [
            modifier("minecraft:base_attack_speed", f64::from(-2.4f32), ADD_VALUE),
            modifier("minecraft:effect.haste", HASTE_SPEED, ADD_MULTIPLIED_TOTAL),
        ]
    );
}

#[test]
fn haste_given_to_a_player_holding_a_sword_tells_both_modifiers_of_attack_speed() {
    let mut region = bare();
    put(&mut region, 0, Some(ItemStack::new(items::IRON_SWORD, 1)));
    region.tick(&TickInputs::default());
    let mut output = TickOutput::default();
    let layer = Layer::new(0, 200, region.tick_number());
    assert!(region.add_player_effect(player(1), haste(), layer, &mut output));
    let speed = told_of(&output, ids::ATTACK_SPEED).expect("attack speed told");
    assert_eq!(
        modifiers(&speed),
        [
            modifier("minecraft:base_attack_speed", f64::from(-2.4f32), ADD_VALUE),
            modifier("minecraft:effect.haste", HASTE_SPEED, ADD_MULTIPLIED_TOTAL),
        ]
    );
}

#[test]
fn a_sweeping_edge_three_sword_gives_three_quarters_of_sweeping_damage() {
    let mut region = bare();
    put(
        &mut region,
        0,
        Some(enchanted(
            items::IRON_SWORD,
            &[("minecraft:sweeping_edge", 3)],
        )),
    );
    let output = region.tick(&TickInputs::default());
    let sweeping = told_of(&output, ids::SWEEPING_DAMAGE_RATIO).expect("told");
    assert_eq!(
        modifiers(&sweeping),
        [modifier(
            "minecraft:enchantment.sweeping_edge/mainhand",
            0.75,
            ADD_VALUE
        )]
    );
}

#[test]
fn a_player_joining_in_armour_is_told_their_armour_without_any_change() {
    let mut region = alone(0, clock_at(0));
    let mut place = entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::<EffectInstance>::new(), 0);
    place
        .inventory
        .set(CHEST, Some(ItemStack::new(items::IRON_CHESTPLATE, 1)));
    let outputs = enter_with(&mut region, 1, place);
    let armour = outputs
        .iter()
        .find_map(|output| told_of(output, ids::ARMOR))
        .expect("armour told at the join");
    assert_eq!(
        modifiers(&armour),
        [modifier("minecraft:armor.chestplate", 6.0, ADD_VALUE)]
    );
    // Told once: the next ticks find nothing changed.
    for _ in 0..3 {
        let output = region.tick(&TickInputs::default());
        assert!(told(&output).is_empty(), "{:?}", told(&output));
    }
}

#[test]
fn the_region_reads_mining_efficiency_from_the_selected_pickaxe_after_the_tick_s_look() {
    let mut region = bare();
    assert_eq!(
        region.player_attribute(player(1), ids::MINING_EFFICIENCY),
        default_of(ids::MINING_EFFICIENCY)
    );
    put(&mut region, 0, Some(efficiency_pickaxe()));
    region.tick(&TickInputs::default());
    assert_eq!(
        region.player_attribute(player(1), ids::MINING_EFFICIENCY),
        10.0
    );
    put(&mut region, 0, None);
    region.tick(&TickInputs::default());
    assert_eq!(
        region.player_attribute(player(1), ids::MINING_EFFICIENCY),
        default_of(ids::MINING_EFFICIENCY)
    );
}

/// The push an explosion of radius 4 three blocks east of player 1, at their feet'
/// height plus a half, gives them when they wear `worn` (feet, legs, chest, head).
fn push_wearing(worn: [Option<ItemStack>; 4]) -> [f64; 3] {
    let mut region = bare();
    for (at, stack) in worn.into_iter().enumerate() {
        put(&mut region, ARMOUR_START + at, stack);
    }
    region.tick(&TickInputs::default());
    let (entity, pose) = region.player(player(1)).expect("player 1 is here");
    let source = EntityId(999_999);
    let explosion = Explosion {
        source,
        centre: [
            pose.position.x + 3.0,
            pose.position.y + 0.5,
            pose.position.z,
        ],
        radius: 4.0,
        owner: None,
        indirect: None,
        decay: false,
        rails_pass: false,
        source_in_water: false,
        fire: false,
        hurts: true,
        resistance_at: None,
    };
    let mut random = region.entity_stream(source).random;
    let mut output = TickOutput::default();
    region.explode(&explosion, &mut random, &mut output);
    let pushes: Vec<[f64; 3]> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion { knockback, .. } => Some(knockback.clone()),
            _ => None,
        })
        .flatten()
        .filter(|(pushed, _)| *pushed == entity)
        .map(|(_, push)| push)
        .collect();
    // A push of nothing may be sent or left out alike.
    pushes.first().copied().unwrap_or([0.0; 3])
}

fn blast_protected(item: i32, level: i32) -> Option<ItemStack> {
    Some(enchanted(item, &[("minecraft:blast_protection", level)]))
}

#[test]
fn blast_protection_one_boots_take_fifteen_hundredths_off_an_explosion_s_push() {
    let bare_push = push_wearing([None, None, None, None]);
    assert!(bare_push[0].abs() > 0.01, "pushed at all: {bare_push:?}");
    let booted = push_wearing([blast_protected(items::IRON_BOOTS, 1), None, None, None]);
    // `linear` gives a float: 0.15f widened.
    let kept = 1.0 - f64::from(0.15f32);
    for axis in 0..3 {
        assert!(
            (booted[axis] - bare_push[axis] * kept).abs() < 1e-9,
            "{booted:?} against {bare_push:?}"
        );
    }
}

#[test]
fn four_pieces_of_blast_protection_four_leave_no_push_as_the_resistance_stops_at_one() {
    let push = push_wearing([
        blast_protected(items::IRON_BOOTS, 4),
        blast_protected(items::IRON_LEGGINGS, 4),
        blast_protected(items::IRON_CHESTPLATE, 4),
        blast_protected(items::IRON_HELMET, 4),
    ]);
    assert_eq!(push.map(f64::abs), [0.0; 3], "{push:?}");
    assert_eq!(
        region_resistance_wearing_four_of_four(),
        1.0,
        "the attribute is bounded at 1"
    );
}

fn region_resistance_wearing_four_of_four() -> f64 {
    let mut region = bare();
    for (at, item) in [
        items::IRON_BOOTS,
        items::IRON_LEGGINGS,
        items::IRON_CHESTPLATE,
        items::IRON_HELMET,
    ]
    .into_iter()
    .enumerate()
    {
        put(&mut region, ARMOUR_START + at, blast_protected(item, 4));
    }
    region.tick(&TickInputs::default());
    region.player_attribute(player(1), ids::EXPLOSION_KNOCKBACK_RESISTANCE)
}

#[test]
fn blast_protection_is_told_once_for_each_armour_slot_it_is_worn_in() {
    let mut region = bare();
    put(&mut region, FEET, blast_protected(items::IRON_BOOTS, 1));
    put(&mut region, HEAD, blast_protected(items::IRON_HELMET, 2));
    let output = region.tick(&TickInputs::default());
    let resistance = told_of(&output, ids::EXPLOSION_KNOCKBACK_RESISTANCE).expect("told");
    assert_eq!(
        modifiers(&resistance),
        [
            modifier(
                "minecraft:enchantment.blast_protection/feet",
                f64::from(0.15f32),
                ADD_VALUE
            ),
            modifier(
                "minecraft:enchantment.blast_protection/head",
                f64::from(0.15f32 + 0.15f32),
                ADD_VALUE
            ),
        ]
    );
}
