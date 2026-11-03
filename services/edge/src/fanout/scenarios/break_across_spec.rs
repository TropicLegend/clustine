//! What the edge does with the steps of a survival break across a region line (step
//! V2c), written from the specification by someone who did not write the code: a
//! step that answers no action (`RemoteAction::UNANSWERED`, START's attack on a note
//! block or redstone ore across the line) is passed on and holds back no later
//! acknowledgement, while an ordinary step under way holds back every acknowledgement
//! above it until the region that took it answers, as the game's client is told up to
//! which sequence its guesses stand (`ServerGamePacketListenerImpl.ackBlockChangesUpTo`).

use super::*;

/// START's attack on the block at `block_of(chunk)` by the stay `entity` of `player`.
fn attacking(player: PlayerId, entity: EntityId, chunk: ChunkPos) -> RemoteAction {
    RemoteAction {
        player,
        entity,
        sequence: RemoteAction::UNANSWERED,
        step: RemoteStep::Attack {
            position: block_of(chunk),
        },
    }
}

fn acknowledged_by_region(player: PlayerId, sequence: i32) -> WorkerToEdge {
    WorkerToEdge::ToPlayer {
        player,
        event: PlayerEvent::Acknowledged { sequence },
    }
}

#[tokio::test]
async fn an_unanswered_step_passed_on_holds_back_no_later_acknowledgement() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let attack = attacking(player(1), edge.entity_of(player(1)), COMMON);
    edge.say(WEST, remote(&attack, Some(NORTH)));
    let passed_on = EdgeToWorker::Remote(attack);
    assert_eq!(edge.next_numbered(NORTH).await, (1, passed_on));
    // A later action the player's region handled itself is acknowledged at once.
    let later = edge.sequence();
    edge.tell(WEST, acknowledged_by_region(player(1), later));
    edge.acknowledged(&mut client, later).await;
    // Nothing negative is ever told the client.
    client.drain();
    assert!(
        client.acknowledged.iter().all(|sequence| *sequence >= 0),
        "{:?}",
        client.acknowledged
    );
    edge.end().await;
}

#[tokio::test]
async fn an_ordinary_step_under_way_holds_back_a_later_acknowledgement_until_it_is_done() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let entity = edge.entity_of(player(1));
    let action = breaking(player(1), entity, edge.sequence(), COMMON);
    edge.say(WEST, remote(&action, Some(NORTH)));
    let passed_on = EdgeToWorker::Remote(action.clone());
    assert_eq!(edge.next_numbered(NORTH).await, (1, passed_on));
    let later = edge.sequence();
    edge.tell(WEST, acknowledged_by_region(player(1), later));
    edge.settle(WEST).await;
    client.drain();
    assert!(
        !client.acknowledged.contains(&later),
        "acknowledged past an action under way: {:?}",
        client.acknowledged
    );
    // The holder answers: everything up to the later one is acknowledged.
    let done = Durable::RemoteDone {
        player: player(1),
        entity,
        sequence: action.sequence,
        shown: Vec::new(),
    };
    edge.say(NORTH, done);
    edge.acknowledged(&mut client, later).await;
    edge.end().await;
}

#[tokio::test]
async fn an_unanswered_step_beside_one_under_way_does_not_end_it() {
    // An `UNANSWERED` step and an ordinary one under way together: the region that
    // takes the attack sends nothing back, and the ordinary one still holds the later
    // acknowledgement until its own answer.
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let entity = edge.entity_of(player(1));
    let action = breaking(player(1), entity, edge.sequence(), COMMON);
    edge.say(WEST, remote(&action, Some(NORTH)));
    let attack = attacking(player(1), entity, COMMON);
    edge.say(WEST, remote(&attack, Some(NORTH)));
    assert_eq!(
        edge.next_numbered(NORTH).await,
        (1, EdgeToWorker::Remote(action.clone()))
    );
    assert_eq!(
        edge.next_numbered(NORTH).await,
        (2, EdgeToWorker::Remote(attack))
    );
    let later = edge.sequence();
    edge.tell(WEST, acknowledged_by_region(player(1), later));
    edge.settle(WEST).await;
    client.drain();
    assert!(!client.acknowledged.contains(&later));
    let done = Durable::RemoteDone {
        player: player(1),
        entity,
        sequence: action.sequence,
        shown: Vec::new(),
    };
    edge.say(NORTH, done);
    edge.acknowledged(&mut client, later).await;
    edge.end().await;
}
