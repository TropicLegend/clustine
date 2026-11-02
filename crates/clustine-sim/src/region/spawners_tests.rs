//! What a spawner's data reads and writes as (`BaseSpawner.load` and `save`, n7
//! BaseSpawner 621–745), against the saved form the features write for a dungeon's
//! spawner, which the judge of features compares with the game's.

use super::*;

/// A new spawner given a zombie writes what the game saves of a dungeon's spawner, byte
/// for byte (`MonsterRoomFeature`'s `setEntityId` on a new block entity).
#[test]
fn a_new_spawner_given_a_mob_saves_what_a_dungeon_s_spawner_saves() {
    let dungeon =
        clustine_features::features::ground::monster_room::spawner_data("minecraft:zombie");
    let written = Spawner::read(&[]).with_type("minecraft:zombie").write();
    assert_eq!(written, dungeon.data);
}

/// A dungeon's spawner reads as the game loads it and writes back unchanged.
#[test]
fn a_dungeon_s_spawner_reads_back_as_it_was_saved() {
    let dungeon =
        clustine_features::features::ground::monster_room::spawner_data("minecraft:cave_spider");
    let spawner = Spawner::read(&dungeon.data);
    assert_eq!(spawner.delay, 20);
    assert_eq!(spawner.min_spawn_delay, 200);
    assert_eq!(spawner.max_spawn_delay, 800);
    assert_eq!(spawner.spawn_count, 4);
    assert_eq!(spawner.max_nearby_entities, 6);
    assert_eq!(spawner.required_player_range, 16);
    assert_eq!(spawner.spawn_range, 4);
    assert!(
        spawner.potentials.is_empty(),
        "a saved empty list stays empty"
    );
    let next = spawner.next.clone().expect("its next spawn");
    assert_eq!(
        next.kind(),
        Some(clustine_data::entity_types::CAVE_SPIDER as u16)
    );
    assert!(next.id_alone());
    assert_eq!(spawner.write(), dungeon.data);
}

/// Data with no potentials is loaded with the next spawn as its one potential of weight
/// 1 (`load`'s `orElseGet`), and a short of any numeric tag reads as the game's
/// `getShortOr` reads it.
#[test]
fn a_spawner_saved_without_potentials_loads_its_next_spawn_as_its_only_one() {
    let next = SpawnData::of_type("minecraft:skeleton");
    let entries = vec![
        Raw {
            tag: TAG_INT,
            name: DELAY.to_owned(),
            payload: 70_000_i32.to_be_bytes().to_vec(),
        },
        Raw {
            tag: TAG_COMPOUND,
            name: SPAWN_DATA.to_owned(),
            payload: next.payload.clone(),
        },
    ];
    let mut data = vec![TAG_COMPOUND];
    data.extend(compound_payload(&entries));
    let spawner = Spawner::read(&data);
    // 70 000 as a short.
    assert_eq!(spawner.delay, 4_464);
    assert_eq!(spawner.potentials, vec![Potential::new(next.clone(), 1)]);
    assert!(spawner.others.is_empty());
    // And it saves them, with everything else at its default.
    let again = Spawner::read(&spawner.write());
    assert_eq!(again, spawner);
}

/// The potentials are picked by one draw below their total weight, the first whose
/// weights so far pass it; a list of no weight draws nothing.
#[test]
fn a_potential_is_picked_by_its_weight() {
    let zombie = SpawnData::of_type("minecraft:zombie");
    let skeleton = SpawnData::of_type("minecraft:skeleton");
    let mut spawner = Spawner::read(&[]);
    spawner.potentials = vec![
        Potential::new(zombie.clone(), 1),
        Potential::new(skeleton.clone(), 3),
    ];
    let mut picked = [0; 2];
    for seed in 0..400 {
        let mut draws = Random::of_block(seed, BlockPos::new(0, 0, 0), 0, DRAWS);
        let start = draws.clone();
        let next = spawner.pick(&mut draws).expect("a weighted list");
        assert_eq!(draws.draws_since(&start), 1);
        picked[usize::from(next == skeleton)] += 1;
    }
    assert!(picked[0] > 60 && picked[1] > 240, "{picked:?}");
    spawner.potentials = vec![Potential::new(zombie, 0)];
    let mut draws = Random::of_block(1, BlockPos::new(0, 0, 0), 0, DRAWS);
    let start = draws.clone();
    assert_eq!(spawner.pick(&mut draws), None);
    assert_eq!(draws.draws_since(&start), 0);
}

/// The potentials read back from what they write, and a list with a negative weight is
/// no list (`NON_NEGATIVE_INT`), so that the next spawn stands in for it.
#[test]
fn potentials_read_back_and_a_negative_weight_spoils_the_list() {
    let mut spawner = Spawner::read(&[]).with_type("minecraft:pig");
    spawner.potentials = vec![
        Potential::new(SpawnData::of_type("minecraft:cow"), 2),
        Potential::new(SpawnData::of_type("minecraft:pig"), 5),
    ];
    let read = Spawner::read(&spawner.write());
    assert_eq!(read.potentials, spawner.potentials);
    spawner.potentials = vec![Potential::new(SpawnData::of_type("minecraft:cow"), -1)];
    let read = Spawner::read(&spawner.write());
    assert_eq!(
        read.potentials,
        vec![Potential::new(SpawnData::of_type("minecraft:pig"), 1)]
    );
}

/// A bare name is the game's own type, an unknown one is none, and an entity with more
/// than its `id` is not finalised.
#[test]
fn a_spawn_s_type_is_read_from_its_entity_s_id() {
    assert_eq!(
        SpawnData::of_type("zombie").kind(),
        Some(clustine_data::entity_types::ZOMBIE as u16)
    );
    assert_eq!(SpawnData::of_type("minecraft:nothing").kind(), None);
    assert_eq!(SpawnData::empty().kind(), None);
    let mut entity = vec![TAG_STRING];
    nbt_put_string(&mut entity, "id");
    nbt_put_string(&mut entity, "minecraft:zombie");
    entity.push(TAG_STRING);
    nbt_put_string(&mut entity, "CustomName");
    nbt_put_string(&mut entity, "Bob");
    entity.push(TAG_END);
    let named = SpawnData::of_entries(&[Raw {
        tag: TAG_COMPOUND,
        name: "entity".to_owned(),
        payload: entity,
    }]);
    assert_eq!(
        named.kind(),
        Some(clustine_data::entity_types::ZOMBIE as u16)
    );
    assert!(!named.id_alone());
}

/// Custom rules read as numbers or as ranges, the missing one 0 to 15.
#[test]
fn custom_rules_read_a_number_or_a_range() {
    let mut range = vec![TAG_INT];
    nbt_put_string(&mut range, "min_inclusive");
    range.extend_from_slice(&3_i32.to_be_bytes());
    range.push(TAG_INT);
    nbt_put_string(&mut range, "max_inclusive");
    range.extend_from_slice(&9_i32.to_be_bytes());
    range.push(TAG_END);
    let mut rules = vec![TAG_COMPOUND];
    nbt_put_string(&mut rules, "block_light_limit");
    rules.extend(range);
    rules.push(TAG_END);
    let mut entity = vec![TAG_STRING];
    nbt_put_string(&mut entity, "id");
    nbt_put_string(&mut entity, "minecraft:zombie");
    entity.push(TAG_END);
    let data = SpawnData::of_entries(&[
        Raw {
            tag: TAG_COMPOUND,
            name: "entity".to_owned(),
            payload: entity,
        },
        Raw {
            tag: TAG_COMPOUND,
            name: "custom_spawn_rules".to_owned(),
            payload: rules,
        },
    ]);
    let custom = data.custom_rules().expect("rules");
    assert_eq!(custom.block, (3, 9));
    assert_eq!(custom.sky, (0, 15));
    assert!(custom.allows(5, 15));
    assert!(!custom.allows(10, 0));
    assert_eq!(SpawnData::of_type("minecraft:zombie").custom_rules(), None);
}
