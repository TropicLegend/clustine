//! Tests of what the edge shows of N9's projectiles (part S12b,
//! `docs/groundwork/n9-plan.md`, section 10), from reports fed by hand as a region would
//! send them: an arrow's, a trident's, a rocket's and a cloud's data, at their add and as
//! they change; the owner in a projectile's add packet, even one not shown yet; an arrow
//! updated with its position and angles at its type's interval and its motion with
//! them, a rocket at its own; a tipped arrow's fading and an arrow picked up.
//!
//! The values are the game's (`defineSynchedData` of `AbstractArrow`, `Arrow`,
//! `ThrownTrident`, `FireworkRocketEntity` and `AreaEffectCloud`, n9), and the bytes of
//! a cloud's particle `EntityDataSerializers.PARTICLE`'s (n7 EntityDataSerializers 562,
//! the seventeenth registered).

use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{EffectParticle, particle};
use clustine_sim::api::Tracked;
use clustine_world::components::{Component, PotionContents};
use clustine_world::projectiles::{Arrow, ArrowKind, Cloud, Firework, InGround, Pickup};
use clustine_world::{Double, Float};

use super::*;

/// The player's own entity, the shooter of most of these projectiles.
const SHOOTER: EntityId = EntityId(5);

/// One player at the spawn, shown `HOME` from the west.
async fn at_home() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), SHOOTER).await;
    edge.quiet().await;
    let there = vec![EntityState {
        entity: SHOOTER,
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

fn state(key: u64, kind: i32, data: EntityData, motion: [f64; 3]) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: kind as u16,
            uuid: Uuid::from_u128(u128::from(key) + 7_000),
            motion,
            hops: 0,
            lived: 0,
            data,
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(2.5, -58.0, 2.5),
            yaw: 30.0,
            pitch: -10.0,
            on_ground: false,
        },
    }
}

fn report(key: u64, position: [f64; 3], motion: [f64; 3], data: EntityData) -> Tracked {
    Tracked {
        entity: EntityId(key),
        chunk: HOME,
        hops: 0,
        position,
        motion,
        yaw: 30.0,
        pitch: -10.0,
        on_ground: false,
        flags: 0,
        data: Some(data),
    }
}

/// A stack of `item` with the potion `potion`.
fn with_potion(item: i32, potion: &str) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some(potion.to_owned()),
            ..PotionContents::default()
        }));
    stack
}

/// Swiftness's colour, Speed's (`MobEffects.SPEED`, 3402751), opaque: one effect at
/// level one mixes to its own colour (`PotionContents.getColorOptional`).
const SWIFTNESS: i32 = 0xFF33_EBFF_u32 as i32;

fn arrow(stack: ItemStack, owner: Option<EntityId>, of_kind: ArrowKind) -> Arrow {
    Arrow {
        pickup: Pickup::Allowed,
        pickup_stack: stack,
        weapon: None,
        owner,
        left_owner: false,
        has_been_shot: false,
        base_damage: Double(2.0),
        crit: true,
        pierce_level: 0,
        pierced: Vec::new(),
        hit_sound: "minecraft:entity.arrow.hit".to_owned(),
        in_ground: None,
        shake_until: 0,
        of_kind,
    }
}

/// What `client` was sent of entities because of `region`'s report of `tick` with
/// `events`, and the ids of every packet it was sent so.
async fn told(
    edge: &mut Harness,
    client: &mut Client,
    tick: u64,
    events: Vec<RegionEvent>,
) -> (Vec<ClientboundPlay>, Vec<SetEntityData>, Vec<i32>) {
    let (tracking, data, ids) = (
        client.tracking.len(),
        client.entity_data.len(),
        client.ids.len(),
    );
    edge.tell(WEST, WorkerToEdge::TickDelta { tick, events });
    edge.settle(WEST).await;
    edge.sync(client).await;
    (
        client.tracking[tracking..].to_vec(),
        client.entity_data[data..].to_vec(),
        client.ids[ids..].to_vec(),
    )
}

/// The add packet `client` was sent of `key`, and the data that came with it.
fn added(client: &Client, key: u64) -> (SpawnEntity, Vec<(u8, EntityDataValue)>) {
    let id = client.naming(key)[0].1;
    let spawn = client
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .cloned()
        .expect("it was added");
    let data = client
        .entity_data
        .iter()
        .find(|data| data.entity_id == id)
        .map(|data| data.values.clone())
        .unwrap_or_default();
    (spawn, data)
}

/// A critical tipped arrow is added with its shooter's id as the add packet's data, its
/// flags and its potion's colour; landing, it is sent its crit gone and its being in the
/// ground; lying long enough, its fading's event and its colour gone; picked up, the
/// pick-up names it and its collector.
#[tokio::test]
async fn a_tipped_arrow_is_shown_with_its_owner_its_flags_and_its_colour() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 1;
    let flying = arrow(
        with_potion(clustine_data::items::TIPPED_ARROW, "minecraft:swiftness"),
        Some(SHOOTER),
        ArrowKind::Arrow,
    );
    assert_eq!(clustine_sim::arrow_colour(&flying.pickup_stack), SWIFTNESS);
    let data = EntityData::Arrow(Box::new(flying.clone()));
    let spawned = state(key, entity_types::ARROW, data.clone(), [0.0, 0.5, 2.0]);
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (spawn, values) = added(&client, key);
    assert_eq!(spawn.kind, entity_types::ARROW);
    assert_eq!(spawn.data, client.own_id(), "the owner is the shooter");
    assert_eq!(
        values,
        [
            (8, EntityDataValue::Byte(1)),
            (11, EntityDataValue::Int(SWIFTNESS)),
        ]
    );

    // It strikes a block in tick 12: no longer critical, and in the ground.
    let mut landed = flying.clone();
    landed.crit = false;
    landed.in_ground = Some(InGround { state: 1, since: 2 });
    let at = [2.5, -58.0, 4.5];
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(report(key, at, [0.0, 0.4, 1.9], data)),
    ];
    told(&mut edge, &mut client, 11, events).await;
    let landing = EntityData::Arrow(Box::new(landed.clone()));
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(report(key, at, [0.0; 3], landing.clone())),
    ];
    told(&mut edge, &mut client, 12, events).await;
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(report(key, at, [0.0; 3], landing)),
    ];
    let (_, sent, _) = told(&mut edge, &mut client, 13, events).await;
    assert_eq!(
        sent.iter()
            .map(|data| data.values.clone())
            .collect::<Vec<_>>(),
        [vec![
            (8, EntityDataValue::Byte(0)),
            (10, EntityDataValue::Boolean(true)),
        ]]
    );

    // Long in the ground, its potion fades (`Arrow.tick`): the event, then the colour
    // of a plain arrow.
    let mut faded = landed;
    faded.pickup_stack = ItemStack::new(clustine_data::items::ARROW, 1);
    let faded = EntityData::Arrow(Box::new(faded));
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityEvent {
            entity: EntityId(key),
            chunk: HOME,
            event: 0,
        },
        RegionEvent::EntityTracked(report(key, at, [0.0; 3], faded.clone())),
    ];
    let (_, _, ids) = told(&mut edge, &mut client, 14, events).await;
    assert!(ids.contains(&clientbound::ENTITY_EVENT), "{ids:?}");
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(report(key, at, [0.0; 3], faded)),
    ];
    let (_, sent, _) = told(&mut edge, &mut client, 15, events).await;
    assert_eq!(
        sent.iter()
            .map(|data| data.values.clone())
            .collect::<Vec<_>>(),
        [vec![(11, EntityDataValue::Int(-1))]]
    );

    // The shooter picks it up.
    let id = client.naming(key)[0].1;
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::ItemTaken {
            item: EntityId(key),
            collector: SHOOTER,
            count: 1,
            chunk: HOME,
        },
        RegionEvent::EntityGone {
            travelled: false,
            entity: EntityId(key),
            chunk: HOME,
            hops: 0,
        },
    ];
    let (tracking, _, ids) = told(&mut edge, &mut client, 16, events).await;
    assert!(tracking.contains(&ClientboundPlay::TakeItemEntity(
        clustine_protocol::packets::play::TakeItemEntity {
            item_id: id,
            collector_id: client.own_id(),
            count: 1,
        }
    )));
    assert!(ids.contains(&clientbound::REMOVE_ENTITIES));
    edge.end().await;
}

/// A projectile's owner that no client is shown is given its id at the add, which the
/// owner keeps when it is shown later (`n9-plan.md` section 10, finding 36); a plain
/// arrow that is not critical has no data to send.
#[tokio::test]
async fn an_owner_not_shown_yet_is_named_by_the_id_it_is_shown_with_later() {
    let (mut edge, mut client) = at_home().await;
    let skeleton = EntityId((1 << 32) + 50);
    let key = (1 << 32) + 2;
    let mut flying = arrow(
        ItemStack::new(clustine_data::items::ARROW, 1),
        Some(skeleton),
        ArrowKind::Arrow,
    );
    flying.crit = false;
    let data = EntityData::Arrow(Box::new(flying));
    let spawned = state(key, entity_types::ARROW, data, [0.0, 0.5, 2.0]);
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (spawn, values) = added(&client, key);
    assert_ne!(spawn.data, 0);
    assert_eq!(client.key(spawn.data), skeleton.0);
    assert_eq!(values, []);
    edge.end().await;
}

/// A trident's loyalty and foil come of its stack's enchantments; returning, it is sent
/// its flag of no physics.
#[tokio::test]
async fn a_trident_is_shown_with_its_loyalty_and_its_foil() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 3;
    let mut stack = ItemStack::new(clustine_data::items::TRIDENT, 1);
    stack.components.set(Component::Enchantments(vec![(
        "minecraft:loyalty".to_owned(),
        3,
    )]));
    let mut thrown = arrow(
        stack,
        Some(SHOOTER),
        ArrowKind::Trident {
            dealt_damage: false,
            returning: 0,
        },
    );
    thrown.crit = false;
    let data = EntityData::Arrow(Box::new(thrown.clone()));
    let spawned = state(key, entity_types::TRIDENT, data, [0.0, 0.5, 2.5]);
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (spawn, values) = added(&client, key);
    assert_eq!(spawn.data, client.own_id());
    assert_eq!(
        values,
        [
            (11, EntityDataValue::Byte(3)),
            (12, EntityDataValue::Boolean(true)),
        ]
    );

    thrown.of_kind = ArrowKind::Trident {
        dealt_damage: true,
        returning: 1,
    };
    let returning = EntityData::Arrow(Box::new(thrown));
    let at = [2.5, -58.0, 4.5];
    let events = vec![
        RegionEvent::BlockEventsBegin,
        RegionEvent::EntityTracked(report(key, at, [0.0; 3], returning)),
    ];
    told(&mut edge, &mut client, 11, events).await;
    let (_, sent, _) = told(&mut edge, &mut client, 12, Vec::new()).await;
    assert_eq!(
        sent.iter()
            .map(|data| data.values.clone())
            .collect::<Vec<_>>(),
        [vec![(8, EntityDataValue::Byte(2))]]
    );
    edge.end().await;
}

/// A rocket is added with its owner, its stack (never the default, which the game
/// compares by identity) and its being shot at an angle; its attachment, always none,
/// is not sent.
#[tokio::test]
async fn a_rocket_is_shown_with_its_stack_and_its_angle() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 4;
    let stack = ItemStack::new(clustine_data::items::FIREWORK_ROCKET, 1);
    let rocket = Firework {
        stack: stack.clone(),
        owner: Some(SHOOTER),
        left_owner: false,
        has_been_shot: false,
        life: 0,
        lifetime: 30,
        shot_at_angle: true,
    };
    let data = EntityData::Firework(Box::new(rocket));
    let spawned = state(key, entity_types::FIREWORK_ROCKET, data, [0.0, 0.05, 0.0]);
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (spawn, values) = added(&client, key);
    assert_eq!(spawn.data, client.own_id());
    assert_eq!(
        values,
        [
            (
                8,
                EntityDataValue::ItemStack(Some(clustine_protocol::item::ItemStack::new(
                    clustine_data::items::FIREWORK_ROCKET,
                    1
                )))
            ),
            (10, EntityDataValue::Boolean(true)),
        ]
    );
    edge.end().await;
}

/// A lingering potion's cloud: added with its particle alone (its radius of 3 and its
/// not waiting are the defaults), no owner in its add packet (it is no projectile);
/// its first tick sets it waiting; at its wait's end it stops waiting and its radius
/// begins to shrink, sent each tick it shrinks.
#[tokio::test]
async fn a_cloud_waits_and_then_shrinks_tick_by_tick() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 5;
    let cloud = Cloud {
        duration: 600,
        wait_time: 10,
        reapplication_delay: 20,
        radius: Float(3.0),
        radius_at: 0,
        radius_on_use: Float(-0.5),
        radius_per_tick: Float(-0.25),
        duration_on_use: 0,
        potion: PotionContents {
            potion: Some("minecraft:swiftness".to_owned()),
            ..PotionContents::default()
        },
        duration_scale: Float(0.25),
        owner: Some(SHOOTER),
        victims: Vec::new(),
    };
    let data = EntityData::Cloud(Box::new(cloud));
    let spawned = state(key, entity_types::AREA_EFFECT_CLOUD, data.clone(), [0.0; 3]);
    let spawn_tick = 20;
    told(
        &mut edge,
        &mut client,
        spawn_tick,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (spawn, values) = added(&client, key);
    assert_eq!(spawn.data, 0, "a cloud's add carries no owner");
    let particle_value = EntityDataValue::Particle(EffectParticle::Colour {
        kind: particle::ENTITY_EFFECT,
        argb: SWIFTNESS,
    });
    assert_eq!(values, [(10, particle_value.clone())]);
    // `EntityDataSerializers.PARTICLE`, 16, then the particle's type and its colour.
    let bytes = packets::encode(&SetEntityData {
        entity_id: 1,
        values: vec![(10, particle_value)],
    });
    let mut tail = vec![10, 16, particle::ENTITY_EFFECT as u8];
    tail.extend(SWIFTNESS.to_be_bytes());
    tail.push(0xFF);
    assert_eq!(bytes[bytes.len() - tail.len()..], tail[..]);

    // Reported every tick it lives, the same record: what is sent is derived from its
    // age.
    let mut sent_each = Vec::new();
    for tick in spawn_tick + 1..=spawn_tick + 13 {
        let events = vec![
            RegionEvent::BlockEventsBegin,
            RegionEvent::EntityTracked(report(key, [2.5, -58.0, 2.5], [0.0; 3], data.clone())),
        ];
        let (_, sent, _) = told(&mut edge, &mut client, tick, events).await;
        let values: Vec<_> = sent.into_iter().flat_map(|data| data.values).collect();
        sent_each.push((tick - spawn_tick, values));
    }
    let (waiting, shrinking) = (
        |value| (9, EntityDataValue::Boolean(value)),
        |radius| (8, EntityDataValue::Float(radius)),
    );
    let particle_again = (
        10,
        EntityDataValue::Particle(EffectParticle::Colour {
            kind: particle::ENTITY_EFFECT,
            argb: SWIFTNESS,
        }),
    );
    // The update of tick `t` sends what the cloud's tick `t - 1` did, its age then
    // `t - 1 - spawn`: the second its waiting; the age of 10 its waiting's end and its
    // first shrink. What it was made with went again with the spawn tick's own update.
    let made_again = client
        .entity_data
        .iter()
        .filter(|data| data.entity_id == spawn.entity_id)
        .take(2)
        .map(|data| data.values.clone())
        .collect::<Vec<_>>();
    assert_eq!(made_again, vec![vec![particle_again.clone()]; 2]);
    let expected = |after: u64| -> Vec<(u8, EntityDataValue)> {
        match after {
            2 => vec![waiting(true)],
            11 => vec![shrinking(2.75), waiting(false)],
            12 => vec![shrinking(2.5)],
            13 => vec![shrinking(2.25)],
            _ => Vec::new(),
        }
    };
    for (after, values) in sent_each {
        assert_eq!(values, expected(after), "{after} ticks after its spawn");
    }
    edge.end().await;
}

/// A cloud met after it was made (here, entering the view from another region at the
/// age of 13) is shown at the radius of its age, not at that of its making.
#[tokio::test]
async fn a_cloud_met_after_it_was_made_is_shown_at_its_age() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 6;
    let cloud = Cloud {
        duration: 600,
        wait_time: 10,
        reapplication_delay: 20,
        radius: Float(3.0),
        radius_at: 0,
        radius_on_use: Float(-0.5),
        radius_per_tick: Float(-0.25),
        duration_on_use: 0,
        potion: PotionContents {
            potion: Some("minecraft:swiftness".to_owned()),
            ..PotionContents::default()
        },
        duration_scale: Float(0.25),
        owner: None,
        victims: Vec::new(),
    };
    let expected = clustine_sim::cloud_radius_at(&cloud, 13).expect("it lives at 13");
    assert!(expected < 3.0, "it has shrunk by then: {expected}");
    let mut entered = state(
        key,
        entity_types::AREA_EFFECT_CLOUD,
        EntityData::Cloud(Box::new(cloud)),
        [0.0; 3],
    );
    if let EntityKind::Other { lived, .. } = &mut entered.kind {
        *lived = 13;
    }
    told(
        &mut edge,
        &mut client,
        40,
        vec![RegionEvent::EntityEntered(entered)],
    )
    .await;
    let (_, values) = added(&client, key);
    assert!(
        values.contains(&(8, EntityDataValue::Float(expected))),
        "{values:?}"
    );
    edge.end().await;
}

/// The ticks of `range` in whose updates `id` was sent a move (relative or whole) and
/// a motion, flying from its spawn in tick 10 with a motion that changes every tick.
async fn flown(
    edge: &mut Harness,
    client: &mut Client,
    key: u64,
    data: &EntityData,
    range: std::ops::RangeInclusive<u64>,
) -> (Vec<u64>, Vec<u64>, Vec<ClientboundPlay>) {
    let id = client.naming(key)[0].1;
    let (mut moved, mut motions, mut all) = (Vec::new(), Vec::new(), Vec::new());
    for tick in range {
        let t = (tick - 10) as f64;
        let position = [2.5, -58.0 + 0.5 * t, 2.5 + t];
        let motion = [0.0, 0.5 - 0.05 * t, 1.0];
        let events = vec![
            RegionEvent::BlockEventsBegin,
            RegionEvent::EntityTracked(report(key, position, motion, data.clone())),
        ];
        let (tracking, _, _) = told(edge, client, tick, events).await;
        for packet in tracking {
            match &packet {
                ClientboundPlay::MoveEntityPos(moved_by) if moved_by.entity_id == id => {
                    moved.push(tick);
                }
                ClientboundPlay::MoveEntityPosRot(moved_by) if moved_by.entity_id == id => {
                    moved.push(tick);
                }
                ClientboundPlay::SyncEntityPosition(moved_by) if moved_by.entity_id == id => {
                    moved.push(tick);
                }
                ClientboundPlay::SetEntityMotion(motion) if motion.entity_id == id => {
                    motions.push(tick);
                }
                _ => {}
            }
            all.push(packet);
        }
    }
    (moved, motions, all)
}

/// An arrow is sent its position and its angles at every update of its type's interval,
/// 20, and its motion with them (`ServerEntity.sendChanges`: the motion is sent inside
/// the update, `trackDeltas` deciding only whether it is); every move it is sent carries
/// its angles, even one that turned nothing.
#[tokio::test]
async fn an_arrow_is_sent_its_place_and_motion_every_twenty_ticks_with_its_angles() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 6;
    let mut flying = arrow(
        ItemStack::new(clustine_data::items::ARROW, 1),
        Some(SHOOTER),
        ArrowKind::Arrow,
    );
    flying.crit = false;
    let data = EntityData::Arrow(Box::new(flying));
    let spawned = state(key, entity_types::ARROW, data.clone(), [0.0, 0.5, 1.0]);
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (moved, motions, all) = flown(&mut edge, &mut client, key, &data, 11..=51).await;
    assert_eq!(moved, [30, 50]);
    assert_eq!(motions, [30, 50]);
    assert!(
        all.iter()
            .all(|packet| !matches!(packet, ClientboundPlay::MoveEntityPos(_))),
        "an arrow's moves carry its angles: {all:?}"
    );
    edge.end().await;
}

/// A rocket is sent its place and its motion every ten ticks, its type's interval.
#[tokio::test]
async fn a_rocket_is_sent_its_place_and_motion_every_ten_ticks() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 7;
    let rocket = Firework {
        stack: ItemStack::new(clustine_data::items::FIREWORK_ROCKET, 1),
        owner: Some(SHOOTER),
        left_owner: false,
        has_been_shot: false,
        life: 0,
        lifetime: 60,
        shot_at_angle: false,
    };
    let data = EntityData::Firework(Box::new(rocket));
    let spawned = state(
        key,
        entity_types::FIREWORK_ROCKET,
        data.clone(),
        [0.0, 0.5, 1.0],
    );
    told(
        &mut edge,
        &mut client,
        10,
        vec![RegionEvent::EntitySpawned(spawned)],
    )
    .await;
    let (moved, motions, _) = flown(&mut edge, &mut client, key, &data, 11..=41).await;
    assert_eq!(moved, [20, 30, 40]);
    assert_eq!(motions, [20, 30, 40]);
    edge.end().await;
}
