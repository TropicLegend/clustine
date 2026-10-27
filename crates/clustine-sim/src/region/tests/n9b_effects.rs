//! Mob effects and what is worn, through the region, from the specification, by someone
//! who wrote none of their code (`docs/groundwork/n9-plan.md`, section 5.2 and its
//! refusals as section 18 decision 2 completes them, with D9 4): one splash's effects
//! refused type by type; resistance taken after the armour a zombie wears; a
//! vindicator's axe.

use clustine_data::{entity_types, items};
use clustine_world::components::{Component, EffectInstance, PotionContents};
use clustine_world::{ChunkPos, Double, Entity, EntityData, EntityId, ItemStack, Pickup};

use super::thrown::{cow, health, lay, mob};
use super::*;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::effects::{self, Effect};
use crate::region::projectiles::{Aim, Making};
use crate::region::tnt::tests::{idle, nothing, world};

const SEED: u64 = 0xEFFE_C750;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type's id")
}

/// A mob of the type `of` at `feet`, at its type's full health, with no AI, made ready
/// by `prepare`.
fn mob_at(
    region: &mut Region,
    of: i32,
    feet: [f64; 3],
    prepare: impl FnOnce(&mut Mob, &mut Entity),
) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    let mut entity = cow(key, feet);
    entity.kind = kind(of);
    let mut made = Mob::decode(&entity.data).expect("a mob");
    made.body.vitals.health = crate::mobs::body::attributes::supplied(
        entity.kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a health") as f32;
    prepare(&mut made, &mut entity);
    entity.data = made.encode();
    lay(region, &entity);
    key
}

fn instance(name: &str, duration: i32) -> EffectInstance {
    EffectInstance {
        effect: name.to_owned(),
        amplifier: 0,
        duration,
        ambient: false,
        visible: true,
        show_icon: true,
    }
}

/// The plan's 5.2 "Refusals" (`canBeAffected`): spider poison, the parched weakness, the
/// wither skeleton wither, the nautilus poison, the undead (`#ignores_poison_and_regen`,
/// which holds `#undead` and so the skeletons, the parched among them) poison and
/// regeneration, the silverfish infested and the slime oozing; a cow refuses nothing. All splashed by one potion, as a player would throw it among them.
#[test]
fn one_splash_s_effects_are_refused_by_each_type_as_its_own_rule_says() {
    let mut region = world(SEED, nothing);
    let names = [
        "minecraft:poison",
        "minecraft:wither",
        "minecraft:regeneration",
        "minecraft:weakness",
        "minecraft:infested",
        "minecraft:oozing",
    ];
    let given = [
        Effect::POISON,
        Effect::WITHER,
        Effect::REGENERATION,
        Effect::WEAKNESS,
        Effect::INFESTED,
        Effect::OOZING,
    ];
    // Each type and what it refuses of the six.
    let cases: [(i32, &[Effect]); 9] = [
        (entity_types::COW, &[]),
        (entity_types::SPIDER, &[Effect::POISON]),
        (entity_types::CAVE_SPIDER, &[Effect::POISON]),
        (
            entity_types::PARCHED,
            &[Effect::WEAKNESS, Effect::POISON, Effect::REGENERATION],
        ),
        (
            entity_types::WITHER_SKELETON,
            &[Effect::WITHER, Effect::POISON, Effect::REGENERATION],
        ),
        (entity_types::NAUTILUS, &[Effect::POISON]),
        (
            entity_types::ZOMBIE,
            &[Effect::POISON, Effect::REGENERATION],
        ),
        (entity_types::SILVERFISH, &[Effect::INFESTED]),
        (entity_types::SLIME, &[Effect::OOZING]),
    ];
    // In a ring of radius 1.5 about where the potion breaks, on the floor.
    let middle = [8.5, -60.0, 8.5];
    let keys: Vec<EntityId> = cases
        .iter()
        .enumerate()
        .map(|(at, (of, _))| {
            let angle = at as f64 / cases.len() as f64 * std::f64::consts::TAU;
            let feet = [
                middle[0] + 1.5 * angle.cos(),
                -60.0,
                middle[2] + 1.5 * angle.sin(),
            ];
            mob_at(&mut region, *of, feet, |_, _| {})
        })
        .collect();
    let mut stack = ItemStack::new(items::SPLASH_POTION, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            custom_effects: names.iter().map(|name| instance(name, 2400)).collect(),
            ..PotionContents::default()
        }));
    let making = Making {
        kind: kind(entity_types::SPLASH_POTION),
        at: [middle[0], -58.0, middle[2]],
        data: EntityData::Thrown {
            stack,
            owner: None,
            left_owner: false,
            has_been_shot: false,
        },
        aim: Aim::Direction([0.0, -1.0, 0.0]),
        power: 0.5,
        uncertainty: 0.0,
    };
    let potion = region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("made");
    for _ in 0..10 {
        idle(&mut region);
        if region.find_entity(potion).is_none() {
            break;
        }
    }
    assert!(region.find_entity(potion).is_none(), "it broke");
    for ((of, refused), key) in cases.iter().zip(&keys) {
        let hit = mob(&region, *key);
        for effect in given {
            assert_eq!(
                hit.body.effects.has(effect),
                !refused.contains(&effect),
                "type {of}, effect {effect:?}"
            );
        }
    }
}

/// A zombie at `feet` wearing an iron chestplate (its `attribute_modifiers`: 6 armour in
/// the chest slot, on its own 2), with `resistance` of amplifier 0 or none.
fn armoured_zombie(region: &mut Region, feet: [f64; 3], resistance: bool) -> EntityId {
    mob_at(region, entity_types::ZOMBIE, feet, |made, entity| {
        made.body.equipment.slots[usize::from(slots::CHEST)] =
            Some(ItemStack::new(items::IRON_CHESTPLATE, 1));
        if resistance {
            assert!(effects::add(
                made,
                entity.kind,
                Effect::RESISTANCE,
                0,
                6000,
                entity.lived
            ));
        }
    })
}

/// An arrow of nobody's at `at` along x at `power`, no crit.
fn arrow_at(region: &mut Region, at: [f64; 3], power: f32) {
    let arrow = clustine_world::Arrow {
        pickup: Pickup::Disallowed,
        pickup_stack: ItemStack::new(items::ARROW, 1),
        weapon: None,
        owner: None,
        left_owner: false,
        has_been_shot: false,
        base_damage: Double(2.0),
        crit: false,
        pierce_level: 0,
        pierced: Vec::new(),
        hit_sound: "minecraft:entity.arrow.hit".to_owned(),
        in_ground: None,
        shake_until: 0,
        of_kind: clustine_world::ArrowKind::Arrow,
    };
    let making = Making {
        kind: kind(entity_types::ARROW),
        at,
        data: EntityData::Arrow(Box::new(arrow)),
        aim: Aim::Direction([1.0, 0.0, 0.0]),
        power,
        uncertainty: 0.0,
    };
    region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("made");
}

/// The plan's 5.2 resistance row (finding 13: n7d LivingEntity 4769–4916): the armour
/// takes its share first (`CombatRules.getDamageAfterAbsorb`: `10 · (1 − clamp(8 − 10 /
/// 2, 8 / 5, 20) / 25) = 8.8` for 8 armour and no toughness), then resistance a fifth of
/// what is left (`· 20 / 25`): 7.04. The other order would take 6.72. The chestplate's
/// armour is its `attribute_modifiers` for the chest slot.
#[test]
fn resistance_takes_its_fifth_after_the_armour_a_zombie_wears() {
    let mut region = world(SEED, nothing);
    let bare = armoured_zombie(&mut region, [4.5, -60.0, 0.5], false);
    let resisting = armoured_zombie(&mut region, [4.5, -60.0, 8.5], true);
    let full = health(&region, bare);
    // At 5 along x: `ceil(5 · 2) = 10`.
    arrow_at(&mut region, [1.5, -59.0, 0.5], 5.0);
    arrow_at(&mut region, [1.5, -59.0, 8.5], 5.0);
    idle(&mut region);
    let armoured = full - health(&region, bare);
    let both = full - health(&region, resisting);
    assert!((armoured - 8.8).abs() < 1.0e-4, "armour alone: {armoured}");
    assert!(
        (both - 7.04).abs() < 1.0e-4,
        "armour then resistance: {both}"
    );
}

/// Section 6's last item and the plan's 8.2: what a mob holds counts by its
/// `attribute_modifiers` for the slot it is in. A vindicator's iron axe adds 8 to its 5,
/// so its hit on a villager takes 13.
#[test]
fn a_vindicator_s_iron_axe_adds_its_eight_to_the_hit() {
    let mut region = world(SEED, nothing);
    let villager = mob_at(
        &mut region,
        entity_types::VILLAGER,
        [5.5, -60.0, 0.5],
        |_, _| {},
    );
    let full = health(&region, villager);
    mob_at(
        &mut region,
        entity_types::VINDICATOR,
        [3.5, -60.0, 0.5],
        |made, _| {
            made.body.flags.no_ai = false;
            made.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
                Some(ItemStack::new(items::IRON_AXE, 1));
        },
    );
    let mut first = None;
    for _ in 0..200 {
        idle(&mut region);
        let now = health(&region, villager);
        if now < full {
            first = Some(full - now);
            break;
        }
    }
    assert_eq!(first, Some(13.0), "the axe's hit");
}
