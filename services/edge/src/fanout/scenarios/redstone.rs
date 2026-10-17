//! Tests of what the edge sends of redstone's events (step R2c of
//! `docs/adr/0026-redstone.md`, section 8): the block changes of a tick before its
//! block events and those after them with the region's next tick, as the game
//! broadcasts them; block events and sounds to the players near them, a sound never to
//! the player whose own client plays it.

use super::*;

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
    first.heard.clear();
    second.heard.clear();
    (edge, first, second)
}

fn tick(tick: u64, events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick, events }
}

fn at(position: BlockPos) -> Position {
    Position {
        x: position.x,
        y: position.y,
        z: position.z,
    }
}

/// A block change before the beginning of block events goes before the block event,
/// and one after it waits for the region's next tick, which may say nothing else.
#[tokio::test]
async fn block_changes_after_the_beginning_of_block_events_go_with_the_regions_next_tick() {
    let (mut edge, mut first, _) = two_at_home().await;
    let before = BlockPos::new(3, -60, 5);
    let after = BlockPos::new(3, -40, 5);
    edge.tell(
        WEST,
        tick(
            2,
            vec![
                RegionEvent::BlockChanged {
                    position: before,
                    state: STONE,
                    seen: true,
                },
                RegionEvent::BlockEventsBegin,
                RegionEvent::BlockEvent {
                    position: before,
                    block: 1,
                    kind: 0,
                    data: 2,
                },
                RegionEvent::BlockChanged {
                    position: after,
                    state: STONE,
                    seen: true,
                },
            ],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    let event = Received::Event(BlockEvent {
        position: at(before),
        action: 0,
        param: 2,
        block: 1,
    });
    assert_eq!(first.heard, [Received::Block(before), event.clone()]);
    edge.tell(WEST, tick(3, Vec::new()));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert_eq!(
        first.heard,
        [Received::Block(before), event, Received::Block(after)]
    );
    edge.end().await;
}

/// A block set without the game's `UPDATE_CLIENTS` (a block a piston moves, which
/// clients move themselves from the piston's block event) is not sent for itself, but
/// a block waiting to be sent goes as it is when it goes, as `broadcastChanges` reads
/// the level then.
#[tokio::test]
async fn a_block_set_without_telling_clients_is_sent_only_where_another_change_sends_it() {
    let (mut edge, mut first, _) = two_at_home().await;
    let moved = BlockPos::new(3, -60, 5);
    let waiting = BlockPos::new(4, -60, 5);
    let changed = |position, state, seen| RegionEvent::BlockChanged {
        position,
        state,
        seen,
    };
    edge.tell(
        WEST,
        tick(
            2,
            vec![
                changed(moved, STONE, false),
                changed(waiting, STONE, true),
                changed(waiting, GRANITE, false),
            ],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    assert_eq!(first.heard, [Received::Block(waiting)]);
    assert_eq!(first.blocks.get(&waiting), Some(&i32::from(GRANITE.0)));
    assert_eq!(first.blocks.get(&moved), None);
    edge.end().await;
}

/// A sound reaches the players near it but the one whose own client plays it, in
/// eighths of a block, from the blocks.
#[tokio::test]
async fn a_sound_is_heard_by_the_players_near_it_but_the_one_who_made_it() {
    let (mut edge, mut first, mut second) = two_at_home().await;
    let door = Vec3::new(3.5, -59.5, 5.5);
    edge.tell(
        WEST,
        tick(
            2,
            vec![RegionEvent::Sound {
                sound: clustine_data::sounds::BLOCK_WOODEN_DOOR_OPEN,
                source: 4,
                position: door,
                volume: 1.0,
                pitch: 0.95,
                except: Some(player(1)),
            }],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert_eq!(first.heard, []);
    let [Received::Sound(sound)] = &second.heard[..] else {
        panic!("{:?}", second.heard);
    };
    assert_eq!(sound.sound, clustine_data::sounds::BLOCK_WOODEN_DOOR_OPEN);
    assert_eq!((sound.source, sound.x, sound.y, sound.z), (4, 28, -476, 44));
    assert_eq!((sound.volume, sound.pitch), (1.0, 0.95));
    edge.end().await;
}

/// A sound further than 16 blocks is not heard, and one of a larger volume is heard
/// further, as the game's `playSeededSound` sends them: measured upwards from the
/// player, within the chunk they have been sent.
#[tokio::test]
async fn a_sound_is_heard_within_sixteen_blocks_or_sixteen_times_its_volume() {
    let (mut edge, mut first, _) = two_at_home().await;
    let above = |blocks: f64, volume: f32| RegionEvent::Sound {
        sound: clustine_data::sounds::BLOCK_LEVER_CLICK,
        source: 4,
        position: Vec3::new(SPAWN.x, SPAWN.y + blocks, SPAWN.z),
        volume,
        pitch: 1.0,
        except: None,
    };
    let heard = |client: &Client| -> Vec<i32> {
        client
            .heard
            .iter()
            .filter_map(|told| match told {
                Received::Sound(sound) => Some(sound.y),
                _ => None,
            })
            .collect()
    };
    edge.tell(
        WEST,
        tick(
            2,
            vec![
                above(15.0, 1.0),
                above(17.0, 1.0),
                above(15.5, 0.5),
                above(17.0, 2.0),
            ],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    let eighths = |blocks: f64| ((SPAWN.y + blocks) * 8.0) as i32;
    assert_eq!(heard(&first), [eighths(15.0), eighths(15.5), eighths(17.0)]);
    edge.end().await;
}

/// A level event the region says goes to the players near who have the chunk, as the
/// game's `levelEvent`: a torch burning out, here.
#[tokio::test]
async fn a_level_event_is_shown_to_the_players_near_it() {
    let (mut edge, mut first, mut second) = two_at_home().await;
    let torch = BlockPos::new(4, -60, 6);
    edge.tell(
        WEST,
        tick(
            2,
            vec![RegionEvent::LevelEvent {
                kind: 1502,
                position: torch,
                data: 0,
            }],
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    let shown = LevelEvent {
        event: 1502,
        position: at(torch),
        data: 0,
        global: false,
    };
    assert_eq!(first.level_events, std::slice::from_ref(&shown));
    assert_eq!(second.level_events, [shown]);
    edge.end().await;
}
