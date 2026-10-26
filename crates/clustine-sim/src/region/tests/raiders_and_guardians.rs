//! The witch, the illagers and the guardians in a region (`n7k-brief.md` 3.6 to 3.8):
//! their eggs make them holding what the game's hold, a guardian on land flops and in
//! water strolls, and its spikes hurt a mob that hits it while it keeps still, and not
//! while it moves.

use clustine_data::{BlockState, entity_types, items};
use clustine_world::{Entity, EntityChange, EntityId};

use super::*;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::hurt::DamageType;
use crate::mobs::types::{guardians, illagers};

const GUARDIAN_KEY: EntityId = EntityId(40);
const COW_KEY: EntityId = EntityId(42);

/// A mob of the type `kind` at `position` as `mob` has it, of its type's full health.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = 300;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
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

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
}

/// A pool of water from x, z = 4 to 11 and y = -61 to -57 in the home chunk, stone
/// under it and round it.
fn pool() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            blocks.push((BlockPos::new(x, -62, z), clustine_data::blocks::STONE));
            for y in -61..=-57 {
                let wall = x == 3 || x == 12 || z == 3 || z == 12;
                let state = if wall {
                    clustine_data::blocks::STONE
                } else {
                    clustine_data::blocks::WATER
                };
                blocks.push((BlockPos::new(x, y, z), state));
            }
        }
    }
    blocks
}

#[test]
fn each_egg_makes_its_raider_or_guardian_holding_what_the_game_s_holds() {
    for (kind, hand) in [
        (entity_types::WITCH, None),
        (entity_types::PILLAGER, Some(items::CROSSBOW)),
        (entity_types::VINDICATOR, Some(items::IRON_AXE)),
        (entity_types::EVOKER, None),
        (entity_types::GUARDIAN, None),
        (entity_types::ELDER_GUARDIAN, None),
    ] {
        let mut region = on_ticking_floor_with(&pool());
        let mut output = TickOutput::default();
        let key = region
            .spawn_mob(
                kind as u16,
                player(1),
                BlockPos::new(6, -61, 6),
                true,
                true,
                &mut output,
            )
            .expect("its egg makes it");
        let made = mob(&region, key);
        assert_eq!(
            made.body.equipment.slots[usize::from(slots::MAIN_HAND)].map(|stack| stack.item),
            hand,
            "{kind}"
        );
        assert_eq!(
            made.body.flags.persistent,
            kind == entity_types::ELDER_GUARDIAN,
            "{kind}"
        );
        // Ticked a while, each lives on (the raiders sink in the pool and float up).
        for _ in 0..40 {
            checked(&mut region, &TickInputs::default());
        }
        assert!(record(&region, key).is_some(), "{kind}");
    }
}

#[test]
fn a_guardian_on_land_flops_up_and_in_water_strolls() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            GUARDIAN_KEY,
            entity_types::GUARDIAN,
            [1.5, -60.0, 1.5],
            Mob::fresh(),
        ),
    );
    // Let it land, then see it leave the ground.
    let mut rose = false;
    for _ in 0..10 {
        let before = record(&region, GUARDIAN_KEY).unwrap();
        checked(&mut region, &TickInputs::default());
        let after = record(&region, GUARDIAN_KEY).unwrap();
        if before.on_ground && after.position[1] > before.position[1] {
            rose = true;
        }
    }
    assert!(rose, "a flop lifts it off the ground");

    let mut region = on_ticking_floor_with(&pool());
    lay(
        &mut region,
        &mob_at(
            GUARDIAN_KEY,
            entity_types::GUARDIAN,
            [7.5, -60.0, 7.5],
            Mob::fresh(),
        ),
    );
    let mut moved = false;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        if guardians::state_of(&mob(&region, GUARDIAN_KEY)).moving {
            moved = true;
            break;
        }
    }
    assert!(moved, "it strolls in water");
    assert!(guardians::state_of(&mob(&region, GUARDIAN_KEY)).in_water);
}

/// A guardian, moving or not, and a cow beside it; the cow hits the guardian.
fn cow_hits_guardian(moving: bool) -> Region {
    let mut region = on_ticking_floor_with(&pool());
    let mut guardian = Mob::fresh();
    guardians::update(&mut guardian, |own| own.moving = moving);
    lay(
        &mut region,
        &mob_at(
            GUARDIAN_KEY,
            entity_types::GUARDIAN,
            [7.5, -60.0, 7.5],
            guardian,
        ),
    );
    lay(
        &mut region,
        &mob_at(COW_KEY, entity_types::COW, [1.5, -60.0, 1.5], Mob::fresh()),
    );
    region.hit_by_mob(
        COW_KEY,
        entity_types::COW as u16,
        [1.5, -60.0, 1.5],
        GUARDIAN_KEY,
        1.0,
        (None, None),
        None,
        &mut TickOutput::default(),
    );
    region
}

#[test]
fn a_still_guardian_s_spikes_hurt_a_mob_that_hits_it_by_two_of_thorns() {
    let region = cow_hits_guardian(false);
    let cow = mob(&region, COW_KEY);
    assert_eq!(cow.body.vitals.health, 10.0 - 2.0);
    let thorns = DamageType::THORNS.id().unwrap();
    assert_eq!(
        cow.body
            .last_damage
            .last
            .map(|(kind, cause, _)| (kind, cause)),
        Some((thorns, Some(GUARDIAN_KEY))),
        "of thorns, by the guardian"
    );
    assert_eq!(mob(&region, GUARDIAN_KEY).body.vitals.health, 30.0 - 1.0);
    assert!(guardians::state_of(&mob(&region, GUARDIAN_KEY)).stroll_triggered);
}

#[test]
fn a_moving_guardian_s_spikes_hurt_nothing() {
    let region = cow_hits_guardian(true);
    assert_eq!(mob(&region, COW_KEY).body.vitals.health, 10.0);
    assert_eq!(mob(&region, GUARDIAN_KEY).body.vitals.health, 30.0 - 1.0);
}

#[test]
fn a_witch_from_its_egg_is_on_no_lead_and_its_raider_state_is_fresh() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let key = region
        .spawn_mob(
            entity_types::WITCH as u16,
            player(1),
            BlockPos::new(1, -61, 1),
            true,
            true,
            &mut output,
        )
        .expect("made");
    let own = illagers::state_of(&mob(&region, key));
    assert!(own.can_join_raid && !own.patrol_leader && own.drinking.is_none());
}
