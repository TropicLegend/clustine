//! Tests of how the edge shows a block breaking (step B3 of ADR-0024), written from
//! the specification of the step by someone who did not write the code: a
//! `RegionEvent::BlockDestroyed` goes to the players within 64 blocks of the block's
//! centre who have been sent its chunk, as the game's level event 2001, and never to
//! its breaker.

use super::*;

/// One of the edge's own players, standing where they entered the world.
fn own(number: u128, entity: EntityId) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Player {
            player: player(number),
            name: format!("Player{number}"),
        },
        pose: Pose::at(SPAWN),
    }
}

fn destroyed(position: BlockPos, state: BlockState, breaker: Option<PlayerId>) -> WorkerToEdge {
    WorkerToEdge::TickDelta {
        tick: 2,
        events: vec![RegionEvent::BlockDestroyed {
            position,
            state,
            breaker,
        }],
    }
}

/// What a client is sent of a block breaking at `position`.
fn broken(position: BlockPos, state: BlockState) -> LevelEvent {
    LevelEvent {
        event: 2001,
        position: Position {
            x: position.x,
            y: position.y,
            z: position.z,
        },
        data: i32::from(state.0),
        global: false,
    }
}

/// Two players at the spawn, both shown `HOME` with each other in it.
async fn two_at_home() -> (Harness, Client, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut second, WEST, HOME).await);
    (edge, first, second)
}

#[tokio::test]
async fn a_block_a_player_breaks_is_shown_breaking_to_others_near_it_but_not_to_them() {
    let (mut edge, mut first, mut second) = two_at_home().await;
    let block = BlockPos::new(3, -61, 5);
    edge.tell(WEST, destroyed(block, STONE, Some(player(1))));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert_eq!(first.level_events, []);
    assert_eq!(second.level_events, [broken(block, STONE)]);
    edge.end().await;
}

#[tokio::test]
async fn a_block_an_update_destroys_is_shown_breaking_to_everyone_near_it() {
    let (mut edge, mut first, mut second) = two_at_home().await;
    let block = BlockPos::new(3, -60, 5);
    edge.tell(WEST, destroyed(block, GRANITE, None));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert_eq!(first.level_events, [broken(block, GRANITE)]);
    assert_eq!(second.level_events, [broken(block, GRANITE)]);
    edge.end().await;
}

/// The distance is from the centre of the block to the player's position, (0.5, -60,
/// 0.5): from (3.5, 3.5, 5.5) it is just under 64 (63.8), from (3.5, 4.5, 5.5) just
/// over; across chunk (2, 2), which the player sees, it is under 64 at (40, -60, 40)
/// and over at (47, -60, 47).
#[tokio::test]
async fn a_block_breaking_is_shown_only_to_players_within_64_blocks_of_it() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let far = ChunkPos::new(2, 2);
    for chunk in [HOME, far] {
        let ask = edge.ask(WEST, chunk);
        let there = if chunk == HOME {
            vec![own(1, EntityId(5))]
        } else {
            Vec::new()
        };
        edge.tell(WEST, snapshot_of(chunk, ask, empty_chunk(), there));
        assert!(edge.shows(&mut client, WEST, chunk).await);
    }
    let near = [BlockPos::new(3, 3, 5), BlockPos::new(40, -60, 40)];
    let beyond = [BlockPos::new(3, 4, 5), BlockPos::new(47, -60, 47)];
    for block in beyond.into_iter().chain(near) {
        edge.tell(WEST, destroyed(block, STONE, None));
    }
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.level_events, near.map(|block| broken(block, STONE)));
    edge.end().await;
}

#[tokio::test]
async fn a_block_breaking_in_a_chunk_a_player_has_not_been_sent_is_not_shown_to_them() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, empty_chunk(), vec![own(1, EntityId(5))]),
    );
    assert!(edge.shows(&mut client, WEST, HOME).await);
    // The chunk east of the spawn is in the player's view and near, but the region
    // has not sent it yet.
    assert!(!client.chunks.contains_key(&STEP_EAST));
    let unsent = BlockPos::new(19, -60, 5);
    let sent = BlockPos::new(3, -60, 5);
    edge.tell(WEST, destroyed(unsent, STONE, None));
    edge.tell(WEST, destroyed(sent, STONE, None));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.level_events, [broken(sent, STONE)]);
    edge.end().await;
}
