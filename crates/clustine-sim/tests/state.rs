//! The state of a region, its changes from tick to tick and its outbox, as
//! `docs/adr/0008-durable-regions-and-resuming.md` describes them in sections 1 and 2.
//! These tests use the crate's public interface only.

use std::collections::BTreeMap;

use clustine_data::{blocks, items};
use clustine_sim::api::{Face, HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent};
use clustine_sim::{
    Durable, EdgeEvent, EdgeState, Holdings, Knowledge, Misdirected, PlayerChange, PlayerJoin,
    PlayerTransfer, Region, RegionConfig, RegionState, RemoteAction, RemoteStep, StateDelta,
    TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Vec3,
};

const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// The chunks with x from -1 to 0: blocks with x from -16 to 15. The region is pinned
/// to them, between a region to the west and one to the east.
const AREA: ChunkArea = ChunkArea {
    min_x: Some(-1),
    max_x: Some(1),
};

/// The regions that hold what lies west and east of the area.
const WESTERN: RegionId = RegionId(0);
const EASTERN: RegionId = RegionId(2);

const A: EdgeId = EdgeId(0xA);
const B: EdgeId = EdgeId(0xB);
const C: EdgeId = EdgeId(0xC);

const STONE: ItemStack = ItemStack {
    item: items::STONE,
    count: 1,
};

fn config() -> RegionConfig {
    let mut starting_hotbar = [None; HOTBAR_SLOTS];
    starting_hotbar[0] = Some(STONE);
    RegionConfig {
        spawn: SPAWN,
        starting_hotbar,
        return_after: 0,
    }
}

/// What the world store says of the region: it is pinned to the area and has been
/// granted the chunks of it that these tests use.
fn holdings() -> Holdings {
    Holdings {
        held: CHUNKS.to_vec(),
        pinned: vec![AREA],
    }
}

fn ids() -> EntityIds {
    EntityIds::block(0).unwrap()
}

fn player(number: u128) -> PlayerId {
    PlayerId(uuid::Uuid::from_u128(number))
}

fn started(edge: EdgeId, start: u64) -> EdgeEvent {
    EdgeEvent::Started { edge, start }
}

fn join(edge: EdgeId, number: u128) -> PlayerChange {
    PlayerChange::Join(
        edge,
        PlayerJoin {
            player: player(number),
            name: format!("Player{number}"),
        },
    )
}

fn walk(x: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, -60.0, 0.5)),
        rotation: None,
        on_ground: true,
    }
}

fn dig(x: i32, sequence: i32) -> PlayerInput {
    PlayerInput::Dig {
        position: BlockPos::new(x, -61, 0),
        sequence,
    }
}

/// A chunk with a floor of stone right below where players stand.
fn floor() -> Chunk {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .unwrap();
    let mut chunk = Chunk::empty(overworld, Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, blocks::STONE);
        }
    }
    chunk
}

/// The chunks of the area around the origin.
const CHUNKS: [ChunkPos; 2] = [ChunkPos::new(-1, 0), ChunkPos::new(0, 0)];

/// The chunks of the neighbours right beyond either end of the area, each with the
/// region that holds it.
const BEYOND: [(ChunkPos, RegionId); 2] = [
    (ChunkPos::new(-2, 0), WESTERN),
    (ChunkPos::new(1, 0), EASTERN),
];

/// Gives `region` what an edge and the store give it in two ticks: a viewer's ticket
/// for each of `CHUNKS`, which are loaded with what `chunk` says, and one for each chunk
/// of `BEYOND`, of which the region asks and is told whose it is. So a player who steps
/// out of the area is let go in the tick of the step, and what is done to a block
/// beyond it is passed on with its region.
fn load(region: &mut Region, chunk: impl Fn(ChunkPos) -> Chunk) {
    let beyond = BEYOND.map(|(position, _)| position);
    let viewer = |position: &ChunkPos| (*position, Ticket::Viewer);
    let output = region.tick(&TickInputs {
        tickets_added: CHUNKS.iter().chain(&beyond).map(viewer).collect(),
        ..TickInputs::default()
    });
    assert_eq!(output.chunk_requests, CHUNKS);
    assert_eq!(output.claims, beyond);
    region.tick(&TickInputs {
        chunks_loaded: CHUNKS
            .iter()
            .map(|position| (*position, chunk(*position)))
            .collect(),
        foreign: BEYOND.to_vec(),
        ..TickInputs::default()
    });
    for (position, holder) in BEYOND {
        assert_eq!(region.knowledge(position), Knowledge::Foreign(holder));
    }
}

/// Ticks `region` and checks that the delta turns the state before into the one after.
fn tick(region: &mut Region, inputs: &TickInputs) -> TickOutput {
    let mut state = region.state();
    let output = region.tick(inputs);
    state.apply(&output.delta);
    assert_eq!(state, region.state(), "tick {}", output.tick);
    output
}

/// A region on a floor that knows edges `A` and `B` with start 1, with player 1 of `A`
/// and player 2 of `B` at the spawn point.
fn populated() -> Region {
    let mut region = Region::new(config(), ids(), holdings());
    load(&mut region, |_| floor());
    tick(
        &mut region,
        &TickInputs {
            edges: vec![started(A, 1), started(B, 1)],
            player_changes: vec![join(A, 1), join(B, 2)],
            ..TickInputs::default()
        },
    );
    region
}

fn removed(entity: i32, chunk: ChunkPos) -> RegionEvent {
    RegionEvent::EntityRemoved {
        entity: EntityId(entity),
        chunk,
    }
}

/// The entries of `output` for `edge`, with their numbers.
fn entries(output: &TickOutput, edge: EdgeId) -> Vec<(u64, Durable)> {
    output
        .durable
        .iter()
        .filter(|(to, ..)| *to == edge)
        .map(|(_, number, entry)| (*number, entry.clone()))
        .collect()
}

/// Lets player 1 of `A` step out of the area east of the origin, so that a `Departed`
/// for them is in the outbox of `A`.
fn depart(region: &mut Region) -> PlayerTransfer {
    let output = tick(
        region,
        &TickInputs {
            inputs: vec![(A, player(1), EntityId(1), 1, walk(20.5))],
            ..TickInputs::default()
        },
    );
    let [(number, Durable::Departed { transfer, .. })] = &entries(&output, A)[..] else {
        panic!("expected a departure, got {:?}", output.durable);
    };
    assert_eq!(region.edge(A).unwrap().outbox.len(), 1);
    assert_eq!(*number, region.edge(A).unwrap().sent);
    transfer.clone()
}

#[test]
fn a_new_region_starts_from_the_state_of_its_entity_ids() {
    let region = Region::new(config(), ids(), holdings());
    let state = RegionState::new(ids());
    assert_eq!(region.state(), state);
    assert_eq!(state.tick, 0);
    assert_eq!(state.next_entity_id, ids().first);
    assert!(state.players.is_empty() && state.edges.is_empty());
}

#[test]
fn a_players_state_is_what_the_whole_state_has_of_them() {
    let region = populated();
    let state = region.state();
    assert!(!state.players.is_empty());
    for (id, expected) in &state.players {
        assert_eq!(region.player_state(*id).as_ref(), Some(expected));
    }
    assert_eq!(region.player_state(player(99)), None);
}

#[test]
fn an_unknown_edge_is_noted_with_nothing_applied_or_sent() {
    let mut region = Region::new(config(), ids(), holdings());
    let output = tick(
        &mut region,
        &TickInputs {
            edges: vec![started(A, 7)],
            ..TickInputs::default()
        },
    );
    assert_eq!(
        region.edge(A),
        Some(&EdgeState {
            start: 7,
            // Made in the tick that has just run.
            since: region.tick_number(),
            applied: 0,
            sent: 0,
            outbox: BTreeMap::new(),
        })
    );
    assert!(output.events.is_empty());
    assert!(!output.delta.changes_only_the_tick());
}

#[test]
fn a_higher_start_resets_the_edge_and_removes_whatever_of_it_is_shown() {
    let mut region = populated();
    let transfer = depart(&mut region);
    // Player 3 of `A` is still in the world, and `A` has had messages applied.
    tick(
        &mut region,
        &TickInputs {
            applied: vec![(A, 12)],
            player_changes: vec![join(A, 3)],
            ..TickInputs::default()
        },
    );
    let before = region.state();
    assert_eq!(before.edges[&A].applied, 12);

    let output = tick(
        &mut region,
        &TickInputs {
            edges: vec![started(A, 2)],
            ..TickInputs::default()
        },
    );
    // The player still there and the one that departed but was never passed on are
    // gone from every screen; player 2 belongs to another edge and stays.
    let origin = ChunkPos::new(0, 0);
    let entity_3 = before.players[&player(3)].entity_id.0;
    let departed_to = ChunkPos::containing(transfer.pose.position.x, transfer.pose.position.z);
    assert_eq!(
        output.events,
        [
            removed(entity_3, origin),
            removed(transfer.entity_id.0, departed_to),
        ]
    );
    assert!(output.durable.is_empty());
    assert_eq!(
        region.edge(A),
        Some(&EdgeState {
            start: 2,
            // Made in the tick that has just run.
            since: region.tick_number(),
            ..EdgeState::default()
        })
    );
    let state = region.state();
    assert_eq!(state.players.keys().collect::<Vec<_>>(), [&player(2)]);
    assert_eq!(state.edges[&B], before.edges[&B]);

    // What the edge is sent from now on is numbered from 1 again.
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(A, 4)],
            inputs: vec![(A, player(4), EntityId(4), 1, walk(-20.5))],
            ..TickInputs::default()
        },
    );
    assert!(matches!(
        &entries(&output, A)[..],
        [(1, Durable::Departed { .. })]
    ));
}

#[test]
fn the_same_start_or_a_lower_one_changes_nothing() {
    let mut region = populated();
    depart(&mut region);
    for start in [1, 0] {
        let before = region.state();
        let output = tick(
            &mut region,
            &TickInputs {
                edges: vec![started(A, start), started(B, start)],
                ..TickInputs::default()
            },
        );
        assert!(output.events.is_empty());
        assert!(output.delta.changes_only_the_tick(), "{:?}", output.delta);
        assert_eq!(
            region.state(),
            RegionState {
                tick: before.tick + 1,
                ..before
            }
        );
    }
}

#[test]
fn a_gone_edge_is_forgotten_with_its_players_and_departures() {
    let mut region = populated();
    let transfer = depart(&mut region);
    tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(A, 3)],
            ..TickInputs::default()
        },
    );
    let output = tick(
        &mut region,
        &TickInputs {
            edges: vec![EdgeEvent::Gone { edge: A }],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.events.len(), 2);
    assert!(
        output
            .events
            .contains(&removed(transfer.entity_id.0, ChunkPos::new(1, 0)))
    );
    assert_eq!(region.edge(A), None);
    assert_eq!(region.player_count(), 1);
    assert_eq!(output.delta.edges, [(A, None)]);

    // Forgotten means unknown: nothing it sends counts until it starts again, and then
    // it starts afresh, whatever its start.
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(A, 5)],
            ..TickInputs::default()
        },
    );
    assert!(output.events.is_empty() && output.player_events.is_empty());
    tick(
        &mut region,
        &TickInputs {
            edges: vec![started(A, 1)],
            ..TickInputs::default()
        },
    );
    assert_eq!(
        region.edge(A),
        Some(&EdgeState {
            start: 1,
            // Made in the tick that has just run.
            since: region.tick_number(),
            ..EdgeState::default()
        })
    );
}

#[test]
fn a_gone_edge_that_starts_again_in_the_same_tick_has_an_empty_outbox() {
    let mut region = populated();
    depart(&mut region);
    let mut state = region.state();
    let output = region.tick(&TickInputs {
        edges: vec![EdgeEvent::Gone { edge: A }, started(A, 1)],
        ..TickInputs::default()
    });
    state.apply(&output.delta);
    assert_eq!(state, region.state());
    assert_eq!(
        region.edge(A),
        Some(&EdgeState {
            start: 1,
            // Made in the tick that has just run.
            since: region.tick_number(),
            ..EdgeState::default()
        })
    );
}

#[test]
fn confirmed_entries_are_dropped_from_the_outbox_and_only_those() {
    let mut region = populated();
    // Three entries for `B`: its player digs beyond the area three times.
    tick(
        &mut region,
        &TickInputs {
            inputs: vec![
                (B, player(2), EntityId(2), 1, walk(14.5)),
                (B, player(2), EntityId(2), 2, dig(16, 1)),
                (B, player(2), EntityId(2), 3, dig(17, 2)),
                (B, player(2), EntityId(2), 4, dig(18, 3)),
            ],
            ..TickInputs::default()
        },
    );
    let numbers =
        |region: &Region| -> Vec<u64> { region.edge(B).unwrap().outbox.keys().copied().collect() };
    assert_eq!(numbers(&region), [1, 2, 3]);

    let output = tick(
        &mut region,
        &TickInputs {
            edges: vec![EdgeEvent::Confirmed { edge: B, number: 2 }],
            ..TickInputs::default()
        },
    );
    assert_eq!(numbers(&region), [3]);
    let [(edge, Some(delta))] = &output.delta.edges[..] else {
        panic!("{:?}", output.delta);
    };
    assert_eq!((*edge, delta.confirmed, delta.added.len()), (B, 2, 0));
    assert_eq!(region.edge(B).unwrap().sent, 3);

    // Confirming again what is confirmed already changes nothing at all.
    let output = tick(
        &mut region,
        &TickInputs {
            edges: vec![
                EdgeEvent::Confirmed { edge: B, number: 1 },
                EdgeEvent::Confirmed { edge: C, number: 9 },
            ],
            ..TickInputs::default()
        },
    );
    assert!(output.delta.changes_only_the_tick());
    tick(
        &mut region,
        &TickInputs {
            edges: vec![EdgeEvent::Confirmed {
                edge: B,
                number: u64::MAX,
            }],
            ..TickInputs::default()
        },
    );
    assert!(numbers(&region).is_empty());
}

#[test]
fn a_leave_needs_no_entity_but_has_to_come_from_the_players_edge() {
    let mut region = populated();
    // From another edge it is about an earlier connection, and so is the input.
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![PlayerChange::Leave(B, player(1), None)],
            inputs: vec![(B, player(1), EntityId(1), 1, walk(5.5))],
            ..TickInputs::default()
        },
    );
    assert!(output.events.is_empty(), "{:?}", output.events);
    assert_eq!(region.player(player(1)).unwrap().1, Pose::at(SPAWN));

    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![PlayerChange::Leave(A, player(1), None)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.events, [removed(1, ChunkPos::new(0, 0))]);
    assert_eq!(region.player(player(1)), None);
}

#[test]
fn a_leave_through_another_edge_keeps_what_the_players_edge_passed_on() {
    let mut inputs = TickInputs::default();
    inputs.input(A, player(1), EntityId(1), 1, walk(3.5));
    inputs.change(PlayerChange::Leave(B, player(1), None));
    inputs.input(B, player(1), EntityId(1), 2, walk(4.5));
    inputs.change(PlayerChange::Leave(B, player(1), None));
    assert_eq!(inputs.inputs, [(A, player(1), EntityId(1), 1, walk(3.5))]);
    inputs.change(PlayerChange::Leave(A, player(1), None));
    assert!(inputs.inputs.is_empty());

    // A join or an arrival ends whatever came before, through any edge.
    inputs.input(A, player(1), EntityId(1), 1, walk(3.5));
    inputs.input(B, player(1), EntityId(1), 2, walk(4.5));
    inputs.change(join(B, 1));
    assert!(inputs.inputs.is_empty());
}

#[test]
fn a_join_through_another_edge_replaces_the_player() {
    let mut region = populated();
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(B, 1)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.events.len(), 2);
    assert_eq!(output.events[0], removed(1, ChunkPos::new(0, 0)));
    assert!(
        matches!(&output.events[1], RegionEvent::EntitySpawned(state) if state.entity == EntityId(3))
    );
    assert_eq!(output.player_events.len(), 1);
    assert_eq!(region.state().players[&player(1)].edge, B);

    // Through the same edge, a join is ignored.
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(B, 1)],
            ..TickInputs::default()
        },
    );
    assert!(output.events.is_empty() && output.player_events.is_empty());
    assert!(output.delta.changes_only_the_tick());

    // The edge the player had has no say any more.
    tick(
        &mut region,
        &TickInputs {
            player_changes: vec![PlayerChange::Leave(A, player(1), None)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.player(player(1)).unwrap().0, EntityId(3));
}

#[test]
fn what_concerns_an_edge_goes_to_its_outbox_numbered_on_from_what_it_was_sent() {
    // Entity ids for two players only, so that a third is refused.
    let few = EntityIds {
        first: EntityId(1),
        end: EntityId(3),
    };
    let mut region = Region::new(config(), few, holdings());
    load(&mut region, |_| floor());
    tick(
        &mut region,
        &TickInputs {
            edges: vec![started(A, 1), started(B, 1), started(C, 1)],
            player_changes: vec![join(A, 1), join(B, 2)],
            ..TickInputs::default()
        },
    );
    // Something for `B` first, so that its numbers are ahead of those of `A`.
    tick(
        &mut region,
        &TickInputs {
            inputs: vec![
                (B, player(2), EntityId(2), 1, walk(14.5)),
                (B, player(2), EntityId(2), 2, dig(16, 1)),
            ],
            ..TickInputs::default()
        },
    );
    let against = BlockPos::new(15, -61, 0);
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(B, 3)],
            remote_actions: vec![
                (
                    A,
                    RemoteAction {
                        player: player(8),
                        sequence: 5,
                        step: RemoteStep::Break {
                            position: BlockPos::new(-3, -61, 0),
                        },
                    },
                ),
                (
                    C,
                    RemoteAction {
                        player: player(9),
                        sequence: 6,
                        step: RemoteStep::PlaceAgainst {
                            against,
                            target: Face::East.neighbour(against),
                            block: blocks::STONE,
                            placer: Vec3::new(20.5, -60.0, 0.5),
                        },
                    },
                ),
            ],
            inputs: vec![
                (B, player(2), EntityId(2), 3, dig(17, 2)),
                (A, player(1), EntityId(1), 1, walk(-20.5)),
            ],
            ..TickInputs::default()
        },
    );
    let next = RemoteAction {
        player: player(9),
        sequence: 6,
        step: RemoteStep::Place {
            target: Face::East.neighbour(against),
            block: blocks::STONE,
            placer: Vec3::new(20.5, -60.0, 0.5),
        },
    };
    let request = RemoteAction {
        player: player(2),
        sequence: 2,
        step: RemoteStep::Break {
            position: BlockPos::new(17, -61, 0),
        },
    };
    let Durable::Departed { transfer, .. } = &output.durable[4].2 else {
        panic!("{:?}", output.durable);
    };
    assert_eq!(
        output.durable,
        [
            (B, 2, Durable::Refused { player: player(3) }),
            (
                A,
                1,
                Durable::RemoteDone {
                    player: player(8),
                    sequence: 5,
                },
            ),
            // Each names the region the store has said holds the chunk concerned: the
            // spot east of the area, the block east of it, and where the player went.
            (
                C,
                1,
                Durable::Remote {
                    action: next,
                    to: Some(EASTERN),
                },
            ),
            (
                B,
                3,
                Durable::Remote {
                    action: request,
                    to: Some(EASTERN),
                },
            ),
            (
                A,
                2,
                Durable::Departed {
                    player: player(1),
                    transfer: transfer.clone(),
                    to: WESTERN,
                },
            ),
        ]
    );
    // Each entry is in the outbox of its edge.
    for (edge, number, entry) in &output.durable {
        assert_eq!(region.edge(*edge).unwrap().outbox.get(number), Some(entry));
    }
    let sent = |edge| region.edge(edge).unwrap().sent;
    assert_eq!([sent(A), sent(B), sent(C)], [2, 3, 1]);
    // None of these is a player event any more.
    assert!(output.player_events.is_empty());
}

#[test]
fn applied_is_noted_for_edges_the_region_knows() {
    let mut region = populated();
    let output = tick(
        &mut region,
        &TickInputs {
            applied: vec![(A, 4), (C, 9)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.edge(A).unwrap().applied, 4);
    assert_eq!(region.edge(C), None);
    assert_eq!(output.delta.edges.len(), 1);
    // The same number again is no change.
    let output = tick(
        &mut region,
        &TickInputs {
            applied: vec![(A, 4)],
            ..TickInputs::default()
        },
    );
    assert!(output.delta.changes_only_the_tick());
}

#[test]
fn nothing_through_an_unknown_edge_is_taken_on() {
    let mut region = populated();
    let transfer = PlayerTransfer {
        entity_id: EntityId(5000),
        name: "Visitor".to_owned(),
        pose: Pose::at(Vec3::new(3.5, -60.0, 0.5)),
        hotbar: [None; HOTBAR_SLOTS],
        selected_slot: 0,
        last_input: 0,
    };
    let before = region.state();
    let output = tick(
        &mut region,
        &TickInputs {
            player_changes: vec![join(C, 7), PlayerChange::Arrive(C, player(8), transfer)],
            remote_actions: vec![(
                C,
                RemoteAction {
                    player: player(9),
                    sequence: 1,
                    step: RemoteStep::Break {
                        position: BlockPos::new(3, -61, 0),
                    },
                },
            )],
            ..TickInputs::default()
        },
    );
    // The entity that arrived could be shown to nobody, and is no more.
    assert_eq!(output.events, [removed(5000, ChunkPos::new(0, 0))]);
    assert!(output.durable.is_empty() && output.player_events.is_empty());
    assert_eq!(
        region.state(),
        RegionState {
            tick: before.tick + 1,
            ..before
        }
    );
}

#[test]
fn handled_covers_the_players_own_actions_on_blocks_of_the_region_only() {
    let mut region = populated();
    let handled = |region: &Region| region.state().players[&player(1)].handled;
    assert_eq!(handled(&region), None);
    tick(
        &mut region,
        &TickInputs {
            inputs: vec![
                (A, player(1), EntityId(1), 1, dig(2, 4)),
                (A, player(1), EntityId(1), 2, walk(14.5)),
            ],
            ..TickInputs::default()
        },
    );
    assert_eq!(handled(&region), Some(4));
    // Beyond the area: passed on, and not handled here.
    let output = tick(
        &mut region,
        &TickInputs {
            inputs: vec![(A, player(1), EntityId(1), 3, dig(16, 9))],
            ..TickInputs::default()
        },
    );
    assert_eq!(entries(&output, A).len(), 1);
    assert_eq!(handled(&region), Some(4));
    // A lower one later leaves the highest in place, though it is acknowledged.
    let output = tick(
        &mut region,
        &TickInputs {
            inputs: vec![(A, player(1), EntityId(1), 4, dig(12, 2))],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.player_events.len(), 1);
    assert_eq!(handled(&region), Some(4));
}

#[test]
fn a_tick_in_which_nothing_happens_changes_only_its_number() {
    let mut region = populated();
    let output = tick(&mut region, &TickInputs::default());
    assert_eq!(
        output.delta,
        StateDelta {
            tick: output.tick,
            ..StateDelta::default()
        }
    );
    assert!(output.delta.changes_only_the_tick());
}

/// A small pseudo-random generator, so that runs are the same every time.
struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }

    fn once_in(&mut self, times: u64) -> bool {
        self.below(times) == 0
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len() as u64) as usize]
    }

    /// A coordinate from -24 to 24, so that a good part lies beyond the area.
    fn coordinate(&mut self) -> f64 {
        self.below(480) as f64 / 10.0 - 24.0
    }
}

/// Makes up the inputs of tick after tick for a region of `AREA`, with everything a
/// runner may pass on: edges starting, starting again, being confirmed and gone, players
/// joining, leaving, arriving and being discarded through the three edges, what they do,
/// and actions of players of other regions.
struct Scenario {
    random: Random,
    /// The start each edge has, as far as the runner knows.
    starts: BTreeMap<EdgeId, u64>,
    /// The number of the next input; all players share it.
    next_input: u64,
    /// The number of the next message per edge.
    messages: BTreeMap<EdgeId, u64>,
    /// Players that were let go, who may come back.
    departed: Vec<(PlayerId, PlayerTransfer)>,
}

impl Scenario {
    fn new(seed: u64) -> Self {
        Self {
            random: Random(seed),
            starts: BTreeMap::new(),
            next_input: 1,
            messages: BTreeMap::new(),
            departed: Vec::new(),
        }
    }

    /// What comes next for `region`, as of its state now.
    fn inputs(&mut self, region: &Region) -> TickInputs {
        let random = &mut self.random;
        let mut inputs = TickInputs::default();
        let edges = [A, B, C];

        for edge in edges {
            let known = region.edge(edge);
            if random.once_in(25) {
                let start = self.starts.entry(edge).or_insert(1);
                // Mostly the same start, now and then a later one; a lower one never
                // reaches the tick.
                if random.once_in(3) {
                    *start += 1;
                }
                inputs.edges.push(started(edge, *start));
            }
            if let Some(known) = known
                && random.once_in(3)
            {
                let number = random.below(known.sent + 2);
                inputs.edges.push(EdgeEvent::Confirmed { edge, number });
            }
            if random.once_in(120) {
                inputs.edges.push(EdgeEvent::Gone { edge });
            }
        }

        let state = region.state();
        let players: Vec<_> = (1..=8).map(player).collect();
        for _ in 0..random.below(4) {
            let id = random.pick(&players);
            let edge = random.pick(&edges);
            let change = match random.below(10) {
                0..=3 => PlayerChange::Join(
                    edge,
                    PlayerJoin {
                        player: id,
                        name: format!("{id:?}"),
                    },
                ),
                4..=6 => {
                    // Mostly through the edge the player belongs to.
                    let edge = state
                        .players
                        .get(&id)
                        .filter(|_| !random.once_in(4))
                        .map_or(edge, |player| player.edge);
                    PlayerChange::Leave(edge, id, None)
                }
                7 | 8 if !self.departed.is_empty() => {
                    let index = random.below(self.departed.len() as u64) as usize;
                    let (id, mut transfer) = self.departed.remove(index);
                    // Back where they came from, or somewhere else entirely.
                    if random.once_in(2) {
                        transfer.pose.position.x = random.coordinate();
                    }
                    PlayerChange::Arrive(edge, id, transfer)
                }
                _ => PlayerChange::Discard {
                    entity: EntityId(random.below(40) as i32),
                    chunk: ChunkPos::new(random.below(5) as i32 - 2, 0),
                },
            };
            inputs.change(change);
        }

        for _ in 0..random.below(4) {
            let edge = random.pick(&edges);
            let block = BlockPos::new(random.below(48) as i32 - 24, -61, 0);
            let step = match random.below(3) {
                0 => RemoteStep::Break { position: block },
                1 => RemoteStep::PlaceAgainst {
                    against: block,
                    target: random.pick(&FACES).neighbour(block),
                    block: blocks::STONE,
                    placer: Vec3::new(random.coordinate(), -60.0, 0.5),
                },
                _ => RemoteStep::Place {
                    target: block.offset(0, 1, 0),
                    block: blocks::STONE,
                    placer: Vec3::new(random.coordinate(), -60.0, 0.5),
                },
            };
            let action = RemoteAction {
                player: random.pick(&players),
                sequence: random.below(100) as i32,
                step,
            };
            inputs.remote_actions.push((edge, action));
        }

        for _ in 0..random.below(6) {
            let id = random.pick(&players);
            // Mostly through the edge the player belongs to.
            let edge = match state.players.get(&id) {
                Some(player) if !random.once_in(5) => player.edge,
                _ => random.pick(&edges),
            };
            let block = BlockPos::new(random.below(48) as i32 - 24, -61, 0);
            let input = match random.below(8) {
                0..=2 => PlayerInput::Move {
                    position: Some(Vec3::new(random.coordinate(), -60.0, 0.5)),
                    rotation: Some((random.below(360) as f32, 0.0)),
                    on_ground: random.once_in(2),
                },
                3 => PlayerInput::Dig {
                    position: block,
                    sequence: random.below(100) as i32,
                },
                4 => PlayerInput::UseItemOn {
                    position: block,
                    face: random.pick(&FACES),
                    sequence: random.below(100) as i32,
                },
                5 => PlayerInput::SelectSlot {
                    slot: random.below(10) as u8,
                },
                6 => PlayerInput::SetHotbarSlot {
                    slot: random.below(9) as u8,
                    stack: random.once_in(2).then_some(STONE),
                },
                _ => walk(random.coordinate()),
            };
            // Now and then one that was applied before.
            let number = if random.once_in(10) {
                self.next_input.saturating_sub(random.below(5))
            } else {
                self.next_input += 1;
                self.next_input
            };
            // With the entity the player has, as an edge names it. Of a player who is
            // not there an edge knows none that the region could have.
            let entity = state
                .players
                .get(&id)
                .map_or(EntityId(0), |player| player.entity_id);
            inputs.input(edge, id, entity, number, input);
        }

        for edge in edges {
            if random.once_in(2) {
                let number = self.messages.entry(edge).or_default();
                *number += 1 + random.below(3);
                inputs.applied.push((edge, *number));
            }
        }
        inputs
    }

    /// Takes note of the players `output` let go or sent on when they arrived, who may
    /// arrive again later.
    fn note(&mut self, output: &TickOutput) {
        for (_, _, entry) in &output.durable {
            match entry {
                Durable::Departed {
                    player, transfer, ..
                }
                | Durable::NotMine {
                    what: Misdirected::Arrival { player, transfer },
                    ..
                } => self.departed.push((*player, transfer.clone())),
                _ => {}
            }
        }
    }
}

const FACES: [Face; 6] = [
    Face::Bottom,
    Face::Top,
    Face::North,
    Face::South,
    Face::West,
    Face::East,
];

/// What a run made up by `Scenario` did, to make sure it did what is worth checking.
#[derive(Debug, Default)]
struct Seen {
    joins: usize,
    refusals: usize,
    departures: usize,
    arrivals: usize,
    remote: usize,
    remote_done: usize,
    /// Arrivals for a chunk of a neighbour, which went on to it, and remote actions
    /// about one.
    sent_on: usize,
    not_mine: usize,
    resets: usize,
    confirmed: usize,
    forgotten: usize,
    removed: usize,
}

impl Seen {
    fn note(&mut self, before: &RegionState, output: &TickOutput) {
        for (_, _, entry) in &output.durable {
            match entry {
                Durable::Departed { .. } => self.departures += 1,
                Durable::Refused { .. } => self.refusals += 1,
                Durable::Remote { .. } => self.remote += 1,
                Durable::RemoteDone { .. } => self.remote_done += 1,
                Durable::NotMine {
                    what: Misdirected::Arrival { .. },
                    ..
                } => self.sent_on += 1,
                Durable::NotMine {
                    what: Misdirected::Remote(_),
                    ..
                } => self.not_mine += 1,
                Durable::Absorbed { .. } | Durable::SplitOff { .. } => {
                    panic!("a region made {entry:?}, which none does yet")
                }
            }
        }
        for event in &output.events {
            match event {
                RegionEvent::EntitySpawned(state) if state.entity.0 >= 1000 => self.arrivals += 1,
                RegionEvent::EntitySpawned(_) => self.joins += 1,
                RegionEvent::EntityRemoved { .. } => self.removed += 1,
                _ => {}
            }
        }
        for (edge, delta) in &output.delta.edges {
            match delta {
                None => self.forgotten += 1,
                Some(delta) => {
                    if delta.cleared && before.edges.contains_key(edge) {
                        self.resets += 1;
                    }
                    if delta.confirmed > 0 {
                        self.confirmed += 1;
                    }
                }
            }
        }
    }

    fn assert_worth_it(&self) {
        let counts = [
            self.joins,
            self.refusals,
            self.departures,
            self.arrivals,
            self.remote,
            self.remote_done,
            self.sent_on,
            self.not_mine,
            self.resets,
            self.confirmed,
            self.forgotten,
            self.removed,
        ];
        assert!(counts.iter().all(|count| *count >= 5), "{self:?}");
    }
}

/// A region that has a floor and gives out few entity ids, so that some players are
/// refused. Arriving players have ids of another block.
fn generated_region() -> Region {
    let few = EntityIds {
        first: EntityId(1),
        end: EntityId(60),
    };
    let mut region = Region::new(config(), few, holdings());
    load(&mut region, |_| floor());
    region
}

/// Gives arriving players entity ids of their own, as another region would have.
fn with_foreign_entities(mut inputs: TickInputs) -> TickInputs {
    for change in &mut inputs.player_changes {
        if let PlayerChange::Arrive(_, _, transfer) = change
            && transfer.entity_id.0 < 1000
        {
            transfer.entity_id.0 += 1000;
        }
    }
    inputs
}

#[test]
fn the_delta_of_every_tick_turns_the_state_before_it_into_the_state_after_it() {
    let mut seen = Seen::default();
    for seed in 1..=20 {
        let mut region = generated_region();
        let mut scenario = Scenario::new(0x9E37_79B9_7F4A_7C15 ^ seed);
        let first = region.state();
        let mut applied = first.clone();
        for _ in 0..400 {
            let inputs = with_foreign_entities(scenario.inputs(&region));
            let before = region.state();
            let output = region.tick(&inputs);
            let after = region.state();

            let mut state = before.clone();
            state.apply(&output.delta);
            assert_eq!(state, after, "seed {seed}, tick {}", output.tick);
            applied.apply(&output.delta);

            // Each edge's entries are numbered on without a gap, up to what it was sent.
            let mut last: BTreeMap<EdgeId, u64> = BTreeMap::new();
            for (edge, number, entry) in &output.durable {
                let previous = last.get(edge).copied().unwrap_or_else(|| {
                    let reset = output.delta.edges.iter().any(|(id, delta)| {
                        id == edge && delta.as_ref().is_some_and(|delta| delta.cleared)
                    });
                    if reset { 0 } else { before.edges[edge].sent }
                });
                assert_eq!(*number, previous + 1, "seed {seed}");
                assert_eq!(after.edges[edge].outbox.get(number), Some(entry));
                last.insert(*edge, *number);
            }
            for (edge, number) in last {
                assert_eq!(after.edges[&edge].sent, number);
            }
            // Every player belongs to an edge the region knows.
            for player in after.players.values() {
                assert!(after.edges.contains_key(&player.edge), "seed {seed}");
            }

            seen.note(&before, &output);
            scenario.note(&output);
        }
        assert_eq!(applied, region.state(), "seed {seed}");
    }
    seen.assert_worth_it();
}

#[test]
fn a_restored_region_carries_on_as_the_one_it_was_restored_from() {
    for seed in 1..=10 {
        let mut region = generated_region();
        let mut scenario = Scenario::new(0x2545_F491_4F6C_DD1D ^ seed);
        // The state as the world store would have it: the first one with every delta
        // applied since.
        let mut stored = region.state();
        let mut restored: Option<Region> = None;
        let mut compared = 0;
        for tick in 0..600 {
            if tick % 150 == 100 {
                // A restored region has no chunk until it is given its tickets again and
                // storage delivers. The one it is compared with idles meanwhile, which
                // changes nothing in it.
                let mut copy = Region::restore(config(), stored.clone(), holdings());
                assert_eq!(copy.state(), region.state());
                load(&mut copy, |position| {
                    region.chunk(position).unwrap().clone()
                });
                for _ in 0..2 {
                    let output = region.tick(&TickInputs::default());
                    stored.apply(&output.delta);
                }
                assert_eq!(copy.state(), region.state());
                restored = Some(copy);
            }
            let inputs = with_foreign_entities(scenario.inputs(&region));
            let output = region.tick(&inputs);
            stored.apply(&output.delta);
            if let Some(copy) = &mut restored {
                assert_eq!(copy.tick(&inputs), output, "seed {seed}, tick {tick}");
                assert_eq!(copy.state(), region.state());
                for position in CHUNKS {
                    assert_eq!(copy.chunk(position), region.chunk(position));
                }
                compared += 1;
            }
            scenario.note(&output);
        }
        assert!(compared > 400);
    }
}

#[test]
fn states_and_deltas_survive_serialisation() {
    let mut region = generated_region();
    // A seed with which the run is full enough, see below: how full it is varies a
    // good deal from seed to seed, and with every change to what a region does.
    let mut scenario = Scenario::new(4);
    let mut full = 0;
    for _ in 0..300 {
        let inputs = with_foreign_entities(scenario.inputs(&region));
        let output = region.tick(&inputs);
        let bytes = postcard::to_stdvec(&output.delta).unwrap();
        let delta: StateDelta = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(delta, output.delta);

        let state = region.state();
        let bytes = postcard::to_stdvec(&state).unwrap();
        let back: RegionState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, state);
        let outboxes = state.edges.values().any(|edge| !edge.outbox.is_empty());
        full += usize::from(!state.players.is_empty() && outboxes);
        scenario.note(&output);
    }
    // Most of the time there was something to serialise.
    assert!(full > 150, "{full}");
}
