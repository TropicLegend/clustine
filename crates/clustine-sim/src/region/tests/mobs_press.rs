//! Tests of what mobs press and step on, from the game's classes: a pig keeps a wooden
//! plate down and a zombie a stone one for as long as they stand on it
//! (`BasePressurePlateBlock.entityInside` and its looks again by `tick`, with
//! `PressurePlateBlock.getSignalStrength` counting `Entity` or `LivingEntity`), a mob
//! trips tripwire, a bat presses nothing (`Bat.isIgnoringBlockTriggers`), and a mob lights
//! the redstone ore it stands on (`RedStoneOreBlock.stepOn`).
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61. Each mob stands in a pen one block wide
//! and two high, so that whatever its mind wants, it stays where the test put it.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::{Entity, EntityChange, EntityId};

use super::redstone::{block_at, sounds, state, ticking_with};
use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use clustine_blocks::data::set_flag;
use clustine_data::sounds as s;

const MOB: EntityId = EntityId(40);

/// Where the plate (or whatever the mob stands in) is, with a lamp under it, which the
/// plate's strong power below it lights.
const SPOT: BlockPos = BlockPos::new(3, -60, 3);
const LAMP: BlockPos = BlockPos::new(3, -61, 3);

/// A mob of the type `kind` at `position`, of its type's full health and air.
fn mob_at(kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    Entity {
        portal: Default::default(),
        key: MOB,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(MOB.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

/// A zombie wearing a carved pumpkin, which keeps the sun off it.
fn zombie() -> Mob {
    let mut zombie = Mob::fresh();
    zombie.body.equipment.slots[usize::from(crate::living::slots::HEAD)] =
        Some(ItemStack::new(items::CARVED_PUMPKIN, 1));
    zombie
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// A region with `at_spot` at [`SPOT`], a lamp under it, and stone around it two high.
fn pen(at_spot: BlockState) -> Region {
    let mut blocks = vec![(SPOT, at_spot), (LAMP, blocks::REDSTONE_LAMP)];
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        for y in [-60, -59] {
            blocks.push((BlockPos::new(SPOT.x + dx, y, SPOT.z + dz), blocks::STONE));
        }
    }
    ticking_with(&blocks)
}

fn lit(region: &Region) -> bool {
    block_at(region, LAMP).and_then(|lamp| lamp.property("lit")) == Some("true")
}

/// The plates' clicks among the sounds of a tick.
fn clicks(output: &TickOutput) -> Vec<i32> {
    let plates = [
        s::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_ON,
        s::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_OFF,
        s::BLOCK_STONE_PRESSURE_PLATE_CLICK_ON,
        s::BLOCK_STONE_PRESSURE_PLATE_CLICK_OFF,
        s::BLOCK_METAL_PRESSURE_PLATE_CLICK_ON,
        s::BLOCK_METAL_PRESSURE_PLATE_CLICK_OFF,
    ];
    sounds(output)
        .into_iter()
        .map(|(sound, ..)| sound)
        .filter(|sound| plates.contains(sound))
        .collect()
}

fn the_middle_of_the_spot() -> [f64; 3] {
    [
        f64::from(SPOT.x) + 0.5,
        f64::from(SPOT.y),
        f64::from(SPOT.z) + 0.5,
    ]
}

/// A plate that a mob of the type `kind` stands on goes down in its first tick, lights
/// the lamp under it, and stays down past several of its looks again (every 20 ticks).
fn kept_down_by(kind: i32, mob: Mob, plate: &str) {
    let mut region = pen(state(&format!("minecraft:{plate}[powered=false]")));
    lay(&mut region, &mob_at(kind, the_middle_of_the_spot(), mob));
    idle(&mut region);
    assert_eq!(
        block_at(&region, SPOT),
        Some(state(&format!("minecraft:{plate}[powered=true]"))),
        "the plate goes down in the mob's first tick"
    );
    assert!(lit(&region));
    for tick in 0..90 {
        let output = idle(&mut region);
        // The plate's looks again find the mob on it, so it never comes up between
        // them: no click, and no neighbour told.
        assert_eq!(
            clicks(&output),
            Vec::<i32>::new(),
            "the plate makes no sound {tick} ticks on"
        );
        assert_eq!(
            block_at(&region, SPOT),
            Some(state(&format!("minecraft:{plate}[powered=true]"))),
            "the plate is down {tick} ticks on"
        );
        assert!(lit(&region), "the lamp is lit {tick} ticks on");
    }
}

#[test]
fn a_pig_keeps_a_wooden_plate_down_and_its_lamp_lit_while_it_stands_on_it() {
    kept_down_by(entity_types::PIG, Mob::fresh(), "oak_pressure_plate");
}

#[test]
fn a_zombie_keeps_a_stone_plate_down_and_its_lamp_lit_while_it_stands_on_it() {
    kept_down_by(entity_types::ZOMBIE, zombie(), "stone_pressure_plate");
}

#[test]
fn a_pig_keeps_a_polished_blackstone_plate_down_as_a_living_thing() {
    kept_down_by(
        entity_types::PIG,
        Mob::fresh(),
        "polished_blackstone_pressure_plate",
    );
}

/// A pressed plate whose mob is taken away comes up at its next look, twenty ticks
/// after it went down, and the lamp goes out.
#[test]
fn a_stone_plate_comes_up_at_its_look_after_its_mob_is_gone() {
    let mut region = pen(state("minecraft:stone_pressure_plate[powered=false]"));
    lay(
        &mut region,
        &mob_at(entity_types::PIG, the_middle_of_the_spot(), Mob::fresh()),
    );
    idle(&mut region);
    assert!(lit(&region));
    let chunk = SPOT.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Gone(MOB));
    region.entity_chunks.remove(&MOB);
    for _ in 1..20 {
        idle(&mut region);
        assert_eq!(
            block_at(&region, SPOT),
            Some(state("minecraft:stone_pressure_plate[powered=true]"))
        );
    }
    idle(&mut region);
    assert_eq!(
        block_at(&region, SPOT),
        Some(state("minecraft:stone_pressure_plate[powered=false]"))
    );
}

/// A light weighted plate under one pig has a power of one fifteenth of its most,
/// ceiled: 1.
#[test]
fn a_light_weighted_plate_under_a_pig_has_a_power_of_one() {
    let mut region = pen(state("minecraft:light_weighted_pressure_plate[power=0]"));
    lay(
        &mut region,
        &mob_at(entity_types::PIG, the_middle_of_the_spot(), Mob::fresh()),
    );
    for _ in 0..40 {
        idle(&mut region);
        assert_eq!(
            block_at(&region, SPOT),
            Some(state("minecraft:light_weighted_pressure_plate[power=1]"))
        );
    }
}

/// A bat ignores block triggers (`Bat.isIgnoringBlockTriggers` returns true): one
/// whose box is on a plate is not counted, by the plate's look or by its own walk.
#[test]
fn a_bat_on_a_wooden_plate_is_not_counted() {
    let mut region = pen(state("minecraft:oak_pressure_plate[powered=false]"));
    let bat = mob_at(entity_types::BAT, the_middle_of_the_spot(), Mob::fresh());
    lay(&mut region, &bat);
    let touch = [
        f64::from(SPOT.x) + 1.0 / 16.0,
        f64::from(SPOT.y),
        f64::from(SPOT.z) + 1.0 / 16.0,
        f64::from(SPOT.x) + 15.0 / 16.0,
        f64::from(SPOT.y) + 0.25,
        f64::from(SPOT.z) + 15.0 / 16.0,
    ];
    assert_eq!(region.pressing(&touch, false, None), 0);
    let pig = mob_at(entity_types::PIG, the_middle_of_the_spot(), Mob::fresh());
    let pig = Entity {
        key: EntityId(41),
        uuid: uuid::Uuid::from_u128(41),
        ..pig
    };
    lay(&mut region, &pig);
    assert_eq!(region.pressing(&touch, false, None), 1);
    assert_eq!(region.pressing(&touch, true, None), 1);
}

/// A mob in tripwire trips it, and the hooks at its ends power what they hang on.
#[test]
fn a_pig_in_tripwire_trips_it() {
    let tripwire = state(
        "minecraft:tripwire[attached=false,disarmed=false,east=false,north=false,powered=false,south=false,west=false]",
    );
    let mut region = pen(tripwire);
    lay(
        &mut region,
        &mob_at(entity_types::PIG, the_middle_of_the_spot(), Mob::fresh()),
    );
    for tick in 0..40 {
        idle(&mut region);
        assert_eq!(
            block_at(&region, SPOT).and_then(|wire| wire.property("powered")),
            Some("true"),
            "the tripwire is down {tick} ticks on"
        );
    }
}

/// A mob standing on redstone ore lights it (`RedStoneOreBlock.stepOn`, which only an
/// entity stepping carefully does not).
#[test]
fn a_pig_standing_on_redstone_ore_lights_it() {
    let mut region = ticking_with(&[(LAMP, blocks::REDSTONE_ORE)]);
    lay(
        &mut region,
        &mob_at(entity_types::PIG, the_middle_of_the_spot(), Mob::fresh()),
    );
    // Its first tick begins in the air as far as its body knows; the second on the ore.
    idle(&mut region);
    idle(&mut region);
    assert_eq!(
        block_at(&region, LAMP),
        Some(state("minecraft:redstone_ore[lit=true]"))
    );
}

/// The walk of an entity that ignores block triggers (a bat) through an unpowered plate
/// or tripwire presses neither (`TripWireBlock.checkPressed` with a list of only it
/// finds nothing pressing; the plate's count leaves it out); the same walk of one that
/// does not presses both.
#[test]
fn a_bat_s_walk_through_a_plate_or_tripwire_presses_neither() {
    let tripwire = state(
        "minecraft:tripwire[attached=false,disarmed=false,east=false,north=false,powered=false,south=false,west=false]",
    );
    for (block, pressed) in [
        (
            state("minecraft:oak_pressure_plate[powered=false]"),
            state("minecraft:oak_pressure_plate[powered=true]"),
        ),
        (tripwire, set_flag(tripwire, "powered", true)),
    ] {
        for ignores in [true, false] {
            let mut region = pen(block);
            let [x, y, z] = the_middle_of_the_spot();
            let toucher = crate::region::pressed::Toucher {
                key: MOB,
                bounds: crate::physics::aabb::Aabb::new(
                    [x - 0.25, y, z - 0.25],
                    [x + 0.25, y + 0.9, z + 0.25],
                ),
                living: true,
                ignores,
            };
            let mut output = TickOutput::default();
            region.entity_touched(SPOT, &toucher, &mut output);
            let expected = if ignores { block } else { pressed };
            assert_eq!(
                block_at(&region, SPOT),
                Some(expected),
                "ignores: {ignores}"
            );
        }
    }
}

/// At a line, a mob of the copy across it whose box is on the region's own stone plate
/// is counted by the plate as a living thing, so that its looks again keep it down
/// (`docs/groundwork/n6-plan.md`, section 6 item 6); a bat there is not.
#[test]
fn a_mob_across_a_line_counts_on_a_stone_plate_and_a_bat_does_not() {
    let mut region = crate::region::tnt::tests::west_of_a_line(0x0EFF_EC75, |_| None);
    let plate = BlockPos::new(15, -60, 0);
    let touch = [
        f64::from(plate.x) + 1.0 / 16.0,
        f64::from(plate.y),
        f64::from(plate.z) + 1.0 / 16.0,
        f64::from(plate.x) + 15.0 / 16.0,
        f64::from(plate.y) + 0.25,
        f64::from(plate.z) + 15.0 / 16.0,
    ];
    let across = |kind, mob| {
        let entity = mob_at(kind, [16.15, -60.0, 0.5], mob);
        assert_eq!(entity.chunk(), ChunkPos::new(1, 0));
        entity
    };
    let bat = across(entity_types::BAT, Mob::fresh());
    region
        .copies
        .get_mut(&bat.chunk())
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(bat));
    assert_eq!(region.pressing(&touch, true, None), 0);
    let zombie = Entity {
        key: EntityId(41),
        uuid: uuid::Uuid::from_u128(41),
        ..across(entity_types::ZOMBIE, zombie())
    };
    region
        .copies
        .get_mut(&zombie.chunk())
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(zombie));
    assert_eq!(region.pressing(&touch, true, None), 1);
}
