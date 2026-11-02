//! The ender dragon at the edge (family E9 of the End's milestone,
//! `docs/groundwork/p-plan.md`, section 19.5 item 10), from a report fed by hand as a
//! region would send it: the client numbers a dragon's eight parts after the dragon's own
//! id (`EnderDragon.recreateFromPacket`, d EnderDragon.txt 2959–2986), so the edge keeps
//! those ids for them when it first shows the dragon; a hit the client sends on a part's
//! id reaches the region as a hit on the dragon at that part.

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
            data: Default::default(),
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

fn mob(key: u64, kind: i32) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: kind as u16,
            uuid: Uuid::from_u128(u128::from(key) + 7_000),
            motion: [0.0; 3],
            hops: 0,
            lived: 0,
            data: EntityData::Mob {
                body: Vec::new(),
                mind: Vec::new(),
                state: Default::default(),
            },
            riding: Default::default(),
        },
        pose: Pose::at(Vec3::new(2.5, -50.0, 2.5)),
    }
}

async fn tick(edge: &mut Harness, client: &mut Client, tick: u64, events: Vec<RegionEvent>) {
    edge.tell(WEST, WorkerToEdge::TickDelta { tick, events });
    edge.settle(WEST).await;
    edge.sync(client).await;
}

/// A dragon spawned with its id: the next entity's id comes nine after it, the eight
/// between its parts'; the client's hit on the id three after the dragon's (the body,
/// part 2) goes to the region as the player's attack on the dragon at part 2, and a hit
/// on its own id as one on no part.
#[tokio::test]
async fn a_dragons_parts_have_the_ids_after_its_own_and_a_hit_on_one_names_the_part() {
    let (mut edge, mut client) = at_home().await;
    let dragon = (1 << 32) + 3;
    let after = (1 << 32) + 4;
    let spawned = vec![RegionEvent::EntitySpawned(mob(
        dragon,
        entity_types::ENDER_DRAGON,
    ))];
    tick(&mut edge, &mut client, 10, spawned).await;
    let id = client.naming(dragon)[0].1;
    let spawn = client
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == id)
        .expect("the dragon is spawned");
    assert_eq!(spawn.kind, entity_types::ENDER_DRAGON);
    let spawned = vec![RegionEvent::EntitySpawned(mob(after, entity_types::COW))];
    tick(&mut edge, &mut client, 11, spawned).await;
    assert_eq!(client.naming(after)[0].1, id + 9);
    edge.said(WEST).await;
    for (hit, part) in [(id + 3, Some(2)), (id, None), (id + 8, Some(7))] {
        let command = Command::OnEntity {
            session: client.session,
            player: client.player,
            id: hit,
            what: OnEntity::Attack,
        };
        edge.commands.send(command).await.unwrap();
        edge.drained().await;
        let said = edge.said(WEST).await;
        let named = said.iter().any(|message| {
            matches!(
                &message.body,
                EdgeToWorker::Input {
                    input: PlayerInput::Attack { entity, part: named },
                    ..
                } if *entity == EntityId(dragon) && *named == part
            )
        });
        assert!(named, "{hit}: {said:?}");
    }
    edge.end().await;
}
