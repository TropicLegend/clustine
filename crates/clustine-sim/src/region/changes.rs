//! A tick's changes to entities, merged before they leave the region (N7f's
//! measurement, `docs/groundwork/n7-plan.md`, section 11): a mob in a crowd is pushed by
//! each neighbour, and each push is committed as it is made, so a tick made two or three
//! changes of one animal, each with its key, most of them its motion again. Every field
//! of a change sets a definite value, so the changes of one entity in one chunk that no
//! `Whole` or `Gone` of it comes between say as much as one change of the last value of
//! each field: the record the log and a copier make of them is the same. The fields of
//! the merged change keep the order of their last setting, and a `Data`, which sets the
//! body, the mind and the type's state with the rest, supersedes those set before it.

use std::collections::BTreeMap;

use clustine_world::{ChunkPos, EntityChange, EntityField, EntityId};

/// Merges the `Set`s of one entity in one chunk in `changes`, where no other change of
/// it comes between, into the first of them.
pub(super) fn merge(changes: &mut Vec<(ChunkPos, EntityChange)>) {
    let mut merged: Vec<(ChunkPos, EntityChange)> = Vec::with_capacity(changes.len());
    // Where in `merged` the `Set` of an entity and chunk is that later ones join.
    let mut open: BTreeMap<(EntityId, ChunkPos), usize> = BTreeMap::new();
    for (chunk, change) in changes.drain(..) {
        match change {
            EntityChange::Set(key, fields) => {
                if let Some(&at) = open.get(&(key, chunk))
                    && let (_, EntityChange::Set(_, earlier)) = &mut merged[at]
                {
                    for field in fields {
                        set_after(earlier, field);
                    }
                    continue;
                }
                open.insert((key, chunk), merged.len());
                merged.push((chunk, EntityChange::Set(key, fields)));
            }
            other => {
                let key = other.key();
                open.retain(|(open_key, _), _| *open_key != key);
                merged.push((chunk, other));
            }
        }
    }
    *changes = merged;
}

/// Adds `field` after `fields`, dropping what it sets anew.
fn set_after(fields: &mut Vec<EntityField>, field: EntityField) {
    let whole_data = matches!(field, EntityField::Data(_));
    let what = setting(&field);
    fields.retain(|earlier| {
        let superseded = whole_data
            && matches!(
                earlier,
                EntityField::BodyEntry(_)
                    | EntityField::BodyGone(_)
                    | EntityField::MindEntry(_)
                    | EntityField::MindGone(_)
                    | EntityField::TypeState(_)
            );
        !superseded && setting(earlier) != what
    });
    fields.push(field);
}

/// What a field sets: its kind, and a part of the body or the mind by its id, which an
/// entry and its removal both set.
fn setting(field: &EntityField) -> (u8, u16) {
    match field {
        EntityField::Position(_) => (0, 0),
        EntityField::Motion(_) => (1, 0),
        EntityField::Rotation { .. } => (2, 0),
        EntityField::OnGround(_) => (3, 0),
        EntityField::Lived(_) => (4, 0),
        EntityField::FreshAt(_) => (5, 0),
        EntityField::Hops(_) => (6, 0),
        EntityField::Data(_) => (7, 0),
        EntityField::BodyEntry(entry) => (8, entry.id),
        EntityField::BodyGone(id) => (8, *id),
        EntityField::MindEntry(entry) => (9, entry.id),
        EntityField::MindGone(id) => (9, *id),
        EntityField::TypeState(_) => (10, 0),
        EntityField::FallDistance(_) => (11, 0),
        EntityField::FireTicks(_) => (12, 0),
        EntityField::Riding(_) => (13, 0),
        EntityField::Portal(_) => (14, 0),
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::{Entity, EntityData, Entry, TypeState};

    use super::*;

    const HERE: ChunkPos = ChunkPos::new(0, 0);
    const THERE: ChunkPos = ChunkPos::new(1, 0);

    fn entry(id: u16, byte: u8) -> Entry {
        Entry {
            id,
            version: 1,
            bytes: vec![byte],
        }
    }

    fn mob(key: u64) -> Entity {
        Entity {
            portal: Default::default(),
            key: EntityId(key),
            kind: 0,
            uuid: uuid::Uuid::from_u128(u128::from(key)),
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
            data: EntityData::Mob {
                body: vec![entry(1, 1), entry(4, 4)],
                mind: vec![entry(2, 2)],
                state: TypeState::default(),
            },
        }
    }

    /// The records `changes` leave of `entities`, applied one after another.
    fn applied(entities: &[Entity], changes: &[(ChunkPos, EntityChange)]) -> Vec<Entity> {
        let mut records: Vec<Entity> = entities.to_vec();
        for (_, change) in changes {
            match change {
                EntityChange::Set(key, fields) => {
                    if let Some(record) = records.iter_mut().find(|record| record.key == *key) {
                        for field in fields {
                            field.set(record);
                        }
                    }
                }
                EntityChange::Whole(entity) => {
                    records.retain(|record| record.key != entity.key);
                    records.push(entity.clone());
                }
                EntityChange::Gone(key) => records.retain(|record| record.key != *key),
            }
        }
        records.sort_by_key(|record| record.key);
        records
    }

    #[test]
    fn the_pushes_of_a_tick_become_one_change_with_the_last_motion() {
        let mut changes = vec![
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::Motion([0.1, 0.0, 0.0])]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(2), vec![EntityField::Motion([0.0, 0.0, 0.1])]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::Motion([0.2, 0.0, 0.0])]),
            ),
            (
                HERE,
                EntityChange::Set(
                    EntityId(1),
                    vec![
                        EntityField::Position([0.7, -60.0, 0.5]),
                        EntityField::Motion([0.15, 0.0, 0.0]),
                    ],
                ),
            ),
        ];
        let entities = [mob(1), mob(2)];
        let before = applied(&entities, &changes);
        merge(&mut changes);
        assert_eq!(applied(&entities, &changes), before);
        assert_eq!(
            changes,
            [
                (
                    HERE,
                    EntityChange::Set(
                        EntityId(1),
                        vec![
                            EntityField::Position([0.7, -60.0, 0.5]),
                            EntityField::Motion([0.15, 0.0, 0.0]),
                        ]
                    )
                ),
                (
                    HERE,
                    EntityChange::Set(EntityId(2), vec![EntityField::Motion([0.0, 0.0, 0.1])])
                ),
            ]
        );
    }

    #[test]
    fn a_whole_or_a_gone_between_keeps_the_changes_apart() {
        let moved = mob(1);
        let mut changes = vec![
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::Lived(3)]),
            ),
            (HERE, EntityChange::Gone(EntityId(1))),
            (THERE, EntityChange::Whole(moved.clone())),
            (
                THERE,
                EntityChange::Set(EntityId(1), vec![EntityField::Lived(4)]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::Lived(5)]),
            ),
        ];
        let expected = changes.clone();
        merge(&mut changes);
        assert_eq!(changes, expected);
    }

    #[test]
    fn a_part_set_and_dropped_is_dropped_and_data_supersedes_the_parts_before_it() {
        let mut data = mob(1).data;
        if let EntityData::Mob { body, .. } = &mut data {
            body.push(entry(9, 9));
        }
        let mut changes = vec![
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::BodyEntry(entry(4, 40))]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::BodyGone(4)]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::MindEntry(entry(2, 20))]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::Data(data)]),
            ),
            (
                HERE,
                EntityChange::Set(EntityId(1), vec![EntityField::BodyEntry(entry(1, 10))]),
            ),
        ];
        let entities = [mob(1)];
        let before = applied(&entities, &changes);
        merge(&mut changes);
        assert_eq!(applied(&entities, &changes), before);
        let [(_, EntityChange::Set(_, fields))] = &changes[..] else {
            panic!("one change: {changes:?}");
        };
        assert!(matches!(
            fields[..],
            [EntityField::Data(_), EntityField::BodyEntry(_)]
        ));
    }
}
