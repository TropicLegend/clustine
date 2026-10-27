//! Tests of a dispenser's projectiles from the game's rules (`docs/groundwork/n9-plan.md`,
//! section 4.9, read against `ProjectileDispenseBehavior`, `ProjectileItem$DispenseConfig`
//! and its builder, and each item's `asProjectile` and `createDispenseConfig`): where
//! each is made, its shot, the stack, the sound, the records, the dispenser's own stream
//! and a dispenser at a line. The worlds are those of the tests of TNT (`tnt/tests.rs`):
//! a stone floor at y = -61, the blocks at -60.
//!
//! A shot's motion is computed exactly as the game draws it: the projectile's own
//! stream (a fresh entity's, drawn aside in no entity's tick), two longs of its UUID,
//! then `shoot`'s three triangles.

use clustine_data::{BlockState, Face, entity_types, items};
use clustine_world::{
    ArrowKind, BlockPos, ChunkPos, Component, Entity, EntityData, Pickup,
    components::PotionContents,
};

use super::super::super::Region;
use super::super::super::entities::insecure_uuid;
use super::super::super::random_ticks::Random;
use super::super::super::tnt::tests::{at, records, state, west_of_a_line, world};
use super::super::{DISPENSED, DISPENSING, FAILED, SHOOT_SMOKE};
use super::{FIREWORK_SHOT, PROJECTILE_LAUNCHED};
use crate::api::{ItemStack, RegionEvent, TickOutput};
use crate::physics::Vec3;

/// Where the tests' dispensers stand.
const AT: BlockPos = BlockPos::new(2, -60, 2);

const SEED: u64 = 7;

fn dispenser(facing: &str) -> BlockState {
    state(&format!(
        "minecraft:dispenser[facing={facing},triggered=false]"
    ))
}

/// A world with a dispenser facing `facing` at `pos`, the floor everywhere else.
fn with_dispenser(pos: BlockPos, facing: &str) -> Region {
    let block = dispenser(facing);
    world(SEED, move |at| (at == pos).then_some(block))
}

/// The dispenser at `pos` given `stack` in its first slot.
fn fill(region: &mut Region, pos: BlockPos, stack: ItemStack) {
    let mut container = region.container_at(pos).expect("a container");
    container.slots[0] = Some(stack);
    region.set_container(pos, container, &mut TickOutput::default());
}

fn first_slot(region: &Region, pos: BlockPos) -> Option<ItemStack> {
    region.container_at(pos).expect("a container").slots[0].clone()
}

/// The tick of the dispenser at `pos`, run now.
fn fire(region: &mut Region, pos: BlockPos) -> TickOutput {
    let mut output = TickOutput::default();
    let state = region.block(pos).expect("loaded");
    assert!(region.dispenser_tick(pos, state, &mut output));
    output
}

fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
            _ => None,
        })
        .collect()
}

fn of_kind(region: &Region, kind: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == kind)
        .collect()
}

/// The one entity of `kind` the region has.
fn the_one(region: &Region, kind: i32) -> Entity {
    let found = of_kind(region, kind);
    let [one] = &found[..] else {
        panic!("one of kind {kind}: {found:?}");
    };
    one.clone()
}

/// `getDispensePosition(source, 0.7, (0, 0.1, 0))`: the middle, plus `0.7 · step` plus
/// the offset, as `Vec3.add` adds them.
fn made_at(pos: BlockPos, step: [f64; 3]) -> Vec3 {
    [
        (f64::from(pos.x) + 0.5) + (0.7 * step[0] + 0.0),
        (f64::from(pos.y) + 0.5) + (0.7 * step[1] + 0.1),
        (f64::from(pos.z) + 0.5) + (0.7 * step[2] + 0.0),
    ]
}

/// What a fresh projectile under `key` is shot with: its own stream aside, as a making
/// outside an entity's tick draws it, its UUID's two longs, then `shoot`.
fn expected_shot(
    region: &Region,
    key: clustine_world::EntityId,
    direction: Vec3,
    power: f32,
    uncertainty: f32,
) -> (uuid::Uuid, crate::physics::thrown::Shot) {
    let mut own = Random::of_entity_aside(region.config.seed, key, 0, region.tick);
    let uuid = insecure_uuid(own.next_u64(), own.next_u64());
    let shot = crate::physics::thrown::shoot(direction, power, uncertainty, &mut own);
    (uuid, shot)
}

/// Section 4.9's table, each item that a dispenser shoots: made at the middle + 0.7 out
/// and 0.1 up, shot along the facing at the configuration's power and uncertainty, the
/// stack one fewer, the click 1002 and the smoke.
#[test]
fn each_projectile_item_is_shot_out_of_the_front_at_its_configurations_power() {
    let cases = [
        (items::ARROW, entity_types::ARROW, 1.1_f32, 6.0_f32),
        (items::TIPPED_ARROW, entity_types::ARROW, 1.1, 6.0),
        (
            items::SPECTRAL_ARROW,
            entity_types::SPECTRAL_ARROW,
            1.1,
            6.0,
        ),
        (items::EGG, entity_types::EGG, 1.1, 6.0),
        (items::BLUE_EGG, entity_types::EGG, 1.1, 6.0),
        (items::BROWN_EGG, entity_types::EGG, 1.1, 6.0),
        (items::SNOWBALL, entity_types::SNOWBALL, 1.1, 6.0),
        (
            items::SPLASH_POTION,
            entity_types::SPLASH_POTION,
            1.375,
            3.0,
        ),
        (
            items::LINGERING_POTION,
            entity_types::LINGERING_POTION,
            1.375,
            3.0,
        ),
        (
            items::EXPERIENCE_BOTTLE,
            entity_types::EXPERIENCE_BOTTLE,
            1.375,
            3.0,
        ),
    ];
    for (facing, name, step) in [
        (Face::East, "east", [1.0, 0.0, 0.0]),
        (Face::Up, "up", [0.0, 1.0, 0.0]),
        (Face::North, "north", [0.0, 0.0, -1.0]),
    ] {
        for (item, kind, power, uncertainty) in cases {
            let mut region = with_dispenser(AT, name);
            fill(&mut region, AT, ItemStack::new(item, 3));
            let output = fire(&mut region, AT);
            let made = the_one(&region, kind);
            assert_eq!(
                made.position.map(f64::to_bits),
                made_at(AT, step).map(f64::to_bits),
                "{item} facing {name}"
            );
            let (uuid, shot) = expected_shot(&region, made.key, step, power, uncertainty);
            assert_eq!(made.uuid, uuid, "{item}");
            assert_eq!(
                made.motion.map(f64::to_bits),
                shot.motion.map(f64::to_bits),
                "{item} facing {name}"
            );
            assert_eq!((made.yaw, made.pitch), (shot.yaw, shot.pitch));
            assert_eq!(first_slot(&region, AT), Some(ItemStack::new(item, 2)));
            assert_eq!(
                level_events(&output),
                [
                    (PROJECTILE_LAUNCHED, AT, 0),
                    (SHOOT_SMOKE, AT, facing as i32)
                ],
                "{item}"
            );
            // Nothing is dropped beside it.
            assert!(of_kind(&region, entity_types::ITEM).is_empty());
        }
    }
}

/// The potions' and the bottle's power is `1.1f * 1.25f` in floats, which is 1.375
/// exactly; their uncertainty half of 6.
#[test]
fn the_potions_configuration_is_a_quarter_more_power_and_half_the_uncertainty() {
    for kind in [
        entity_types::SPLASH_POTION,
        entity_types::LINGERING_POTION,
        entity_types::EXPERIENCE_BOTTLE,
    ] {
        let config = super::config(kind);
        assert_eq!(config.power, 1.375);
        assert_eq!(config.uncertainty, 3.0);
        assert_eq!(config.event, PROJECTILE_LAUNCHED);
    }
    let rocket = super::config(entity_types::FIREWORK_ROCKET);
    assert_eq!((rocket.power, rocket.uncertainty), (0.5, 1.0));
    assert_eq!(rocket.event, FIREWORK_SHOT);
}

/// A shot from the last arrow empties the slot.
#[test]
fn the_last_arrow_shot_empties_the_slot() {
    let mut region = with_dispenser(AT, "east");
    fill(&mut region, AT, ItemStack::new(items::ARROW, 1));
    fire(&mut region, AT);
    assert_eq!(of_kind(&region, entity_types::ARROW).len(), 1);
    assert_eq!(first_slot(&region, AT), None);
}

/// `ArrowItem.asProjectile`: an arrow of one of the stack and no weapon, its pickup
/// set to ALLOWED, no owner, and the constructor's base damage of 2.
#[test]
fn a_dispensed_arrow_may_be_picked_up_has_no_owner_and_deals_two() {
    for (item, spectral) in [(items::ARROW, false), (items::SPECTRAL_ARROW, true)] {
        let mut region = with_dispenser(AT, "east");
        fill(&mut region, AT, ItemStack::new(item, 5));
        fire(&mut region, AT);
        let kind = if spectral {
            entity_types::SPECTRAL_ARROW
        } else {
            entity_types::ARROW
        };
        let made = the_one(&region, kind);
        let EntityData::Arrow(arrow) = &made.data else {
            panic!("an arrow's record: {:?}", made.data);
        };
        assert_eq!(arrow.pickup, Pickup::Allowed);
        assert_eq!(arrow.owner, None);
        assert_eq!(arrow.base_damage.0, 2.0);
        assert_eq!(arrow.pickup_stack, ItemStack::new(item, 1));
        assert_eq!(arrow.weapon, None);
        assert!(!arrow.crit);
        assert_eq!(arrow.pierce_level, 0);
        assert_eq!(
            matches!(arrow.of_kind, ArrowKind::Spectral { .. }),
            spectral
        );
    }
}

/// A tipped arrow is `ArrowItem`'s arrow of `copyWithCount(1)`: its pickup stack keeps
/// the stack's potion contents.
#[test]
fn a_dispensed_tipped_arrow_keeps_its_potion() {
    let mut tipped = ItemStack::new(items::TIPPED_ARROW, 4);
    tipped
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:poison".to_owned()),
            ..PotionContents::default()
        }));
    let mut region = with_dispenser(AT, "east");
    fill(&mut region, AT, tipped.clone());
    fire(&mut region, AT);
    let made = the_one(&region, entity_types::ARROW);
    let EntityData::Arrow(arrow) = &made.data else {
        panic!("an arrow's record: {:?}", made.data);
    };
    assert_eq!(arrow.pickup_stack, tipped.with_count(1));
    assert_eq!(arrow.pickup, Pickup::Allowed);
    assert_eq!(first_slot(&region, AT), Some(tipped.with_count(3)));
}

/// The thrown items keep one of the stack (`setItem`), its components with it, and
/// have no owner.
#[test]
fn a_dispensed_thrown_item_carries_one_of_the_stack_and_no_owner() {
    let mut splash = ItemStack::new(items::SPLASH_POTION, 2);
    splash
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:healing".to_owned()),
            ..PotionContents::default()
        }));
    for (stack, kind) in [
        (ItemStack::new(items::BLUE_EGG, 2), entity_types::EGG),
        (ItemStack::new(items::SNOWBALL, 2), entity_types::SNOWBALL),
        (splash, entity_types::SPLASH_POTION),
    ] {
        let mut region = with_dispenser(AT, "south");
        fill(&mut region, AT, stack.clone());
        fire(&mut region, AT);
        let made = the_one(&region, kind);
        let EntityData::Thrown {
            stack: carried,
            owner,
            ..
        } = &made.data
        else {
            panic!("a thrown item's record: {:?}", made.data);
        };
        assert_eq!(*carried, stack.with_count(1));
        assert_eq!(*owner, None);
    }
}

/// A rocket is made just outside the block (the middle + 0.50001 · step), shot at an
/// angle, with the event 1004; while nothing makes a rocket (`make_firework` before
/// part S3), the stack is kept and the dispenser clicks its failure.
#[test]
fn a_rocket_is_kept_with_a_failures_click_while_nothing_makes_one() {
    assert_eq!(
        super::projectile_position(AT, Face::East, entity_types::FIREWORK_ROCKET),
        [2.5 + 0.500_009_999_999_747_4, -59.5, 2.5]
    );
    let mut region = with_dispenser(AT, "east");
    fill(&mut region, AT, ItemStack::new(items::FIREWORK_ROCKET, 3));
    let output = fire(&mut region, AT);
    let rockets = of_kind(&region, entity_types::FIREWORK_ROCKET);
    if rockets.is_empty() {
        assert_eq!(
            first_slot(&region, AT),
            Some(ItemStack::new(items::FIREWORK_ROCKET, 3))
        );
        assert_eq!(
            level_events(&output),
            [(FAILED, AT, 0), (SHOOT_SMOKE, AT, Face::East as i32)]
        );
    } else {
        // Once part S3 makes rockets: one, the stack shrunk, the rocket's own event.
        assert_eq!(rockets.len(), 1);
        assert_eq!(
            first_slot(&region, AT),
            Some(ItemStack::new(items::FIREWORK_ROCKET, 2))
        );
        assert_eq!(
            level_events(&output),
            [(FIREWORK_SHOT, AT, 0), (SHOOT_SMOKE, AT, Face::East as i32)]
        );
    }
    assert!(of_kind(&region, entity_types::ITEM).is_empty());
}

/// Tridents and ender pearls have no dispense behaviour of their own in the game and are
/// dropped as items (section 4.9).
#[test]
fn tridents_and_ender_pearls_are_still_dropped() {
    for item in [items::TRIDENT, items::ENDER_PEARL] {
        let mut region = with_dispenser(AT, "east");
        fill(&mut region, AT, ItemStack::new(item, 1));
        let output = fire(&mut region, AT);
        let dropped = the_one(&region, entity_types::ITEM);
        let EntityData::Item { stack, .. } = &dropped.data else {
            panic!("an item: {:?}", dropped.data);
        };
        assert_eq!(stack.item, item);
        assert!(of_kind(&region, entity_types::TRIDENT).is_empty());
        assert!(of_kind(&region, entity_types::ENDER_PEARL).is_empty());
        assert_eq!(
            level_events(&output),
            [(DISPENSED, AT, 0), (SHOOT_SMOKE, AT, Face::East as i32)]
        );
    }
}

/// `ProjectileDispenseBehavior` reads no level random: of the dispenser's stream only
/// the slot's draw (`nextInt(1)` for a lone stack) is taken, as a dispense that makes
/// nothing takes it, so the stream after the shot is as it would be without it.
#[test]
fn a_projectile_draws_nothing_from_the_dispensers_stream() {
    for item in [
        items::ARROW,
        items::EGG,
        items::SPLASH_POTION,
        items::EXPERIENCE_BOTTLE,
        items::FIREWORK_ROCKET,
    ] {
        let mut region = with_dispenser(AT, "east");
        fill(&mut region, AT, ItemStack::new(item, 2));
        let mut without = Random::of_block(region.config.seed, AT, region.tick, DISPENSING);
        assert_eq!(without.next_int(1), 0);
        fire(&mut region, AT);
        let after = region
            .block_streams
            .get(&AT)
            .expect("the dispenser's stream kept for the tick");
        assert_eq!(*after, without, "{item}");
    }
}

/// Section 4.1's rule: a dispenser in its own land just west of a line, facing east,
/// makes its arrow in the copy east of it under a key of its own, sent to the chunk's
/// holder with its own draws made, and its own slot shrunk with the click of a shot.
#[test]
fn a_dispenser_at_a_line_sends_its_arrow_to_the_holder_of_the_copy() {
    let pos = at(15, -60, 2);
    let block = dispenser("east");
    let mut region = west_of_a_line(SEED, move |at| (at == pos).then_some(block));
    fill(&mut region, pos, ItemStack::new(items::ARROW, 2));
    let output = fire(&mut region, pos);
    assert!(of_kind(&region, entity_types::ARROW).is_empty());
    let [(to, sent)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    assert_eq!(*to, ChunkPos::new(1, 0));
    assert_eq!(i32::from(sent.kind), entity_types::ARROW);
    let step = [1.0, 0.0, 0.0];
    assert_eq!(
        sent.position.map(f64::to_bits),
        made_at(pos, step).map(f64::to_bits)
    );
    assert_eq!(sent.position[0], 16.2);
    let (uuid, shot) = expected_shot(&region, sent.key, step, 1.1, 6.0);
    assert_eq!(sent.uuid, uuid);
    assert_eq!(sent.motion.map(f64::to_bits), shot.motion.map(f64::to_bits));
    let EntityData::Arrow(arrow) = &sent.data else {
        panic!("an arrow's record: {:?}", sent.data);
    };
    assert_eq!(arrow.pickup, Pickup::Allowed);
    assert_eq!(arrow.owner, None);
    assert_eq!(
        first_slot(&region, pos),
        Some(ItemStack::new(items::ARROW, 1))
    );
    assert_eq!(
        level_events(&output),
        [
            (PROJECTILE_LAUNCHED, pos, 0),
            (SHOOT_SMOKE, pos, Face::East as i32)
        ]
    );
}
