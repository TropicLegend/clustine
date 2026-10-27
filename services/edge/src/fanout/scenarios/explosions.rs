//! Tests of how the edge shows explosions and primed TNT (step N6c's edge part,
//! `docs/groundwork/n6-plan.md`; `docs/groundwork/n6-brief.md`, section 9.1): an
//! explosion goes to every player within 64 blocks of its centre, each with their own
//! push or none, and primed TNT is shown with its fuse as it burns.

use super::*;
use clustine_protocol::packets::play::{EntityDataValue, particle};

fn own(number: u128, entity: EntityId, at: Vec3) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Player {
            player: player(number),
            name: format!("Player{number}"),
        },
        pose: Pose::at(at),
    }
}

fn ticked(events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick: 2, events }
}

fn explosion(centre: [f64; 3], knockback: Vec<(EntityId, [f64; 3])>) -> RegionEvent {
    RegionEvent::Explosion {
        centre,
        radius: 4.0,
        blocks: 1061,
        knockback,
        large: true,
        sound: true,
    }
}

#[tokio::test]
async fn an_explosion_reaches_each_player_within_64_blocks_with_their_own_push() {
    let mut edge = Harness::start().await;
    let mut near = edge.joined(player(1), EntityId(5)).await;
    let mut beside = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let there = vec![
        own(1, EntityId(5), SPAWN),
        own(2, EntityId(6), Vec3::new(3.5, -60.0, 0.5)),
    ];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut near, WEST, HOME).await);
    assert!(edge.shows(&mut beside, WEST, HOME).await);

    let push = [0.25, 0.5, -0.125];
    edge.tell(
        WEST,
        ticked(vec![explosion(
            [2.5, -60.0, 0.5],
            vec![(EntityId(5), push)],
        )]),
    );
    edge.quiet().await;
    near.drain();
    beside.drain();
    let [shown] = &near.explosions[..] else {
        panic!("{:?}", near.explosions);
    };
    assert_eq!(shown.knockback, Some(push));
    assert_eq!(
        (shown.particle, shown.sound),
        (particle::EXPLOSION_EMITTER, 703)
    );
    assert_eq!(shown.block_count, 1061);
    let [seen] = &beside.explosions[..] else {
        panic!("{:?}", beside.explosions);
    };
    assert_eq!(seen.knockback, None, "a player not hit is not pushed");

    // 64 blocks away is too far.
    edge.tell(
        WEST,
        ticked(vec![explosion([64.5, -60.0, 0.5], Vec::new())]),
    );
    edge.quiet().await;
    near.drain();
    assert_eq!(near.explosions.len(), 1);
    edge.end().await;
}

#[tokio::test]
async fn primed_tnt_is_shown_with_its_fuse_and_a_new_viewer_is_told_what_it_burns_at() {
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let tnt = |fuse: i16| EntityState {
        entity: EntityId(1 << 32),
        kind: EntityKind::Other {
            kind: entity_types::TNT as u16,
            uuid: Uuid::from_u128(77),
            motion: [0.0; 3],
            hops: 0,
            lived: 0,
            data: EntityData::PrimedTnt {
                fuse,
                block: clustine_data::blocks::TNT.0,
                power: clustine_world::Float(4.0),
                owner: None,
            },
            riding: Default::default(),
        },
        pose: Pose::at(Vec3::new(2.5, -60.0, 0.5)),
    };
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(
            HOME,
            ask,
            empty_chunk(),
            vec![own(1, EntityId(5), SPAWN), tnt(79)],
        ),
    );
    assert!(edge.shows(&mut watcher, WEST, HOME).await);
    watcher.drain();
    let fuse_of = |client: &Client| {
        client
            .entity_data
            .iter()
            .flat_map(|data| data.values.iter())
            .filter_map(|(index, value)| match value {
                EntityDataValue::Int(fuse) if *index == 8 => Some(*fuse),
                _ => None,
            })
            .collect::<Vec<i32>>()
    };
    assert_eq!(
        fuse_of(&watcher),
        [79],
        "a fuse that is not the default is paired"
    );
    assert!(
        watcher
            .entity_data
            .iter()
            .flat_map(|data| data.values.iter())
            .all(|(index, _)| *index != 9),
        "the default block is not"
    );
    edge.end().await;
}
