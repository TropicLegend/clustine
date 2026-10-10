//! Tests of a region's durable state, written from sections 1 and 2 of
//! `docs/adr/0008-durable-regions-and-resuming.md` and the public API alone, by someone
//! who has not read how the region does it.
//!
//! Every tick goes through [`checked_tick`], which holds the region to what the record
//! says of every tick: the delta turns the state before into the state after, the
//! entries made are numbered on from `sent` and are in the outbox, and every
//! acknowledgement can be worked out again from `handled`.

use std::collections::BTreeSet;

use clustine_data::{blocks, items};
use clustine_sim::api::{
    Face, HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent, RemoteAction, RemoteStep,
};
use clustine_sim::{
    Durable, EdgeEvent, EdgeState, Holdings, Knowledge, Misdirected, PlayerChange, PlayerEvent,
    PlayerJoin, PlayerTransfer, Region, RegionConfig, RegionState, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Section, Vec3,
};
use uuid::Uuid;

const E: EdgeId = EdgeId(1);
const F: EdgeId = EdgeId(2);

/// The chunk region A has, and the one region B has east of it.
const CHUNK_A: ChunkPos = ChunkPos::new(0, 0);
const CHUNK_B: ChunkPos = ChunkPos::new(1, 0);

/// The regions as the world store numbers stripes: from west to east.
const REGION_A: RegionId = RegionId(0);
const REGION_B: RegionId = RegionId(1);

/// Blocks either side of the border between A and B, at the top of the stone.
const OWN_BLOCK: BlockPos = BlockPos::new(14, 63, 8);
const BORDER_BLOCK_A: BlockPos = BlockPos::new(15, 63, 8);
const BORDER_BLOCK_B: BlockPos = BlockPos::new(16, 63, 8);

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack {
        item: items::STONE,
        count: 64,
    });
    hotbar
}

/// Region A: every chunk with x below 1, with its spawn two blocks from region B.
fn config_a() -> RegionConfig {
    RegionConfig {
        spawn: Vec3::new(14.5, 64.0, 8.5),
        starting_hotbar: hotbar(),
        return_after: 0,
        place_by_store: false,
        lowest_y: -64,
    }
}

/// What the world store says of region A: it is pinned to its stripe and has been
/// granted the chunk these tests use of it.
fn holdings_a() -> Holdings {
    Holdings {
        held: vec![CHUNK_A],
        pinned: vec![ChunkArea {
            min_x: None,
            max_x: Some(1),
        }],
    }
}

/// Region B: the chunks with x equal to 1.
fn config_b() -> RegionConfig {
    RegionConfig {
        spawn: Vec3::new(24.5, 64.0, 8.5),
        starting_hotbar: hotbar(),
        return_after: 0,
        place_by_store: false,
        lowest_y: -64,
    }
}

/// What the world store says of region B, as of region A.
fn holdings_b() -> Holdings {
    Holdings {
        held: vec![CHUNK_B],
        pinned: vec![ChunkArea {
            min_x: Some(1),
            max_x: Some(2),
        }],
    }
}

/// An overworld column of stone up to y = 63 and air above.
fn stone_chunk() -> Chunk {
    let sections = (0..24)
        .map(|index| {
            let state = if index < 8 {
                blocks::STONE
            } else {
                blocks::AIR
            };
            Section::filled(state, Biome(0))
        })
        .collect();
    Chunk::from_sections(-64, sections)
}

fn ids() -> EntityIds {
    EntityIds::block(3).expect("block 3 exists")
}

/// Advances the region by one tick and checks what section 2 says of every tick.
fn checked_tick(region: &mut Region, inputs: &TickInputs) -> TickOutput {
    let before = region.state();
    let output = region.tick(inputs);
    let after = region.state();

    assert_eq!(output.tick, before.tick + 1, "ticks are numbered on");
    assert_eq!(output.delta.tick, output.tick);
    assert_eq!(after.tick, output.tick);
    assert_eq!(region.tick_number(), output.tick);

    let mut applied = before.clone();
    applied.apply(&output.delta);
    assert_eq!(
        applied, after,
        "state_before.apply(&delta) must equal state_after"
    );

    // The entries made in the tick are numbered on, one by one, up to the edge's `sent`,
    // and are in its outbox after the tick.
    let edges: BTreeSet<EdgeId> = output.durable.iter().map(|(edge, ..)| *edge).collect();
    for edge in &edges {
        let numbers: Vec<u64> = output
            .durable
            .iter()
            .filter(|(e, ..)| e == edge)
            .map(|(_, number, _)| *number)
            .collect();
        let state = after
            .edges
            .get(edge)
            .expect("an edge with new entries is known after the tick");
        let last = *numbers.last().expect("at least one number");
        assert_eq!(last, state.sent, "`sent` is the number of the last entry");
        for pair in numbers.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "numbers of an edge are consecutive");
        }
        let first = numbers[0];
        let previous = before.edges.get(edge).map_or(0, |state| state.sent);
        assert!(
            first == previous + 1 || first == 1,
            "entries are numbered on from `sent`, or from 1 after a reset: {first} after {previous}"
        );
    }
    for (edge, number, entry) in &output.durable {
        assert_eq!(
            after.edges[edge].outbox.get(number),
            Some(entry),
            "an entry made in the tick is in the outbox after it"
        );
    }
    // Conversely, nothing enters an outbox without being among `durable`.
    for (edge, state) in &after.edges {
        for (number, entry) in &state.outbox {
            let old = before
                .edges
                .get(edge)
                .and_then(|state| state.outbox.get(number));
            if old != Some(entry) {
                assert!(
                    output
                        .durable
                        .iter()
                        .any(|(e, n, d)| e == edge && n == number && d == entry),
                    "entry {number} of {edge:?} is new but not among `durable`"
                );
            }
        }
    }

    // Acknowledged can be worked out again from `handled`.
    for (id, event) in &output.player_events {
        if let PlayerEvent::Acknowledged { sequence } = event {
            let state = after
                .players
                .get(id)
                .expect("an acknowledged player is in the region");
            assert_eq!(state.handled, Some(*sequence));
        }
    }
    for (id, state) in &after.players {
        let Some(old) = before.players.get(id) else {
            continue;
        };
        if old.entity_id == state.entity_id && old.handled != state.handled {
            assert!(
                output.player_events.iter().any(|(p, event)| p == id
                    && *event
                        == PlayerEvent::Acknowledged {
                            sequence: state.handled.expect("handled only grows")
                        }),
                "a change of `handled` is acknowledged"
            );
        }
    }

    output
}

/// Region A or B, whichever `chunk` is of, with that chunk loaded, through tickets as
/// for any region. It knows its neighbour: it has been told, for a viewer's sake, that
/// the chunk across the border is the other region's, so that a player who steps
/// across is let go in the tick of the step and a block across the border is passed on
/// with its region.
fn loaded(config: RegionConfig, chunk: ChunkPos) -> Region {
    let (holdings, across, neighbour) = if chunk == CHUNK_A {
        (holdings_a(), CHUNK_B, REGION_B)
    } else {
        (holdings_b(), CHUNK_A, REGION_A)
    };
    let mut region = Region::new(config, ids(), holdings);
    load(&mut region, chunk);
    learn(&mut region, across, neighbour);
    region
}

/// Has the region learn that `chunk` is `holder`'s, in two ticks: a viewer's ticket, for
/// which it asks, then what the store answers. It believes so while the ticket is there.
fn learn(region: &mut Region, chunk: ChunkPos, holder: RegionId) {
    let output = checked_tick(
        region,
        &TickInputs {
            tickets_added: vec![(chunk, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.claims, vec![chunk]);
    checked_tick(
        region,
        &TickInputs {
            foreign: vec![(chunk, holder)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.knowledge(chunk), Knowledge::Foreign(holder));
}

/// Loads `chunk`, which the region holds, into it in two ticks: a ticket, then what
/// storage delivers.
fn load(region: &mut Region, chunk: ChunkPos) {
    let output = checked_tick(
        region,
        &TickInputs {
            tickets_added: vec![(chunk, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, vec![chunk]);
    checked_tick(
        region,
        &TickInputs {
            chunks_loaded: vec![(chunk, stone_chunk())],
            ..TickInputs::default()
        },
    );
    assert!(region.chunk(chunk).is_some());
}

fn edges(events: Vec<EdgeEvent>) -> TickInputs {
    TickInputs {
        edges: events,
        ..TickInputs::default()
    }
}

fn started(edge: EdgeId, start: u64) -> EdgeEvent {
    EdgeEvent::Started { edge, start }
}

/// The attempt that a join of `id` names.
fn attempt(id: PlayerId) -> u64 {
    7000 + id.0.as_u128() as u64
}

fn join(edge: EdgeId, id: PlayerId) -> PlayerChange {
    PlayerChange::Join(
        edge,
        PlayerJoin {
            player: id,
            name: format!("player-{}", id.0.as_u128()),
            attempt: attempt(id),
        },
    )
}

fn changes(list: Vec<PlayerChange>) -> TickInputs {
    let mut inputs = TickInputs::default();
    for change in list {
        inputs.change(change);
    }
    inputs
}

fn single_input(
    edge: EdgeId,
    id: PlayerId,
    entity: EntityId,
    number: u64,
    input: PlayerInput,
) -> TickInputs {
    let mut inputs = TickInputs::default();
    inputs.input(edge, id, entity, number, input);
    inputs
}

/// The entity of the `n`th player to enter a region that gives out [`ids`], counted
/// from 1: what an edge is told when the player has spawned, and names in their inputs.
fn entity(n: i32) -> EntityId {
    EntityId(ids().first.0 + n - 1)
}

fn move_to(x: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, 64.0, 8.5)),
        rotation: None,
        on_ground: true,
    }
}

fn dig(position: BlockPos, sequence: i32) -> PlayerInput {
    PlayerInput::Dig { position, sequence }
}

fn removed(output: &TickOutput) -> Vec<EntityId> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityRemoved { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect()
}

fn spawned(output: &TickOutput) -> Vec<EntityId> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state) => Some(state.entity),
            _ => None,
        })
        .collect()
}

fn entity_of(region: &Region, id: PlayerId) -> EntityId {
    region.player(id).expect("the player is in the region").0
}

fn edge_of(region: &Region, id: PlayerId) -> EdgeId {
    region.state().players[&id].edge
}

fn outbox_numbers(region: &Region, edge: EdgeId) -> Vec<u64> {
    region
        .edge(edge)
        .expect("the edge is known")
        .outbox
        .keys()
        .copied()
        .collect()
}

/// The entity that `id` has in another region, as the remote actions here name it.
fn elsewhere(id: PlayerId) -> EntityId {
    EntityId(9000 + id.0.as_u128() as i32)
}

/// A remote action of a player of another region.
fn remote_break(id: PlayerId, sequence: i32, position: BlockPos) -> RemoteAction {
    RemoteAction {
        player: id,
        entity: elsewhere(id),
        sequence,
        step: RemoteStep::Break { position },
    }
}

/// Region A with edges E and F at start 10, P1 and P2 through E and P3 through F. P2 has
/// walked into region B, so E's outbox holds their `Departed` (1), followed by a remote
/// break of P1 (2). Returns P2's entity.
fn handing_over() -> (Region, EntityId) {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    checked_tick(
        &mut region,
        &changes(vec![
            join(E, player(1)),
            join(E, player(2)),
            join(F, player(3)),
        ]),
    );
    let departing = entity_of(&region, player(2));
    let output = checked_tick(
        &mut region,
        &single_input(E, player(2), entity(2), 1, move_to(17.5)),
    );
    assert!(region.player(player(2)).is_none(), "P2 has left region A");
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::Departed { player: p, transfer, to: REGION_B })]
            if *p == player(2) && transfer.entity_id == departing
    ));
    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, dig(BORDER_BLOCK_B, 1)),
    );
    assert!(matches!(
        output.durable.as_slice(),
        [(
            E,
            2,
            Durable::Remote {
                to: Some(REGION_B),
                ..
            }
        )]
    ));
    (region, departing)
}

// Section 2, `Started`.

#[test]
fn an_unknown_edge_is_noted_with_nothing_applied_or_sent() {
    let mut region = loaded(config_a(), CHUNK_A);
    let output = checked_tick(&mut region, &edges(vec![started(E, 42)]));
    assert_eq!(
        region.edge(E),
        Some(&EdgeState {
            start: 42,
            // Made in the tick that has just run.
            since: region.tick_number(),
            applied: 0,
            sent: 0,
            outbox: Default::default(),
        })
    );
    assert!(output.events.is_empty());
    assert!(output.durable.is_empty());
}

#[test]
fn a_higher_start_removes_the_edges_players_and_departing_entities_and_drops_its_outbox() {
    let (mut region, departing) = handing_over();
    checked_tick(
        &mut region,
        &TickInputs {
            applied: vec![(E, 9)],
            ..TickInputs::default()
        },
    );
    let p1 = entity_of(&region, player(1));
    let p3 = entity_of(&region, player(3));
    let f_before = region.edge(F).cloned();

    let output = checked_tick(&mut region, &edges(vec![started(E, 20)]));

    let gone = removed(&output);
    assert!(gone.contains(&p1), "the edge's player is reported removed");
    assert!(
        gone.contains(&departing),
        "the entity of a Departed in the outbox is reported removed"
    );
    assert!(!gone.contains(&p3), "another edge's player stays");
    assert_eq!(gone.len(), 2);
    assert!(region.player(player(1)).is_none());
    assert!(region.player(player(3)).is_some());
    assert_eq!(
        region.edge(E),
        Some(&EdgeState {
            start: 20,
            // Made in the tick that has just run.
            since: region.tick_number(),
            applied: 0,
            sent: 0,
            outbox: Default::default(),
        })
    );
    assert_eq!(
        region.edge(F).cloned(),
        f_before,
        "another edge is untouched"
    );
    assert!(output.durable.is_empty());
}

#[test]
fn entries_after_a_reset_are_numbered_from_one() {
    let (mut region, _) = handing_over();
    checked_tick(&mut region, &edges(vec![started(E, 20)]));
    checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    let entity = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity, 1, dig(BORDER_BLOCK_B, 1)),
    );
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::Remote { .. })]
    ));
    assert_eq!(outbox_numbers(&region, E), vec![1]);
}

#[test]
fn an_equal_start_changes_nothing() {
    let (mut region, _) = handing_over();
    checked_tick(
        &mut region,
        &TickInputs {
            applied: vec![(E, 9)],
            ..TickInputs::default()
        },
    );
    let mut before = region.state();

    let output = checked_tick(&mut region, &edges(vec![started(E, 10)]));

    assert!(output.events.is_empty(), "nobody is removed");
    assert!(output.delta.changes_only_the_tick());
    before.tick += 1;
    assert_eq!(region.state(), before);

    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 2, dig(BORDER_BLOCK_B, 2)),
    );
    assert!(
        matches!(output.durable.as_slice(), [(E, 3, Durable::Remote { .. })]),
        "numbering goes on"
    );
}

#[test]
fn a_lower_start_changes_nothing() {
    // The record says a lower start never reaches the tick; `EdgeEvent::Started` says
    // that it changes nothing if it does.
    let (mut region, _) = handing_over();
    let mut before = region.state();
    let output = checked_tick(&mut region, &edges(vec![started(E, 5)]));
    assert!(output.events.is_empty());
    before.tick += 1;
    assert_eq!(region.state(), before);
}

// Section 2, `Confirmed` and `Gone`.

/// Region A with E's outbox holding remote breaks of P1 numbered 1 to 3.
fn three_entries() -> Region {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10)]));
    checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    for n in 1..=3 {
        checked_tick(
            &mut region,
            &single_input(E, player(1), entity(1), n, dig(BORDER_BLOCK_B, n as i32)),
        );
    }
    assert_eq!(outbox_numbers(&region, E), vec![1, 2, 3]);
    region
}

#[test]
fn confirmed_drops_the_entries_up_to_the_number_and_no_more() {
    let mut region = three_entries();
    checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Confirmed { edge: E, number: 2 }]),
    );
    assert_eq!(outbox_numbers(&region, E), vec![3]);
    assert_eq!(region.edge(E).map(|e| e.sent), Some(3));

    let before = region.state();
    let output = checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Confirmed { edge: E, number: 2 }]),
    );
    assert!(
        output.delta.changes_only_the_tick(),
        "confirming again changes nothing"
    );
    assert_eq!(region.state().edges, before.edges);

    checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Confirmed { edge: E, number: 3 }]),
    );
    assert_eq!(outbox_numbers(&region, E), Vec::<u64>::new());
    assert_eq!(region.edge(E).map(|e| e.sent), Some(3), "`sent` stays");
}

#[test]
fn entries_stay_in_the_outbox_until_confirmed() {
    let mut region = three_entries();
    for _ in 0..5 {
        checked_tick(&mut region, &TickInputs::default());
    }
    assert_eq!(outbox_numbers(&region, E), vec![1, 2, 3]);
    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 4, dig(BORDER_BLOCK_B, 4)),
    );
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 4, Durable::Remote { .. })]
    ));
    assert_eq!(outbox_numbers(&region, E), vec![1, 2, 3, 4]);
}

#[test]
fn a_confirmation_of_an_unknown_edge_changes_nothing() {
    let mut region = three_entries();
    let output = checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Confirmed { edge: F, number: 2 }]),
    );
    assert!(output.delta.changes_only_the_tick());
    assert!(region.edge(F).is_none());
}

#[test]
fn gone_removes_the_edges_players_and_departing_entities_and_forgets_the_edge() {
    let (mut region, departing) = handing_over();
    let p1 = entity_of(&region, player(1));
    let p3 = entity_of(&region, player(3));

    let output = checked_tick(&mut region, &edges(vec![EdgeEvent::Gone { edge: E }]));

    let gone = removed(&output);
    assert!(gone.contains(&p1));
    assert!(gone.contains(&departing));
    assert!(!gone.contains(&p3));
    assert_eq!(gone.len(), 2);
    assert!(region.edge(E).is_none(), "the edge is forgotten");
    assert!(!region.state().edges.contains_key(&E));
    assert!(region.player(player(1)).is_none());
    assert_eq!(edge_of(&region, player(3)), F);
}

#[test]
fn an_edge_started_again_after_it_was_gone_starts_from_nothing() {
    let (mut region, _) = handing_over();
    checked_tick(
        &mut region,
        &TickInputs {
            applied: vec![(E, 9)],
            ..TickInputs::default()
        },
    );
    checked_tick(&mut region, &edges(vec![EdgeEvent::Gone { edge: E }]));

    // A join through the forgotten edge is ignored: the region does not know it.
    let output = checked_tick(&mut region, &changes(vec![join(E, player(4))]));
    assert!(region.player(player(4)).is_none());
    assert!(output.player_events.is_empty());

    // The same start as before: section 2 review item 3.
    checked_tick(&mut region, &edges(vec![started(E, 10)]));
    assert_eq!(
        region.edge(E),
        Some(&EdgeState {
            start: 10,
            // Made in the tick that has just run.
            since: region.tick_number(),
            applied: 0,
            sent: 0,
            outbox: Default::default(),
        })
    );
    checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    let entity = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity, 1, dig(BORDER_BLOCK_B, 1)),
    );
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::Remote { .. })]
    ));
}

#[test]
fn gone_and_started_in_one_tick_start_the_edge_from_nothing() {
    let (mut region, departing) = handing_over();
    let p1 = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Gone { edge: E }, started(E, 10)]),
    );
    let gone = removed(&output);
    assert!(gone.contains(&p1));
    assert!(gone.contains(&departing));
    assert_eq!(
        region.edge(E),
        Some(&EdgeState {
            start: 10,
            // Made in the tick that has just run.
            since: region.tick_number(),
            applied: 0,
            sent: 0,
            outbox: Default::default(),
        })
    );
}

#[test]
fn gone_of_an_unknown_edge_changes_nothing() {
    let (mut region, _) = handing_over();
    let output = checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Gone { edge: EdgeId(99) }]),
    );
    assert!(output.events.is_empty());
    assert!(output.delta.changes_only_the_tick());
}

// Section 2, joining, arriving and leaving.

/// Region A with edges E and F, and P1 joined through E.
fn one_player() -> Region {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let output = checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    assert!(matches!(
        output.player_events.as_slice(),
        [(p, PlayerEvent::Spawned { .. })] if *p == player(1)
    ));
    assert_eq!(edge_of(&region, player(1)), E);
    region
}

#[test]
fn a_join_through_another_edge_replaces_the_player() {
    let mut region = one_player();
    let old = entity_of(&region, player(1));
    let output = checked_tick(&mut region, &changes(vec![join(F, player(1))]));
    assert_eq!(
        removed(&output),
        vec![old],
        "the old entity is reported removed"
    );
    let new = entity_of(&region, player(1));
    assert_eq!(
        spawned(&output),
        vec![new],
        "the player enters the world anew"
    );
    assert!(matches!(
        output.player_events.as_slice(),
        [(p, PlayerEvent::Spawned { entity_id, .. })] if *p == player(1) && *entity_id == new
    ));
    assert_eq!(edge_of(&region, player(1)), F);
}

#[test]
fn a_join_through_the_same_edge_begins_a_new_stay_too() {
    // `docs/adr/0014-merging-and-splitting.md`, section 2.1: a join begins a new stay
    // whatever the region has. Until then a join through the player's own edge was
    // ignored.
    let mut region = one_player();
    let old = entity_of(&region, player(1));
    checked_tick(
        &mut region,
        &single_input(E, player(1), old, 6, dig(OWN_BLOCK, 40)),
    );
    let before = region.state();
    let output = checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    assert_eq!(
        removed(&output),
        vec![old],
        "the old entity is reported removed"
    );
    let new = entity_of(&region, player(1));
    assert_eq!(new, before.next_entity_id, "with the next id");
    assert_eq!(
        spawned(&output),
        vec![new],
        "the player enters the world anew"
    );
    assert!(matches!(
        output.player_events.as_slice(),
        [(p, PlayerEvent::Spawned { entity_id, .. })] if *p == player(1) && *entity_id == new
    ));
    let state = &region.state().players[&player(1)];
    assert_eq!((state.edge, state.last_input, state.handled), (E, 0, None));
}

#[test]
fn a_leave_through_the_players_edge_removes_them() {
    let mut region = one_player();
    let entity = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Leave(E, player(1), None, None)]),
    );
    assert_eq!(removed(&output), vec![entity]);
    assert!(region.player(player(1)).is_none());
    assert!(!region.state().players.contains_key(&player(1)));
}

#[test]
fn a_leave_through_another_edge_does_not_end_the_current_connection() {
    let mut region = one_player();
    let mut before = region.state();
    let output = checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Leave(F, player(1), None, None)]),
    );
    assert!(output.events.is_empty());
    before.tick += 1;
    assert_eq!(region.state(), before);
}

#[test]
fn a_leave_in_the_tick_of_the_join_removes_the_player() {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10)]));
    let output = checked_tick(
        &mut region,
        &changes(vec![
            join(E, player(1)),
            PlayerChange::Leave(E, player(1), None, None),
        ]),
    );
    assert!(region.player(player(1)).is_none());
    // Whatever was shown of the player within the tick is taken away again, so that
    // nobody is left seeing a ghost.
    let gone = removed(&output);
    for entity in spawned(&output) {
        assert!(gone.contains(&entity));
    }
}

#[test]
fn a_leave_of_a_player_the_edge_was_never_told_had_spawned_removes_them() {
    // The player quit while loading: the edge never saw `Spawned`, so it knows no entity.
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10)]));
    checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    let entity = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Leave(E, player(1), None, None)]),
    );
    assert_eq!(removed(&output), vec![entity]);
    assert!(region.player(player(1)).is_none());
}

#[test]
fn a_leave_of_the_earlier_connection_after_a_rejoin_in_the_same_tick_is_ignored() {
    // The player reconnects through F, and the leave of their connection through E
    // arrives after it.
    let mut region = one_player();
    let output = checked_tick(
        &mut region,
        &changes(vec![
            join(F, player(1)),
            PlayerChange::Leave(E, player(1), None, None),
        ]),
    );
    assert!(
        region.player(player(1)).is_some(),
        "the new connection stays"
    );
    assert_eq!(edge_of(&region, player(1)), F);
    let entity = entity_of(&region, player(1));
    assert!(!removed(&output).contains(&entity));
}

#[test]
fn a_leave_and_a_rejoin_through_the_same_edge_in_one_tick_enter_the_player_anew() {
    let mut region = one_player();
    let old = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &changes(vec![
            PlayerChange::Leave(E, player(1), None, None),
            join(E, player(1)),
        ]),
    );
    assert!(removed(&output).contains(&old));
    assert_eq!(edge_of(&region, player(1)), E);
    assert!(
        output
            .player_events
            .iter()
            .any(|(p, event)| *p == player(1) && matches!(event, PlayerEvent::Spawned { .. }))
    );
}

fn transfer(entity: EntityId, last_input: u64) -> PlayerTransfer {
    PlayerTransfer {
        entity_id: entity,
        name: "traveller".to_owned(),
        pose: Pose::at(Vec3::new(12.5, 64.0, 8.5)),
        hotbar: hotbar(),
        selected_slot: 0,
        last_input,
        hops: 0,
        flying: false,
        attempt: None,
    }
}

#[test]
fn an_arrival_makes_the_player_the_edges() {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let entity = EntityId(7_000_001);
    checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(
            F,
            player(5),
            transfer(entity, 17),
        )]),
    );
    let state = region.state().players[&player(5)].clone();
    assert_eq!(state.edge, F);
    assert_eq!(state.entity_id, entity, "the player keeps their entity");
    assert_eq!(state.last_input, 17);

    // Only F acts for them now: an input through E is ignored, one through F is taken.
    checked_tick(
        &mut region,
        &single_input(E, player(5), entity, 18, move_to(10.5)),
    );
    assert_eq!(region.state().players[&player(5)].last_input, 17);
    checked_tick(
        &mut region,
        &single_input(F, player(5), entity, 18, move_to(10.5)),
    );
    assert_eq!(region.state().players[&player(5)].last_input, 18);
}

#[test]
fn an_arrival_through_an_unknown_edge_reports_the_entity_removed() {
    let mut region = loaded(config_a(), CHUNK_A);
    let entity = EntityId(7_000_001);
    let output = checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(
            E,
            player(5),
            transfer(entity, 3),
        )]),
    );
    assert!(region.player(player(5)).is_none());
    assert_eq!(removed(&output), vec![entity]);
}

#[test]
fn an_input_numbered_not_above_the_last_applied_one_is_ignored() {
    let mut region = one_player();
    checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 5, move_to(12.5)),
    );
    assert_eq!(region.state().players[&player(1)].last_input, 5);
    checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 5, move_to(10.5)),
    );
    assert_eq!(region.player(player(1)).map(|p| p.1.position.x), Some(12.5));
}

// Section 2, where outbox entries go.

#[test]
fn departed_goes_to_the_outbox_of_the_players_edge() {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    checked_tick(&mut region, &changes(vec![join(F, player(1))]));
    let entity = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &single_input(F, player(1), entity, 4, move_to(17.5)),
    );
    match output.durable.as_slice() {
        [
            (
                F,
                1,
                Durable::Departed {
                    player: p,
                    transfer,
                    to,
                },
            ),
        ] => {
            assert_eq!(*p, player(1));
            assert_eq!(transfer.entity_id, entity);
            assert_eq!(transfer.last_input, 4);
            assert_eq!(*to, REGION_B);
        }
        other => panic!("expected one Departed for F, got {other:?}"),
    }
    assert!(region.edge(E).expect("E is known").outbox.is_empty());
    assert!(
        !removed(&output).contains(&entity),
        "a departing entity is not reported removed"
    );
}

#[test]
fn refused_goes_to_the_outbox_of_the_players_edge() {
    let one_id = EntityIds {
        first: EntityId(500),
        end: EntityId(501),
    };
    let mut region = Region::new(config_a(), one_id, holdings_a());
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let output = checked_tick(
        &mut region,
        &changes(vec![join(E, player(1)), join(F, player(2))]),
    );
    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::Refused {
                player: player(2),
                attempt: attempt(player(2)),
            }
        )]
    );
    assert!(region.player(player(2)).is_none());
    assert_eq!(region.state().next_entity_id, EntityId(501));
}

#[test]
fn a_players_own_remote_action_goes_to_the_outbox_of_their_edge() {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    checked_tick(&mut region, &changes(vec![join(F, player(1))]));
    let output = checked_tick(
        &mut region,
        &single_input(F, player(1), entity(1), 1, dig(BORDER_BLOCK_B, 9)),
    );
    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::Remote {
                action: RemoteAction {
                    player: player(1),
                    entity: entity(1),
                    sequence: 9,
                    step: RemoteStep::Break {
                        position: BORDER_BLOCK_B
                    },
                },
                to: Some(REGION_B),
            }
        )]
    );
}

#[test]
fn remote_done_goes_to_the_edge_the_action_came_from() {
    let mut region = loaded(config_b(), CHUNK_B);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    // P2 is in region B through F; the action is of P1, who is in region A through E.
    checked_tick(&mut region, &changes(vec![join(F, player(2))]));
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(E, remote_break(player(1), 6, BORDER_BLOCK_B))],
            ..TickInputs::default()
        },
    );
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::RemoteDone {
                player: player(1),
                entity: elsewhere(player(1)),
                sequence: 6
            }
        )]
    );
    assert!(output.events.contains(&RegionEvent::BlockChanged {
        position: BORDER_BLOCK_B,
        state: blocks::AIR,
    }));
    assert!(region.edge(F).expect("F is known").outbox.is_empty());
}

#[test]
fn a_remote_that_continues_a_remote_action_goes_to_the_edge_it_came_from() {
    let mut region = loaded(config_b(), CHUNK_B);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let target = BORDER_BLOCK_A.offset(0, 1, 0);
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(
                F,
                RemoteAction {
                    player: player(1),
                    entity: elsewhere(player(1)),
                    sequence: 3,
                    step: RemoteStep::PlaceAgainst {
                        against: BORDER_BLOCK_B,
                        target,
                        block: blocks::STONE,
                        placer: Vec3::new(10.5, 64.0, 8.5),
                    },
                },
            )],
            ..TickInputs::default()
        },
    );
    match output.durable.as_slice() {
        [(F, 1, Durable::Remote { action, to })] => {
            assert_eq!(action.player, player(1));
            assert_eq!(action.sequence, 3);
            assert_eq!(action.step.concerns(), target);
            assert_eq!(*to, Some(REGION_A));
        }
        other => panic!("expected one Remote for F, got {other:?}"),
    }
}

#[test]
fn a_remote_action_through_an_unknown_edge_is_ignored() {
    let mut region = loaded(config_b(), CHUNK_B);
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(E, remote_break(player(1), 6, BORDER_BLOCK_B))],
            ..TickInputs::default()
        },
    );
    assert!(output.durable.is_empty());
    assert!(region.edge(E).is_none());
}

#[test]
fn outbox_numbers_are_consecutive_per_edge_and_continue_across_ticks() {
    let mut region = loaded(config_b(), CHUNK_B);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let mut numbers = Vec::new();
    for round in 0..3 {
        let output = checked_tick(
            &mut region,
            &TickInputs {
                remote_actions: vec![
                    (E, remote_break(player(1), round * 3, BORDER_BLOCK_B)),
                    (F, remote_break(player(2), round * 3 + 1, BORDER_BLOCK_B)),
                    (E, remote_break(player(1), round * 3 + 2, BORDER_BLOCK_B)),
                ],
                ..TickInputs::default()
            },
        );
        numbers.extend(
            output
                .durable
                .iter()
                .map(|(edge, number, _)| (*edge, *number)),
        );
    }
    assert_eq!(
        numbers,
        vec![
            (E, 1),
            (F, 1),
            (E, 2),
            (E, 3),
            (F, 2),
            (E, 4),
            (E, 5),
            (F, 3),
            (E, 6),
        ]
    );
    assert_eq!(outbox_numbers(&region, E), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(outbox_numbers(&region, F), vec![1, 2, 3]);
}

// Section 2, `applied`.

#[test]
fn applied_is_noted_as_the_edges_applied() {
    let mut region = one_player();
    checked_tick(
        &mut region,
        &TickInputs {
            applied: vec![(E, 7), (F, 3)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.edge(E).map(|e| e.applied), Some(7));
    assert_eq!(region.edge(F).map(|e| e.applied), Some(3));

    // Of an edge the region does not know, nothing is noted.
    checked_tick(
        &mut region,
        &TickInputs {
            applied: vec![(EdgeId(99), 4)],
            ..TickInputs::default()
        },
    );
    assert!(region.edge(EdgeId(99)).is_none());
}

#[test]
fn applied_in_the_tick_of_a_reset_counts_for_the_new_start() {
    // Edge events come first, so the messages of the tick are the new start's.
    let mut region = one_player();
    checked_tick(
        &mut region,
        &TickInputs {
            applied: vec![(E, 7)],
            ..TickInputs::default()
        },
    );
    checked_tick(
        &mut region,
        &TickInputs {
            edges: vec![started(E, 20)],
            applied: vec![(E, 2)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.edge(E).map(|e| (e.start, e.applied)), Some((20, 2)));
}

// Section 1, `handled`.

#[test]
fn handled_covers_only_the_players_own_actions_on_blocks_of_this_region() {
    let mut region = one_player();
    assert_eq!(region.state().players[&player(1)].handled, None);

    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 5)),
    );
    assert_eq!(region.state().players[&player(1)].handled, Some(5));
    assert!(
        output
            .player_events
            .contains(&(player(1), PlayerEvent::Acknowledged { sequence: 5 }))
    );

    // Passed on to region B: not handled here, and not acknowledged.
    let output = checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 2, dig(BORDER_BLOCK_B, 6)),
    );
    assert_eq!(region.state().players[&player(1)].handled, Some(5));
    assert!(
        !output
            .player_events
            .iter()
            .any(|(_, event)| matches!(event, PlayerEvent::Acknowledged { .. }))
    );

    // Placing against a block of this region, into region B: also passed on.
    let output = checked_tick(
        &mut region,
        &single_input(
            E,
            player(1),
            entity(1),
            3,
            PlayerInput::UseItemOn {
                position: BORDER_BLOCK_A,
                face: Face::East,
                sequence: 7,
            },
        ),
    );
    assert!(matches!(
        output.durable.as_slice(),
        [(
            E,
            _,
            Durable::Remote {
                action: RemoteAction { sequence: 7, .. },
                ..
            }
        )]
    ));
    assert_eq!(region.state().players[&player(1)].handled, Some(5));

    checked_tick(
        &mut region,
        &single_input(
            E,
            player(1),
            entity(1),
            4,
            dig(OWN_BLOCK.offset(0, -1, 0), 8),
        ),
    );
    assert_eq!(region.state().players[&player(1)].handled, Some(8));
}

#[test]
fn a_remote_action_does_not_count_as_handled_for_a_player_of_this_region() {
    // P1 is in region B through E, and a remote action of P1 comes through F: it was
    // not one of P1's own actions in this region.
    let mut region = loaded(config_b(), CHUNK_B);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    checked_tick(&mut region, &changes(vec![join(E, player(1))]));
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(F, remote_break(player(1), 4, BORDER_BLOCK_B))],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.state().players[&player(1)].handled, None);
    assert!(
        output
            .player_events
            .iter()
            .all(|(_, event)| !matches!(event, PlayerEvent::Acknowledged { .. }))
    );
}

#[test]
fn a_player_who_enters_anew_starts_with_nothing_handled() {
    // A new connection numbers its actions from the start again, so what the earlier
    // connection had handled must not be acknowledged to it, nor answered in presence.
    let mut region = one_player();
    checked_tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 40)),
    );
    assert_eq!(region.state().players[&player(1)].handled, Some(40));
    checked_tick(&mut region, &changes(vec![join(F, player(1))]));
    assert_eq!(region.state().players[&player(1)].handled, None);
    let anew = entity_of(&region, player(1));
    let output = checked_tick(
        &mut region,
        &single_input(F, player(1), anew, 1, dig(OWN_BLOCK.offset(0, -1, 0), 1)),
    );
    assert!(
        output
            .player_events
            .contains(&(player(1), PlayerEvent::Acknowledged { sequence: 1 }))
    );
}

#[test]
fn an_arriving_player_starts_with_nothing_handled_here() {
    let mut region = loaded(config_a(), CHUNK_A);
    checked_tick(&mut region, &edges(vec![started(E, 10)]));
    checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(
            E,
            player(5),
            transfer(EntityId(7_000_001), 3),
        )]),
    );
    assert_eq!(region.state().players[&player(5)].handled, None);
}

#[test]
fn an_arrival_of_an_earlier_stay_of_a_player_the_region_has_leaves_them_as_they_are() {
    // `docs/adr/0014-merging-and-splitting.md`, section 2.1: of two stays of a player
    // the one with the lower entity id is the earlier. An arrival with a higher one
    // than the player has would take their place.
    let mut region = one_player();
    let entity = entity_of(&region, player(1));
    let mut before = region.state();
    let other = EntityId(entity.0 - 1);
    let output = checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(F, player(1), transfer(other, 9))]),
    );
    assert_eq!(
        removed(&output),
        vec![other],
        "the entity on its way is removed"
    );
    assert_eq!(entity_of(&region, player(1)), entity);
    before.tick += 1;
    assert_eq!(region.state(), before);
}

#[test]
fn a_discard_changes_nothing_of_the_state() {
    let mut region = one_player();
    let entity = EntityId(7_000_001);
    let output = checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Discard {
            entity,
            chunk: CHUNK_A,
        }]),
    );
    assert_eq!(
        output.events,
        vec![RegionEvent::EntityRemoved {
            entity,
            chunk: CHUNK_A
        }]
    );
    assert!(output.delta.changes_only_the_tick());
}

#[test]
fn confirmations_out_of_order_in_one_tick_drop_up_to_the_highest() {
    let mut region = three_entries();
    checked_tick(
        &mut region,
        &edges(vec![
            EdgeEvent::Confirmed { edge: E, number: 2 },
            EdgeEvent::Confirmed { edge: E, number: 1 },
        ]),
    );
    assert_eq!(outbox_numbers(&region, E), vec![3]);
}

#[test]
fn a_confirmation_before_a_reset_in_one_tick_leaves_the_new_start_clean() {
    let mut region = three_entries();
    let mut inputs = edges(vec![
        EdgeEvent::Confirmed { edge: E, number: 2 },
        started(E, 20),
    ]);
    inputs.change(join(E, player(1)));
    // The join of this tick makes P1 the second player to enter the region.
    inputs.input(E, player(1), entity(2), 1, dig(BORDER_BLOCK_B, 1));
    inputs.input(E, player(1), entity(2), 2, dig(BORDER_BLOCK_B, 2));
    checked_tick(&mut region, &inputs);
    assert_eq!(outbox_numbers(&region, E), vec![1, 2]);
}

#[test]
fn refusals_go_in_the_order_of_the_joins_and_use_no_entity_id() {
    let one_id = EntityIds {
        first: EntityId(500),
        end: EntityId(501),
    };
    let mut region = Region::new(config_a(), one_id, holdings_a());
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let output = checked_tick(
        &mut region,
        &changes(vec![
            join(E, player(1)),
            join(F, player(3)),
            join(E, player(2)),
            join(F, player(4)),
        ]),
    );
    let refused = |number| Durable::Refused {
        player: player(number),
        attempt: attempt(player(number)),
    };
    assert_eq!(
        output.durable,
        vec![(F, 1, refused(3)), (E, 1, refused(2)), (F, 2, refused(4)),]
    );
    assert_eq!(region.state().next_entity_id, EntityId(501));
}

#[test]
fn a_remote_action_on_a_chunk_that_is_not_loaded_is_still_answered() {
    let mut region = Region::new(config_b(), ids(), holdings_b());
    checked_tick(&mut region, &edges(vec![started(E, 10)]));
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(E, remote_break(player(1), 6, BORDER_BLOCK_B))],
            ..TickInputs::default()
        },
    );
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::RemoteDone {
                player: player(1),
                entity: elsewhere(player(1)),
                sequence: 6
            }
        )]
    );
}

#[test]
fn a_remote_action_for_another_region_is_answered_for_the_edge_it_came_from() {
    // Answered one for one: an action that is not this region's to take is passed on.
    let mut region = loaded(config_b(), CHUNK_B);
    checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(F, remote_break(player(1), 6, OWN_BLOCK))],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.durable.len(), 1);
    assert_eq!(output.durable[0].0, F);
    // The store has said that the block's chunk is region A's: the action is for it.
    assert_eq!(
        output.durable[0].2,
        Durable::NotMine {
            what: Misdirected::Remote(remote_break(player(1), 6, OWN_BLOCK)),
            holder: REGION_A,
        }
    );
}

#[test]
fn a_reset_does_not_report_removed_a_departed_entity_that_came_back_through_another_edge() {
    // P2 walked into region B and straight back, arriving through F, while their
    // `Departed` waited unconfirmed in E's outbox. The record says the entity of every
    // `Departed` in a reset outbox is reported removed; here that entity is alive in the
    // region under another edge.
    let (mut region, departing) = handing_over();
    checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(
            F,
            player(2),
            transfer(departing, 1),
        )]),
    );
    assert_eq!(entity_of(&region, player(2)), departing);
    let output = checked_tick(&mut region, &edges(vec![started(E, 20)]));
    assert!(region.player(player(2)).is_some(), "F's player stays");
    assert!(
        !removed(&output).contains(&departing),
        "a living entity of another edge's player is reported removed"
    );
}

#[test]
fn gone_does_not_report_removed_a_departed_entity_that_came_back_through_another_edge() {
    // As above, with the edge forgotten instead of reset.
    let (mut region, departing) = handing_over();
    checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(
            F,
            player(2),
            transfer(departing, 1),
        )]),
    );
    let output = checked_tick(&mut region, &edges(vec![EdgeEvent::Gone { edge: E }]));
    assert!(region.player(player(2)).is_some());
    assert!(!removed(&output).contains(&departing));
}

#[test]
fn a_reset_reports_a_departed_entity_that_came_back_through_the_same_edge_removed_once() {
    // P2 is back through E itself, so the reset removes them as one of E's players. Their
    // `Departed` in E's outbox names the same entity, which must not be reported again.
    let (mut region, departing) = handing_over();
    checked_tick(
        &mut region,
        &changes(vec![PlayerChange::Arrive(
            E,
            player(2),
            transfer(departing, 1),
        )]),
    );
    let output = checked_tick(&mut region, &edges(vec![started(E, 20)]));
    assert!(region.player(player(2)).is_none());
    let times = removed(&output)
        .iter()
        .filter(|entity| **entity == departing)
        .count();
    assert_eq!(times, 1);
}

#[test]
fn a_reset_does_not_report_a_confirmed_departure_removed() {
    // Once the edge has confirmed the Departed, the entity is the next region's concern.
    let (mut region, departing) = handing_over();
    let p1 = entity_of(&region, player(1));
    checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Confirmed { edge: E, number: 1 }]),
    );
    let output = checked_tick(&mut region, &edges(vec![started(E, 20)]));
    assert_eq!(removed(&output), vec![p1]);
    assert!(!removed(&output).contains(&departing));
}

// Orderings within a tick.

#[test]
fn a_reset_and_a_join_of_the_same_edge_in_one_tick_enter_the_player_anew() {
    // The edge restarted and its player connected again before the tick: the reset
    // removes the old player, and the join is not taken for one under the same edge.
    let mut region = one_player();
    let old = entity_of(&region, player(1));
    let mut inputs = edges(vec![started(E, 20)]);
    inputs.change(join(E, player(1)));
    let output = checked_tick(&mut region, &inputs);
    assert!(removed(&output).contains(&old));
    assert!(region.player(player(1)).is_some());
    assert_eq!(edge_of(&region, player(1)), E);
    assert!(
        output
            .player_events
            .iter()
            .any(|(p, event)| *p == player(1) && matches!(event, PlayerEvent::Spawned { .. }))
    );
    let new = entity_of(&region, player(1));
    assert!(spawned(&output).contains(&new));
}

#[test]
fn a_join_in_the_tick_its_edge_is_first_started_is_taken() {
    let mut region = loaded(config_a(), CHUNK_A);
    let mut inputs = edges(vec![started(E, 10)]);
    inputs.change(join(E, player(1)));
    checked_tick(&mut region, &inputs);
    assert_eq!(edge_of(&region, player(1)), E);
}

#[test]
fn a_confirmation_in_the_tick_of_a_reset_does_not_drop_the_new_entries() {
    let (mut region, _) = handing_over();
    let mut inputs = edges(vec![
        started(E, 20),
        EdgeEvent::Confirmed { edge: E, number: 1 },
    ]);
    inputs.change(join(E, player(1)));
    // The join of this tick makes P1 the fourth player to enter the region.
    inputs.input(E, player(1), entity(4), 1, dig(BORDER_BLOCK_B, 1));
    let output = checked_tick(&mut region, &inputs);
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::Remote { .. })]
    ));
    assert_eq!(outbox_numbers(&region, E), vec![1]);
}

#[test]
fn a_departure_in_the_tick_of_a_reset_survives_it() {
    // F's player walks out while E is reset: only E's outbox is dropped.
    let (mut region, _) = handing_over();
    let mut inputs = edges(vec![started(E, 20)]);
    inputs.input(F, player(3), entity(3), 1, move_to(17.5));
    let output = checked_tick(&mut region, &inputs);
    assert!(matches!(
        output.durable.as_slice(),
        [(F, 1, Durable::Departed { .. })]
    ));
}

// Section 2, `Region::state` and `Region::restore`.

#[test]
fn a_new_region_is_the_restored_state_of_one_that_never_ran() {
    let region = Region::new(config_a(), ids(), holdings_a());
    assert_eq!(region.state(), RegionState::new(ids()));
    let restored = Region::restore(config_a(), RegionState::new(ids()), holdings_a());
    assert_eq!(restored.state(), region.state());
}

#[test]
fn a_restored_region_has_the_state_it_was_restored_from_and_no_chunk() {
    let (region, _) = handing_over();
    let state = region.state();
    let restored = Region::restore(config_a(), state.clone(), holdings_a());
    assert_eq!(restored.state(), state);
    assert_eq!(restored.tick_number(), state.tick);
    assert_eq!(restored.loaded_chunk_count(), 0);
    assert_eq!(restored.player(player(1)), region.player(player(1)));
    assert_eq!(restored.edge(E), region.edge(E));
    // Of chunks it knows what the store says it holds and nothing else: what the
    // original believed of its neighbour's chunk is not part of a state.
    assert_eq!(region.knowledge(CHUNK_B), Knowledge::Foreign(REGION_B));
    assert_eq!(restored.knowledge(CHUNK_A), Knowledge::Held);
    assert_eq!(restored.knowledge(CHUNK_B), Knowledge::Unknown);
    assert_eq!(restored.held_chunk_count(), 1);
}

/// The inputs of a scenario with a bit of everything: players of two edges, entries
/// that stay unconfirmed, a hand-over, a refusal, remote actions both ways, a reset, a
/// confirmation, a rejoin and a `Gone`.
fn scenario() -> Vec<TickInputs> {
    let mut script = Vec::new();
    script.push(edges(vec![started(E, 10), started(F, 10)]));
    script.push(changes(vec![
        join(E, player(1)),
        join(E, player(2)),
        join(F, player(3)),
    ]));
    let mut inputs = TickInputs::default();
    inputs.input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 1));
    inputs.input(F, player(3), entity(3), 1, dig(BORDER_BLOCK_B, 1));
    inputs.applied = vec![(E, 4), (F, 2)];
    script.push(inputs);
    // P2 walks into region B and stays in E's outbox, unconfirmed.
    script.push(single_input(E, player(2), entity(2), 1, move_to(17.5)));
    let mut inputs = TickInputs {
        remote_actions: vec![(F, remote_break(player(9), 4, BORDER_BLOCK_A))],
        ..TickInputs::default()
    };
    inputs.input(E, player(1), entity(1), 2, move_to(12.5));
    inputs.input(
        F,
        player(3),
        entity(3),
        2,
        PlayerInput::SelectSlot { slot: 3 },
    );
    script.push(inputs);
    script.push(TickInputs::default());
    script.push(edges(vec![EdgeEvent::Confirmed { edge: F, number: 1 }]));
    script.push(changes(vec![
        join(F, player(1)),
        PlayerChange::Leave(E, player(1), None, None),
    ]));
    let mut inputs = TickInputs::default();
    // P1 was the fourth to enter the region when they joined through F.
    inputs.input(
        F,
        player(1),
        entity(4),
        1,
        PlayerInput::UseItemOn {
            position: BORDER_BLOCK_A,
            face: Face::East,
            sequence: 2,
        },
    );
    inputs.input(
        F,
        player(3),
        entity(3),
        3,
        dig(OWN_BLOCK.offset(-1, 0, 0), 3),
    );
    script.push(inputs);
    script.push(edges(vec![started(F, 20)]));
    script.push(changes(vec![join(F, player(3)), join(E, player(4))]));
    script.push(edges(vec![EdgeEvent::Gone { edge: E }]));
    // After the reset, P3 was the fifth to enter the region.
    script.push(single_input(F, player(3), entity(5), 1, move_to(13.5)));
    script.push(edges(vec![started(E, 30)]));
    script.push(changes(vec![join(E, player(2))]));
    script
}

/// The parts of a tick's output that can be serialised.
fn serialised(output: &TickOutput) -> Vec<u8> {
    postcard::to_stdvec(&(
        output.tick,
        &output.player_events,
        &output.events,
        &output.chunk_requests,
        &output.durable,
        &output.delta,
    ))
    .expect("the output serialises")
}

#[test]
fn a_restored_region_carries_on_as_the_original_from_every_tick_of_a_scenario() {
    let script = scenario();
    for restore_after in 0..=script.len() {
        let mut original = loaded(config_a(), CHUNK_A);
        for inputs in &script[..restore_after] {
            checked_tick(&mut original, inputs);
        }
        let state = original.state();
        let mut restored = Region::restore(config_a(), state.clone(), holdings_a());
        assert_eq!(restored.state(), state);

        // The restored region is given its tickets again, loads its chunk as the
        // original has it, which is what the store would deliver, and is told once more
        // what it has to ask again: whose the chunk across the border is. The original
        // goes on idly meanwhile; nothing but the chunk requests and the claim may
        // differ.
        let chunk = original.chunk(CHUNK_A).expect("loaded").clone();
        let viewer = |chunk| (chunk, Ticket::Viewer);
        let loading = [
            TickInputs {
                tickets_added: vec![viewer(CHUNK_A), viewer(CHUNK_B)],
                ..TickInputs::default()
            },
            TickInputs {
                chunks_loaded: vec![(CHUNK_A, chunk)],
                foreign: vec![(CHUNK_B, REGION_B)],
                ..TickInputs::default()
            },
        ];
        for inputs in &loading {
            let mut expected = checked_tick(&mut original, &TickInputs::default());
            let mut actual = checked_tick(&mut restored, inputs);
            expected.chunk_requests.clear();
            actual.chunk_requests.clear();
            actual.claims.clear();
            assert_eq!(actual, expected, "restored after tick {restore_after}");
        }
        assert_eq!(restored.chunk(CHUNK_A), original.chunk(CHUNK_A));
        for chunk in [CHUNK_A, CHUNK_B] {
            assert_eq!(restored.knowledge(chunk), original.knowledge(chunk));
        }

        for inputs in &script[restore_after..] {
            let expected = checked_tick(&mut original, inputs);
            let actual = checked_tick(&mut restored, inputs);
            assert_eq!(
                actual, expected,
                "restored after tick {restore_after}, at tick {}",
                expected.tick
            );
            assert_eq!(restored.state(), original.state());
            assert_eq!(
                restored.entities().collect::<Vec<_>>(),
                original.entities().collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a_region_restored_mid_hand_over_reports_the_departing_entity_on_a_reset() {
    let (original, departing) = handing_over();
    let mut restored = Region::restore(config_a(), original.state(), holdings_a());
    let output = checked_tick(&mut restored, &edges(vec![started(E, 20)]));
    assert!(removed(&output).contains(&departing));
}

#[test]
fn a_region_restored_mid_hand_over_reports_the_departing_entity_when_its_edge_is_gone() {
    let (original, departing) = handing_over();
    let mut restored = Region::restore(config_a(), original.state(), holdings_a());
    let output = checked_tick(&mut restored, &edges(vec![EdgeEvent::Gone { edge: E }]));
    assert!(removed(&output).contains(&departing));
}

#[test]
fn a_restored_region_keeps_unconfirmed_entries_and_numbers_on() {
    let original = three_entries();
    let mut restored = Region::restore(config_a(), original.state(), holdings_a());
    assert_eq!(outbox_numbers(&restored, E), vec![1, 2, 3]);
    checked_tick(
        &mut restored,
        &edges(vec![EdgeEvent::Confirmed { edge: E, number: 1 }]),
    );
    let output = checked_tick(
        &mut restored,
        &TickInputs {
            remote_actions: vec![(E, remote_break(player(8), 1, BORDER_BLOCK_B))],
            ..TickInputs::default()
        },
    );
    // The answer is the fourth entry. The block is of a chunk whose holder the restored
    // region has not heard again, so the action goes on without a region named.
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 4, Durable::Remote { to: None, .. })]
    ));
    assert_eq!(outbox_numbers(&restored, E), vec![2, 3, 4]);
}

// Section 2: a tick is a function of the region and its inputs.

#[test]
fn the_same_inputs_give_byte_identical_outputs_and_states() {
    let run = || {
        let mut region = loaded(config_a(), CHUNK_A);
        let mut bytes = Vec::new();
        for inputs in scenario() {
            let output = checked_tick(&mut region, &inputs);
            bytes.push(serialised(&output));
            bytes.push(postcard::to_stdvec(&region.state()).expect("the state serialises"));
        }
        bytes
    };
    assert_eq!(run(), run());
}

#[test]
fn a_state_and_a_delta_survive_serialisation() {
    // The store keeps them as postcard bytes (section 3), so they must read back equal.
    let mut region = loaded(config_a(), CHUNK_A);
    for inputs in scenario() {
        let output = checked_tick(&mut region, &inputs);
        let bytes = postcard::to_stdvec(&output.delta).expect("the delta serialises");
        let delta: clustine_sim::StateDelta = postcard::from_bytes(&bytes).expect("reads back");
        assert_eq!(delta, output.delta);
        let state = region.state();
        let bytes = postcard::to_stdvec(&state).expect("the state serialises");
        let back: RegionState = postcard::from_bytes(&bytes).expect("reads back");
        assert_eq!(back, state);
    }
}

// ---------------------------------------------------------------------------------------
// Since when a region knows an edge
//
// From section 2.6 of `docs/adr/0012-the-tick-on-chunks.md` and scenario S16 of its
// section 8, by someone who has not read how the region does it.
// ---------------------------------------------------------------------------------------

/// A region that has not run a tick. The one place below that makes a region without
/// the fixtures above, so that a change to how a region is made is a change here.
fn never_ran() -> Region {
    Region::new(config_a(), ids(), holdings_a())
}

/// The region as another owner has it, who was given `state` by the store.
fn restored_from(state: RegionState) -> Region {
    Region::restore(config_a(), state, holdings_a())
}

fn since(region: &Region, edge: EdgeId) -> u64 {
    region.edge(edge).expect("the edge is known").since
}

/// Lets `ticks` ticks pass in which nothing happens, so that the number of a tick is
/// not by chance that of a fixture's.
fn idle(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked_tick(region, &TickInputs::default());
    }
}

/// What the delta of a tick says of `edge`, if the edge changed and is still known.
fn edge_delta(output: &TickOutput, edge: EdgeId) -> Option<&clustine_sim::EdgeDelta> {
    output
        .delta
        .edges
        .iter()
        .find(|(id, _)| *id == edge)
        .and_then(|(_, change)| change.as_ref())
}

#[test]
fn since_is_the_number_of_the_tick_that_noted_the_edge() {
    let mut region = loaded(config_a(), CHUNK_A);
    idle(&mut region, 5);
    let before = region.tick_number();

    let output = checked_tick(&mut region, &edges(vec![started(E, 10)]));
    assert_eq!(output.tick, before + 1);
    assert_eq!(since(&region, E), before + 1);

    // Another edge, noted later, has the number of its own tick, and the first keeps
    // its own.
    idle(&mut region, 3);
    let output = checked_tick(&mut region, &edges(vec![started(F, 10)]));
    assert_eq!(since(&region, F), output.tick);
    assert_eq!(since(&region, F), before + 5);
    assert_eq!(since(&region, E), before + 1);
}

#[test]
fn an_edge_noted_in_a_regions_first_tick_is_known_since_tick_one() {
    // No state is made in tick 0, so 0 stays free for an edge that has been told none.
    let mut region = never_ran();
    assert_eq!(region.tick_number(), 0);
    let output = checked_tick(&mut region, &edges(vec![started(E, 10)]));
    assert_eq!(output.tick, 1);
    assert_eq!(since(&region, E), 1);
}

#[test]
fn two_edges_noted_in_one_tick_are_known_since_that_tick() {
    let mut region = loaded(config_a(), CHUNK_A);
    idle(&mut region, 2);
    let output = checked_tick(&mut region, &edges(vec![started(E, 10), started(F, 77)]));
    assert_eq!(since(&region, E), output.tick);
    assert_eq!(since(&region, F), output.tick);
}

#[test]
fn a_higher_start_gives_a_new_since() {
    let (mut region, _) = handing_over();
    let first = since(&region, E);
    let others = since(&region, F);
    assert!(first > 0);
    idle(&mut region, 2);

    let output = checked_tick(&mut region, &edges(vec![started(E, 20)]));
    assert!(output.tick > first);
    assert_eq!(since(&region, E), output.tick);
    assert_eq!(since(&region, F), others, "another edge keeps its own");

    // And again for the next start, each state with the number of the tick that made it.
    let second = output.tick;
    idle(&mut region, 1);
    let output = checked_tick(&mut region, &edges(vec![started(E, 21)]));
    assert_eq!(since(&region, E), output.tick);
    assert_eq!(output.tick, second + 2);
}

#[test]
fn the_same_start_and_a_lower_one_leave_since() {
    let (mut region, _) = handing_over();
    let noted = since(&region, E);
    assert!(noted < region.tick_number(), "the fixture ran on after it");

    let start = region.edge(E).expect("the edge is known").start;
    for again in [start, start - 1, start, 0] {
        let output = checked_tick(&mut region, &edges(vec![started(E, again)]));
        assert_eq!(since(&region, E), noted, "after start {again}");
        assert!(
            edge_delta(&output, E).is_none_or(|delta| delta.since == noted),
            "the delta of a tick that left the edge alone names another since"
        );
    }
    // Several times in one tick, the same.
    checked_tick(
        &mut region,
        &edges(vec![started(E, start), started(E, start)]),
    );
    assert_eq!(since(&region, E), noted);
}

#[test]
fn a_confirmation_leaves_since() {
    let mut region = three_entries();
    let noted = since(&region, E);
    assert!(noted > 0 && noted < region.tick_number());

    for number in [1, 1, 3, 99] {
        let output = checked_tick(
            &mut region,
            &edges(vec![EdgeEvent::Confirmed { edge: E, number }]),
        );
        assert_eq!(since(&region, E), noted, "after confirming {number}");
        assert!(
            edge_delta(&output, E).is_none_or(|delta| delta.since == noted),
            "the delta that drops entries carries the since the edge has"
        );
    }
    assert_eq!(outbox_numbers(&region, E), Vec::<u64>::new());
}

#[test]
fn applied_leaves_since() {
    let mut region = one_player();
    let noted = since(&region, E);
    let others = since(&region, F);
    for number in [1, 7, 7, 8] {
        let output = checked_tick(
            &mut region,
            &TickInputs {
                applied: vec![(E, number)],
                ..TickInputs::default()
            },
        );
        assert_eq!(region.edge(E).map(|edge| edge.applied), Some(number));
        assert_eq!(since(&region, E), noted, "after applied {number}");
        assert!(
            edge_delta(&output, E).is_none_or(|delta| delta.since == noted),
            "the delta that notes what is applied carries the since the edge has"
        );
    }
    assert_eq!(since(&region, F), others);
}

#[test]
fn making_outbox_entries_and_what_players_do_leave_since() {
    // Both fixtures ran ticks with joins, inputs and new outbox entries, and one of them
    // a hand-over, after the tick that noted the edge.
    let (region, _) = handing_over();
    let noted = since(&region, E);
    assert!(noted > 0 && noted < region.tick_number());
    assert!(region.edge(E).expect("the edge is known").sent > 0);

    let mut region = three_entries();
    let noted = since(&region, E);
    assert!(noted > 0 && noted < region.tick_number());
    // One more entry, through a remote action that the edge passed on.
    let output = checked_tick(
        &mut region,
        &TickInputs {
            remote_actions: vec![(E, remote_break(player(8), 1, OWN_BLOCK))],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.durable.len(), 1, "the remote action is answered");
    assert_eq!(since(&region, E), noted);
    assert_eq!(
        edge_delta(&output, E).map(|delta| delta.since),
        Some(noted),
        "the delta that adds an entry carries the since the edge has"
    );
}

#[test]
fn an_edge_that_is_gone_and_comes_back_in_a_later_tick_has_the_number_of_that_tick() {
    let (mut region, _) = handing_over();
    let first = since(&region, E);
    let start = region.edge(E).expect("the edge is known").start;
    let others = since(&region, F);

    checked_tick(&mut region, &edges(vec![EdgeEvent::Gone { edge: E }]));
    assert!(region.edge(E).is_none());
    idle(&mut region, 3);

    // With the start it had: it is the tick that makes the state that counts.
    let output = checked_tick(&mut region, &edges(vec![started(E, start)]));
    assert_eq!(since(&region, E), output.tick);
    assert!(output.tick > first, "a since is not given out twice");
    assert_eq!(since(&region, F), others);

    // And with a lower start than it had, as the region knows nothing of it any more.
    checked_tick(&mut region, &edges(vec![EdgeEvent::Gone { edge: E }]));
    let output = checked_tick(&mut region, &edges(vec![started(E, start - 1)]));
    assert_eq!(
        region.edge(E).map(|edge| (edge.start, edge.since)),
        Some((start - 1, output.tick))
    );
}

#[test]
fn an_edge_that_is_gone_and_comes_back_in_the_same_tick_has_the_number_of_that_tick() {
    let (mut region, _) = handing_over();
    let first = since(&region, E);
    let start = region.edge(E).expect("the edge is known").start;
    let others = since(&region, F);
    idle(&mut region, 2);

    let output = checked_tick(
        &mut region,
        &edges(vec![EdgeEvent::Gone { edge: E }, started(E, start)]),
    );
    assert_eq!(since(&region, E), output.tick);
    assert_ne!(since(&region, E), first);
    assert_eq!(since(&region, F), others);
    assert_eq!(
        edge_delta(&output, E).map(|delta| (delta.start, delta.since)),
        Some((start, output.tick)),
        "the delta has the state that the tick made"
    );
}

#[test]
fn an_edge_noted_gone_and_noted_again_within_one_tick_has_the_number_of_that_tick() {
    let mut region = loaded(config_a(), CHUNK_A);
    idle(&mut region, 1);
    let output = checked_tick(
        &mut region,
        &edges(vec![
            started(E, 10),
            EdgeEvent::Gone { edge: E },
            started(E, 10),
            started(E, 10),
        ]),
    );
    assert_eq!(since(&region, E), output.tick);
}

#[test]
fn the_delta_of_the_tick_that_makes_a_state_for_an_edge_carries_its_since() {
    let mut region = loaded(config_a(), CHUNK_A);
    idle(&mut region, 4);

    let output = checked_tick(&mut region, &edges(vec![started(E, 10)]));
    assert_eq!(
        edge_delta(&output, E).map(|delta| (delta.start, delta.since)),
        Some((10, output.tick))
    );

    // A state that has only the deltas to go by comes to the same since: what the store
    // has of a region between two checkpoints is its deltas.
    let mut from_deltas = region.state();
    idle(&mut region, 2);
    let reset = checked_tick(&mut region, &edges(vec![started(E, 11)]));
    assert_eq!(
        edge_delta(&reset, E).map(|delta| (delta.start, delta.since)),
        Some((11, reset.tick))
    );
    from_deltas.apply(&reset.delta);
    assert_eq!(from_deltas.edges[&E].since, reset.tick);
    assert_eq!(from_deltas.edges[&E], region.state().edges[&E]);

    // An edge the state has never heard of is made by the delta alone.
    let mut empty = RegionState::new(ids());
    empty.apply(&reset.delta);
    assert_eq!(empty.edges[&E].since, reset.tick);
}

#[test]
fn since_changes_only_when_an_edge_is_noted_or_reset_in_every_tick_of_a_scenario() {
    // What section 2.6 says, played beside the region: the start and the since of every
    // edge, changed by `Started` for an edge that is not known or has a lower start, and
    // by nothing else.
    let mut region = loaded(config_a(), CHUNK_A);
    let mut expected: std::collections::BTreeMap<EdgeId, (u64, u64)> = Default::default();
    let mut made = 0;
    for inputs in scenario() {
        let output = checked_tick(&mut region, &inputs);
        for event in &inputs.edges {
            match event {
                EdgeEvent::Started { edge, start } => {
                    let known = expected.get(edge).map(|(start, _)| *start);
                    if known.is_none_or(|known| known < *start) {
                        expected.insert(*edge, (*start, output.tick));
                        made += 1;
                    }
                }
                EdgeEvent::Gone { edge } => {
                    expected.remove(edge);
                }
                EdgeEvent::Confirmed { .. } => {}
            }
        }
        let actual: std::collections::BTreeMap<EdgeId, (u64, u64)> = region
            .state()
            .edges
            .iter()
            .map(|(edge, state)| (*edge, (state.start, state.since)))
            .collect();
        assert_eq!(actual, expected, "after tick {}", output.tick);
    }
    assert!(
        made >= 4,
        "the scenario notes two edges, resets one and brings one back"
    );
}

#[test]
fn a_restored_region_keeps_each_since_and_gives_a_new_state_its_own_tick() {
    let (original, _) = handing_over();
    let noted = since(&original, E);
    let start = original.edge(E).expect("the edge is known").start;

    let mut restored = restored_from(original.state());
    assert_eq!(since(&restored, E), noted);
    assert_eq!(since(&restored, F), since(&original, F));

    // The edge says hello to the new owner with the start it has: nothing changes.
    checked_tick(&mut restored, &edges(vec![started(E, start)]));
    assert_eq!(since(&restored, E), noted);

    // A state made by the new owner has the number of the tick that makes it, which
    // goes on from the ticks of the owner before.
    let output = checked_tick(&mut restored, &edges(vec![started(E, start + 1)]));
    assert_eq!(output.tick, original.tick_number() + 2);
    assert_eq!(since(&restored, E), output.tick);
    assert_eq!(since(&restored, F), since(&original, F));
}

#[test]
fn a_state_and_a_delta_keep_their_since_through_serialisation() {
    // The next owner compares a hello with the since the store gives back, so it has to
    // come back as it went in.
    let mut region = loaded(config_a(), CHUNK_A);
    idle(&mut region, 3);
    let output = checked_tick(&mut region, &edges(vec![started(E, 10)]));

    let bytes = postcard::to_stdvec(&output.delta).expect("the delta serialises");
    let delta: clustine_sim::StateDelta = postcard::from_bytes(&bytes).expect("reads back");
    let carried = delta
        .edges
        .iter()
        .find(|(id, _)| *id == E)
        .and_then(|(_, change)| change.as_ref())
        .map(|change| change.since);
    assert_eq!(carried, Some(output.tick));

    let bytes = postcard::to_stdvec(&region.state()).expect("the state serialises");
    let state: RegionState = postcard::from_bytes(&bytes).expect("reads back");
    assert_eq!(state.edges[&E].since, output.tick);
}
