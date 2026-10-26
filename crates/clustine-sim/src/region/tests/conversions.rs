//! A mob turned into another where it is (`Mob.convertTo`, `ConversionType.SINGLE`;
//! `docs/groundwork/n7k-brief.md`, section 11.5): what the new mob takes of the old one,
//! the order clients are told in, and its vehicle and passenger.

use clustine_data::{entity_types, items};
use clustine_world::{Entity, EntityChange, EntityId, ItemStack};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;

const OLD: EntityId = EntityId(40);
const CARRIER: EntityId = EntityId(41);
const RIDER: EntityId = EntityId(42);

fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mob: Mob) -> Entity {
    let kind = kind as u16;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 30.0,
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

fn healthy(kind: i32) -> Mob {
    let kind = kind as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob
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

#[test]
fn a_converted_mob_keeps_its_equipment_flags_and_seats_but_not_its_health() {
    let mut region = on_ticking_floor_with(&[]);
    let mut old = healthy(entity_types::ZOMBIE);
    old.body.vitals.health = 5.0;
    old.body.flags.persistent = true;
    old.body.flags.left_handed = true;
    old.body.equipment.slots[0] = Some(ItemStack {
        item: items::IRON_SWORD,
        count: 1,
    });
    old.body.equipment.drop_chances = Some([0.5; 8]);
    lay(
        &mut region,
        &mob_at(
            CARRIER,
            entity_types::CHICKEN,
            [4.5, -60.0, 4.5],
            healthy(entity_types::CHICKEN),
        ),
    );
    lay(
        &mut region,
        &mob_at(OLD, entity_types::ZOMBIE, [4.5, -60.0, 4.5], old),
    );
    lay(
        &mut region,
        &mob_at(
            RIDER,
            entity_types::CHICKEN,
            [4.5, -60.0, 4.5],
            healthy(entity_types::CHICKEN),
        ),
    );
    let mut scratch = TickOutput::default();
    assert!(region.start_riding(OLD, CARRIER, true, &mut scratch));
    assert!(region.start_riding(RIDER, OLD, true, &mut scratch));

    let mut output = TickOutput::default();
    let drowned = entity_types::DROWNED as u16;
    region.convert_mob_to(OLD, drowned, true, Some(1040), &mut output);

    assert!(record(&region, OLD).is_none(), "the old one is gone");
    let new = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .find(|entity| entity.kind == drowned)
        .cloned()
        .expect("a drowned is made");
    assert_ne!(new.key, OLD, "a new key");
    assert_eq!(new.yaw, 30.0);
    let mob = Mob::decode(&new.data).unwrap();
    assert_eq!(
        mob.body.equipment.slots[0].map(|stack| stack.item),
        Some(items::IRON_SWORD)
    );
    assert_eq!(mob.body.equipment.drop_chance(0), 0.5);
    assert!(mob.body.flags.persistent && mob.body.flags.left_handed);
    let full = supplied(drowned, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    assert_eq!(
        mob.body.vitals.health, full,
        "its own health, not the old one's"
    );
    assert_eq!(
        new.riding.vehicle,
        Some(CARRIER),
        "it rides the old one's vehicle"
    );
    assert_eq!(
        new.riding
            .passengers
            .iter()
            .map(|seat| seat.key)
            .collect::<Vec<_>>(),
        [RIDER],
        "the old one's passenger rides it"
    );
    assert_eq!(
        record(&region, RIDER).unwrap().riding.vehicle,
        Some(new.key)
    );
    // Clients are told of the new one before the old one goes, and hear the level event.
    let spawned = output
        .events
        .iter()
        .position(
            |event| matches!(event, RegionEvent::EntitySpawned(state) if state.entity == new.key),
        )
        .expect("the new one is shown");
    let gone = output
        .entity_changes
        .iter()
        .position(|(_, change)| matches!(change, EntityChange::Gone(key) if *key == OLD))
        .expect("the old one goes");
    let added = output
        .entity_changes
        .iter()
        .position(
            |(_, change)| matches!(change, EntityChange::Whole(entity) if entity.key == new.key),
        )
        .expect("the new one is added");
    assert!(added < gone, "added before the old one goes");
    let _ = spawned;
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::LevelEvent { kind: 1040, .. }))
    );
}

#[test]
fn without_keeping_equipment_the_new_mob_wears_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    let mut old = healthy(entity_types::ZOMBIE);
    old.body.equipment.slots[5] = Some(ItemStack {
        item: items::IRON_HELMET,
        count: 1,
    });
    lay(
        &mut region,
        &mob_at(OLD, entity_types::ZOMBIE, [4.5, -60.0, 4.5], old),
    );
    let mut output = TickOutput::default();
    let husk = entity_types::HUSK as u16;
    region.convert_mob_to(OLD, husk, false, None, &mut output);
    let new = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .find(|entity| entity.kind == husk)
        .cloned()
        .expect("made");
    let mob = Mob::decode(&new.data).unwrap();
    assert!(mob.body.equipment.slots.iter().all(Option::is_none));
    assert!(
        !output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::LevelEvent { .. }))
    );
}

/// `IronGolem.doHurtTarget`: a hit that takes flings its target up by 0.4 times what its
/// knockback resistance leaves, after the hurt's own knockback; a hit that does not take
/// flings nothing.
#[test]
fn a_flung_hit_raises_its_target_only_where_it_took() {
    let golem = entity_types::IRON_GOLEM as u16;
    let hit_with = |fling: Option<f32>| {
        let mut region = on_ticking_floor_with(&[]);
        lay(
            &mut region,
            &mob_at(
                OLD,
                entity_types::COW,
                [4.5, -60.0, 4.5],
                healthy(entity_types::COW),
            ),
        );
        let hit = |region: &mut Region, amount| {
            region.hit_by_mob(
                EntityId(77),
                golem,
                [6.5, -60.0, 4.5],
                OLD,
                amount,
                (None, fling),
                None,
                &mut TickOutput::default(),
            );
            record(region, OLD).unwrap().motion
        };
        let first = hit(&mut region, 1.0);
        // Within its hurt's cooldown a weaker hit does not take: no fling.
        let second = hit(&mut region, 0.5);
        (first, second)
    };
    let (plain, plain_again) = hit_with(None);
    let (flung, flung_again) = hit_with(Some(0.4));
    // After the hurt's own knockback, the fling: 0.4 more upwards, a cow resisting none.
    assert_eq!(flung[0], plain[0]);
    assert_eq!(flung[1], plain[1] + f64::from(0.4_f32));
    assert_eq!(flung_again, flung, "a hit that did not take flings nothing");
    assert_eq!(plain_again, plain);
}
