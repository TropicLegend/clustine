use clustine_world::{Entity, EntityData, EntityField, EntityId, Entry, TypeState};

use super::Mob;
use super::part::{self, PartError, body_ids, mind_ids};

/// A cow in a chunk's record, holding `mob`.
fn entity_of(mob: &Mob) -> Entity {
    Entity {
        portal: Default::default(),
        key: EntityId(7),
        kind: clustine_data::entity_types::COW as u16,
        uuid: uuid::Uuid::from_u128(7),
        position: [0.5, -60.0, 0.5],
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

fn healthy(health: f32) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = health;
    mob
}

#[test]
fn a_fresh_mob_keeps_no_part_and_a_part_set_is_its_one_entry() {
    assert_eq!(
        Mob::fresh().encode(),
        EntityData::Mob {
            body: Vec::new(),
            mind: Vec::new(),
            state: TypeState::default(),
        }
    );
    let mob = healthy(10.0);
    let EntityData::Mob { body, mind, .. } = mob.encode() else {
        unreachable!();
    };
    assert_eq!(body.len(), 1);
    assert_eq!((body[0].id, body[0].version), (body_ids::VITALS, 1));
    assert!(mind.is_empty());
    assert_eq!(Mob::decode(&mob.encode()), Ok(mob));
}

#[test]
fn a_tick_s_changes_are_the_parts_it_changed_and_no_other() {
    let mut before = healthy(10.0);
    before.body.head.head_yaw = 5.0;
    before.body.flags.persistent = true;
    let record = before.encode();

    let mut after = before.clone();
    after.body.vitals.health = 9.0;
    let changes = after.changes_from(&record);
    assert_eq!(
        changes,
        [EntityField::BodyEntry(part::encode(&after.body.vitals))]
    );

    // A part back to what a fresh one is leaves the record.
    let mut after = before.clone();
    after.body.flags = Default::default();
    assert_eq!(
        after.changes_from(&record),
        [EntityField::BodyGone(body_ids::FLAGS)]
    );

    // Its type's state, where it changed.
    let mut after = before.clone();
    after.state = TypeState {
        version: 1,
        bytes: vec![3],
    };
    assert_eq!(
        after.changes_from(&record),
        [EntityField::TypeState(after.state.clone())]
    );
    assert_eq!(before.changes_from(&record), []);
}

#[test]
fn the_fields_of_a_tick_s_changes_make_the_record_the_mob_s() {
    let before = healthy(10.0);
    let mut entity = entity_of(&before);
    let mut after = before.clone();
    after.body.vitals.health = 4.0;
    after.body.idle.ambient_since = 12;
    after.mind.slots.push(Entry {
        id: mind_ids::FIRST_GOAL + 2,
        version: 1,
        bytes: vec![7],
    });
    for field in after.changes_from(&entity.data) {
        field.set(&mut entity);
    }
    assert_eq!(entity.data, after.encode());
}

#[test]
fn a_part_of_a_version_this_build_does_not_know_leaves_the_mob_unread() {
    let EntityData::Mob {
        mut body,
        mind,
        state,
    } = healthy(1.0).encode()
    else {
        unreachable!();
    };
    body[0].version = 9;
    assert_eq!(
        Mob::decode(&EntityData::Mob { body, mind, state }),
        Err(PartError::Unreadable {
            id: body_ids::VITALS,
            version: 9
        })
    );
    let garbled = EntityData::Mob {
        body: vec![Entry {
            id: body_ids::VITALS,
            version: 1,
            bytes: vec![],
        }],
        mind: Vec::new(),
        state: TypeState::default(),
    };
    assert!(Mob::decode(&garbled).is_err());
}
