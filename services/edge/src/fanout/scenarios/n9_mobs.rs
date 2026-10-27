//! Tests of what the edge shows of N9's mobs and of the wind's burst (part S12c,
//! `docs/groundwork/n9-plan.md`, section 10), from reports fed by hand as a region would
//! send them: each new type's values at its add and as they change, a guardian's beam by
//! its target's wire id, and the burst's explosion packet with its gusts, its sound and
//! each player's push.
//!
//! The values are the game's (`defineSynchedData` of `Vex`, `AbstractCubeMob` after
//! `AgeableMob`, `Enderman`, `Phantom`, `SpellcasterIllager`, `Pillager` and `Witch`
//! after `Raider`, and `Guardian`); the bytes are those of the one serializer new to N9
//! among them, the enderman's `OPTIONAL_BLOCK_STATE` (the sixteenth registered, n9
//! EntityDataSerializers).

use clustine_protocol::packets::play::particle;
use clustine_sim::api::{ExplosionKind, Tracked};

use super::*;

/// The player's own entity.
const WATCHER: EntityId = EntityId(5);

/// One player at the spawn, shown `HOME` from the west.
async fn at_home() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), WATCHER).await;
    edge.quiet().await;
    let there = vec![EntityState {
        entity: WATCHER,
        kind: EntityKind::Player {
            player: player(1),
            name: "Player1".to_owned(),
        },
        pose: Pose::at(SPAWN),
    }];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    (edge, client)
}

fn state(key: u64, kind: i32, data: EntityData) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: kind as u16,
            uuid: Uuid::from_u128(u128::from(key) + 7_000),
            motion: [0.0; 3],
            hops: 0,
            lived: 0,
            data,
            riding: Default::default(),
        },
        pose: Pose::at(Vec3::new(2.5, -60.0, 2.5)),
    }
}

fn report(key: u64, data: EntityData) -> Tracked {
    Tracked {
        entity: EntityId(key),
        chunk: HOME,
        hops: 0,
        position: [2.5, -60.0, 2.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        flags: 0,
        data: Some(data),
    }
}

/// What `client` was sent of entity data because of the region's report of `tick` with
/// `events`.
async fn told(
    edge: &mut Harness,
    client: &mut Client,
    tick: u64,
    events: Vec<RegionEvent>,
) -> Vec<SetEntityData> {
    let data = client.entity_data.len();
    edge.tell(WEST, WorkerToEdge::TickDelta { tick, events });
    edge.settle(WEST).await;
    edge.sync(client).await;
    client.entity_data[data..].to_vec()
}

/// The values of a type's own (from index 16) among `values`.
fn own(values: &[(u8, EntityDataValue)]) -> Vec<(u8, EntityDataValue)> {
    values
        .iter()
        .filter(|(index, _)| *index >= 16)
        .cloned()
        .collect()
}

/// The data `client` was sent with the add of `key`.
fn added(client: &Client, key: u64) -> Vec<(u8, EntityDataValue)> {
    let id = client.naming(key)[0].1;
    assert!(
        client.spawns.iter().any(|spawn| spawn.entity_id == id),
        "{key} was added"
    );
    client
        .entity_data
        .iter()
        .find(|data| data.entity_id == id)
        .map(|data| data.values.clone())
        .unwrap_or_default()
}

/// Each N9 type and what of its own a client is sent where its values are changed from
/// their defaults.
fn changed_values() -> Vec<(i32, Vec<(u8, EntityDataValue)>)> {
    let dirt = i32::from(clustine_data::blocks::DIRT.0);
    vec![
        // `Vex.FLAG_IS_CHARGING`.
        (entity_types::VEX, vec![(16, EntityDataValue::Byte(1))]),
        // `ID_SIZE`; the ageable mob's baby (16) and age lock (17) stay false.
        (entity_types::SLIME, vec![(18, EntityDataValue::Int(4))]),
        (
            entity_types::ENDERMAN,
            vec![
                (16, EntityDataValue::OptionalBlockState(dirt)),
                (17, EntityDataValue::Boolean(true)),
                (18, EntityDataValue::Boolean(true)),
            ],
        ),
        (entity_types::PHANTOM, vec![(16, EntityDataValue::Int(3))]),
        // `IllagerSpell.FANGS`.
        (entity_types::EVOKER, vec![(17, EntityDataValue::Byte(2))]),
        (
            entity_types::PILLAGER,
            vec![(17, EntityDataValue::Boolean(true))],
        ),
        (
            entity_types::WITCH,
            vec![(17, EntityDataValue::Boolean(true))],
        ),
    ]
}

/// Each type added as a fresh one is sent nothing of its own (every value its default);
/// changed, it is sent what changed, once; added changed, it is sent the same with its
/// add.
#[tokio::test]
async fn each_new_mob_is_sent_its_own_values_at_its_add_and_as_they_change() {
    let (mut edge, mut client) = at_home().await;
    let mut tick = 10;
    for (number, (kind, expected)) in (0u64..).zip(changed_values()) {
        let fresh_key = (1 << 32) + 2 * number + 1;
        let changed_key = fresh_key + 1;
        let fresh = clustine_sim::shown::n9_mob_data_changed(0, None);
        let changed = clustine_sim::shown::n9_mob_data_changed(kind as u16, None);
        let events = vec![
            RegionEvent::EntitySpawned(state(fresh_key, kind, fresh)),
            RegionEvent::EntitySpawned(state(changed_key, kind, changed.clone())),
        ];
        told(&mut edge, &mut client, tick, events).await;
        tick += 1;
        assert_eq!(own(&added(&client, fresh_key)), [], "a fresh {kind}");
        assert_eq!(
            own(&added(&client, changed_key)),
            expected,
            "a changed {kind}"
        );

        // The fresh one changes, and is reported so for a few ticks: sent once.
        let id = client.naming(fresh_key)[0].1;
        let mut sent = Vec::new();
        for _ in 0..4 {
            let events = vec![
                RegionEvent::BlockEventsBegin,
                RegionEvent::EntityTracked(report(fresh_key, changed.clone())),
            ];
            let data = told(&mut edge, &mut client, tick, events).await;
            tick += 1;
            sent.extend(
                data.into_iter()
                    .filter(|data| data.entity_id == id)
                    .map(|data| data.values),
            );
        }
        assert_eq!(sent, [expected], "the change of a {kind}");
    }
    edge.end().await;
}

/// An enderman that puts its block down is sent none (0), its scream and stare gone;
/// the block's state is written as `OPTIONAL_BLOCK_STATE` (15) and the state's id.
#[tokio::test]
async fn an_enderman_putting_its_block_down_is_sent_none_by_the_new_serializer() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 40;
    let kind = entity_types::ENDERMAN;
    let carrying = clustine_sim::shown::n9_mob_data_changed(kind as u16, None);
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(state(key, kind, carrying))],
    )
    .await;
    let id = client.naming(key)[0].1;
    let fresh = clustine_sim::shown::n9_mob_data_changed(0, None);
    let mut sent = Vec::new();
    for tick in 11..14 {
        let events = vec![
            RegionEvent::BlockEventsBegin,
            RegionEvent::EntityTracked(report(key, fresh.clone())),
        ];
        let data = told(&mut edge, &mut client, tick, events).await;
        sent.extend(data.into_iter().filter(|data| data.entity_id == id));
    }
    let [put_down] = &sent[..] else {
        panic!("{sent:?}");
    };
    assert_eq!(
        put_down.values,
        [
            (16, EntityDataValue::OptionalBlockState(0)),
            (17, EntityDataValue::Boolean(false)),
            (18, EntityDataValue::Boolean(false)),
        ]
    );
    // Index 16, serializer 15, then the state's id as a VarInt: dirt's 10.
    let bytes = packets::encode(&SetEntityData {
        entity_id: 1,
        values: vec![(
            16,
            EntityDataValue::OptionalBlockState(i32::from(clustine_data::blocks::DIRT.0)),
        )],
    });
    let tail = [16, 15, 10, 0xFF];
    assert_eq!(bytes[bytes.len() - tail.len()..], tail[..]);
    edge.end().await;
}

/// A guardian moving with its beam on a mob no client is shown yet is sent the id the
/// mob is shown with later (`wire.of`), not 0.
#[tokio::test]
async fn a_guardians_beam_names_its_target_by_the_id_it_is_shown_with() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 50;
    let target = EntityId((1 << 32) + 51);
    let kind = entity_types::GUARDIAN;
    let beaming = clustine_sim::shown::n9_mob_data_changed(kind as u16, Some(target));
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(state(key, kind, beaming))],
    )
    .await;
    let values = own(&added(&client, key));
    let [
        (16, EntityDataValue::Boolean(true)),
        (17, EntityDataValue::Int(beamed)),
    ] = values[..]
    else {
        panic!("{values:?}");
    };
    assert_ne!(beamed, 0);
    assert_eq!(client.key(beamed), target.0);
    edge.end().await;
}

/// A wind burst's explosion: the gust emitters (large from a radius of 2), the breeze's
/// burst sound, no particles over the blocks, and the push of the player it names.
#[tokio::test]
async fn a_wind_burst_is_sent_as_an_explosion_of_gusts_with_its_push() {
    let (mut edge, mut client) = at_home().await;
    let burst = |radius: f32, knockback| RegionEvent::Explosion {
        centre: [2.5, -59.3, 0.5],
        radius,
        blocks: 900,
        knockback,
        large: radius >= 2.0,
        sound: true,
        kind: ExplosionKind::Wind,
    };
    let push = [0.5, 0.25, 0.0];
    let events = vec![
        RegionEvent::BlockEventsBegin,
        burst(4.0, vec![(WATCHER, push)]),
        burst(1.2, Vec::new()),
    ];
    told(&mut edge, &mut client, 10, events).await;
    let [large, small] = &client.explosions[..] else {
        panic!("{:?}", client.explosions);
    };
    assert_eq!(large.particle, particle::GUST_EMITTER_LARGE);
    assert_eq!(small.particle, particle::GUST_EMITTER_SMALL);
    for shown in [large, small] {
        assert_eq!(shown.sound, clustine_data::sounds::ENTITY_BREEZE_WIND_BURST);
        assert_eq!(shown.block_particles, []);
        assert!(shown.play_sound);
    }
    assert_eq!(large.knockback, Some(push));
    assert_eq!(large.block_count, 900);
    assert_eq!(small.knockback, None);
    edge.end().await;
}
