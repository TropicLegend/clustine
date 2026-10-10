//! Tests of the simulation's part of merging and splitting, written from section 2 and
//! the scenarios S19 to S37 of `docs/adr/0014-merging-and-splitting.md` and the public
//! API alone, by someone who has not read how the region does it.
//!
//! The tests play the store and the edges, as those of `chunks.rs` do. Four things are
//! second implementations of what the record says, which the region is held to:
//! [`Stays`] is section 2.1, who is in a region and with which entity;
//! [`absorbed_as_the_record_says`] is section 2.3's `absorb`, step for step;
//! [`split_as_the_record_says`] is section 2.4's `split`; and [`Cluster`] is an edge
//! and a store around several regions that merge and split, by section 8 as far as a
//! test that sees everything needs it, which is compared with one region that holds
//! everything and is given the same joins, inputs and leaves (section 2.6).
//!
//! The runs of the cluster are made up by a generator, five to a test. The environment
//! variable `CLUSTINE_RESHAPE_SEEDS` says how many instead, and `CLUSTINE_RESHAPE_SEED`
//! names the one to run; a run that fails prints its seed and its steps.
//!
//! A test that is marked `ignore` with "finding" says what the record says and fails:
//! it stays as it is until the region or the record is changed.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks, items};
use clustine_sim::api::{
    Face, HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent, RemoteAction, RemoteStep,
};
use clustine_sim::{
    Durable, EdgeEvent, EdgeState, Entered, EnteringState, Holdings, Knowledge, Misdirected,
    NoSplit, Part, Place, PlayerChange, PlayerEvent, PlayerJoin, PlayerState, PlayerTransfer,
    Region, RegionConfig, RegionState, Sides, Splitting, StayNote, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Section, Vec3,
};
use uuid::Uuid;

const E: EdgeId = EdgeId(1);
const F: EdgeId = EdgeId(2);
/// An edge no region of these tests is told of.
const STRANGER: EdgeId = EdgeId(99);

/// The chunk players enter the world in, and the chunks around it. On stripes the line
/// runs between `HOME` and `EAST`: all but `EAST` are of the western stripe.
const HOME: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);
const WEST: ChunkPos = ChunkPos::new(-1, 0);
const NORTH: ChunkPos = ChunkPos::new(0, -1);
const SOUTH: ChunkPos = ChunkPos::new(0, 1);

/// The regions of the stripes, as the world store numbers them from west to east, a
/// third that holds what a test says it does, and the region a split makes.
const REGION_A: RegionId = RegionId(0);
const REGION_B: RegionId = RegionId(1);
const OTHER: RegionId = RegionId(7);
const PART: RegionId = RegionId(12);

/// Where players enter the world: on the stone of `HOME`, two blocks from `EAST`.
const SPAWN: Vec3 = Vec3::new(14.5, 64.0, 8.5);

/// A block at the top of the stone near the spawn point.
const OWN_BLOCK: BlockPos = BlockPos::new(14, 63, 8);

/// The western stripe, which region 0 is pinned to, and the rest, which region 1 is.
const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// The entity of a player who comes from elsewhere: above every id of [`ids`].
const TRAVELLER: EntityId = EntityId(7_000_001);

/// Points in the chunks around the spawn point.
const IN_HOME: Vec3 = Vec3::new(12.5, 64.0, 8.5);
const IN_EAST: Vec3 = Vec3::new(20.5, 64.0, 8.5);
const IN_WEST: Vec3 = Vec3::new(-8.5, 64.0, 8.5);
const IN_SOUTH: Vec3 = Vec3::new(8.5, 64.0, 20.5);
const IN_NORTH: Vec3 = Vec3::new(8.5, 64.0, -3.5);

/// Where a player stands to reach across into `SOUTH`, a block of `SOUTH` within reach
/// from there, and the block of `HOME` that touches it.
const BY_SOUTH: Vec3 = Vec3::new(8.5, 64.0, 14.5);
const SOUTH_BLOCK: BlockPos = BlockPos::new(8, 63, 16);
const BY_SOUTH_BLOCK: BlockPos = BlockPos::new(8, 63, 15);

/// Where the player of a remote action stands: it is not looked at by the region that
/// takes a break, and is out of the way of every placement here.
const ELSEWHERE: Vec3 = Vec3::new(18.5, 64.0, 8.5);

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

fn config(return_after: u64) -> RegionConfig {
    RegionConfig {
        spawn: SPAWN,
        starting_hotbar: hotbar(),
        return_after,
        place_by_store: false,
        lowest_y: -64,
    }
}

fn ids() -> EntityIds {
    EntityIds::block(3).expect("block 3 exists")
}

/// The block of entity ids of a region that a split made.
fn no_ids() -> EntityIds {
    EntityIds {
        first: EntityId(0),
        end: EntityId(0),
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

fn chunk_of(position: Vec3) -> ChunkPos {
    ChunkPos::containing(position.x, position.z)
}

/// The middle of a chunk, on the stone.
fn middle(chunk: ChunkPos) -> Vec3 {
    Vec3::new(
        f64::from(chunk.x) * 16.0 + 8.5,
        64.0,
        f64::from(chunk.z) * 16.0 + 8.5,
    )
}

// What a tick is given.

fn viewer(chunk: ChunkPos) -> (ChunkPos, Ticket) {
    (chunk, Ticket::Viewer)
}

fn guest(chunk: ChunkPos) -> (ChunkPos, Ticket) {
    (chunk, Ticket::Guest)
}

/// Subscriptions that begin.
fn add(tickets: &[(ChunkPos, Ticket)]) -> TickInputs {
    TickInputs {
        tickets_added: tickets.to_vec(),
        ..TickInputs::default()
    }
}

/// Subscriptions that end.
fn remove(tickets: &[(ChunkPos, Ticket)]) -> TickInputs {
    TickInputs {
        tickets_removed: tickets.to_vec(),
        ..TickInputs::default()
    }
}

/// The store grants the chunks.
fn granted(chunks: &[ChunkPos]) -> TickInputs {
    TickInputs {
        granted: chunks.to_vec(),
        ..TickInputs::default()
    }
}

/// The store says who holds the chunks.
fn foreign(chunks: &[(ChunkPos, RegionId)]) -> TickInputs {
    TickInputs {
        foreign: chunks.to_vec(),
        ..TickInputs::default()
    }
}

/// Storage delivers the chunks, each a column of stone.
fn delivered(chunks: &[ChunkPos]) -> TickInputs {
    TickInputs {
        chunks_loaded: chunks.iter().map(|chunk| (*chunk, stone_chunk())).collect(),
        ..TickInputs::default()
    }
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

/// A leave as an edge sends it: it names the entity of the stay if the edge has been
/// told one, and the attempt of the join otherwise (ADR-0020, section 4.4).
fn leave(edge: EdgeId, id: PlayerId, entity: Option<EntityId>) -> PlayerChange {
    let attempt = entity.is_none().then(|| attempt(id));
    PlayerChange::Leave(edge, id, entity, attempt)
}

/// A leave that names no entity and this attempt, or none.
fn leave_of_attempt(edge: EdgeId, id: PlayerId, attempt: Option<u64>) -> PlayerChange {
    PlayerChange::Leave(edge, id, None, attempt)
}

fn changes(list: Vec<PlayerChange>) -> TickInputs {
    let mut inputs = TickInputs::default();
    for change in list {
        inputs.change(change);
    }
    inputs
}

fn arrive(edge: EdgeId, id: PlayerId, transfer: PlayerTransfer) -> TickInputs {
    changes(vec![PlayerChange::Arrive(edge, id, transfer)])
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

fn remotely(edge: EdgeId, action: RemoteAction) -> TickInputs {
    TickInputs {
        remote_actions: vec![(edge, action)],
        ..TickInputs::default()
    }
}

/// The entity of the `n`th player to enter a region that gives out [`ids`], counted
/// from 1: what an edge is told when the player has spawned, and names in their inputs.
fn entity(n: i32) -> EntityId {
    EntityId(ids().first.0 + n - 1)
}

/// A step to the point with these x and z coordinates, on the stone.
fn walk(x: f64, z: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, 64.0, z)),
        rotation: None,
        on_ground: true,
    }
}

fn walk_to(position: Vec3) -> PlayerInput {
    walk(position.x, position.z)
}

/// A step along the row the spawn point is in.
fn move_to(x: f64) -> PlayerInput {
    walk(x, SPAWN.z)
}

fn dig(position: BlockPos, sequence: i32) -> PlayerInput {
    PlayerInput::Dig { position, sequence }
}

fn use_on(position: BlockPos, face: Face, sequence: i32) -> PlayerInput {
    PlayerInput::UseItemOn {
        position,
        face,
        sequence,
    }
}

/// The entity that `id` has in another region, as the remote actions here name it.
fn elsewhere(id: PlayerId) -> EntityId {
    EntityId(9000 + id.0.as_u128() as i32)
}

fn remote(id: PlayerId, sequence: i32, step: RemoteStep) -> RemoteAction {
    RemoteAction {
        player: id,
        entity: elsewhere(id),
        sequence,
        step,
    }
}

/// What is left of an action of `id` in the stay `entity`, as the region the player is
/// in passes it on: it names the entity of the player who acts.
fn own(id: PlayerId, entity: EntityId, sequence: i32, step: RemoteStep) -> RemoteAction {
    RemoteAction {
        player: id,
        entity,
        sequence,
        step,
    }
}

fn break_at(position: BlockPos) -> RemoteStep {
    RemoteStep::Break { position }
}

fn place_at(target: BlockPos, placer: Vec3) -> RemoteStep {
    RemoteStep::Place {
        target,
        block: blocks::STONE,
        placer,
    }
}

fn place_against(against: BlockPos, target: BlockPos, placer: Vec3) -> RemoteStep {
    RemoteStep::PlaceAgainst {
        against,
        target,
        block: blocks::STONE,
        placer,
    }
}

/// A player as another region lets them go, standing at `position`.
fn transfer(entity: EntityId, last_input: u64, position: Vec3) -> PlayerTransfer {
    PlayerTransfer {
        entity_id: entity,
        name: "traveller".to_owned(),
        pose: Pose::at(position),
        hotbar: hotbar(),
        selected_slot: 0,
        last_input,
        hops: 0,
        flying: false,
        attempt: None,
    }
}

/// A player as the region that has them would let them go now: handed on once more
/// (ADR-0020, section 6).
fn transfer_of(state: &PlayerState) -> PlayerTransfer {
    PlayerTransfer {
        entity_id: state.entity_id,
        name: state.name.clone(),
        pose: state.pose,
        hotbar: state.hotbar,
        selected_slot: state.selected_slot,
        last_input: state.last_input,
        hops: state.hops + 1,
        flying: state.flying,
        attempt: state.attempt,
    }
}

/// A player as a region has them right after taking in `transfer` through `edge`.
fn taken_in(transfer: &PlayerTransfer, edge: EdgeId) -> PlayerState {
    PlayerState {
        entity_id: transfer.entity_id,
        name: transfer.name.clone(),
        pose: transfer.pose,
        hotbar: transfer.hotbar,
        selected_slot: transfer.selected_slot,
        last_input: transfer.last_input,
        handled: None,
        edge,
        hops: transfer.hops,
        flying: transfer.flying,
        attempt: transfer.attempt,
    }
}

// What a tick gives back.

fn acknowledged(output: &TickOutput) -> Vec<(PlayerId, i32)> {
    output
        .player_events
        .iter()
        .filter_map(|(id, event)| match event {
            PlayerEvent::Acknowledged { sequence } => Some((*id, *sequence)),
            _ => None,
        })
        .collect()
}

fn block_changes(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged { position, state } => Some((*position, *state)),
            _ => None,
        })
        .collect()
}

fn removed(output: &TickOutput) -> Vec<(EntityId, ChunkPos)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityRemoved { entity, chunk } => Some((*entity, *chunk)),
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

fn moved(output: &TickOutput) -> Vec<EntityId> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityMoved { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect()
}

/// The entries of `output` without the edges and numbers they have.
fn entries(output: &TickOutput) -> Vec<Durable> {
    output
        .durable
        .iter()
        .map(|(_, _, entry)| entry.clone())
        .collect()
}

/// Whether the tick showed and told nobody anything.
fn silent(output: &TickOutput) -> bool {
    output.events.is_empty() && output.player_events.is_empty() && output.durable.is_empty()
}

fn state_of(region: &Region, id: PlayerId) -> PlayerState {
    region
        .player_state(id)
        .expect("the player is in the region")
}

/// What the region has of the block at `position`, if its chunk is loaded.
fn block(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region
        .chunk(position.chunk())
        .and_then(|chunk| chunk.get(x, position.y, z))
}

fn bytes(state: &RegionState) -> Vec<u8> {
    postcard::to_stdvec(state).expect("a state can be serialised")
}

// ---------------------------------------------------------------------------------------
// What holds of every tick
// ---------------------------------------------------------------------------------------

/// What sections 1 and 2 of ADR-0008 say of every tick: the delta turns the state
/// before into the state after, the entries made are numbered on from `sent` and are in
/// the outbox, and every acknowledgement can be worked out again from `handled`.
fn check_state(before: &RegionState, output: &TickOutput, after: &RegionState) {
    assert_eq!(output.tick, before.tick + 1, "ticks are numbered on");
    assert_eq!(output.delta.tick, output.tick);
    assert_eq!(after.tick, output.tick);

    let mut applied = before.clone();
    applied.apply(&output.delta);
    assert_eq!(
        &applied, after,
        "state_before.apply(&delta) must equal state_after, tick {}",
        output.tick
    );

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
        let previous = before.edges.get(edge).map_or(0, |state| state.sent);
        assert!(
            numbers[0] == previous + 1 || numbers[0] == 1,
            "entries are numbered on from `sent`, or from 1 after a reset"
        );
    }
    for (edge, number, entry) in &output.durable {
        assert_eq!(
            after.edges[edge].outbox.get(number),
            Some(entry),
            "an entry made in the tick is in the outbox after it"
        );
    }
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

    // A player who is acknowledged is in the region with that much handled, or was let
    // go in this tick (ADR-0008, section 4).
    for (id, event) in &output.player_events {
        if let PlayerEvent::Acknowledged { sequence } = event {
            match after.players.get(id) {
                Some(state) => assert!(state.handled >= Some(*sequence)),
                None => assert!(
                    output.durable.iter().any(|(_, _, entry)| matches!(
                        entry,
                        Durable::Departed { player, .. } if player == id
                    )),
                    "{id:?} is acknowledged and neither in the region nor let go"
                ),
            }
        }
    }
    // Every player belongs to an edge the region knows.
    for player in after.players.values() {
        assert!(after.edges.contains_key(&player.edge));
    }
}

/// Advances the region by one tick and holds it to what holds of every tick.
fn tick(region: &mut Region, inputs: &TickInputs) -> TickOutput {
    let before = region.state();
    let output = region.tick(inputs);
    check_state(&before, &output, &region.state());
    assert_eq!(region.tick_number(), output.tick);
    assert!(output.claims.is_sorted() && output.returns.is_sorted());
    assert!(output.chunk_requests.is_sorted());
    for chunk in &output.claims {
        assert_eq!(region.knowledge(*chunk), Knowledge::Asked);
    }
    output
}

/// A tick in which nothing happens.
fn idle(region: &mut Region) -> TickOutput {
    tick(region, &TickInputs::default())
}

/// Has the region come to hold `chunk` and load it, in three ticks: a viewer's ticket,
/// for which it asks; the store's grant, with which it asks storage; and what storage
/// delivers.
fn hold(region: &mut Region, chunk: ChunkPos) {
    let output = tick(region, &add(&[viewer(chunk)]));
    assert_eq!(output.claims, [chunk]);
    let output = tick(region, &granted(&[chunk]));
    assert_eq!(output.chunk_requests, [chunk]);
    tick(region, &delivered(&[chunk]));
    assert_eq!(region.knowledge(chunk), Knowledge::Held);
    assert!(region.chunk(chunk).is_some());
}

/// Has the region ask for `chunk`, for a viewer's ticket, and leaves it unanswered.
fn ask(region: &mut Region, chunk: ChunkPos) {
    let output = tick(region, &add(&[viewer(chunk)]));
    assert_eq!(output.claims, [chunk]);
    assert_eq!(region.knowledge(chunk), Knowledge::Asked);
}

/// Has the region learn that `chunk` is `holder`'s, in two ticks: a viewer's ticket, for
/// which it asks, then what the store answers. It believes so while the ticket is there.
fn learn(region: &mut Region, chunk: ChunkPos, holder: RegionId) {
    ask(region, chunk);
    tick(region, &foreign(&[(chunk, holder)]));
    assert_eq!(region.knowledge(chunk), Knowledge::Foreign(holder));
}

/// Region 0 of the stripes with one chunk of each kind around its spawn point: `HOME`
/// held and loaded, `EAST` believed region 1's and `SOUTH` asked for, each for a
/// viewer's ticket, and `NORTH` and `WEST`, of which it knows nothing. It knows the
/// edges `E` and `F`, and player 1 has joined through `E` and has the entity
/// `entity(1)`.
fn at_the_line() -> Region {
    let holdings = Holdings {
        held: Vec::new(),
        pinned: vec![WESTERN],
    };
    let mut region = Region::new(config(0), ids(), holdings);
    hold(&mut region, HOME);
    learn(&mut region, EAST, REGION_B);
    ask(&mut region, SOUTH);
    tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    let output = tick(&mut region, &changes(vec![join(E, player(1))]));
    assert_eq!(spawned(&output), [entity(1)]);
    assert_eq!(region.knowledge(NORTH), Knowledge::Unknown);
    assert!(region.pins(SOUTH) && region.pins(NORTH) && !region.pins(EAST));
    region
}

/// [`at_the_line`], where the store has since called `SOUTH`, a chunk of the region's
/// own stripe, the region `OTHER`'s: a part that was split off holds it.
fn doubting() -> Region {
    let mut region = at_the_line();
    tick(&mut region, &foreign(&[(SOUTH, OTHER)]));
    assert_eq!(region.knowledge(SOUTH), Knowledge::Foreign(OTHER));
    region
}

// ---------------------------------------------------------------------------------------
// S19: a join begins a new stay whatever the region has
// ---------------------------------------------------------------------------------------

/// [`at_the_line`] whose player has made their stay their own: a block broken, another
/// item in another hand, and a walk into `NORTH`. Returns the player as they were right
/// after they joined as well.
fn with_a_stay_under_way() -> (Region, PlayerState) {
    let mut region = at_the_line();
    let fresh = state_of(&region, player(1));
    let stack = Some(ItemStack {
        item: items::STONE,
        count: 7,
    });
    let mut inputs = single_input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 4));
    inputs.input(
        E,
        player(1),
        entity(1),
        2,
        PlayerInput::SetHotbarSlot { slot: 3, stack },
    );
    inputs.input(
        E,
        player(1),
        entity(1),
        3,
        PlayerInput::SelectSlot { slot: 3 },
    );
    inputs.input(E, player(1), entity(1), 4, walk_to(IN_NORTH));
    let output = tick(&mut region, &inputs);
    assert_eq!(acknowledged(&output), [(player(1), 4)]);
    let old = state_of(&region, player(1));
    assert_eq!(
        (
            old.entity_id,
            old.last_input,
            old.handled,
            old.selected_slot
        ),
        (entity(1), 4, Some(4), 3)
    );
    assert_eq!(old.hotbar[3], stack);
    assert_eq!(chunk_of(old.pose.position), NORTH);
    (region, fresh)
}

#[test]
fn a_join_of_a_player_the_region_has_under_the_same_edge_begins_a_new_stay() {
    let (mut region, fresh) = with_a_stay_under_way();
    let output = tick(&mut region, &changes(vec![join(E, player(1))]));

    // The entity they had is removed where it stood, and they enter the world anew:
    // with the next id, at the spawn point, with nothing applied and nothing handled.
    assert_eq!(removed(&output), [(entity(1), NORTH)]);
    assert_eq!(spawned(&output), [entity(2)]);
    assert_eq!(
        output.player_events,
        vec![(
            player(1),
            PlayerEvent::Spawned {
                attempt: attempt(player(1)),
                entity_id: entity(2),
                pose: Pose::at(SPAWN),
                flying: false,
                hotbar: hotbar(),
                selected_slot: 0,
            }
        )]
    );
    assert!(output.durable.is_empty());
    let expected = PlayerState {
        entity_id: entity(2),
        ..fresh
    };
    assert_eq!((expected.last_input, expected.handled), (0, None));
    assert_eq!(state_of(&region, player(1)), expected);
    assert_eq!(region.state().next_entity_id, entity(3));
    assert_eq!(region.player_count(), 1);
    assert!(region.entity(entity(1)).is_none());
    assert!(region.entity(entity(2)).is_some());
}

#[test]
fn a_join_of_a_player_the_region_has_under_another_edge_begins_a_new_stay_under_that_edge() {
    let (mut region, fresh) = with_a_stay_under_way();
    let output = tick(&mut region, &changes(vec![join(F, player(1))]));
    assert_eq!(removed(&output), [(entity(1), NORTH)]);
    assert_eq!(spawned(&output), [entity(2)]);
    let expected = PlayerState {
        entity_id: entity(2),
        edge: F,
        ..fresh
    };
    assert_eq!(state_of(&region, player(1)), expected);
    assert_eq!(region.player_count(), 1);
}

#[test]
fn the_first_input_of_the_new_stay_is_number_one_and_names_the_new_entity() {
    // The edge numbers a player's inputs from 1 with every connection. The old stay had
    // four applied; were `last_input` carried over, the new stay's first four would be
    // passed over.
    let (mut region, _) = with_a_stay_under_way();
    tick(&mut region, &changes(vec![join(E, player(1))]));
    let output = tick(
        &mut region,
        &single_input(E, player(1), entity(2), 1, move_to(11.5)),
    );
    assert_eq!(moved(&output), [entity(2)]);
    let state = state_of(&region, player(1));
    assert_eq!((state.pose.position.x, state.last_input), (11.5, 1));

    // `handled` began anew as well: an action numbered below what the old stay had
    // handled is acknowledged.
    let block = BlockPos::new(11, 63, 8);
    let output = tick(
        &mut region,
        &single_input(E, player(1), entity(2), 2, dig(block, 2)),
    );
    assert_eq!(acknowledged(&output), [(player(1), 2)]);
    assert_eq!(block_changes(&output), [(block, blocks::AIR)]);
    assert_eq!(state_of(&region, player(1)).handled, Some(2));
}

#[test]
fn inputs_in_the_tick_of_a_second_join_are_applied_only_if_they_name_the_stay_it_begins() {
    let (mut region, _) = with_a_stay_under_way();
    let mut inputs = TickInputs::default();
    inputs.input(E, player(1), entity(1), 5, move_to(3.5));
    inputs.change(join(E, player(1)));
    // What the player did before they joined is dropped, as for any join.
    assert!(inputs.inputs.is_empty());
    // An input of the stay that has ended, which reaches the region behind the join,
    // and the first of the stay that begins.
    inputs.input(E, player(1), entity(1), 6, move_to(5.5));
    inputs.input(E, player(1), entity(2), 1, move_to(12.5));
    tick(&mut region, &inputs);
    let state = state_of(&region, player(1));
    assert_eq!(
        (state.entity_id, state.pose.position.x, state.last_input),
        (entity(2), 12.5, 1)
    );
}

#[test]
fn a_join_through_an_edge_the_region_does_not_know_leaves_the_stay_that_is_there() {
    // Whatever names an edge the region does not know is ignored (ADR-0008, section 2):
    // nobody could be told what became of it.
    let (mut region, _) = with_a_stay_under_way();
    let mut expected = region.state();
    let output = tick(&mut region, &changes(vec![join(STRANGER, player(1))]));
    assert!(silent(&output));
    expected.tick += 1;
    assert_eq!(region.state(), expected);
}

#[test]
fn two_joins_of_a_player_in_one_tick_are_two_stays_of_which_the_second_is_left() {
    let mut region = at_the_line();
    let output = tick(
        &mut region,
        &changes(vec![join(E, player(2)), join(F, player(2))]),
    );
    assert_eq!(spawned(&output), [entity(2), entity(3)]);
    assert_eq!(removed(&output), [(entity(2), HOME)]);
    let state = state_of(&region, player(2));
    assert_eq!((state.entity_id, state.edge), (entity(3), F));
    assert_eq!(region.state().next_entity_id, entity(4));
    assert_eq!(region.player_count(), 2);
}

/// A region on open land that holds `HOME`, with player 1 standing in it under `E` with
/// the entity `TRAVELLER`, and no entity id left to give: `ids` is its block.
fn with_no_id_left(ids: EntityIds) -> Region {
    let mut state = RegionState::new(ids);
    state.tick = 30;
    state.next_entity_id = ids.end;
    state.edges.insert(
        E,
        EdgeState {
            start: 10,
            since: 1,
            ..EdgeState::default()
        },
    );
    state
        .players
        .insert(player(1), taken_in(&transfer(TRAVELLER, 5, IN_HOME), E));
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    Region::restore(config(0), state, holdings)
}

#[test]
fn a_join_that_is_refused_still_ends_the_stay_the_region_has() {
    // A join begins a new stay whatever the region has, and a region with no id left
    // refuses it (ADR-0012, section 2.2). The player has connected anew either way: the
    // stay that was there is over. Both a region whose block is used up and the part of
    // a split, which has the empty block.
    for block in [ids(), no_ids()] {
        let mut region = with_no_id_left(block);
        let output = tick(&mut region, &changes(vec![join(E, player(1))]));
        assert_eq!(removed(&output), [(TRAVELLER, HOME)]);
        assert_eq!(output.durable, vec![(E, 1, refused(1))]);
        assert!(spawned(&output).is_empty() && output.player_events.is_empty());
        assert_eq!(region.player_count(), 0);
    }
}

// ---------------------------------------------------------------------------------------
// S20: an arrival and the stay the region has
// ---------------------------------------------------------------------------------------

/// The four chunks of [`at_the_line`] by what the region knows of them, each with a
/// point in it and whether an arrival there is taken in.
fn places() -> [(Vec3, Knowledge, bool); 4] {
    [
        (IN_HOME, Knowledge::Held, true),
        (IN_NORTH, Knowledge::Unknown, true),
        (IN_SOUTH, Knowledge::Asked, true),
        (IN_EAST, Knowledge::Foreign(REGION_B), false),
    ]
}

#[test]
fn an_arrival_with_a_higher_entity_id_takes_the_place_of_the_stay_that_is_there() {
    for (position, known, taken) in places() {
        let mut region = at_the_line();
        assert_eq!(region.knowledge(chunk_of(position)), known);
        let own = state_of(&region, player(1)).entity_id;
        assert!(own < TRAVELLER);
        let arriving = transfer(TRAVELLER, 9, position);
        let output = tick(&mut region, &arrive(F, player(1), arriving.clone()));

        // The entity that was there is removed where it stood, and the arrival goes on
        // as for a player the region does not have: taken in as the transfer says and
        // the arriving edge's from then on, or sent on where the chunk is believed
        // another's.
        assert_eq!(removed(&output), [(own, HOME)], "at {position:?}");
        assert!(region.entity(own).is_none());
        if taken {
            assert_eq!(state_of(&region, player(1)), taken_in(&arriving, F));
            assert_eq!(spawned(&output), [TRAVELLER]);
            assert!(output.durable.is_empty());
            let claims: &[ChunkPos] = if known == Knowledge::Unknown {
                &[NORTH]
            } else {
                &[]
            };
            assert_eq!(output.claims, claims, "at {position:?}");
        } else {
            assert_eq!(region.player_count(), 0);
            assert!(spawned(&output).is_empty(), "nobody is shown the arrival");
            assert_eq!(
                output.durable,
                vec![(
                    F,
                    1,
                    Durable::NotMine {
                        what: Misdirected::Arrival {
                            player: player(1),
                            transfer: arriving,
                        },
                        holder: REGION_B,
                    }
                )]
            );
            assert_eq!(region.knowledge(EAST), Knowledge::Foreign(REGION_B));
        }
    }
}

#[test]
fn an_arrival_with_a_lower_entity_id_is_passed_over_and_its_entity_removed() {
    for (position, _, _) in places() {
        let mut region = at_the_line();
        let mut expected = region.state();
        let own = expected.players[&player(1)].entity_id;
        let earlier = EntityId(own.0 - 1);
        for edge in [E, F] {
            let arriving = transfer(earlier, 9, position);
            let output = tick(&mut region, &arrive(edge, player(1), arriving));
            assert_eq!(
                removed(&output),
                [(earlier, chunk_of(position))],
                "the entity that was on its way, at {position:?}"
            );
            assert!(spawned(&output).is_empty() && output.durable.is_empty());
            assert!(output.player_events.is_empty());
            assert!(output.claims.is_empty(), "nobody came to stand there");
            expected.tick += 1;
            assert_eq!(region.state(), expected, "at {position:?}");
        }
    }
}

#[test]
fn an_arrival_with_the_entity_id_the_region_has_changes_nothing() {
    for (position, known, _) in places() {
        let mut region = at_the_line();
        let mut expected = region.state();
        let own = expected.players[&player(1)].entity_id;
        for edge in [E, F] {
            let output = tick(
                &mut region,
                &arrive(edge, player(1), transfer(own, 9, position)),
            );
            assert!(silent(&output), "at {position:?}: {output:?}");
            assert!(output.claims.is_empty());
            expected.tick += 1;
            assert_eq!(region.state(), expected, "at {position:?}");
            assert_eq!(region.knowledge(chunk_of(position)), known);
        }
    }
}

#[test]
fn an_arrival_through_an_edge_the_region_does_not_know_is_passed_over_whatever_its_entity() {
    // Also where it is the later stay: nobody could be told about the player, so the
    // stay the region has is left alone and the entity on its way is removed.
    for (position, known, _) in places() {
        let mut region = at_the_line();
        let mut expected = region.state();
        let output = tick(
            &mut region,
            &arrive(STRANGER, player(1), transfer(TRAVELLER, 9, position)),
        );
        assert_eq!(removed(&output), [(TRAVELLER, chunk_of(position))]);
        assert!(spawned(&output).is_empty() && output.durable.is_empty());
        assert!(output.claims.is_empty());
        expected.tick += 1;
        assert_eq!(region.state(), expected, "at {position:?}");
        assert_eq!(region.knowledge(chunk_of(position)), known);
    }
}

#[test]
fn of_two_arrivals_in_one_tick_the_one_with_the_higher_entity_id_is_left_in_either_order() {
    let later = EntityId(TRAVELLER.0 + 5);
    for first_is_later in [false, true] {
        let mut region = at_the_line();
        let (first, second) = if first_is_later {
            (later, TRAVELLER)
        } else {
            (TRAVELLER, later)
        };
        let output = tick(
            &mut region,
            &changes(vec![
                PlayerChange::Arrive(E, player(5), transfer(first, 3, IN_HOME)),
                PlayerChange::Arrive(F, player(5), transfer(second, 8, IN_NORTH)),
            ]),
        );
        let state = state_of(&region, player(5));
        if first_is_later {
            assert_eq!(state, taken_in(&transfer(later, 3, IN_HOME), E));
            assert_eq!(removed(&output), [(TRAVELLER, NORTH)]);
            assert!(output.claims.is_empty());
        } else {
            assert_eq!(state, taken_in(&transfer(later, 8, IN_NORTH), F));
            assert_eq!(removed(&output), [(TRAVELLER, HOME)]);
            assert_eq!(output.claims, [NORTH]);
        }
        assert_eq!(region.player_count(), 2);
    }
}

#[test]
fn what_a_player_did_behind_an_arrival_that_replaced_their_stay_is_applied_to_the_new_one() {
    let mut region = at_the_line();
    let mut inputs = TickInputs::default();
    inputs.change(PlayerChange::Arrive(
        E,
        player(1),
        transfer(TRAVELLER, 9, IN_HOME),
    ));
    // Sent again with the arrival: one the region they came from had applied, one of
    // the stay that was here, and two that are new.
    inputs.input(E, player(1), TRAVELLER, 9, move_to(2.5));
    inputs.input(E, player(1), entity(1), 10, move_to(3.5));
    inputs.input(E, player(1), TRAVELLER, 10, move_to(10.5));
    inputs.input(
        E,
        player(1),
        TRAVELLER,
        11,
        dig(BlockPos::new(10, 63, 8), 1),
    );
    let output = tick(&mut region, &inputs);
    let state = state_of(&region, player(1));
    assert_eq!(
        (state.entity_id, state.pose.position.x, state.last_input),
        (TRAVELLER, 10.5, 11)
    );
    assert_eq!(acknowledged(&output), [(player(1), 1)]);
    assert_eq!(block(&region, BlockPos::new(10, 63, 8)), Some(blocks::AIR));
}

// ---------------------------------------------------------------------------------------
// S21: a leave names the stay it ends
// ---------------------------------------------------------------------------------------

/// Entities the player of [`at_the_line`] does not have: of an earlier stay, of a later
/// one, and one from elsewhere.
fn other_entities() -> [EntityId; 3] {
    [EntityId(entity(1).0 - 1), entity(2), TRAVELLER]
}

#[test]
fn a_leave_that_names_the_players_entity_removes_them() {
    let mut region = at_the_line();
    let output = tick(
        &mut region,
        &changes(vec![leave(E, player(1), Some(entity(1)))]),
    );
    assert_eq!(removed(&output), [(entity(1), HOME)]);
    assert_eq!(region.player_count(), 0);
    assert!(output.durable.is_empty());
}

#[test]
fn a_leave_that_names_another_entity_changes_nothing() {
    for other in other_entities() {
        let mut region = at_the_line();
        let mut expected = region.state();
        let output = tick(
            &mut region,
            &changes(vec![leave(E, player(1), Some(other))]),
        );
        assert!(silent(&output), "{other:?}: {output:?}");
        expected.tick += 1;
        assert_eq!(region.state(), expected, "{other:?}");
    }
}

/// Until ADR-0020 a leave that named no entity removed the player whatever their
/// entity. Its section 4.4 holds such a leave to the attempt it names: it ends the
/// stay that carries that attempt, which is one no input of which has been applied.
#[test]
fn a_leave_that_names_no_entity_removes_the_player_whose_stay_carries_the_attempt_it_names() {
    let mut region = at_the_line();
    assert_eq!(
        state_of(&region, player(1)).attempt,
        Some(attempt(player(1)))
    );
    // Another attempt, and none at all: the stay is not the one that is meant.
    for other in [Some(attempt(player(1)) + 1), Some(0), None] {
        let mut expected = region.state();
        let output = tick(
            &mut region,
            &changes(vec![leave_of_attempt(E, player(1), other)]),
        );
        assert!(silent(&output), "{other:?}: {output:?}");
        expected.tick += 1;
        assert_eq!(region.state(), expected, "{other:?}");
    }
    let output = tick(&mut region, &changes(vec![leave(E, player(1), None)]));
    assert_eq!(removed(&output), [(entity(1), HOME)]);
    assert_eq!(region.player_count(), 0);
    assert!(output.durable.is_empty());

    // A stay that has been heard from carries no attempt: its edge knows the entity
    // and names it, and a leave that names the attempt of the join finds nobody.
    let mut region = at_the_line();
    tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, move_to(10.5)),
    );
    assert_eq!(state_of(&region, player(1)).attempt, None);
    for named in [Some(attempt(player(1))), None] {
        let output = tick(
            &mut region,
            &changes(vec![leave_of_attempt(E, player(1), named)]),
        );
        assert!(silent(&output), "{named:?}: {output:?}");
    }
    assert_eq!(region.player_count(), 1);
}

#[test]
fn a_leave_through_another_edge_removes_nobody_whatever_it_names() {
    for edge in [F, STRANGER] {
        for named in [Some(entity(1)), Some(entity(2)), None] {
            let mut region = at_the_line();
            let mut expected = region.state();
            let output = tick(&mut region, &changes(vec![leave(edge, player(1), named)]));
            assert!(silent(&output), "{edge:?}, {named:?}");
            expected.tick += 1;
            assert_eq!(region.state(), expected, "{edge:?}, {named:?}");
        }
    }
}

#[test]
fn an_input_before_a_leave_that_names_another_entity_is_applied() {
    for other in other_entities() {
        let mut region = at_the_line();
        let mut inputs = TickInputs::default();
        inputs.input(E, player(1), entity(1), 1, walk_to(IN_NORTH));
        inputs.change(leave(E, player(1), Some(other)));
        assert_eq!(inputs.inputs.len(), 1, "a leave drops nothing");
        let output = tick(&mut region, &inputs);
        assert_eq!(moved(&output), [entity(1)]);
        assert!(removed(&output).is_empty());
        let state = state_of(&region, player(1));
        assert_eq!((state.pose.position, state.last_input), (IN_NORTH, 1));
        assert_eq!(output.claims, [NORTH]);
    }
}

#[test]
fn an_input_before_a_leave_that_applies_moves_nobody() {
    for named in [Some(entity(1)), None] {
        let mut region = at_the_line();
        let mut inputs = TickInputs::default();
        inputs.input(E, player(1), entity(1), 1, walk_to(IN_NORTH));
        inputs.input(E, player(1), entity(1), 2, dig(OWN_BLOCK, 1));
        inputs.change(leave(E, player(1), named));
        assert_eq!(inputs.inputs.len(), 2, "a leave drops nothing");
        let output = tick(&mut region, &inputs);
        // A tick applies changes before inputs: the player is gone from where they
        // stood, and what they did finds nobody.
        assert_eq!(removed(&output), [(entity(1), HOME)], "{named:?}");
        assert!(moved(&output).is_empty() && block_changes(&output).is_empty());
        assert!(output.player_events.is_empty() && output.durable.is_empty());
        assert!(output.claims.is_empty(), "nobody came to stand in `NORTH`");
        assert_eq!(region.player_count(), 0);
        assert_eq!(block(&region, OWN_BLOCK), Some(blocks::STONE));
    }
}

#[test]
fn inputs_either_side_of_a_leave_are_applied_or_not_together() {
    for (named, stays) in [
        (Some(entity(1)), false),
        (None, false),
        (Some(entity(2)), true),
        (Some(TRAVELLER), true),
    ] {
        let mut region = at_the_line();
        let mut inputs = TickInputs::default();
        inputs.input(E, player(1), entity(1), 1, move_to(10.5));
        inputs.change(leave(E, player(1), named));
        inputs.input(E, player(1), entity(1), 2, move_to(6.5));
        let output = tick(&mut region, &inputs);
        if stays {
            let state = state_of(&region, player(1));
            assert_eq!((state.pose.position.x, state.last_input), (6.5, 2));
            assert!(removed(&output).is_empty());
        } else {
            assert_eq!(region.player_count(), 0, "{named:?}");
            assert!(moved(&output).is_empty());
            assert_eq!(removed(&output), [(entity(1), HOME)]);
        }
    }
}

#[test]
fn an_input_before_a_leave_and_a_join_is_not_applied_to_the_new_stay() {
    // Through `change`, the join drops what waits.
    let mut region = at_the_line();
    let fresh = state_of(&region, player(1));
    let mut inputs = TickInputs::default();
    inputs.input(E, player(1), entity(1), 1, walk_to(IN_NORTH));
    inputs.change(leave(E, player(1), Some(entity(1))));
    assert_eq!(inputs.inputs.len(), 1, "a leave drops nothing");
    inputs.change(join(E, player(1)));
    assert!(inputs.inputs.is_empty(), "a join drops what waits");
    let output = tick(&mut region, &inputs);
    assert_eq!(removed(&output), [(entity(1), HOME)]);
    assert_eq!(spawned(&output), [entity(2)]);
    assert!(moved(&output).is_empty());
    let expected = PlayerState {
        entity_id: entity(2),
        ..fresh.clone()
    };
    assert_eq!(state_of(&region, player(1)), expected);

    // And if it reaches the tick all the same, it names a stay that has ended.
    let mut region = at_the_line();
    let inputs = TickInputs {
        player_changes: vec![leave(E, player(1), Some(entity(1))), join(E, player(1))],
        inputs: vec![(E, player(1), entity(1), 1, walk_to(IN_NORTH))],
        ..TickInputs::default()
    };
    let output = tick(&mut region, &inputs);
    assert!(moved(&output).is_empty());
    assert_eq!(state_of(&region, player(1)), expected);
    assert!(output.claims.is_empty());
}

#[test]
fn a_late_leave_for_the_stay_before_does_not_end_the_stay_a_join_began() {
    // Section 2.1, the shortest case for the leave: the edge tells the region that the
    // stay with the old entity has ended after the new stay is there. In one tick and
    // in two.
    let mut region = at_the_line();
    let output = tick(
        &mut region,
        &changes(vec![
            join(E, player(1)),
            leave(E, player(1), Some(entity(1))),
        ]),
    );
    assert_eq!(removed(&output), [(entity(1), HOME)]);
    assert_eq!(state_of(&region, player(1)).entity_id, entity(2));

    let output = tick(
        &mut region,
        &changes(vec![leave(E, player(1), Some(entity(1)))]),
    );
    assert!(silent(&output));
    assert_eq!(state_of(&region, player(1)).entity_id, entity(2));
}

#[test]
fn a_leave_in_the_tick_of_the_join_ends_the_stay_only_if_it_names_it_or_none() {
    for (named, stays) in [
        (Some(entity(2)), false),
        (None, false),
        (Some(entity(1)), true),
        (Some(entity(3)), true),
    ] {
        let mut region = at_the_line();
        let mut inputs = changes(vec![join(E, player(2)), leave(E, player(2), named)]);
        inputs.input(E, player(2), entity(2), 1, move_to(10.5));
        let output = tick(&mut region, &inputs);
        // The id is given out either way.
        assert_eq!(region.state().next_entity_id, entity(3), "{named:?}");
        if stays {
            let state = state_of(&region, player(2));
            assert_eq!(
                (state.entity_id, state.pose.position.x, state.last_input),
                (entity(2), 10.5, 1)
            );
            assert_eq!(spawned(&output), [entity(2)]);
            assert!(removed(&output).is_empty());
        } else {
            assert!(region.player(player(2)).is_none(), "{named:?}");
            assert!(region.entity(entity(2)).is_none());
            // Whoever was shown the entity is shown that it is gone.
            assert_eq!(
                spawned(&output).contains(&entity(2)),
                removed(&output).contains(&(entity(2), HOME)),
                "{named:?}: {:?}",
                output.events
            );
            assert!(moved(&output).is_empty());
        }
        assert_eq!(region.player_count(), 1 + usize::from(stays));
    }
}

#[test]
fn a_leave_in_the_tick_of_the_arrival_ends_the_stay_only_if_it_names_it_or_none() {
    for (edge, named, stays) in [
        (F, Some(TRAVELLER), false),
        (F, None, false),
        (F, Some(entity(1)), true),
        (F, Some(EntityId(TRAVELLER.0 + 1)), true),
        (E, Some(TRAVELLER), true),
        (E, None, true),
    ] {
        let mut region = at_the_line();
        // The stay comes with the attempt of its join still: no region has applied an
        // input of it, so a leave that names no entity can name that (ADR-0020,
        // section 4.4).
        let arriving = PlayerTransfer {
            attempt: Some(attempt(player(5))),
            ..transfer(TRAVELLER, 0, IN_HOME)
        };
        let mut inputs = changes(vec![
            PlayerChange::Arrive(F, player(5), arriving),
            leave(edge, player(5), named),
        ]);
        inputs.input(F, player(5), TRAVELLER, 1, move_to(10.5));
        let output = tick(&mut region, &inputs);
        if stays {
            let state = state_of(&region, player(5));
            assert_eq!(
                (state.entity_id, state.pose.position.x, state.last_input),
                (TRAVELLER, 10.5, 1),
                "{edge:?}, {named:?}"
            );
            assert!(removed(&output).is_empty());
        } else {
            assert!(region.player(player(5)).is_none(), "{edge:?}, {named:?}");
            assert_eq!(
                spawned(&output).contains(&TRAVELLER),
                removed(&output).contains(&(TRAVELLER, HOME)),
                "{:?}",
                output.events
            );
        }
        assert!(output.durable.is_empty());
    }
}

// ---------------------------------------------------------------------------------------
// S22: an input names the stay it is of
// ---------------------------------------------------------------------------------------

#[test]
fn an_input_that_names_the_players_entity_is_applied() {
    let mut region = at_the_line();
    let output = tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, move_to(10.5)),
    );
    assert_eq!(moved(&output), [entity(1)]);
    let state = state_of(&region, player(1));
    assert_eq!((state.pose.position.x, state.last_input), (10.5, 1));
}

#[test]
fn an_input_that_names_another_entity_is_not_applied_whatever_its_number() {
    let mut region = at_the_line();
    tick(
        &mut region,
        &single_input(E, player(1), entity(1), 5, move_to(12.5)),
    );
    let mut expected = region.state();
    assert_eq!(expected.players[&player(1)].last_input, 5);
    for other in other_entities() {
        // Numbered below the player's last, at it, the next one, and far above.
        for number in [3, 5, 6, 41] {
            for input in [move_to(3.5), dig(OWN_BLOCK, 9)] {
                let output = tick(
                    &mut region,
                    &single_input(E, player(1), other, number, input),
                );
                assert!(silent(&output), "{other:?}, {number}: {output:?}");
                expected.tick += 1;
                assert_eq!(region.state(), expected, "{other:?}, {number}");
            }
        }
    }
    assert_eq!(block(&region, OWN_BLOCK), Some(blocks::STONE));

    // None of them counted: the stay's own next input is taken.
    tick(
        &mut region,
        &single_input(E, player(1), entity(1), 6, move_to(11.5)),
    );
    let state = state_of(&region, player(1));
    assert_eq!((state.pose.position.x, state.last_input), (11.5, 6));
}

#[test]
fn an_input_of_the_stay_before_with_a_high_number_does_not_pass_for_one_of_the_stay_that_is() {
    // The scenario's own numbers: a stay of the second entity with nothing applied, an
    // input numbered 41 that names the first, and one numbered 1 that names the second.
    for together in [true, false] {
        let mut region = at_the_line();
        tick(&mut region, &changes(vec![join(E, player(1))]));
        let state = state_of(&region, player(1));
        assert_eq!((state.entity_id, state.last_input), (entity(2), 0));

        let late = (E, player(1), entity(1), 41, move_to(3.5));
        let real = (E, player(1), entity(2), 1, move_to(12.5));
        if together {
            let mut inputs = TickInputs::default();
            inputs.input(late.0, late.1, late.2, late.3, late.4);
            inputs.input(real.0, real.1, real.2, real.3, real.4);
            tick(&mut region, &inputs);
        } else {
            let output = tick(
                &mut region,
                &single_input(late.0, late.1, late.2, late.3, late.4),
            );
            assert!(silent(&output));
            assert_eq!(state_of(&region, player(1)), state);
            tick(
                &mut region,
                &single_input(real.0, real.1, real.2, real.3, real.4),
            );
        }
        let state = state_of(&region, player(1));
        assert_eq!((state.pose.position.x, state.last_input), (12.5, 1));
    }
}

#[test]
fn an_input_that_names_the_players_entity_through_another_edge_is_not_applied() {
    for edge in [F, STRANGER] {
        let mut region = at_the_line();
        let mut expected = region.state();
        let output = tick(
            &mut region,
            &single_input(edge, player(1), entity(1), 1, move_to(3.5)),
        );
        assert!(silent(&output));
        expected.tick += 1;
        assert_eq!(region.state(), expected);
    }
}

#[test]
fn inputs_behind_an_arrival_are_applied_if_they_name_the_entity_that_arrived() {
    let mut region = at_the_line();
    let mut inputs = TickInputs::default();
    inputs.change(PlayerChange::Arrive(
        F,
        player(5),
        transfer(TRAVELLER, 17, IN_HOME),
    ));
    inputs.input(F, player(5), EntityId(TRAVELLER.0 - 1), 18, move_to(1.5));
    inputs.input(F, player(5), TRAVELLER, 17, move_to(2.5));
    inputs.input(F, player(5), TRAVELLER, 18, move_to(9.5));
    inputs.input(F, player(5), EntityId(TRAVELLER.0 + 1), 19, move_to(4.5));
    tick(&mut region, &inputs);
    let state = state_of(&region, player(5));
    assert_eq!((state.pose.position.x, state.last_input), (9.5, 18));
}

// ---------------------------------------------------------------------------------------
// Section 2.1 as a whole, in made-up ticks
// ---------------------------------------------------------------------------------------

/// A stay as far as section 2.1 is about it.
#[derive(Debug, Clone, PartialEq)]
struct StayHere {
    entity: EntityId,
    edge: EdgeId,
    last_input: u64,
    position: Vec3,
    /// How often the stay was handed on, and the attempt of its join for as long as no
    /// input of it has been applied (ADR-0020, sections 6 and 4.3).
    hops: u32,
    attempt: Option<u64>,
}

/// What a tick shows of stays: the entities removed, each with the chunk, the entities
/// spawned, and the joins refused, each in the order it happened.
#[derive(Debug, Default, PartialEq)]
struct Shown {
    removed: Vec<(EntityId, ChunkPos)>,
    spawned: Vec<EntityId>,
    refused: Vec<(EdgeId, PlayerId)>,
}

/// Section 2.1 of the record as a second implementation, for a region that holds every
/// chunk its players come to, so that nobody is let go or sent on: who is in the
/// region, with which entity, under which edge, how far their inputs are applied and
/// where they stand.
struct Stays {
    /// The edges the region knows.
    known: Vec<EdgeId>,
    entity_ids: EntityIds,
    next_entity_id: EntityId,
    players: BTreeMap<PlayerId, StayHere>,
}

impl Stays {
    /// One tick: every change in its order, then every input in its order.
    fn tick(&mut self, inputs: &TickInputs) -> Shown {
        let mut shown = Shown::default();
        for change in &inputs.player_changes {
            match change {
                // A join begins a new stay whatever the region has.
                PlayerChange::Join(edge, join) if self.known.contains(edge) => {
                    if let Some(old) = self.players.remove(&join.player) {
                        shown.removed.push((old.entity, chunk_of(old.position)));
                    }
                    if !self.entity_ids.contains(self.next_entity_id) {
                        shown.refused.push((*edge, join.player));
                        continue;
                    }
                    let stay = StayHere {
                        entity: self.next_entity_id,
                        edge: *edge,
                        last_input: 0,
                        position: SPAWN,
                        hops: 0,
                        attempt: Some(join.attempt),
                    };
                    self.next_entity_id.0 += 1;
                    shown.spawned.push(stay.entity);
                    self.players.insert(join.player, stay);
                }
                // A leave names the stay it ends: by its entity, or, where it names
                // none, by the attempt of its join (ADR-0020, section 4.4).
                PlayerChange::Leave(edge, id, named, attempt) => {
                    let ends = |stay: &StayHere| {
                        stay.edge == *edge
                            && match named {
                                Some(entity) => *entity == stay.entity,
                                None => attempt.is_some() && *attempt == stay.attempt,
                            }
                    };
                    if self.players.get(id).is_some_and(ends) {
                        let old = self.players.remove(id).expect("it was there");
                        shown.removed.push((old.entity, chunk_of(old.position)));
                    }
                }
                PlayerChange::Arrive(edge, id, transfer) => {
                    let on_its_way = (transfer.entity_id, chunk_of(transfer.pose.position));
                    if !self.known.contains(edge) {
                        shown.removed.push(on_its_way);
                        continue;
                    }
                    // The later stay stays: an arrival replaces a lower entity id, and
                    // of two copies of one stay the one that was handed on more often
                    // stays, without anything being shown removed (ADR-0020,
                    // section 6).
                    match self.players.get(id) {
                        Some(has) if has.entity == transfer.entity_id => {
                            if has.hops >= transfer.hops {
                                continue;
                            }
                        }
                        Some(has) if has.entity > transfer.entity_id => {
                            shown.removed.push(on_its_way);
                            continue;
                        }
                        Some(has) => shown.removed.push((has.entity, chunk_of(has.position))),
                        None => {}
                    }
                    let stay = StayHere {
                        entity: transfer.entity_id,
                        edge: *edge,
                        last_input: transfer.last_input,
                        position: transfer.pose.position,
                        hops: transfer.hops,
                        attempt: transfer.attempt,
                    };
                    shown.spawned.push(stay.entity);
                    self.players.insert(*id, stay);
                }
                PlayerChange::Join(..) | PlayerChange::Discard { .. } => {}
            }
        }
        // An input names the stay it is of.
        for (edge, id, entity, number, input) in &inputs.inputs {
            let Some(stay) = self.players.get_mut(id) else {
                continue;
            };
            if stay.edge != *edge || stay.entity != *entity || *number <= stay.last_input {
                continue;
            }
            stay.last_input = *number;
            // The edge has been heard from as this entity.
            stay.attempt = None;
            if let PlayerInput::Move {
                position: Some(position),
                ..
            } = input
            {
                stay.position = *position;
            }
        }
        shown
    }
}

#[test]
fn made_up_joins_arrivals_leaves_and_inputs_keep_to_what_the_record_says_of_stays() {
    let chunks = [HOME, WEST, NORTH, SOUTH];
    let points = [SPAWN, IN_HOME, IN_WEST, IN_NORTH, IN_SOUTH];
    let mut seen = [0usize; 6];
    for seed in 1..=6u64 {
        let mut random = Random(0xA076_1D64_78BD_642F ^ seed);
        // A region with entity ids to spare, and one that runs out of them on the way.
        let mut state = RegionState::new(ids());
        if seed % 3 == 0 {
            state.next_entity_id = EntityId(ids().end.0 - 60);
        }
        state.edges = [
            (E, edge_state(START, 1, 0, 0, &[])),
            (F, edge_state(START, 1, 0, 0, &[])),
        ]
        .into();
        let mut model = Stays {
            known: vec![E, F],
            entity_ids: state.entity_ids,
            next_entity_id: state.next_entity_id,
            players: BTreeMap::new(),
        };
        let mut region = Region::restore(config(0), state, pinned_everywhere(&chunks));

        for round in 0..4000 {
            let mut inputs = TickInputs::default();
            for _ in 0..random.below(6) {
                let n = 1 + u128::from(random.below(4));
                let id = player(n);
                let has = model.players.get(&id).cloned();
                let own = has
                    .as_ref()
                    .map_or(model.next_entity_id, |stay| stay.entity);
                // The entity the player has or will get, the one before, the one after,
                // and one from elsewhere.
                let named = EntityId(own.0 + random.pick(&[0, 0, 0, -1, 1, 3]));
                let edge = random.pick(&[E, E, E, F, F, STRANGER]);
                let point = random.pick(&points);
                match random.below(12) {
                    0 | 1 => inputs.change(join(edge, id)),
                    2 | 3 => {
                        // One in four names no entity: the attempt of the player's
                        // join, as an edge does, or now and then another, or none.
                        if random.once_in(4) {
                            let attempt = match random.below(6) {
                                0 => Some(attempt(id) + 1),
                                1 => None,
                                _ => Some(attempt(id)),
                            };
                            inputs.change(leave_of_attempt(edge, id, attempt));
                        } else {
                            inputs.change(leave(edge, id, Some(named)));
                        }
                    }
                    4 | 5 => {
                        // Every third arrival is of a stay that began elsewhere. No
                        // entity is of two players (section 2.5), and none arrives
                        // that the region has yet to give out.
                        let named = if random.once_in(3) {
                            EntityId(TRAVELLER.0 + random.below(6) as i32)
                        } else {
                            named
                        };
                        let taken = model
                            .players
                            .iter()
                            .any(|(other, stay)| *other != id && stay.entity == named);
                        let to_come = ids().contains(named) && named >= model.next_entity_id;
                        if taken || to_come {
                            continue;
                        }
                        // Not the player's own entity through an edge the region does
                        // not know: the record does not say whether an entity that is
                        // there is reported removed then.
                        if edge == STRANGER && has.is_some_and(|stay| stay.entity == named) {
                            continue;
                        }
                        // Handed on more or less often than the copy that may be there,
                        // and now and then with the attempt of the join still.
                        let last = random.below(30);
                        let arriving = PlayerTransfer {
                            hops: random.pick(&[0, 0, 1, 1, 2, 3]),
                            attempt: random.once_in(3).then(|| attempt(id)),
                            ..transfer(named, last, point)
                        };
                        inputs.change(PlayerChange::Arrive(edge, id, arriving));
                    }
                    _ => {
                        let last = has.map_or(0, |stay| stay.last_input);
                        let number = (last + random.pick(&[1, 1, 1, 2, 5, 41]))
                            .saturating_sub(random.pick(&[0, 0, 0, 1, 3]));
                        inputs.input(edge, id, named, number, walk_to(point));
                    }
                }
            }

            let expected = model.tick(&inputs);
            let output = tick(&mut region, &inputs);
            let state = region.state();
            let found: BTreeMap<PlayerId, StayHere> = state
                .players
                .iter()
                .map(|(id, player)| {
                    let stay = StayHere {
                        entity: player.entity_id,
                        edge: player.edge,
                        last_input: player.last_input,
                        position: player.pose.position,
                        hops: player.hops,
                        attempt: player.attempt,
                    };
                    (*id, stay)
                })
                .collect();
            let context = format!("seed {seed}, round {round}: {inputs:#?}");
            assert_eq!(found, model.players, "{context}");
            assert_eq!(state.next_entity_id, model.next_entity_id, "{context}");
            let refused: Vec<Durable> = expected
                .refused
                .iter()
                .map(|(_, id)| Durable::Refused {
                    player: *id,
                    attempt: attempt(*id),
                })
                .collect();
            assert_eq!(entries(&output), refused, "{context}");
            let edges: Vec<EdgeId> = output.durable.iter().map(|(edge, ..)| *edge).collect();
            let refusing: Vec<EdgeId> = expected.refused.iter().map(|(edge, _)| *edge).collect();
            assert_eq!(edges, refusing, "{context}");

            // What is shown of entities, but for one that came and went within the
            // tick: whether anyone is shown that, the record does not say.
            let both: BTreeSet<EntityId> = expected
                .spawned
                .iter()
                .filter(|entity| expected.removed.iter().any(|(gone, _)| gone == *entity))
                .copied()
                .collect();
            let mut gone = removed(&output);
            gone.retain(|(entity, _)| !both.contains(entity));
            let mut expected_gone = expected.removed.clone();
            expected_gone.retain(|(entity, _)| !both.contains(entity));
            assert_eq!(gone, expected_gone, "{context}");
            let mut come = spawned(&output);
            come.retain(|entity| !both.contains(entity));
            let mut expected_come = expected.spawned.clone();
            expected_come.retain(|entity| !both.contains(entity));
            assert_eq!(come, expected_come, "{context}");
            assert!(output.claims.is_empty() && output.returns.is_empty());

            seen[0] += expected.removed.len();
            seen[1] += expected.spawned.len();
            seen[2] += expected.refused.len();
            seen[3] += usize::from(!both.is_empty());
            seen[4] += moved(&output).len();
            seen[5] += model.players.len();
        }
    }
    assert!(seen.iter().all(|count| *count >= 300), "{seen:?}");
}

// ---------------------------------------------------------------------------------------
// S23: in its own pinned areas a region doubts what it believes
// ---------------------------------------------------------------------------------------

#[test]
fn an_arrival_for_a_chunk_of_the_regions_own_area_believed_anothers_is_taken_in() {
    let mut region = doubting();
    let arriving = transfer(TRAVELLER, 3, IN_SOUTH);
    let output = tick(&mut region, &arrive(F, player(5), arriving.clone()));
    assert!(output.durable.is_empty(), "{:?}", output.durable);
    assert_eq!(state_of(&region, player(5)), taken_in(&arriving, F));
    assert_eq!(spawned(&output), [TRAVELLER]);
    // The belief is dropped in that tick, and the chunk is claimed because they stand
    // in it.
    assert_eq!(output.claims, [SOUTH]);
    assert_eq!(region.knowledge(SOUTH), Knowledge::Asked);
}

#[test]
fn what_the_store_answers_after_a_doubt_is_the_truth_of_that_moment() {
    // `granted`, and the player stays.
    let mut region = doubting();
    tick(
        &mut region,
        &arrive(F, player(5), transfer(TRAVELLER, 3, IN_SOUTH)),
    );
    let output = tick(&mut region, &granted(&[SOUTH]));
    assert!(output.durable.is_empty());
    assert_eq!(region.knowledge(SOUTH), Knowledge::Held);
    assert!(region.player(player(5)).is_some());

    // `foreign`, and they are let go once more, to a region that holds the chunk.
    let mut region = doubting();
    let arriving = transfer(TRAVELLER, 3, IN_SOUTH);
    tick(&mut region, &arrive(F, player(5), arriving.clone()));
    let output = tick(&mut region, &foreign(&[(SOUTH, OTHER)]));
    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::Departed {
                player: player(5),
                // As they came, and handed on once more (ADR-0020, section 6).
                transfer: PlayerTransfer {
                    hops: arriving.hops + 1,
                    ..arriving
                },
                to: OTHER,
            }
        )]
    );
    assert!(region.player(player(5)).is_none());
    assert_eq!(region.knowledge(SOUTH), Knowledge::Foreign(OTHER));
}

#[test]
fn an_arrival_of_a_later_stay_for_a_doubted_chunk_replaces_the_stay_and_drops_the_belief() {
    let mut region = doubting();
    let arriving = transfer(TRAVELLER, 3, IN_SOUTH);
    let output = tick(&mut region, &arrive(F, player(1), arriving.clone()));
    assert_eq!(removed(&output), [(entity(1), HOME)]);
    assert_eq!(state_of(&region, player(1)), taken_in(&arriving, F));
    assert_eq!(output.claims, [SOUTH]);
}

#[test]
fn a_remote_action_about_a_chunk_of_the_regions_own_area_believed_anothers_goes_on_without_a_region()
 {
    let block = SOUTH_BLOCK;
    let steps = [
        break_at(block),
        place_against(block, block.offset(0, 1, 0), ELSEWHERE),
        place_at(block, ELSEWHERE),
    ];
    for step in steps {
        let mut region = doubting();
        let action = remote(player(9), 4, step);
        let output = tick(&mut region, &remotely(F, action.clone()));
        assert_eq!(
            output.durable,
            vec![(F, 1, Durable::Remote { action, to: None })]
        );
        assert!(output.events.is_empty());
        // The ticket that kept the belief alive still wants the chunk.
        assert_eq!(output.claims, [SOUTH]);
        assert_eq!(region.knowledge(SOUTH), Knowledge::Asked);
    }
}

#[test]
fn a_doubted_chunk_that_nothing_wants_at_the_end_of_the_tick_is_unknown_and_not_claimed() {
    // The last ticket goes in the tick of the action: the belief is still there when
    // the action is judged (it is dropped for want of a ticket only at the end of a
    // tick), and nothing is left to claim the chunk for.
    let mut region = doubting();
    let action = remote(player(9), 4, break_at(SOUTH_BLOCK));
    let mut inputs = remotely(F, action.clone());
    inputs.tickets_removed = vec![viewer(SOUTH)];
    let output = tick(&mut region, &inputs);
    assert_eq!(entries(&output), [Durable::Remote { action, to: None }]);
    assert!(output.claims.is_empty());
    assert_eq!(region.knowledge(SOUTH), Knowledge::Unknown);

    // The store's word comes in the tick of the action, for a chunk nothing wants:
    // between a `foreign` and the end of its tick the chunk is believed that region's.
    let mut region = at_the_line();
    let action = remote(player(9), 4, break_at(BlockPos::new(8, 63, -1)));
    let mut inputs = remotely(F, action.clone());
    inputs.foreign = vec![(NORTH, OTHER)];
    let output = tick(&mut region, &inputs);
    assert_eq!(entries(&output), [Durable::Remote { action, to: None }]);
    assert!(output.claims.is_empty());
    assert_eq!(region.knowledge(NORTH), Knowledge::Unknown);
}

#[test]
fn outside_its_pinned_areas_a_region_answers_not_mine_as_before() {
    // `EAST` is not of the stripe the region is pinned to.
    let mut region = at_the_line();
    let arriving = transfer(TRAVELLER, 3, IN_EAST);
    let action = remote(player(9), 4, break_at(BlockPos::new(16, 63, 8)));
    let mut inputs = arrive(F, player(5), arriving.clone());
    inputs.remote_actions.push((E, action.clone()));
    let output = tick(&mut region, &inputs);
    assert_eq!(
        output.durable,
        vec![
            (
                F,
                1,
                Durable::NotMine {
                    what: Misdirected::Arrival {
                        player: player(5),
                        transfer: arriving,
                    },
                    holder: REGION_B,
                }
            ),
            (
                E,
                1,
                Durable::NotMine {
                    what: Misdirected::Remote(action),
                    holder: REGION_B,
                }
            ),
        ]
    );
    assert!(region.player(player(5)).is_none());
    assert!(output.claims.is_empty());
    assert_eq!(region.knowledge(EAST), Knowledge::Foreign(REGION_B));
}

#[test]
fn what_is_passed_over_for_another_reason_leaves_the_belief_as_it_is() {
    let own = entity(1);
    let earlier = EntityId(own.0 - 1);
    let passed_over = [
        // An arrival of an earlier stay of a player who is there, and of the same.
        arrive(E, player(1), transfer(earlier, 3, IN_SOUTH)),
        arrive(F, player(1), transfer(own, 3, IN_SOUTH)),
        // An arrival and an action through an edge the region does not know.
        arrive(STRANGER, player(5), transfer(TRAVELLER, 3, IN_SOUTH)),
        remotely(STRANGER, remote(player(9), 4, break_at(SOUTH_BLOCK))),
    ];
    for inputs in passed_over {
        let mut region = doubting();
        let output = tick(&mut region, &inputs);
        assert!(output.durable.is_empty(), "{inputs:?}");
        assert!(output.claims.is_empty(), "{inputs:?}");
        assert_eq!(
            region.knowledge(SOUTH),
            Knowledge::Foreign(OTHER),
            "{inputs:?}"
        );
        assert_eq!(region.player_count(), 1);
    }
}

#[test]
fn a_players_own_action_on_a_doubted_chunk_names_the_believed_region_as_before() {
    let mut region = doubting();
    tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, walk_to(BY_SOUTH)),
    );
    let output = tick(
        &mut region,
        &single_input(E, player(1), entity(1), 2, dig(SOUTH_BLOCK, 6)),
    );
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Remote {
                action: own(player(1), entity(1), 6, break_at(SOUTH_BLOCK)),
                to: Some(OTHER),
            }
        )]
    );
    assert!(output.claims.is_empty());
    assert_eq!(region.knowledge(SOUTH), Knowledge::Foreign(OTHER));
}

#[test]
fn a_player_who_steps_into_a_doubted_chunk_is_let_go_to_the_believed_region_as_before() {
    // No `NotMine` would have been made here either: the region lets its own player go
    // to the region it believes to hold the chunk, in the tick of the step.
    let mut region = doubting();
    let output = tick(
        &mut region,
        &single_input(E, player(1), entity(1), 1, walk_to(IN_SOUTH)),
    );
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::Departed { player: id, transfer, to: OTHER })]
            if *id == player(1) && transfer.pose.position == IN_SOUTH && transfer.last_input == 1
    ));
    assert!(region.player(player(1)).is_none());
    assert!(output.claims.is_empty());
    assert_eq!(region.knowledge(SOUTH), Knowledge::Foreign(OTHER));
}

#[test]
fn a_remote_placement_whose_step_concerns_a_held_chunk_leaves_the_belief_about_its_target() {
    // The step concerns the block it is placed against, which is the region's; it goes
    // on to the target with the believed region's name, where no `NotMine` would have
    // been made, so the belief stays (section 2.2: "and nowhere else").
    let mut region = doubting();
    let action = remote(
        player(9),
        4,
        place_against(BY_SOUTH_BLOCK, SOUTH_BLOCK, ELSEWHERE),
    );
    let output = tick(&mut region, &remotely(F, action));
    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::Remote {
                action: remote(player(9), 4, place_at(SOUTH_BLOCK, ELSEWHERE)),
                to: Some(OTHER),
            }
        )]
    );
    assert!(output.claims.is_empty());
    assert_eq!(region.knowledge(SOUTH), Knowledge::Foreign(OTHER));
}

#[test]
fn an_arrival_and_an_action_for_a_doubted_chunk_in_one_tick_are_both_handled_as_for_an_unknown_one()
{
    let mut region = doubting();
    let arriving = transfer(TRAVELLER, 3, IN_SOUTH);
    let action = remote(player(9), 4, break_at(SOUTH_BLOCK));
    let mut inputs = arrive(F, player(5), arriving);
    inputs.remote_actions.push((E, action.clone()));
    let output = tick(&mut region, &inputs);
    assert_eq!(
        output.durable,
        vec![(E, 1, Durable::Remote { action, to: None })]
    );
    assert!(region.player(player(5)).is_some());
    assert_eq!(output.claims, [SOUTH]);
}

// ---------------------------------------------------------------------------------------
// States made by hand, and a generator
// ---------------------------------------------------------------------------------------

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
}

/// A player of a state made by hand, standing in the middle of `chunk`, who differs
/// from a player with another entity in everything a state says of a player.
fn someone(entity: EntityId, edge: EdgeId, chunk: ChunkPos) -> PlayerState {
    let n = entity.0;
    let mut bar = hotbar();
    bar[(n % 8 + 1) as usize] = Some(ItemStack {
        item: items::STONE,
        count: n % 60 + 1,
    });
    PlayerState {
        entity_id: entity,
        name: format!("entity-{n}"),
        pose: Pose {
            position: middle(chunk),
            yaw: (n % 360) as f32,
            pitch: -10.0,
            on_ground: true,
        },
        hotbar: bar,
        selected_slot: (n % 9) as u8,
        last_input: n as u64 % 50 + 2,
        handled: Some(n % 30),
        edge,
        hops: 0,
        flying: false,
        attempt: None,
    }
}

fn edge_state(
    start: u64,
    since: u64,
    applied: u64,
    sent: u64,
    outbox: &[(u64, Durable)],
) -> EdgeState {
    EdgeState {
        start,
        since,
        applied,
        sent,
        outbox: outbox.iter().cloned().collect(),
    }
}

fn a_state(
    tick: u64,
    entity_ids: EntityIds,
    next_entity_id: EntityId,
    players: &[(PlayerId, PlayerState)],
    edges: &[(EdgeId, EdgeState)],
) -> RegionState {
    RegionState {
        tick,
        entity_ids,
        next_entity_id,
        players: players.iter().cloned().collect(),
        edges: edges.iter().cloned().collect(),
        entering: BTreeMap::new(),
    }
}

/// The block of entity ids of the region that is absorbed.
fn other_ids() -> EntityIds {
    EntityIds::block(4).expect("block 4 exists")
}

fn refused(n: u128) -> Durable {
    Durable::Refused {
        player: player(n),
        attempt: attempt(player(n)),
    }
}

fn done(n: u128, sequence: i32) -> Durable {
    Durable::RemoteDone {
        player: player(n),
        entity: elsewhere(player(n)),
        sequence,
    }
}

fn departed(n: u128, entity: EntityId, to: RegionId) -> Durable {
    Durable::Departed {
        player: player(n),
        transfer: transfer(entity, 4, IN_EAST),
        to,
    }
}

/// The entry a merge makes, without what stands behind it.
fn absorbed(region: RegionId, since: u64, applied: u64, numbers: &[u64]) -> Durable {
    Durable::Absorbed {
        region,
        since,
        applied,
        numbers: numbers.to_vec(),
    }
}

/// Region 0 of the stripes as a restore makes it of `state`, holding the home chunk.
fn survivor(state: RegionState) -> Region {
    let holdings = Holdings {
        held: vec![HOME],
        pinned: vec![WESTERN],
    };
    Region::restore(config(0), state, holdings)
}

/// Section 2.3 of the record, step for step: the state a region with the state `ours`
/// has after absorbing the region `absorbed`, whose state is `theirs`.
fn absorbed_as_the_record_says(
    ours: &RegionState,
    absorbed: RegionId,
    theirs: &RegionState,
) -> RegionState {
    // 1. The tick is `M`.
    let tick = ours.tick + 1;
    let mut state = ours.clone();
    state.tick = tick;

    // 2. Edges, in ascending order. `reset_away` are the edges for which the other's
    // side counts as not there.
    let edges: BTreeSet<EdgeId> = ours
        .edges
        .keys()
        .chain(theirs.edges.keys())
        .copied()
        .collect();
    let mut reset_away = BTreeSet::new();
    let anew = |start| EdgeState {
        start,
        since: tick,
        applied: 0,
        sent: 0,
        outbox: BTreeMap::new(),
    };
    for edge in &edges {
        match (ours.edges.get(edge), theirs.edges.get(edge)) {
            (Some(a), Some(b)) if b.start < a.start => {
                reset_away.insert(*edge);
            }
            (Some(a), Some(b)) if a.start < b.start => {
                state.players.retain(|_, player| player.edge != *edge);
                // Its entering stays of the edge go where its players of it go
                // (ADR-0020, section 9).
                state.entering.retain(|_, entering| entering.edge != *edge);
                state.edges.insert(*edge, anew(b.start));
            }
            (None, Some(b)) => {
                state.edges.insert(*edge, anew(b.start));
            }
            _ => {}
        }
    }

    // 3. Players of the other state whose edge was not reset away, in ascending order.
    // Since ADR-0020 the later stay is the one with the higher entity id, and of the
    // same entity the one with more hand-overs (its section 6). The entering stays are
    // this region's alone: what the other state has of them is dropped (its section 9).
    for (id, theirs) in &theirs.players {
        if reset_away.contains(&theirs.edge) {
            continue;
        }
        let stays = (state.players.get(id))
            .is_some_and(|own| (own.entity_id, own.hops) >= (theirs.entity_id, theirs.hops));
        if !stays {
            state.players.insert(*id, theirs.clone());
        }
    }

    // 4. Entries, for each edge of step 2 in ascending order.
    for edge in &edges {
        let b = theirs
            .edges
            .get(edge)
            .filter(|_| !reset_away.contains(edge));
        let a = state.edges.get_mut(edge).expect("step 2 made it");
        a.sent += 1;
        a.outbox.insert(
            a.sent,
            Durable::Absorbed {
                region: absorbed,
                since: b.map_or(0, |b| b.since),
                applied: b.map_or(0, |b| b.applied),
                numbers: b.map_or_else(Vec::new, |b| b.outbox.keys().copied().collect()),
            },
        );
        for entry in b.into_iter().flat_map(|b| b.outbox.values()) {
            a.sent += 1;
            a.outbox.insert(a.sent, entry.clone());
        }
    }

    // 5. `entity_ids` and `next_entity_id` are this region's: they were never touched.
    state
}

/// A state as some run could have left it: up to four edges with starts that differ
/// now and then and outboxes with gaps, and up to six players spread over them.
fn made_up_state(random: &mut Random, entity_ids: EntityIds) -> RegionState {
    let mut state = RegionState::new(entity_ids);
    state.tick = 1 + random.below(500);
    state.next_entity_id = EntityId(entity_ids.first.0 + random.below(40) as i32);
    for edge in 1..=4 {
        if random.once_in(3) {
            continue;
        }
        let sent = random.below(8);
        let mut outbox = BTreeMap::new();
        for number in 1..=sent {
            if random.once_in(2) {
                outbox.insert(number, made_up_entry(random));
            }
        }
        let edge_state = EdgeState {
            start: random.pick(&[5, 10, 10]),
            since: 1 + random.below(state.tick),
            applied: random.below(60),
            sent,
            outbox,
        };
        state.edges.insert(EdgeId(edge), edge_state);
    }
    let known: Vec<EdgeId> = state.edges.keys().copied().collect();
    if known.is_empty() {
        return state;
    }
    let chunks = [HOME, EAST, WEST, SOUTH, ChunkPos::new(3, 1)];
    for n in 1..=6 {
        if random.once_in(2) {
            continue;
        }
        // Few entities for many players, so that one player is in both states with the
        // lower, the same and the higher id, and two players have one id now and then.
        let entity = EntityId(ids().first.0 + random.below(12) as i32);
        let edge = random.pick(&known);
        let chunk = random.pick(&chunks);
        // Handed on more or less often, so that two copies of a stay differ in it.
        let stay = PlayerState {
            hops: random.below(3) as u32,
            ..someone(entity, edge, chunk)
        };
        state.players.insert(player(n), stay);
    }
    // Now and then a stay that is entering (ADR-0020, section 4), of a player the
    // region has or has not.
    for n in 1..=6 {
        if random.once_in(5) {
            let entering = EnteringState {
                entity_id: EntityId(ids().first.0 + 20 + random.below(12) as i32),
                name: format!("entering-{n}"),
                edge: random.pick(&known),
                attempt: 100 + random.below(50),
            };
            state.entering.insert(player(n), entering);
        }
    }
    state
}

fn made_up_entry(random: &mut Random) -> Durable {
    let n = u128::from(random.below(6)) + 1;
    let entity = EntityId(ids().first.0 + random.below(12) as i32);
    match random.below(6) {
        0 => refused(n),
        1 => done(n, random.below(40) as i32),
        2 => departed(n, entity, random.pick(&[REGION_A, REGION_B, OTHER])),
        3 => Durable::Remote {
            action: remote(player(n), random.below(40) as i32, break_at(OWN_BLOCK)),
            to: random.pick(&[None, Some(REGION_A), Some(REGION_B)]),
        },
        4 => absorbed(OTHER, random.below(9), random.below(9), &[2, 5]),
        _ => Durable::SplitOff {
            region: PART,
            players: vec![(player(n), entity, None)],
        },
    }
}

// ---------------------------------------------------------------------------------------
// S24 and S25: the players of a merge
// ---------------------------------------------------------------------------------------

/// Two edges as two regions that never lost them know them, with nothing to carry over.
fn quiet_edges(since: u64) -> [(EdgeId, EdgeState); 2] {
    [
        (E, edge_state(10, since, 17, 2, &[])),
        (F, edge_state(10, since, 5, 0, &[])),
    ]
}

#[test]
fn absorbing_a_state_with_other_players_gives_a_state_with_all_of_them_as_they_were() {
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[
            (player(1), someone(entity(1), E, HOME)),
            (player(4), someone(entity(4), F, WEST)),
        ],
        &quiet_edges(3),
    );
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[
            (player(2), someone(entity(2), E, EAST)),
            (player(3), someone(entity(3), F, ChunkPos::new(2, 1))),
        ],
        &quiet_edges(9),
    );
    let merged = survivor(ours.clone()).absorb(REGION_B, &theirs);

    assert_eq!(
        merged.tick, 41,
        "the tick is the one after the region's last"
    );
    let mut everyone = ours.players.clone();
    everyone.extend(theirs.players.clone());
    assert_eq!(everyone.len(), 4);
    assert_eq!(merged.players, everyone);
    assert_eq!(
        merged,
        absorbed_as_the_record_says(&ours, REGION_B, &theirs)
    );
}

#[test]
fn of_a_player_in_both_states_the_stay_with_the_higher_entity_id_stays() {
    // Whichever side has it, and with the edge it is of.
    for ours_is_later in [true, false] {
        let (own, other) = if ours_is_later {
            (entity(6), entity(2))
        } else {
            (entity(2), entity(6))
        };
        let here = someone(own, E, HOME);
        let there = someone(other, F, EAST);
        let bystander = someone(entity(3), F, WEST);
        let ours = a_state(
            40,
            ids(),
            entity(9),
            &[(player(1), here.clone()), (player(3), bystander.clone())],
            &quiet_edges(3),
        );
        let theirs = a_state(
            77,
            other_ids(),
            other_ids().first,
            &[(player(1), there.clone())],
            &quiet_edges(9),
        );
        let merged = survivor(ours).absorb(REGION_B, &theirs);
        let later = if ours_is_later { here } else { there };
        assert_eq!(later.entity_id, entity(6));
        assert_eq!(merged.players.len(), 2);
        assert_eq!(merged.players[&player(1)], later);
        assert_eq!(merged.players[&player(3)], bystander);
    }
}

#[test]
fn a_merge_keeps_two_players_who_have_one_entity_id_and_its_own_of_two_stays_with_one_id() {
    // Section 2.5 says why neither can be, and that `absorb` does not look: it keeps
    // both players, and of one player with one id on both sides the region's own, as
    // the other's id is not higher.
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[
            (player(1), someone(entity(5), E, HOME)),
            (player(2), someone(entity(4), E, HOME)),
        ],
        &quiet_edges(3),
    );
    let mut same = someone(entity(4), F, EAST);
    same.last_input = 900;
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[(player(2), same), (player(3), someone(entity(5), F, EAST))],
        &quiet_edges(9),
    );
    let merged = survivor(ours.clone()).absorb(REGION_B, &theirs);
    assert_eq!(merged.players.len(), 3);
    assert_eq!(merged.players[&player(1)], ours.players[&player(1)]);
    assert_eq!(merged.players[&player(2)], ours.players[&player(2)]);
    assert_eq!(merged.players[&player(3)], theirs.players[&player(3)]);
}

// ---------------------------------------------------------------------------------------
// S26: the edges of a merge
// ---------------------------------------------------------------------------------------

#[test]
fn an_edge_both_know_with_one_start_keeps_its_numbers_and_gets_the_others_entries_behind_the_entry()
{
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[],
        &[(
            E,
            edge_state(10, 3, 17, 6, &[(5, refused(1)), (6, done(1, 2))]),
        )],
    );
    let on_their_way = departed(2, entity(2), REGION_A);
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[],
        &[(
            E,
            edge_state(
                10,
                9,
                30,
                12,
                &[
                    (8, on_their_way.clone()),
                    (11, refused(2)),
                    (12, done(3, 7)),
                ],
            ),
        )],
    );
    let merged = survivor(ours).absorb(REGION_B, &theirs);
    // The survivor's `since`, `applied` and numbers stay; the entry is next, with what
    // a welcome of the other would have said; the other's entries stand behind it
    // under the next numbers, as they were, with their old numbers in the entry; and
    // `sent` is the last of them.
    let expected = edge_state(
        10,
        3,
        17,
        10,
        &[
            (5, refused(1)),
            (6, done(1, 2)),
            (7, absorbed(REGION_B, 9, 30, &[8, 11, 12])),
            (8, on_their_way),
            (9, refused(2)),
            (10, done(3, 7)),
        ],
    );
    assert_eq!(merged.edges, [(E, expected)].into());
}

#[test]
fn an_edge_only_the_survivor_knows_gets_an_entry_with_nothing_behind_it() {
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[(player(1), someone(entity(1), F, HOME))],
        &[(F, edge_state(10, 3, 5, 2, &[(2, refused(1))]))],
    );
    let theirs = a_state(77, other_ids(), other_ids().first, &[], &[]);
    let merged = survivor(ours.clone()).absorb(REGION_B, &theirs);
    let expected = edge_state(
        10,
        3,
        5,
        3,
        &[(2, refused(1)), (3, absorbed(REGION_B, 0, 0, &[]))],
    );
    assert_eq!(merged.edges, [(F, expected)].into());
    assert_eq!(merged.players, ours.players);
}

#[test]
fn an_edge_only_the_absorbed_region_knows_is_known_since_the_merge_with_the_entry_numbered_one() {
    let ours = a_state(40, ids(), entity(9), &[], &[]);
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[(player(2), someone(entity(2), E, EAST))],
        &[(E, edge_state(4, 9, 8, 3, &[(3, done(2, 1))]))],
    );
    let merged = survivor(ours).absorb(REGION_B, &theirs);
    // Nothing applied and nothing confirmed here; what the edge shared with the other
    // is in the entry.
    let expected = edge_state(
        4,
        41,
        0,
        2,
        &[(1, absorbed(REGION_B, 9, 8, &[3])), (2, done(2, 1))],
    );
    assert_eq!(merged.edges, [(E, expected)].into());
    assert_eq!(merged.players, theirs.players);
}

#[test]
fn an_edge_the_absorbed_region_knows_with_a_lower_start_is_as_one_it_did_not_know() {
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[(player(1), someone(entity(1), E, HOME))],
        &[
            (E, edge_state(10, 3, 5, 4, &[(4, refused(1))])),
            (F, edge_state(10, 3, 0, 0, &[])),
        ],
    );
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[
            // Of the edge that is reset away: with a later stay of a player the survivor
            // has, and alone.
            (player(1), someone(entity(6), E, EAST)),
            (player(2), someone(entity(2), E, EAST)),
            // Of an edge that is not.
            (player(3), someone(entity(3), F, EAST)),
        ],
        &[
            (
                E,
                edge_state(7, 2, 99, 5, &[(5, departed(4, entity(4), REGION_A))]),
            ),
            (F, edge_state(10, 2, 6, 0, &[])),
        ],
    );
    let merged = survivor(ours.clone()).absorb(REGION_B, &theirs);
    // Its players of that edge do not come in, its entries are not carried over, and
    // the entry says that it had forgotten the edge.
    let expected = edge_state(
        10,
        3,
        5,
        5,
        &[(4, refused(1)), (5, absorbed(REGION_B, 0, 0, &[]))],
    );
    assert_eq!(merged.edges[&E], expected);
    assert_eq!(
        merged.players,
        [
            (player(1), ours.players[&player(1)].clone()),
            (player(3), theirs.players[&player(3)].clone()),
        ]
        .into()
    );
}

#[test]
fn an_edge_the_survivor_knows_with_a_lower_start_is_reset_and_then_as_one_only_the_other_knows() {
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[
            // Of the edge that is reset: alone, and with an earlier stay on the other
            // side under an edge that is not.
            (player(1), someone(entity(1), E, HOME)),
            (player(4), someone(entity(8), E, HOME)),
            (player(5), someone(entity(5), F, HOME)),
        ],
        &[
            (
                E,
                edge_state(5, 3, 40, 9, &[(9, departed(6, entity(6), REGION_B))]),
            ),
            (F, edge_state(10, 3, 0, 0, &[])),
        ],
    );
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[
            (player(2), someone(entity(2), E, EAST)),
            (player(4), someone(entity(4), F, EAST)),
        ],
        &[
            (
                E,
                edge_state(8, 20, 6, 2, &[(1, refused(7)), (2, done(2, 3))]),
            ),
            (F, edge_state(10, 2, 6, 0, &[])),
        ],
    );
    let merged = survivor(ours.clone()).absorb(REGION_B, &theirs);
    // Its own players of that edge are gone and its outbox is dropped; the edge is
    // known since the merge with the other's start, and the other's entries follow the
    // entry from 1.
    let expected = edge_state(
        8,
        41,
        0,
        3,
        &[
            (1, absorbed(REGION_B, 20, 6, &[1, 2])),
            (2, refused(7)),
            (3, done(2, 3)),
        ],
    );
    assert_eq!(merged.edges[&E], expected);
    // Player 4's stay here went with the edge, so the other side's comes in whole,
    // though its entity id is the lower: the region does not have the player any more
    // when the other's players are looked at (steps 2 and 3).
    assert_eq!(
        merged.players,
        [
            (player(2), theirs.players[&player(2)].clone()),
            (player(4), theirs.players[&player(4)].clone()),
            (player(5), ours.players[&player(5)].clone()),
        ]
        .into()
    );
}

#[test]
fn every_edge_either_region_knows_gets_its_entry_whatever_the_others_get() {
    // The five cases side by side, on edges in an order that mixes them.
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[],
        &[
            (EdgeId(1), edge_state(10, 3, 1, 0, &[])),
            (EdgeId(3), edge_state(10, 4, 2, 7, &[(7, refused(1))])),
            (EdgeId(4), edge_state(5, 5, 3, 2, &[(1, refused(2))])),
            (EdgeId(5), edge_state(10, 6, 4, 1, &[])),
        ],
    );
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[],
        &[
            (EdgeId(2), edge_state(6, 11, 21, 1, &[(1, done(1, 1))])),
            (EdgeId(3), edge_state(10, 12, 22, 4, &[(4, done(2, 2))])),
            (EdgeId(4), edge_state(9, 13, 23, 0, &[])),
            (EdgeId(5), edge_state(2, 14, 24, 3, &[(3, done(3, 3))])),
        ],
    );
    let merged = survivor(ours).absorb(REGION_B, &theirs);
    let expected: BTreeMap<EdgeId, EdgeState> = [
        (
            EdgeId(1),
            edge_state(10, 3, 1, 1, &[(1, absorbed(REGION_B, 0, 0, &[]))]),
        ),
        (
            EdgeId(2),
            edge_state(
                6,
                41,
                0,
                2,
                &[(1, absorbed(REGION_B, 11, 21, &[1])), (2, done(1, 1))],
            ),
        ),
        (
            EdgeId(3),
            edge_state(
                10,
                4,
                2,
                9,
                &[
                    (7, refused(1)),
                    (8, absorbed(REGION_B, 12, 22, &[4])),
                    (9, done(2, 2)),
                ],
            ),
        ),
        (
            EdgeId(4),
            edge_state(9, 41, 0, 1, &[(1, absorbed(REGION_B, 13, 23, &[]))]),
        ),
        (
            EdgeId(5),
            edge_state(10, 6, 4, 2, &[(2, absorbed(REGION_B, 0, 0, &[]))]),
        ),
    ]
    .into();
    assert_eq!(merged.edges, expected);
}

#[test]
fn entries_that_name_either_region_or_tell_of_an_earlier_merge_or_split_are_carried_as_they_are() {
    // Nothing of an entry is rewritten (section 2.3): not one that names the absorbed
    // region in the survivor's outbox, nor one of the absorbed region that names the
    // survivor. An `Absorbed` or a `SplitOff` of the absorbed region stands behind the
    // new entry like any other (rules 44 and 50).
    let own = [
        (1, departed(1, entity(1), REGION_B)),
        (
            2,
            Durable::NotMine {
                what: Misdirected::Remote(remote(player(1), 3, break_at(OWN_BLOCK))),
                holder: REGION_B,
            },
        ),
    ];
    let inner = [
        (2, departed(2, entity(2), REGION_A)),
        (3, absorbed(OTHER, 6, 7, &[1, 4])),
        (4, refused(3)),
        (
            5,
            Durable::Remote {
                action: remote(player(4), 8, break_at(OWN_BLOCK)),
                to: Some(REGION_A),
            },
        ),
        (
            6,
            Durable::SplitOff {
                region: PART,
                players: vec![(player(5), entity(5), None), (player(6), entity(4), None)],
            },
        ),
    ];
    let ours = a_state(
        40,
        ids(),
        entity(9),
        &[],
        &[(E, edge_state(10, 3, 1, 2, &own))],
    );
    let theirs = a_state(
        77,
        other_ids(),
        other_ids().first,
        &[],
        &[(E, edge_state(10, 9, 30, 6, &inner))],
    );
    let merged = survivor(ours).absorb(REGION_B, &theirs);
    let mut outbox: Vec<(u64, Durable)> = own.to_vec();
    outbox.push((3, absorbed(REGION_B, 9, 30, &[2, 3, 4, 5, 6])));
    outbox.extend(
        inner
            .iter()
            .zip(4..)
            .map(|((_, entry), number)| (number, entry.clone())),
    );
    assert_eq!(merged.edges[&E], edge_state(10, 3, 1, 8, &outbox));
}

#[test]
fn absorbing_made_up_states_gives_what_the_record_says_step_for_step() {
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let mut seen = [0usize; 6];
    for round in 0..3000 {
        let ours = made_up_state(&mut random, ids());
        let theirs = made_up_state(&mut random, other_ids());
        let region = survivor(ours.clone());
        let merged = region.absorb(REGION_B, &theirs);
        let expected = absorbed_as_the_record_says(&ours, REGION_B, &theirs);
        assert_eq!(
            merged, expected,
            "round {round}: absorbing\n{theirs:#?}\ninto\n{ours:#?}"
        );
        assert_eq!(
            region.state(),
            ours,
            "round {round}: `absorb` changes nothing"
        );

        // What the run was good for.
        for (edge, a) in &ours.edges {
            match theirs.edges.get(edge) {
                None => seen[0] += 1,
                Some(b) if b.start == a.start => seen[1] += 1,
                Some(b) if b.start < a.start => seen[2] += 1,
                Some(_) => seen[3] += 1,
            }
        }
        seen[4] += theirs
            .edges
            .keys()
            .filter(|edge| !ours.edges.contains_key(edge))
            .count();
        seen[5] += theirs
            .players
            .keys()
            .filter(|id| ours.players.contains_key(id))
            .count();
    }
    assert!(seen.iter().all(|count| *count >= 300), "{seen:?}");
}

// ---------------------------------------------------------------------------------------
// S28 and S29: entity ids, and that a plan changes nothing
// ---------------------------------------------------------------------------------------

/// The state of a region that is absorbed by [`before_a_merge`]: players 2 and 3, who
/// entered the world at home and have their entities from its block, east of the line.
fn absorbed_state() -> RegionState {
    a_state(
        90,
        other_ids(),
        other_ids().first,
        &[
            (player(2), someone(entity(2), E, EAST)),
            (player(3), someone(entity(3), F, ChunkPos::new(2, 1))),
        ],
        &[
            (E, edge_state(10, 5, 12, 3, &[(3, done(2, 6))])),
            (F, edge_state(10, 5, 4, 0, &[])),
        ],
    )
}

/// The area the absorbed region was pinned to, a chunk of it that the survivor
/// believed a third region to hold, a chunk the survivor holds outside every area, and
/// a chunk outside every area that nobody has said anything of.
const AREA_B: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: Some(4),
};
const THIRD: ChunkPos = ChunkPos::new(3, 0);
const FAR: ChunkPos = ChunkPos::new(5, 5);
const BEYOND: ChunkPos = ChunkPos::new(7, 0);

/// Region 0 of the stripes as it is before it absorbs its neighbour, knowing chunks in
/// every way a region can:
///
/// - `HOME` and `WEST`, of its stripe: held and loaded for a viewer, and `HOME` with a
///   block broken;
/// - `NORTH`, of its stripe: held for a guest who has gone, and not loaded;
/// - `FAR`, outside its stripe: granted, and loaded for a guest;
/// - `EAST` believed region 1's and `THIRD` believed `OTHER`'s, each for a viewer;
/// - `SOUTH` asked for.
///
/// It knows `E` and `F`. Player 1 is there with `entity(1)`; players 2 and 3 entered
/// the world here as well, and are gone.
fn before_a_merge(return_after: u64) -> Region {
    let holdings = Holdings {
        held: Vec::new(),
        pinned: vec![WESTERN],
    };
    let mut region = Region::new(config(return_after), ids(), holdings);
    hold(&mut region, HOME);
    hold(&mut region, WEST);
    let output = tick(&mut region, &add(&[guest(NORTH)]));
    assert_eq!(output.claims, [NORTH]);
    tick(
        &mut region,
        &TickInputs {
            granted: vec![NORTH],
            tickets_removed: vec![guest(NORTH)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.knowledge(NORTH), Knowledge::Held);
    let output = tick(
        &mut region,
        &TickInputs {
            granted: vec![FAR],
            tickets_added: vec![guest(FAR)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, [FAR]);
    tick(&mut region, &delivered(&[FAR]));
    learn(&mut region, EAST, REGION_B);
    learn(&mut region, THIRD, OTHER);
    ask(&mut region, SOUTH);
    tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    tick(
        &mut region,
        &changes(vec![
            join(E, player(1)),
            join(E, player(2)),
            join(F, player(3)),
        ]),
    );
    let mut inputs = changes(vec![
        leave(E, player(2), Some(entity(2))),
        leave(F, player(3), Some(entity(3))),
    ]);
    inputs.input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 1));
    tick(&mut region, &inputs);

    assert_eq!(region.player_count(), 1);
    assert_eq!(region.state().next_entity_id, entity(4));
    assert_eq!(block(&region, OWN_BLOCK), Some(blocks::AIR));
    assert_eq!(region.loaded_chunk_count(), 3);
    assert_eq!(region.held_chunk_count(), 4);
    assert!(region.chunk(NORTH).is_none());
    region
}

#[test]
fn the_merged_state_has_the_survivors_block_and_a_join_gets_the_id_it_would_have_got() {
    let mut region = before_a_merge(0);
    let before = region.state();
    let state = region.absorb(REGION_B, &absorbed_state());
    assert_eq!(state.entity_ids, ids());
    assert_eq!(state.next_entity_id, before.next_entity_id);

    // A region that never merged gives the next player the same entity.
    let mut never = region.clone();
    let output = tick(&mut never, &changes(vec![join(E, player(8))]));
    assert_eq!(spawned(&output), [entity(4)]);

    region.take_absorbed(state, &[], &[AREA_B]);
    let output = tick(&mut region, &changes(vec![join(E, player(8))]));
    assert_eq!(spawned(&output), [entity(4)]);
    assert_eq!(state_of(&region, player(8)).entity_id, entity(4));
    assert_eq!(region.state().next_entity_id, entity(5));
    assert_eq!(region.player_count(), 4);
}

/// What a region is given in the ticks after a plan that was not taken: its player
/// walks, digs and is let go, a player joins, and the store answers.
fn carrying_on() -> Vec<TickInputs> {
    let mut walking = single_input(E, player(1), entity(1), 2, move_to(12.5));
    walking.input(E, player(1), entity(1), 3, dig(BlockPos::new(12, 63, 8), 2));
    vec![
        walking,
        changes(vec![join(F, player(8))]),
        TickInputs {
            granted: vec![SOUTH],
            tickets_removed: vec![viewer(THIRD)],
            ..TickInputs::default()
        },
        delivered(&[SOUTH]),
        single_input(F, player(8), entity(4), 1, walk_to(IN_NORTH)),
        single_input(E, player(1), entity(1), 4, walk_to(IN_EAST)),
        remotely(F, remote(player(9), 3, break_at(BlockPos::new(3, 63, 3)))),
        TickInputs::default(),
    ]
}

#[test]
fn a_plan_that_is_not_taken_changes_nothing() {
    let mut region = before_a_merge(3);
    let mut copy = region.clone();

    // Both plans, more than once.
    let merged = region.absorb(REGION_B, &absorbed_state());
    assert_eq!(region.absorb(REGION_B, &absorbed_state()), merged);
    assert_eq!(region, copy, "`absorb` changed the region");
    let split = region.split(&[WEST, HOME, FAR], PART, &[]);
    assert_eq!(region.split(&[WEST, HOME, FAR], PART, &[]), split);
    assert_eq!(region, copy, "`split` changed the region");
    assert_eq!(region.state(), copy.state());

    // Its later ticks are those of a region that never planned.
    for (index, inputs) in carrying_on().iter().enumerate() {
        let output = tick(&mut region, inputs);
        assert_eq!(output, tick(&mut copy, inputs), "tick {index} after");
        assert_eq!(region, copy, "tick {index} after");
    }
    assert!(region.player(player(1)).is_none(), "player 1 was let go");
    assert_eq!(region.player_count(), 1);
}

#[test]
fn a_split_that_is_planned_with_players_to_go_and_not_taken_changes_nothing() {
    // The plan above is off, as the only player stands in the home chunk. This one is a
    // split: player 1 has walked into `WEST`.
    let mut region = before_a_merge(3);
    tick(
        &mut region,
        &single_input(E, player(1), entity(1), 2, walk_to(IN_WEST)),
    );
    let mut copy = region.clone();
    let splitting = region.split(&[WEST], PART, &[]).expect("player 1 goes");
    assert_eq!(splitting.part.players.len(), 1);
    assert_eq!(region, copy);
    let mut inputs = single_input(E, player(1), entity(1), 3, walk_to(IN_HOME));
    inputs.change(join(F, player(8)));
    for inputs in [inputs, TickInputs::default()] {
        assert_eq!(tick(&mut region, &inputs), tick(&mut copy, &inputs));
        assert_eq!(region, copy);
    }
}

// ---------------------------------------------------------------------------------------
// S27: taking a merge
// ---------------------------------------------------------------------------------------

/// Chunks that came with the merge: one of the absorbed region's area, and one it had
/// been granted outside it.
const NAMED: [ChunkPos; 2] = [ChunkPos::new(6, 2), ChunkPos::new(2, 0)];

/// [`before_a_merge`] after it has absorbed and taken region 1, which was pinned to
/// `AREA_B` and had been granted `NAMED`. Returns the region, the state it took and the
/// chunks it gave back.
fn merged(return_after: u64) -> (Region, RegionState, Vec<(ChunkPos, Chunk)>) {
    let mut region = before_a_merge(return_after);
    let state = region.absorb(REGION_B, &absorbed_state());
    let loaded = region.take_absorbed(state.clone(), &NAMED, &[AREA_B]);
    (region, state, loaded)
}

/// What the store says a region holds and is pinned to after the merge of [`merged`].
fn merged_holdings() -> Holdings {
    Holdings {
        held: vec![HOME, WEST, NORTH, FAR, NAMED[0], NAMED[1]],
        pinned: vec![WESTERN, AREA_B],
    }
}

#[test]
fn after_a_merge_the_region_is_what_a_restore_makes_of_the_state_and_what_it_holds() {
    for return_after in [0, 5] {
        let (region, state, _) = merged(return_after);
        // What it held, the chunk of its stripe that it had claimed among them; the
        // chunks named; and its areas with those named, its own first.
        let restored = Region::restore(config(return_after), state.clone(), merged_holdings());
        assert_eq!(region, restored);
        assert_eq!(region.state(), state);
        assert_eq!(region.tick_number(), state.tick);
        assert_eq!(region.player_count(), 3);
    }
}

#[test]
fn after_a_merge_the_region_knows_what_it_holds_and_the_new_areas_and_nothing_else() {
    let (region, _, _) = merged(0);
    for chunk in [HOME, WEST, NORTH, FAR, NAMED[0], NAMED[1]] {
        assert_eq!(region.knowledge(chunk), Knowledge::Held, "{chunk:?}");
    }
    assert_eq!(region.held_chunk_count(), 6);
    // Every chunk it believed any region to hold, the absorbed one or another, and what
    // it had asked for.
    for chunk in [EAST, THIRD, SOUTH, BEYOND, ChunkPos::new(1, 4)] {
        assert_eq!(region.knowledge(chunk), Knowledge::Unknown, "{chunk:?}");
    }
    // It learned the areas that came with the merge.
    for chunk in [EAST, THIRD, NAMED[1], ChunkPos::new(1, -40)] {
        assert!(region.pins(chunk), "{chunk:?}");
    }
    for chunk in [HOME, WEST, NORTH, SOUTH] {
        assert!(region.pins(chunk), "{chunk:?} is of its own stripe");
    }
    for chunk in [FAR, BEYOND, NAMED[0], ChunkPos::new(4, 0)] {
        assert!(!region.pins(chunk), "{chunk:?}");
    }
}

#[test]
fn taking_a_merge_returns_every_chunk_that_was_loaded_block_for_block() {
    let before = before_a_merge(0);
    let (region, _, loaded) = merged(0);
    let mut broken = stone_chunk();
    let (x, z) = OWN_BLOCK.in_chunk();
    broken.set(x, OWN_BLOCK.y, z, blocks::AIR);
    assert_eq!(
        loaded,
        vec![(WEST, stone_chunk()), (HOME, broken), (FAR, stone_chunk())],
        "in ascending order"
    );
    for (chunk, blocks) in &loaded {
        assert_eq!(before.chunk(*chunk), Some(blocks));
    }
    assert_eq!(region.loaded_chunk_count(), 0);
    for chunk in [HOME, WEST, FAR] {
        assert!(region.chunk(chunk).is_none());
    }
}

#[test]
fn after_a_merge_the_region_has_no_ticket() {
    // A ticket shows in what a tick asks: a held chunk with one is asked of storage, an
    // unknown one with a viewer's is claimed. Before the merge `HOME`, `WEST` and `FAR`
    // had tickets, and so had `EAST`, `THIRD` and `SOUTH`.
    let (mut region, _, _) = merged(0);
    let output = idle(&mut region);
    assert!(
        output.chunk_requests.is_empty(),
        "{:?}",
        output.chunk_requests
    );
    // The players that came in stand in chunks of the new area, which are claimed for
    // them, and no other chunk is.
    assert_eq!(output.claims, [EAST, ChunkPos::new(2, 1)]);
    assert!(silent(&output));

    // Releasing the tickets it had releases nothing, and one new ticket is one ticket.
    let had = [viewer(HOME), viewer(WEST), guest(FAR), viewer(THIRD)];
    let output = tick(&mut region, &remove(&had));
    assert!(output.chunk_requests.is_empty() && output.claims.is_empty());
    let output = tick(&mut region, &add(&[viewer(HOME), viewer(THIRD)]));
    assert_eq!(output.chunk_requests, [HOME]);
    assert_eq!(output.claims, [THIRD]);
    let output = tick(&mut region, &remove(&[viewer(HOME)]));
    assert!(output.chunk_requests.is_empty());
    let output = tick(&mut region, &delivered(&[HOME]));
    assert!(region.chunk(HOME).is_none(), "its one ticket had gone");
    assert!(output.chunk_requests.is_empty());
}

#[test]
fn after_a_merge_a_guests_ticket_claims_a_chunk_of_a_newly_named_area_and_of_no_other() {
    let (mut region, _, _) = merged(0);
    idle(&mut region);
    let inside = ChunkPos::new(2, 3);
    let output = tick(
        &mut region,
        &add(&[guest(inside), guest(BEYOND), guest(ChunkPos::new(4, 0))]),
    );
    assert_eq!(output.claims, [inside]);
    assert_eq!(region.knowledge(inside), Knowledge::Asked);
    assert_eq!(region.knowledge(BEYOND), Knowledge::Unknown);
    // The store grants it, as the area is the region's now, and it is never given back.
    let output = tick(&mut region, &granted(&[inside]));
    assert_eq!(output.chunk_requests, [inside]);
    tick(&mut region, &remove(&[guest(inside)]));
    for _ in 0..3 {
        assert!(idle(&mut region).returns.is_empty());
    }
    assert_eq!(region.knowledge(inside), Knowledge::Held);
}

#[test]
fn after_a_merge_no_chunk_is_given_back_for_the_time_before_a_return() {
    // Each chunk the region holds counts as used up to the tick of the merge. `FAR` had
    // a ticket until then, and the chunks named were not the region's.
    let return_after = 5;
    let (mut region, state, _) = merged(return_after);
    for tick_after in 1..=return_after {
        let output = idle(&mut region);
        assert!(
            output.returns.is_empty(),
            "tick {tick_after} after the merge: {:?}",
            output.returns
        );
    }
    // Then what is neither of an area nor the home chunk goes: nothing used it at the
    // end of the first tick after the merge or of the five since.
    let output = idle(&mut region);
    assert_eq!(output.tick, state.tick + return_after + 1);
    assert_eq!(output.returns, [FAR, NAMED[0]]);
    assert_eq!(region.knowledge(FAR), Knowledge::Unknown);
    assert_eq!(region.knowledge(NAMED[1]), Knowledge::Held);
    assert_eq!(region.held_chunk_count(), 4);
}

/// What a merged region is given in its first ticks: the hellos of its edges, the
/// store's answers, and what its players do, those who came in among them.
fn after_the_merge() -> Vec<TickInputs> {
    let around = [
        viewer(HOME),
        viewer(WEST),
        viewer(EAST),
        guest(NAMED[1]),
        viewer(THIRD),
        guest(BEYOND),
        guest(FAR),
    ];
    let mut hello = add(&around);
    hello.edges = vec![started(E, 10), started(F, 10)];
    hello
        .edges
        .push(EdgeEvent::Confirmed { edge: E, number: 2 });
    let mut answers = granted(&[EAST, ChunkPos::new(2, 1)]);
    answers.foreign = vec![(THIRD, OTHER)];
    answers.chunks_loaded = delivered(&[HOME, WEST, NAMED[1], FAR]).chunks_loaded;
    let coming_in = someone(entity(2), E, EAST);
    let mut acting = single_input(
        E,
        player(2),
        entity(2),
        coming_in.last_input + 1,
        dig(BlockPos::new(24, 63, 9), 40),
    );
    acting.input(E, player(1), entity(1), 2, move_to(15.5));
    acting.input(E, player(1), entity(1), 3, dig(BlockPos::new(16, 63, 8), 2));
    acting.change(join(F, player(8)));
    let mut leaving = changes(vec![leave(F, player(3), Some(entity(3)))]);
    leaving.input(E, player(1), entity(1), 4, walk_to(IN_EAST));
    leaving
        .remote_actions
        .push((F, remote(player(9), 3, break_at(BlockPos::new(50, 63, 8)))));
    let mut inputs = vec![hello, answers, delivered(&[EAST]), acting, leaving];
    inputs.push(single_input(E, player(1), entity(1), 5, move_to(50.5)));
    inputs.push(remove(&[guest(FAR)]));
    inputs.extend(std::iter::repeat_n(TickInputs::default(), 8));
    inputs
}

#[test]
fn ticking_on_from_a_merge_is_ticking_on_from_the_restore_input_for_input() {
    for return_after in [0, 4] {
        let (mut region, state, _) = merged(return_after);
        let mut restored = Region::restore(config(return_after), state, merged_holdings());
        let mut entries = 0;
        let mut events = 0;
        for (index, inputs) in after_the_merge().iter().enumerate() {
            let output = tick(&mut region, inputs);
            assert_eq!(output, tick(&mut restored, inputs), "tick {index} after");
            assert_eq!(region, restored, "tick {index} after");
            entries += output.durable.len();
            events += output.events.len();
        }
        // The ticks were about something: the player who came in dug, player 1 walked
        // into the new area and on, out of it, and what was no longer used went back.
        assert_eq!(block(&region, BlockPos::new(24, 63, 9)), Some(blocks::AIR));
        assert_eq!(block(&region, BlockPos::new(16, 63, 8)), Some(blocks::AIR));
        assert!(
            entries >= 1 && events >= 5,
            "{entries} entries, {events} events"
        );
        assert_eq!(region.knowledge(FAR), Knowledge::Unknown);
    }
}

#[test]
fn the_order_in_which_a_merge_names_its_chunks_changes_nothing() {
    let (region, state, loaded) = merged(0);
    let mut turned = before_a_merge(0);
    let again = turned.absorb(REGION_B, &absorbed_state());
    assert_eq!(bytes(&again), bytes(&state));
    let chunks = [NAMED[1], NAMED[0], NAMED[1]];
    assert_eq!(turned.take_absorbed(again, &chunks, &[AREA_B]), loaded);
    assert_eq!(turned, region);

    // Nor does the order in which the store names what a restored region holds.
    let mut holdings = merged_holdings();
    holdings.held.reverse();
    assert_eq!(Region::restore(config(0), state, holdings), region);
}

#[test]
fn a_merge_that_names_a_chunk_the_region_holds_already_holds_it_once() {
    // The runner names what the store had granted the region in answer to claims no
    // tick has been told of; a grant of a chunk that is held changes nothing.
    let mut region = before_a_merge(0);
    let state = region.absorb(REGION_B, &absorbed_state());
    region.take_absorbed(state.clone(), &[FAR, NAMED[0], NAMED[1], HOME], &[AREA_B]);
    assert_eq!(region.held_chunk_count(), 6);
    assert_eq!(region, Region::restore(config(0), state, merged_holdings()));
}

#[test]
fn the_chunks_a_merge_names_are_held_whatever_the_region_knew_of_them() {
    // One it believed the absorbed region to hold, one it believed a third to hold, and
    // one it had asked for and was granted before any tick was told.
    let mut region = before_a_merge(0);
    let state = region.absorb(REGION_B, &absorbed_state());
    let named = [EAST, THIRD, SOUTH];
    region.take_absorbed(state.clone(), &named, &[]);
    for chunk in named {
        assert_eq!(region.knowledge(chunk), Knowledge::Held, "{chunk:?}");
    }
    assert!(!region.pins(EAST), "no area came with this merge");
    let holdings = Holdings {
        held: vec![HOME, WEST, NORTH, FAR, EAST, THIRD, SOUTH],
        pinned: vec![WESTERN],
    };
    assert_eq!(region, Region::restore(config(0), state, holdings));
    // Nothing is claimed for the player who came in and stands in `EAST`.
    let output = idle(&mut region);
    assert_eq!(output.claims, [ChunkPos::new(2, 1)]);
}

#[test]
fn a_region_that_holds_nothing_and_is_pinned_to_nothing_can_absorb() {
    // The least there is to merge: no chunk, no area, no edge, no player.
    let mut region = Region::new(config(0), ids(), Holdings::default());
    let theirs = RegionState::new(other_ids());
    let state = region.absorb(REGION_B, &theirs);
    let mut expected = RegionState::new(ids());
    expected.tick = 1;
    assert_eq!(state, expected);
    assert!(region.take_absorbed(state.clone(), &[], &[]).is_empty());
    assert_eq!(
        region,
        Region::restore(config(0), state, Holdings::default())
    );
    assert_eq!(idle(&mut region).tick, 2);
}

// ---------------------------------------------------------------------------------------
// The split as the record says it
// ---------------------------------------------------------------------------------------

/// How far two chunks are apart: in chunks along the longer of the two axes, worked
/// out in 64 bits.
fn apart(a: ChunkPos, b: ChunkPos) -> i64 {
    let along_x = (i64::from(a.x) - i64::from(b.x)).abs();
    let along_z = (i64::from(a.z) - i64::from(b.z)).abs();
    along_x.max(along_z)
}

/// Section 2.4 of the record, step for step: what `split` gives for a region with
/// `state` that holds `held`, is pinned to some area or to none, and has its spawn
/// point in `HOME`.
fn split_as_the_record_says(
    state: &RegionState,
    held: &BTreeSet<ChunkPos>,
    pinned: bool,
    named: &[ChunkPos],
    part: RegionId,
) -> Result<Splitting, NoSplit> {
    let stands_in = |player: &PlayerState| chunk_of(player.pose.position);

    // 1. Seeds.
    let seeds: BTreeSet<ChunkPos> = named
        .iter()
        .copied()
        .filter(|chunk| held.contains(chunk) && *chunk != HOME)
        .filter(|chunk| {
            state
                .players
                .values()
                .any(|player| stands_in(player) == *chunk)
        })
        .collect();
    if seeds.is_empty() {
        return Err(NoSplit::Nobody);
    }

    // 2. Who goes.
    let goes = |player: &PlayerState| seeds.contains(&stands_in(player));
    let nobody_stays = state.players.values().all(goes);
    if nobody_stays && !held.contains(&HOME) && !pinned {
        return Err(NoSplit::NothingStays);
    }

    // 3. Where the stayers are.
    let mut stayers: BTreeSet<ChunkPos> = state
        .players
        .values()
        .filter(|player| !goes(player))
        .map(stands_in)
        .collect();
    if held.contains(&HOME) {
        stayers.insert(HOME);
    }

    // 4. The chunks of the part.
    let nearest = |chunk: ChunkPos, to: &BTreeSet<ChunkPos>| {
        to.iter().map(|other| apart(chunk, *other)).min()
    };
    let chunks: Vec<ChunkPos> = held
        .iter()
        .copied()
        .filter(|chunk| {
            let to_seed = nearest(*chunk, &seeds).expect("there is a seed");
            nearest(*chunk, &stayers).is_none_or(|to_stayer| to_seed < to_stayer)
        })
        .collect();

    // 5 and 6. The two states.
    let tick = state.tick + 1;
    let mut ours = state.clone();
    ours.tick = tick;
    let mut theirs = RegionState::new(no_ids());
    theirs.tick = tick;
    let mut went: BTreeMap<EdgeId, Vec<(PlayerId, EntityId, Option<u64>)>> = BTreeMap::new();
    for (id, player) in &state.players {
        if goes(player) {
            ours.players.remove(id);
            // Whole, but for one hand-over more: the split is one to each of them
            // (ADR-0020, section 9).
            let handed_on = PlayerState {
                hops: player.hops + 1,
                ..player.clone()
            };
            theirs.players.insert(*id, handed_on);
            went.entry(player.edge)
                .or_default()
                .push((*id, player.entity_id, player.attempt));
        }
    }
    for (edge, players) in went {
        let own = ours.edges.get_mut(&edge).expect("a player's edge is known");
        own.sent += 1;
        own.outbox.insert(
            own.sent,
            Durable::SplitOff {
                region: part,
                players,
            },
        );
        theirs
            .edges
            .insert(edge, edge_state(own.start, tick, 0, 0, &[]));
    }
    Ok(Splitting {
        state: ours,
        part: theirs,
        chunks,
        sides: Sides {
            seeds: seeds.iter().copied().collect(),
            staying: stayers.iter().copied().collect(),
        },
    })
}

/// What the store says of a region on open land that holds `held`.
fn on_open_land(held: &[ChunkPos]) -> Holdings {
    Holdings {
        held: held.to_vec(),
        pinned: Vec::new(),
    }
}

/// What the store says of a region that is pinned to the whole world and has claimed
/// `held` of it.
fn pinned_everywhere(held: &[ChunkPos]) -> Holdings {
    Holdings {
        held: held.to_vec(),
        pinned: vec![ChunkArea::EVERYWHERE],
    }
}

/// The edges of [`with_players`], as the region knows them before the split.
fn edges_before_a_split() -> [(EdgeId, EdgeState); 2] {
    [
        (E, edge_state(10, 3, 17, 4, &[(4, refused(9))])),
        (F, edge_state(11, 5, 6, 0, &[])),
    ]
}

/// A region as a restore makes it at tick 40 of a state with a player in the middle of
/// each chunk named: player `n` has `entity(n)`. Its home chunk is `HOME`, whether it
/// holds it or not.
fn with_players(holdings: Holdings, players: &[(i32, EdgeId, ChunkPos)]) -> Region {
    let players: Vec<(PlayerId, PlayerState)> = players
        .iter()
        .map(|(n, edge, chunk)| (player(*n as u128), someone(entity(*n), *edge, *chunk)))
        .collect();
    let state = a_state(40, ids(), entity(20), &players, &edges_before_a_split());
    Region::restore(config(0), state, holdings)
}

/// The players of `state`, by the number [`with_players`] was given for each.
fn numbers_of(state: &RegionState) -> Vec<i32> {
    state
        .players
        .values()
        .map(|player| player.entity_id.0 - ids().first.0 + 1)
        .collect()
}

/// The chunks of the row z = 0 from x = `from` to x = `to`.
fn row(from: i32, to: i32) -> Vec<ChunkPos> {
    (from..=to).map(|x| ChunkPos::new(x, 0)).collect()
}

const TEN: ChunkPos = ChunkPos::new(10, 0);

// ---------------------------------------------------------------------------------------
// S30: when a split is off
// ---------------------------------------------------------------------------------------

#[test]
fn a_split_is_off_when_no_named_chunk_has_a_player_the_region_could_send() {
    for holdings in [on_open_land, pinned_everywhere] {
        let held = [HOME, EAST, TEN];
        let asked = ChunkPos::new(4, 4);
        let players = [(1, E, HOME), (2, E, EAST), (3, F, asked)];
        let region = with_players(holdings(&held), &players);
        let off: [&[ChunkPos]; 7] = [
            // Nothing is named.
            &[],
            // A chunk that is held and empty, and one nobody has anything to do with.
            &[TEN],
            &[TEN, ChunkPos::new(-30, 2)],
            // The only named chunk with a player is not held,
            &[asked],
            // or is the home chunk,
            &[HOME],
            // or both, with an empty one beside them.
            &[HOME, asked, TEN],
            &[asked, asked, HOME, HOME],
        ];
        for named in off {
            assert_eq!(
                region.split(named, PART, &[]),
                Err(NoSplit::Nobody),
                "{named:?}"
            );
        }
        assert!(region.split(&[EAST], PART, &[]).is_ok());
    }
}

#[test]
fn a_split_is_off_when_nobody_would_stay_in_a_region_without_home_chunk_or_area() {
    let held = [EAST, TEN];
    let region = with_players(
        on_open_land(&held),
        &[(1, E, EAST), (2, F, TEN), (3, F, TEN)],
    );
    assert_eq!(
        region.split(&[EAST, TEN], PART, &[]),
        Err(NoSplit::NothingStays)
    );
    assert_eq!(
        region.split(&[TEN, EAST, HOME, WEST, TEN], PART, &[]),
        Err(NoSplit::NothingStays)
    );
    // With one who stays it is a split.
    for named in [[EAST], [TEN]] {
        let splitting = region.split(&named, PART, &[]).expect("somebody stays");
        assert_eq!(splitting.chunks, named);
    }

    // That nobody is in a named chunk is found first: a region with nobody at all is
    // not split for that reason, whatever it would be left with.
    let empty = with_players(on_open_land(&held), &[]);
    assert_eq!(empty.split(&[EAST, TEN], PART, &[]), Err(NoSplit::Nobody));
    let elsewhere = with_players(on_open_land(&[]), &[(1, E, EAST)]);
    assert_eq!(elsewhere.split(&[EAST], PART, &[]), Err(NoSplit::Nobody));
}

#[test]
fn it_is_a_split_when_nobody_stays_in_a_region_that_is_pinned_or_holds_the_home_chunk() {
    let players = [(1, E, EAST), (2, F, TEN)];

    // Pinned, without the home chunk: everything it holds goes, and it stays pinned.
    let held = [EAST, TEN, ChunkPos::new(-7, 3)];
    let region = with_players(pinned_everywhere(&held), &players);
    let splitting = region.split(&[EAST, TEN], PART, &[]).expect("it is pinned");
    assert!(splitting.state.players.is_empty());
    assert_eq!(numbers_of(&splitting.part), [1, 2]);
    assert_eq!(splitting.chunks, [ChunkPos::new(-7, 3), EAST, TEN]);

    // Pinned to an area that has none of its chunks in it.
    let mut holdings = on_open_land(&held);
    holdings.pinned = vec![ChunkArea {
        min_x: Some(500),
        max_x: Some(501),
    }];
    let region = with_players(holdings, &players);
    assert!(region.split(&[EAST, TEN], PART, &[]).is_ok());

    // On open land with the home chunk, which stays, and what is nearer to it.
    let held = [HOME, EAST, TEN, ChunkPos::new(-7, 3)];
    let region = with_players(on_open_land(&held), &players);
    let splitting = region
        .split(&[EAST, TEN], PART, &[])
        .expect("it holds home");
    assert!(splitting.state.players.is_empty());
    assert_eq!(splitting.chunks, [EAST, TEN]);
}

// ---------------------------------------------------------------------------------------
// S31: who goes
// ---------------------------------------------------------------------------------------

#[test]
fn exactly_the_players_standing_in_named_chunks_that_are_held_and_not_home_go() {
    for holdings in [on_open_land, pinned_everywhere] {
        let asked = ChunkPos::new(4, 4);
        let beside = ChunkPos::new(11, 0);
        let held = [HOME, EAST, TEN, beside, ChunkPos::new(12, 0)];
        let players = [
            // Two in a named chunk, through different edges.
            (1, E, TEN),
            (2, F, TEN),
            // In a held chunk beside it that is not named.
            (3, E, beside),
            // In the home chunk, named.
            (4, F, HOME),
            // In a named chunk that is not held.
            (5, E, asked),
            // In a held chunk far from all of it.
            (6, E, EAST),
        ];
        let region = with_players(holdings(&held), &players);
        let named = [asked, TEN, HOME, ChunkPos::new(12, 0)];
        let splitting = region.split(&named, PART, &[]).expect("two players go");
        assert_eq!(numbers_of(&splitting.part), [1, 2]);
        assert_eq!(numbers_of(&splitting.state), [3, 4, 5, 6]);
        // Whoever stays keeps the chunk they stand in, and what is nearer to it: the
        // empty chunk named beyond player 3 is nearer to them than to those who go.
        assert_eq!(splitting.chunks, [TEN]);

        // Named as well, player 3 goes, and the chunks beyond with them.
        let named = [beside, TEN];
        let splitting = region.split(&named, PART, &[]).expect("three players go");
        assert_eq!(numbers_of(&splitting.part), [1, 2, 3]);
        assert_eq!(splitting.chunks, [TEN, beside, ChunkPos::new(12, 0)]);
    }
}

// ---------------------------------------------------------------------------------------
// S32: which chunks go
// ---------------------------------------------------------------------------------------

/// The chunks a region holds in the scenario: the row z = 0 from x = -2 to 13, the
/// chunk the scenario names off the row, and others off it either side of the middle.
fn held_for_the_row() -> Vec<ChunkPos> {
    let mut held = row(-2, 13);
    held.extend([
        ChunkPos::new(6, 7),
        ChunkPos::new(7, 7),
        ChunkPos::new(8, 7),
        ChunkPos::new(6, -3),
        ChunkPos::new(4, 20),
        ChunkPos::new(5, 3),
        ChunkPos::new(30, 12),
        ChunkPos::new(-30, 40),
    ]);
    held
}

/// Which of [`held_for_the_row`] go with a player at (10, 0) while the chunk (0, 0)
/// stays: nearer to the one than to the other along the longer axis, ties staying.
fn going_from_the_row() -> Vec<ChunkPos> {
    let mut going = vec![
        // 4 from the one and 6 from the other.
        ChunkPos::new(6, -3),
    ];
    going.extend(row(6, 13));
    // 7 from the one and 8 from the other; and 20 against 30. The chunk 40 along z
    // from both stays.
    going.push(ChunkPos::new(8, 7));
    going.push(ChunkPos::new(30, 12));
    going.sort();
    going
}

#[test]
fn the_chunks_nearer_to_a_player_who_goes_than_to_one_who_stays_go_and_ties_stay() {
    for holdings in [on_open_land, pinned_everywhere] {
        let held = held_for_the_row();
        let region = with_players(holdings(&held), &[(1, E, TEN), (2, F, HOME)]);
        let splitting = region.split(&[TEN], PART, &[]).expect("player 1 goes");
        assert_eq!(splitting.chunks, going_from_the_row());
        let went: BTreeSet<ChunkPos> = splitting.chunks.iter().copied().collect();
        // The row: x above 5 goes and x = 5 stays.
        for x in -2..=13 {
            assert_eq!(went.contains(&ChunkPos::new(x, 0)), x > 5, "x = {x}");
        }
        // As far from the one as from the other: 7 along z from both.
        assert!(!went.contains(&ChunkPos::new(6, 7)));
        assert!(!went.contains(&ChunkPos::new(7, 7)));
        assert!(!went.contains(&ChunkPos::new(4, 20)));
    }
}

#[test]
fn the_home_chunk_counts_as_a_place_where_somebody_stays() {
    // The same chunks go when nobody stays and the region holds the home chunk at
    // (0, 0), whether anyone stands there or not.
    for holdings in [on_open_land, pinned_everywhere] {
        let held = held_for_the_row();
        let region = with_players(holdings(&held), &[(1, E, TEN)]);
        let splitting = region
            .split(&[TEN], PART, &[])
            .expect("the home chunk stays");
        assert_eq!(splitting.chunks, going_from_the_row());
        assert!(!splitting.chunks.contains(&HOME));
    }
}

#[test]
fn with_nobody_staying_and_no_home_chunk_every_chunk_a_pinned_region_holds_goes() {
    let mut held = held_for_the_row();
    held.retain(|chunk| *chunk != HOME);
    let region = with_players(pinned_everywhere(&held), &[(1, E, TEN)]);
    let splitting = region.split(&[TEN], PART, &[]).expect("it is pinned");
    held.sort();
    assert_eq!(splitting.chunks, held);

    // A home chunk it is pinned to and has not claimed is not a place where anyone
    // stays: only one it holds is (step 3).
    assert_eq!(region.knowledge(HOME), Knowledge::Unknown);
    assert!(region.pins(HOME));
}

#[test]
fn a_chunk_goes_with_the_nearest_of_several_who_go_unless_one_who_stays_is_as_near() {
    let held = row(0, 20);
    let players = [
        (1, E, ChunkPos::new(4, 0)),
        (2, E, ChunkPos::new(8, 0)),
        (3, F, ChunkPos::new(14, 0)),
        (4, F, ChunkPos::new(20, 0)),
    ];
    let region = with_players(on_open_land(&held), &players);
    // Players 1 and 3 go. The home chunk at 0 and players 2 and 4 stay, at 8 and 20.
    let named = [ChunkPos::new(14, 0), ChunkPos::new(4, 0)];
    let splitting = region.split(&named, PART, &[]).expect("two go");
    let going: Vec<ChunkPos> = [3, 4, 5, 12, 13, 14, 15, 16]
        .into_iter()
        .map(|x| ChunkPos::new(x, 0))
        .collect();
    assert_eq!(splitting.chunks, going);
}

#[test]
fn a_named_chunk_without_a_player_and_one_that_is_not_held_are_no_seeds() {
    let held = row(0, 12);
    let outside = ChunkPos::new(40, 0);
    let players = [(1, E, TEN), (2, F, outside)];
    let region = with_players(on_open_land(&held), &players);
    let plain = region.split(&[TEN], PART, &[]).expect("player 1 goes");
    assert_eq!(plain.chunks, row(6, 12));

    // An empty chunk next to the home chunk, named, does not go and takes nothing with
    // it; nor does the chunk player 2 stands in, which the region does not hold. Player
    // 2 stays, far to the east, and what they are nearer to stays with them: nothing,
    // as every chunk held is nearer to the home chunk or to player 1.
    let named = [ChunkPos::new(2, 0), TEN, outside];
    assert_eq!(
        region.split(&named, PART, &[]).expect("player 1 goes"),
        plain
    );

    // Named twice, and in another order.
    let named = [outside, TEN, TEN, ChunkPos::new(2, 0), TEN];
    assert_eq!(
        region.split(&named, PART, &[]).expect("player 1 goes"),
        plain
    );
}

#[test]
fn how_far_chunks_are_apart_is_worked_out_in_64_bits() {
    // A player can stand no further out than a block position reaches, but a chunk a
    // region holds can lie anywhere a chunk position does: at the western end of them
    // it is more than 32 bits hold from the one who goes, at (10, 0), and from the one
    // who stays, at (20, 0), and nearer to the first by ten.
    let stayer = ChunkPos::new(20, 0);
    let held = [
        TEN,
        stayer,
        ChunkPos::new(i32::MIN, 0),
        // At the eastern end the one who stays is the nearer.
        ChunkPos::new(i32::MAX, 0),
        // As far along z from both as z reaches: ties.
        ChunkPos::new(15, i32::MIN),
        ChunkPos::new(0, i32::MAX),
        // Further along x than along z, and nearer to the one who goes.
        ChunkPos::new(i32::MIN, i32::MAX),
    ];
    let region = with_players(on_open_land(&held), &[(1, E, TEN), (2, F, stayer)]);
    let splitting = region.split(&[TEN], PART, &[]).expect("player 1 goes");
    assert_eq!(
        splitting.chunks,
        [
            ChunkPos::new(i32::MIN, 0),
            ChunkPos::new(i32::MIN, i32::MAX),
            TEN
        ]
    );
}

// ---------------------------------------------------------------------------------------
// S33: the two states of a split
// ---------------------------------------------------------------------------------------

#[test]
fn the_part_has_the_players_who_go_whole_the_empty_block_and_their_edges_known_since_the_split() {
    let held = held_for_the_row();
    let players = [(1, E, TEN), (2, E, HOME), (3, E, TEN)];
    let region = with_players(on_open_land(&held), &players);
    let before = region.state();
    let splitting = region.split(&[TEN], PART, &[]).expect("two go");

    let part = &splitting.part;
    assert_eq!(part.tick, 41, "the tick after the region's last");
    assert_eq!(part.entity_ids, no_ids());
    assert_eq!(part.next_entity_id, EntityId(0));
    let went: BTreeMap<PlayerId, PlayerState> = [player(1), player(3)]
        .into_iter()
        .map(|id| (id, before.players[&id].clone()))
        .collect();
    // As they were, and handed on once more (ADR-0020, section 9).
    let went: BTreeMap<PlayerId, PlayerState> = went
        .into_iter()
        .map(|(id, stay)| {
            let hops = stay.hops + 1;
            (id, PlayerState { hops, ..stay })
        })
        .collect();
    assert_eq!(part.players, went, "as they were");
    // The one edge that has a player who goes, with the split region's start for it,
    // nothing applied and nothing sent. No other edge is known to the part.
    assert_eq!(part.edges, [(E, edge_state(10, 41, 0, 0, &[]))].into());
}

#[test]
fn the_split_region_loses_those_who_go_and_tells_each_of_their_edges_once() {
    let held = held_for_the_row();
    // Players in an order that their entities do not follow.
    let ordered = [
        (player(1), someone(entity(7), E, TEN)),
        (player(2), someone(entity(5), E, HOME)),
        (player(3), someone(entity(3), E, TEN)),
        (player(4), someone(entity(6), F, TEN)),
        (player(5), someone(entity(1), E, TEN)),
    ];
    let mut edges = edges_before_a_split().to_vec();
    let untouched = edge_state(10, 9, 2, 6, &[(6, done(4, 4))]);
    edges.push((EdgeId(3), untouched.clone()));
    let before = a_state(40, ids(), entity(20), &ordered, &edges);
    let region = Region::restore(config(0), before.clone(), on_open_land(&held));
    let splitting = region.split(&[TEN], PART, &[]).expect("four go");

    let state = &splitting.state;
    assert_eq!(state.tick, 41);
    assert_eq!(state.entity_ids, ids());
    assert_eq!(state.next_entity_id, entity(20));
    assert_eq!(
        state.players,
        [(player(2), before.players[&player(2)].clone())].into()
    );
    // One entry for each edge that has a player who goes, under the next number, with
    // those of that edge and their entities, in ascending order of the players.
    let to_e = Durable::SplitOff {
        region: PART,
        players: vec![
            (player(1), entity(7), None),
            (player(3), entity(3), None),
            (player(5), entity(1), None),
        ],
    };
    let to_f = Durable::SplitOff {
        region: PART,
        players: vec![(player(4), entity(6), None)],
    };
    let expected: BTreeMap<EdgeId, EdgeState> = [
        (E, edge_state(10, 3, 17, 5, &[(4, refused(9)), (5, to_e)])),
        (F, edge_state(11, 5, 6, 1, &[(1, to_f)])),
        (EdgeId(3), untouched),
    ]
    .into();
    assert_eq!(state.edges, expected);

    // The part knows the two edges with the starts the split region has for them.
    let expected: BTreeMap<EdgeId, EdgeState> = [
        (E, edge_state(10, 41, 0, 0, &[])),
        (F, edge_state(11, 41, 0, 0, &[])),
    ]
    .into();
    assert_eq!(splitting.part.edges, expected);
    assert_eq!(splitting.part.players.len(), 4);
}

#[test]
fn splitting_made_up_regions_gives_what_the_record_says_step_for_step() {
    let mut random = Random(0xD1B5_4A32_D192_ED03);
    let mut outcomes = [0usize; 3];
    let mut went = 0;
    let mut in_part = 0;
    for round in 0..3000 {
        // Chunks on a small board, so that players share chunks and distances tie.
        let mut board = Vec::new();
        for x in -3..=6 {
            for z in -2..=4 {
                board.push(ChunkPos::new(x, z));
            }
        }
        let density = 2 + random.below(4);
        let mut held: BTreeSet<ChunkPos> = board
            .iter()
            .copied()
            .filter(|_| !random.once_in(density))
            .collect();
        if random.once_in(3) {
            held.remove(&HOME);
        } else {
            held.insert(HOME);
        }
        if random.once_in(8) {
            held.retain(|_| random.once_in(6));
        }
        let pinned = random.once_in(2);

        let mut state = RegionState::new(ids());
        state.tick = 1 + random.below(90);
        state.next_entity_id = entity(30);
        state.edges = edges_before_a_split().into();
        let crowd = random.below(7);
        for n in 1..=crowd {
            let edge = random.pick(&[E, E, F]);
            let chunk = random.pick(&board);
            let stay = someone(entity(random.below(25) as i32 + 1), edge, chunk);
            state.players.insert(player(u128::from(n)), stay);
        }
        let mut named = Vec::new();
        for _ in 0..random.below(5) {
            let chunk = match state.players.values().next() {
                Some(_) if !random.once_in(3) => {
                    let stays: Vec<&PlayerState> = state.players.values().collect();
                    chunk_of(random.pick(&stays).pose.position)
                }
                _ => random.pick(&board),
            };
            named.push(chunk);
        }

        let holdings = Holdings {
            held: held.iter().copied().collect(),
            pinned: if pinned {
                vec![ChunkArea::EVERYWHERE]
            } else {
                Vec::new()
            },
        };
        let region = Region::restore(config(0), state.clone(), holdings);
        let split = region.split(&named, PART, &[]);
        let expected = split_as_the_record_says(&state, &held, pinned, &named, PART);
        assert_eq!(
            split, expected,
            "round {round}: {named:?} of a region that holds {held:?}, pinned: {pinned}, \
             with\n{state:#?}"
        );
        assert_eq!(
            region.state(),
            state,
            "round {round}: `split` changes nothing"
        );

        match &split {
            Ok(splitting) => {
                outcomes[0] += 1;
                went += splitting.chunks.len();
                assert!(splitting.chunks.is_sorted());
                assert!(!splitting.chunks.contains(&HOME));
                in_part += usize::from(splitting.chunks.len() < held.len());
            }
            Err(NoSplit::Nobody) => outcomes[1] += 1,
            Err(NoSplit::NothingStays) => outcomes[2] += 1,
        }
    }
    assert!(outcomes[0] >= 1000 && outcomes[1] >= 300, "{outcomes:?}");
    assert!(outcomes[2] >= 20, "{outcomes:?}");
    assert!(
        went >= 5000 && in_part >= 500,
        "{went} chunks went, {in_part}"
    );
}

// ---------------------------------------------------------------------------------------
// S34: taking a split
// ---------------------------------------------------------------------------------------

/// Chunks of [`before_a_split`]: two east of `EAST`, one far out that goes with the
/// part, and one the region has asked for.
const SECOND: ChunkPos = ChunkPos::new(2, 0);
const OUTPOST: ChunkPos = ChunkPos::new(9, 2);
const WANTED: ChunkPos = ChunkPos::new(7, 7);

/// A block of `THIRD` that player 2 of [`before_a_split`] has broken.
const THIRD_BLOCK: BlockPos = BlockPos::new(56, 63, 9);

/// The time before a return of the regions that are split here: long, so that a chunk
/// nobody watches is still held when the split comes.
const LONG: u64 = 1000;

/// A region before a split, pinned to the whole world or to nothing. It holds `HOME`,
/// `EAST`, `SECOND` and `THIRD`, loaded for a viewer each, and `OUTPOST`, which is not
/// loaded, and has asked for `WANTED`. It knows `E` and `F`. Player 1 stands at the
/// spawn point and has broken a block there; player 2 stands in `THIRD` and has broken
/// one there; player 3, the only one of `F`, stands in `SECOND`.
fn before_a_split(pinned: bool) -> Region {
    let held = [HOME, EAST, SECOND, THIRD, OUTPOST];
    let holdings = if pinned {
        pinned_everywhere(&held)
    } else {
        on_open_land(&held)
    };
    let mut region = Region::new(config(LONG), ids(), holdings);
    let loaded = [HOME, EAST, SECOND, THIRD];
    let mut tickets: Vec<(ChunkPos, Ticket)> = loaded.iter().map(|chunk| viewer(*chunk)).collect();
    tickets.push(viewer(WANTED));
    let output = tick(&mut region, &add(&tickets));
    assert_eq!(output.chunk_requests, loaded);
    assert_eq!(output.claims, [WANTED]);
    tick(&mut region, &delivered(&loaded));
    tick(&mut region, &edges(vec![started(E, 10), started(F, 10)]));
    tick(
        &mut region,
        &changes(vec![
            join(E, player(1)),
            join(E, player(2)),
            join(F, player(3)),
        ]),
    );
    let mut inputs = single_input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 1));
    inputs.input(E, player(2), entity(2), 1, walk_to(middle(THIRD)));
    inputs.input(E, player(2), entity(2), 2, dig(THIRD_BLOCK, 1));
    inputs.input(F, player(3), entity(3), 1, walk_to(middle(SECOND)));
    let output = tick(&mut region, &inputs);
    assert_eq!(
        block_changes(&output),
        [(OWN_BLOCK, blocks::AIR), (THIRD_BLOCK, blocks::AIR)]
    );
    assert!(output.durable.is_empty() && output.claims.is_empty());
    assert_eq!(region.knowledge(WANTED), Knowledge::Asked);
    assert_eq!(region.held_chunk_count(), 5);
    assert_eq!(region.loaded_chunk_count(), 4);
    region
}

/// [`before_a_split`] after the players in `THIRD` were split off and the split taken,
/// with `WANTED` granted meanwhile, which is as near to those who stay as to the one
/// who goes and so stays. Returns the region, the plan, the chunks that stay loaded and
/// the part.
fn taken(pinned: bool) -> (Region, Splitting, Vec<(ChunkPos, Chunk)>, Part) {
    let mut region = before_a_split(pinned);
    let splitting = region
        .split(&[THIRD], PART, &[WANTED])
        .expect("player 2 goes");
    let (kept, part) = region.take_split(splitting.clone(), &[WANTED]);
    (region, splitting, kept, part)
}

/// A column of stone with one block broken.
fn stone_without(position: BlockPos) -> Chunk {
    let mut chunk = stone_chunk();
    let (x, z) = position.in_chunk();
    chunk.set(x, position.y, z, blocks::AIR);
    chunk
}

#[test]
fn the_plan_of_the_split_is_as_the_scenario_has_it() {
    for pinned in [false, true] {
        let region = before_a_split(pinned);
        let splitting = region.split(&[THIRD], PART, &[]).expect("player 2 goes");
        // `THIRD` with its player, and the outpost, which is six chunks from them and
        // seven from player 3. `EAST` is as near to the home chunk as to player 3, and
        // nearer to both than to player 2.
        assert_eq!(splitting.chunks, [THIRD, OUTPOST]);
        assert_eq!(
            splitting.part.players.keys().copied().collect::<Vec<_>>(),
            [player(2)]
        );
        assert_eq!(splitting.state.players.len(), 2);
        assert_eq!(
            splitting.state.edges[&E].outbox,
            [(
                1,
                Durable::SplitOff {
                    region: PART,
                    players: vec![(player(2), entity(2), None)],
                }
            )]
            .into()
        );
        assert!(splitting.state.edges[&F].outbox.is_empty());
        assert_eq!(splitting.state.edges[&F].sent, 0);
    }
}

#[test]
fn after_a_split_both_regions_are_what_a_restore_makes_of_their_states_and_chunks() {
    for pinned in [false, true] {
        let (region, splitting, _, part) = taken(pinned);
        // What it held less the part's chunks, with what was handed in as granted, and
        // its areas.
        let held = [HOME, EAST, SECOND, WANTED];
        let holdings = if pinned {
            pinned_everywhere(&held)
        } else {
            on_open_land(&held)
        };
        let restored = Region::restore(config(LONG), splitting.state.clone(), holdings);
        assert_eq!(region, restored);
        assert_eq!(region.state(), splitting.state);

        // The part: its state, its chunks, no area, and the split region's
        // configuration.
        let holdings = on_open_land(&[THIRD, OUTPOST]);
        let restored = Region::restore(config(LONG), splitting.part.clone(), holdings);
        assert_eq!(part.region, restored);
        assert_eq!(part.region.state(), splitting.part);
        assert_eq!(part.region.tick_number(), region.tick_number());
    }
}

#[test]
fn the_loaded_chunks_are_divided_between_the_two_regions_block_for_block() {
    for pinned in [false, true] {
        let before = before_a_split(pinned);
        let (region, _, kept, part) = taken(pinned);
        assert_eq!(
            kept,
            vec![
                (HOME, stone_without(OWN_BLOCK)),
                (EAST, stone_chunk()),
                (SECOND, stone_chunk()),
            ]
        );
        assert_eq!(part.chunks, vec![(THIRD, stone_without(THIRD_BLOCK))]);
        // None is lost, and neither region has one loaded: they begin as a restored
        // region does.
        for (chunk, blocks) in kept.iter().chain(&part.chunks) {
            assert_eq!(before.chunk(*chunk), Some(blocks));
        }
        assert_eq!(kept.len() + part.chunks.len(), before.loaded_chunk_count());
        assert_eq!(region.loaded_chunk_count(), 0);
        assert_eq!(part.region.loaded_chunk_count(), 0);
    }
}

#[test]
fn after_a_split_the_region_knows_nothing_of_the_parts_chunks_and_the_part_holds_them() {
    for pinned in [false, true] {
        let (mut region, _, _, mut part) = taken(pinned);
        for chunk in [HOME, EAST, SECOND, WANTED] {
            assert_eq!(region.knowledge(chunk), Knowledge::Held, "{chunk:?}");
        }
        for chunk in [THIRD, OUTPOST] {
            assert_eq!(region.knowledge(chunk), Knowledge::Unknown, "{chunk:?}");
            assert_eq!(part.region.knowledge(chunk), Knowledge::Held, "{chunk:?}");
            // The region stays pinned to its areas, the part's chunks among them; the
            // part is pinned to nothing.
            assert_eq!(region.pins(chunk), pinned);
            assert!(!part.region.pins(chunk));
        }
        assert_eq!(region.held_chunk_count(), 4);
        assert_eq!(part.region.held_chunk_count(), 2);
        for chunk in [HOME, EAST, SECOND, WANTED] {
            assert_eq!(part.region.knowledge(chunk), Knowledge::Unknown);
        }

        // Neither has a ticket: nothing is asked of storage, and nothing is claimed but
        // for a player.
        let output = idle(&mut region);
        assert!(output.chunk_requests.is_empty() && output.claims.is_empty());
        assert!(silent(&output) && output.returns.is_empty());
        let output = idle(&mut part.region);
        assert!(output.chunk_requests.is_empty() && output.claims.is_empty());
        assert!(silent(&output) && output.returns.is_empty());
    }
}

#[test]
fn a_player_who_stays_and_steps_into_a_chunk_of_the_part_is_let_go_to_it_when_the_store_has_said() {
    for pinned in [false, true] {
        let (mut region, _, _, mut part) = taken(pinned);
        // Player 3 stays, the chunk is claimed because they stand in it,
        let output = tick(
            &mut region,
            &single_input(F, player(3), entity(3), 2, walk_to(middle(THIRD))),
        );
        assert!(output.durable.is_empty());
        assert_eq!(output.claims, [THIRD]);
        assert!(region.player(player(3)).is_some());
        let leaving = transfer_of(&state_of(&region, player(3)));
        // and with the store's word that the part holds it they are let go to it.
        let output = tick(&mut region, &foreign(&[(THIRD, PART)]));
        assert_eq!(
            output.durable,
            vec![(
                F,
                1,
                Durable::Departed {
                    player: player(3),
                    transfer: leaving.clone(),
                    to: PART,
                }
            )]
        );
        assert!(region.player(player(3)).is_none());

        // The part knows only the edge of the player it was made with. An arrival
        // through another is passed over until that edge has said hello.
        assert!(part.region.edge(F).is_none());
        let mut early = part.region.clone();
        let output = tick(&mut early, &arrive(F, player(3), leaving.clone()));
        assert_eq!(removed(&output), [(entity(3), THIRD)]);
        assert!(early.player(player(3)).is_none());

        let mut inputs = arrive(F, player(3), leaving.clone());
        inputs.edges = vec![started(F, 10)];
        let output = tick(&mut part.region, &inputs);
        assert_eq!(spawned(&output), [entity(3)]);
        assert_eq!(state_of(&part.region, player(3)), taken_in(&leaving, F));
        assert_eq!(part.region.player_count(), 2);
        let known = part.region.edge(F).expect("it has said hello");
        assert_eq!((known.start, known.since), (10, output.tick));
    }
}

#[test]
fn a_join_at_the_part_is_refused() {
    let (_, _, _, mut part) = taken(false);
    let output = tick(&mut part.region, &changes(vec![join(E, player(7))]));
    assert_eq!(output.durable, vec![(E, 1, refused(7))]);
    assert!(spawned(&output).is_empty());
    assert_eq!(part.region.player_count(), 1);
    assert_eq!(part.region.state().next_entity_id, EntityId(0));
}

#[test]
fn a_player_who_went_and_walks_back_is_let_go_by_the_part_and_taken_in_with_the_same_entity() {
    for pinned in [false, true] {
        let (mut region, _, _, mut part) = taken(pinned);
        assert!(region.player(player(2)).is_none());
        let went = state_of(&part.region, player(2));
        assert_eq!(
            (went.entity_id, went.last_input, went.edge),
            (entity(2), 2, E)
        );

        // The part takes what they do, as the region did: it has them whole.
        let output = tick(
            &mut part.region,
            &single_input(E, player(2), entity(2), 3, walk_to(middle(SECOND))),
        );
        assert_eq!(moved(&output), [entity(2)]);
        // It knows nothing of the split region's chunks, and asks.
        assert!(output.durable.is_empty());
        assert_eq!(output.claims, [SECOND]);
        let leaving = transfer_of(&state_of(&part.region, player(2)));
        assert_eq!(leaving.last_input, 3);
        let output = tick(&mut part.region, &foreign(&[(SECOND, REGION_A)]));
        assert_eq!(
            output.durable,
            vec![(
                E,
                1,
                Durable::Departed {
                    player: player(2),
                    transfer: leaving.clone(),
                    to: REGION_A,
                }
            )]
        );
        assert_eq!(part.region.player_count(), 0);

        // An input the split region is sent again for the stay that went finds nobody
        // there; the arrival brings them back with the entity they always had.
        let mut inputs = single_input(E, player(2), entity(2), 3, walk_to(IN_HOME));
        let output = tick(&mut region, &inputs);
        assert!(silent(&output));
        inputs = arrive(E, player(2), leaving.clone());
        inputs.input(E, player(2), entity(2), 4, walk_to(IN_HOME));
        let output = tick(&mut region, &inputs);
        assert_eq!(spawned(&output), [entity(2)]);
        let back = state_of(&region, player(2));
        assert_eq!(
            (back.entity_id, back.pose.position, back.last_input),
            (entity(2), IN_HOME, 4)
        );
        assert_eq!(region.player_count(), 3);
    }
}

#[test]
fn an_arrival_and_an_action_for_a_chunk_of_the_part_are_handled_as_for_a_chunk_never_heard_of() {
    for pinned in [false, true] {
        let (mut region, _, _, _) = taken(pinned);
        let arriving = transfer(TRAVELLER, 3, middle(THIRD));
        let action = remote(player(9), 4, break_at(THIRD_BLOCK.offset(1, 0, 0)));
        let mut inputs = arrive(F, player(5), arriving.clone());
        inputs.remote_actions.push((E, action.clone()));
        let output = tick(&mut region, &inputs);
        // The player is taken in and the chunk claimed; the action goes on without a
        // region, to whoever serves the edge the chunk.
        assert_eq!(
            output.durable,
            vec![(E, 2, Durable::Remote { action, to: None })]
        );
        assert_eq!(state_of(&region, player(5)), taken_in(&arriving, F));
        assert_eq!(output.claims, [THIRD]);
        assert!(block_changes(&output).is_empty());
    }
}

#[test]
fn a_ticket_for_a_chunk_of_the_part_claims_it_as_one_for_any_chunk_the_region_does_not_hold() {
    for pinned in [false, true] {
        // A viewer's claims it anywhere.
        let (mut region, _, _, _) = taken(pinned);
        let output = tick(&mut region, &add(&[viewer(THIRD)]));
        assert_eq!(output.claims, [THIRD]);
        assert!(output.chunk_requests.is_empty());
        tick(&mut region, &foreign(&[(THIRD, PART)]));
        assert_eq!(region.knowledge(THIRD), Knowledge::Foreign(PART));

        // A guest's claims it in the areas the region is pinned to, and it still is.
        let (mut region, _, _, _) = taken(pinned);
        let output = tick(&mut region, &add(&[guest(THIRD), guest(OUTPOST)]));
        let claims: &[ChunkPos] = if pinned { &[THIRD, OUTPOST] } else { &[] };
        assert_eq!(output.claims, claims);
    }
}

#[test]
fn once_told_that_the_part_holds_a_chunk_the_region_sends_on_what_comes_for_it_or_doubts() {
    for pinned in [false, true] {
        let (mut region, _, _, _) = taken(pinned);
        tick(&mut region, &add(&[viewer(THIRD)]));
        tick(&mut region, &foreign(&[(THIRD, PART)]));
        let arriving = transfer(TRAVELLER, 3, middle(THIRD));
        let action = remote(player(9), 4, break_at(THIRD_BLOCK.offset(1, 0, 0)));
        let mut inputs = arrive(F, player(5), arriving.clone());
        inputs.remote_actions.push((E, action.clone()));
        let output = tick(&mut region, &inputs);
        if pinned {
            // The chunk is of its own areas: it may have come back without a word
            // (section 2.2).
            assert_eq!(
                output.durable,
                vec![(E, 2, Durable::Remote { action, to: None })]
            );
            assert!(region.player(player(5)).is_some());
            assert_eq!(output.claims, [THIRD]);
        } else {
            assert_eq!(
                output.durable,
                vec![
                    (
                        F,
                        1,
                        Durable::NotMine {
                            what: Misdirected::Arrival {
                                player: player(5),
                                transfer: arriving,
                            },
                            holder: PART,
                        }
                    ),
                    (
                        E,
                        2,
                        Durable::NotMine {
                            what: Misdirected::Remote(action),
                            holder: PART,
                        }
                    ),
                ]
            );
            assert!(region.player(player(5)).is_none());
            assert_eq!(region.knowledge(THIRD), Knowledge::Foreign(PART));
        }
    }
}

#[test]
fn the_part_serves_its_chunks_and_claims_nothing_for_a_guest() {
    let (_, _, _, mut part) = taken(false);
    let mut hello = add(&[guest(THIRD), viewer(OUTPOST), guest(HOME), guest(SECOND)]);
    hello.edges = vec![started(E, 10), started(F, 10)];
    let output = tick(&mut part.region, &hello);
    // What it holds and someone watches is asked of storage; it is pinned to nothing,
    // so a guest is no reason to claim.
    assert_eq!(output.chunk_requests, [THIRD, OUTPOST]);
    assert!(output.claims.is_empty());
    let mut delivery = delivered(&[OUTPOST]);
    delivery.chunks_loaded.extend(part.chunks.clone());
    tick(&mut part.region, &delivery);
    assert_eq!(block(&part.region, THIRD_BLOCK), Some(blocks::AIR));

    // Its player goes on where they were.
    let beside = THIRD_BLOCK.offset(1, 0, 0);
    let output = tick(
        &mut part.region,
        &single_input(E, player(2), entity(2), 3, dig(beside, 2)),
    );
    assert_eq!(block_changes(&output), [(beside, blocks::AIR)]);
    assert_eq!(acknowledged(&output), [(player(2), 2)]);
    // The edge it was made with is known since the split, and one that says hello
    // later since then.
    let state = part.region.state();
    assert_eq!(state.edges[&E].since, state.tick - 3);
    assert_eq!(state.edges[&F].since, state.tick - 2);
}

#[test]
fn ticking_on_from_a_split_is_ticking_on_from_the_restores_input_for_input() {
    for pinned in [false, true] {
        let (mut region, splitting, kept, mut part) = taken(pinned);
        let held = [HOME, EAST, SECOND, WANTED];
        let holdings = if pinned {
            pinned_everywhere(&held)
        } else {
            on_open_land(&held)
        };
        let mut restored = Region::restore(config(LONG), splitting.state.clone(), holdings);
        let holdings = on_open_land(&[THIRD, OUTPOST]);
        let mut restored_part = Region::restore(config(LONG), splitting.part.clone(), holdings);

        let mut hello = add(&[viewer(HOME), viewer(EAST), viewer(SECOND), viewer(THIRD)]);
        hello.edges = vec![started(E, 10), started(F, 10)];
        hello
            .edges
            .push(EdgeEvent::Confirmed { edge: E, number: 1 });
        let mut answers = foreign(&[(THIRD, PART)]);
        answers.chunks_loaded = kept.clone();
        let mut acting = single_input(E, player(1), entity(1), 2, move_to(12.5));
        // Player 3 walks up to the part, breaks a block of it and steps across.
        acting.input(F, player(3), entity(3), 2, walk(47.5, 8.5));
        acting.input(F, player(3), entity(3), 3, dig(BlockPos::new(48, 63, 8), 5));
        acting.input(F, player(3), entity(3), 4, walk_to(middle(THIRD)));
        acting.change(join(F, player(8)));
        let script = [hello, answers, acting, TickInputs::default()];
        for (index, inputs) in script.iter().enumerate() {
            let output = tick(&mut region, inputs);
            assert_eq!(output, tick(&mut restored, inputs), "tick {index} after");
            assert_eq!(region, restored, "tick {index} after");
        }
        assert!(region.player(player(3)).is_none(), "player 3 was let go");
        assert_eq!(block(&region, OWN_BLOCK), Some(blocks::AIR));

        let mut hello = add(&[guest(THIRD), guest(SECOND)]);
        hello.edges = vec![started(E, 10), started(F, 10)];
        let delivery = TickInputs {
            chunks_loaded: part.chunks.clone(),
            ..TickInputs::default()
        };
        let mut acting = single_input(E, player(2), entity(2), 3, move_to(50.5));
        acting
            .remote_actions
            .push((F, remote(player(3), 5, break_at(BlockPos::new(48, 63, 8)))));
        acting.change(join(E, player(7)));
        let script = [hello, delivery, acting, TickInputs::default()];
        for (index, inputs) in script.iter().enumerate() {
            let output = tick(&mut part.region, inputs);
            assert_eq!(output, tick(&mut restored_part, inputs), "tick {index}");
            assert_eq!(part.region, restored_part, "tick {index} after");
        }
        assert_eq!(
            block(&part.region, BlockPos::new(48, 63, 8)),
            Some(blocks::AIR)
        );
    }
}

// ---------------------------------------------------------------------------------------
// S37: the same region and arguments give the same
// ---------------------------------------------------------------------------------------

#[test]
fn the_same_region_and_arguments_give_byte_identical_states_and_identical_chunk_lists() {
    // Two regions that came to be what they are by the same ticks, and one of them
    // asked twice.
    let (one, other) = (before_a_merge(0), before_a_merge(0));
    assert_eq!(one, other);
    let theirs = absorbed_state();
    let merged = one.absorb(REGION_B, &theirs);
    assert_eq!(bytes(&merged), bytes(&other.absorb(REGION_B, &theirs)));
    assert_eq!(bytes(&merged), bytes(&one.absorb(REGION_B, &theirs)));
    let back: RegionState = postcard::from_bytes(&bytes(&merged)).expect("it can be read");
    assert_eq!(back, merged);

    for pinned in [false, true] {
        let (one, other) = (before_a_split(pinned), before_a_split(pinned));
        let named = [THIRD, SECOND];
        let split = one.split(&named, PART, &[]).expect("two players go");
        assert_eq!(split.part.players.len(), 2);
        for again in [other.split(&named, PART, &[]), one.split(&named, PART, &[])] {
            let again = again.expect("as before");
            assert_eq!(bytes(&again.state), bytes(&split.state));
            assert_eq!(bytes(&again.part), bytes(&split.part));
            assert_eq!(again.chunks, split.chunks);
        }
        // The chunks named are a set: their order and number change nothing.
        let turned = [SECOND, WANTED, THIRD, SECOND, HOME, THIRD];
        let again = other.split(&turned, PART, &[]).expect("as before");
        assert_eq!(bytes(&again.state), bytes(&split.state));
        assert_eq!(bytes(&again.part), bytes(&split.part));
        assert_eq!(again.chunks, split.chunks);

        for state in [&split.state, &split.part] {
            let back: RegionState = postcard::from_bytes(&bytes(state)).expect("it can be read");
            assert_eq!(&back, state);
        }
    }
}

#[test]
fn the_order_in_which_a_split_is_handed_its_granted_chunks_changes_nothing() {
    for pinned in [false, true] {
        let more = [ChunkPos::new(-4, 4), WANTED, ChunkPos::new(0, 9)];
        let mut one = before_a_split(pinned);
        let splitting = one.split(&[THIRD], PART, &more).expect("player 2 goes");
        let (kept, part) = one.take_split(splitting.clone(), &more);

        let mut other = before_a_split(pinned);
        let turned = [more[2], more[0], more[1]];
        let again = other.split(&[THIRD], PART, &turned).expect("player 2 goes");
        assert_eq!(again, splitting);
        let (kept_again, part_again) = other.take_split(again, &turned);
        assert_eq!(one, other);
        assert_eq!(kept, kept_again);
        assert_eq!(part, part_again);
        assert_eq!(one.held_chunk_count(), 6);
    }
}

// ---------------------------------------------------------------------------------------
// S35 and S36: against one region
// ---------------------------------------------------------------------------------------

/// The chunks the runs are about: two rows of eight. On stripes the line runs between
/// x = 0 and x = 1, through the middle.
fn grid() -> Vec<ChunkPos> {
    (-3..=4)
        .flat_map(|x| [ChunkPos::new(x, 0), ChunkPos::new(x, 1)])
        .collect()
}

/// Where players enter the world in the runs: in `HOME`, in a row of blocks that nobody
/// builds in, so that a block is never placed where someone stands.
const ENTRANCE: Vec3 = Vec3::new(14.5, 64.0, 7.5);

/// The start every edge of the runs has.
const START: u64 = 10;

/// What the regions of a run are made with: `on` is whether the world store keeps the
/// players' places (ADR-0020), which the runs are played with and without.
fn run_config(on: bool) -> RegionConfig {
    RegionConfig {
        spawn: ENTRANCE,
        starting_hotbar: hotbar(),
        return_after: 0,
        place_by_store: on,
        lowest_y: -64,
    }
}

/// Where a player is and what they hold, as the store keeps it.
fn place_of(state: &PlayerState) -> Place {
    Place {
        pose: state.pose,
        flying: state.flying,
        hotbar: state.hotbar,
        selected_slot: state.selected_slot,
    }
}

/// What the world store keeps of a player: the latest stay, how often it was handed
/// on, and its place (ADR-0020, section 2).
#[derive(Debug, Clone, PartialEq)]
struct Kept {
    stay: EntityId,
    hops: u32,
    place: Option<Place>,
}

/// The world store as far as stays go: the table of section 3 of ADR-0020, which the
/// regions of a run are held to. A note of a stay that was never issued, and a copy of
/// the living stay with fewer hand-overs, are what the record says cannot be: no run
/// makes one.
#[derive(Debug, Clone, Default)]
struct Places {
    records: BTreeMap<PlayerId, Kept>,
    /// How many answers named a place, and how many notes were answered `Dead`.
    kept: usize,
    dead: usize,
}

impl Places {
    /// Takes the notes of one commit of a region, which is the home region or not.
    /// `holder` says who holds a chunk, `None` for the home region itself. Returns the
    /// answers `Enter` and `Dead` for that region, and the `Dead` every region is told.
    #[allow(clippy::type_complexity)]
    fn take(
        &mut self,
        home: bool,
        notes: &[StayNote],
        holder: impl Fn(ChunkPos) -> Option<RegionId>,
    ) -> (
        Vec<Entered>,
        Vec<(PlayerId, EntityId, u32)>,
        Vec<(PlayerId, EntityId, u32)>,
    ) {
        let (mut enter, mut dead, mut all) = (Vec::new(), Vec::new(), Vec::new());
        for note in notes {
            match note {
                StayNote::Entering { player, entity } => {
                    assert!(home, "{note:?} from a region that is not joined");
                    let record = self.records.entry(*player).or_insert(Kept {
                        stay: EntityId(0),
                        hops: 0,
                        place: None,
                    });
                    if *entity < record.stay {
                        self.dead += 1;
                        dead.push((*player, record.stay, record.hops));
                        continue;
                    }
                    if *entity > record.stay {
                        record.stay = *entity;
                        record.hops = 0;
                        all.push((*player, *entity, 0));
                    }
                    let place = record.place.clone();
                    self.kept += usize::from(place.is_some());
                    let holder = place
                        .as_ref()
                        .and_then(|place| holder(chunk_of(place.pose.position)));
                    enter.push(Entered {
                        player: *player,
                        entity: *entity,
                        place,
                        holder,
                    });
                }
                StayNote::Has {
                    player,
                    entity,
                    hops,
                    place,
                } => {
                    let record = self.records.get_mut(player);
                    let record =
                        record.unwrap_or_else(|| panic!("{note:?} of a stay never issued"));
                    assert!(*entity <= record.stay, "{note:?} of a stay never issued");
                    if *entity < record.stay {
                        self.dead += 1;
                        dead.push((*player, record.stay, record.hops));
                        continue;
                    }
                    assert!(*hops >= record.hops, "{note:?} against {record:?}");
                    record.hops = *hops;
                    record.place = Some(place.clone());
                }
            }
        }
        (enter, dead, all)
    }
}

/// The world store as far as chunks go (ADR-0011, sections 1.3, 2 and 3): who holds a
/// chunk is the region it is granted to, else the region pinned to an area with it.
#[derive(Debug, Clone)]
struct Grants {
    pinned: Vec<(ChunkArea, RegionId)>,
    granted: BTreeMap<ChunkPos, RegionId>,
    /// The id the next region a split makes has.
    next: u32,
}

impl Grants {
    fn holder(&self, chunk: ChunkPos) -> Option<RegionId> {
        self.granted.get(&chunk).copied().or_else(|| {
            self.pinned
                .iter()
                .find(|(area, _)| area.contains(chunk))
                .map(|(_, region)| *region)
        })
    }

    /// The areas `region` is pinned to, in the order the store has them.
    fn areas_of(&self, region: RegionId) -> Vec<ChunkArea> {
        self.pinned
            .iter()
            .filter(|(_, pinned)| *pinned == region)
            .map(|(area, _)| *area)
            .collect()
    }

    /// Answers a claim of `region` as the store would: `granted` and `foreign`, each in
    /// the order of the claims. Every chunk of these worlds is somebody's.
    fn answer(
        &self,
        region: RegionId,
        claims: &[ChunkPos],
    ) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
        let mut granted = Vec::new();
        let mut foreign = Vec::new();
        for chunk in claims {
            match self.holder(*chunk).expect("every chunk is somebody's") {
                holder if holder == region => granted.push(*chunk),
                holder => foreign.push((*chunk, holder)),
            }
        }
        (granted, foreign)
    }

    /// `AbsorbCommit`: the absorbed region's grants are the survivor's, and its areas
    /// are appended to the survivor's. Returns the grants that moved and the areas, as
    /// the store's answer names them.
    fn absorb(
        &mut self,
        survivor: RegionId,
        absorbed: RegionId,
    ) -> (Vec<ChunkPos>, Vec<ChunkArea>) {
        let mut chunks = Vec::new();
        for (chunk, holder) in &mut self.granted {
            if *holder == absorbed {
                *holder = survivor;
                chunks.push(*chunk);
            }
        }
        let areas = self.areas_of(absorbed);
        self.pinned.retain(|(_, region)| *region != absorbed);
        self.pinned
            .extend(areas.iter().map(|area| (*area, survivor)));
        (chunks, areas)
    }

    /// `SplitCommit`: the chunks are granted to the new region, whether the region
    /// that is split held them by grant or by being pinned.
    fn split(&mut self, region: RegionId, part: RegionId, chunks: &[ChunkPos]) {
        assert_eq!(part, RegionId(self.next));
        self.next += 1;
        for chunk in chunks {
            assert_eq!(self.holder(*chunk), Some(region), "{chunk:?}");
            self.granted.insert(*chunk, part);
        }
    }
}

/// One region of a [`Cluster`], with what its next tick is to be given.
struct Site {
    region: Region,
    /// What the edges and the store have sent the region and no tick has taken.
    next: TickInputs,
    /// Whether the region has begun anew and its edges' hellos are not answered yet:
    /// until every chunk they named is answered, everything they sent behind waits
    /// (ADR-0012, section 4.5; here, rule 34).
    holding: bool,
}

impl Site {
    /// What a tick of a region that is still answering hellos is given: what the store
    /// and storage say, the hellos themselves, and nothing its edges sent behind them.
    fn answers(&mut self) -> TickInputs {
        let next = &mut self.next;
        TickInputs {
            edges: std::mem::take(&mut next.edges),
            tickets_added: std::mem::take(&mut next.tickets_added),
            tickets_removed: std::mem::take(&mut next.tickets_removed),
            chunks_loaded: std::mem::take(&mut next.chunks_loaded),
            granted: std::mem::take(&mut next.granted),
            foreign: std::mem::take(&mut next.foreign),
            entered: std::mem::take(&mut next.entered),
            dead: std::mem::take(&mut next.dead),
            ..TickInputs::default()
        }
    }

    /// Whether every chunk the hellos named is answered: none is asked for, and what
    /// the region holds is loaded.
    fn answered(&self) -> bool {
        let next = &self.next;
        next.granted.is_empty()
            && next.foreign.is_empty()
            && next.chunks_loaded.is_empty()
            && next.tickets_added.is_empty()
            && grid()
                .into_iter()
                .all(|chunk| match self.region.knowledge(chunk) {
                    Knowledge::Asked => false,
                    Knowledge::Held => self.region.chunk(chunk).is_some(),
                    Knowledge::Foreign(_) | Knowledge::Unknown => true,
                })
    }

    /// Whether nothing waits for the region, and it waits for nothing: no stay is
    /// entering.
    fn idle(&self) -> bool {
        let next = &self.next;
        !self.holding
            && next.player_changes.is_empty()
            && next.inputs.is_empty()
            && next.remote_actions.is_empty()
            && next.granted.is_empty()
            && next.foreign.is_empty()
            && next.chunks_loaded.is_empty()
            && next.entered.is_empty()
            && next.dead.is_empty()
            && self.region.state().entering.is_empty()
    }
}

/// A player's stay as the edge has it.
#[derive(Debug, Clone)]
struct Stay {
    edge: EdgeId,
    /// Which connection it is: the attempt its join names.
    attempt: u64,
    /// The entity, once a region has said it.
    entity: Option<EntityId>,
    /// The region the edge takes the stay to be in.
    site: RegionId,
    /// Everything the player did in this stay, numbered from 1, to send again after a
    /// hand-over.
    made: Vec<PlayerInput>,
}

/// Regions with one store and the edges `E` and `F` between them, which the cluster
/// plays: it answers each tick's claims into the next tick, delivers what is asked of
/// storage as the regions left it, handles every outbox entry as section 5 of ADR-0012
/// has an edge do, and merges and splits regions, doing for each what section 8 of
/// ADR-0014 has the runner, the store and an edge do, as far as a test that reads every
/// entry in the tick that makes it needs to:
///
/// - the region that absorbed or was split begins anew: its tickets are gone and are
///   given again, as its edges' hellos do, and what the edges sent it waits until the
///   chunks are answered ([`Site::holding`]);
/// - an absorbed region stands for its survivor (rule 39), and what the edge kept for
///   it goes to the survivor, behind what it kept for the survivor (rule 42);
/// - a stay is the region's that says it has it (rule 38), and one the edge has given
///   up is ended there with a leave that names its entity;
/// - a stay a `SplitOff` names is the part's, with the inputs the edge still keeps
///   (rule 45).
///
/// Every region has a ticket of one kind on every chunk of [`grid`], so that whoever
/// holds a chunk has it loaded.
struct Cluster {
    grants: Grants,
    sites: BTreeMap<RegionId, Site>,
    /// The region players enter the world in.
    home: RegionId,
    /// The regions that were absorbed, each with the region it went into.
    absorbed: BTreeMap<RegionId, RegionId>,
    /// What the store has of each chunk that a region had loaded when it began anew or
    /// was absorbed: nothing was unsaved then (section 2.4).
    storage: BTreeMap<ChunkPos, Chunk>,
    /// The kind of ticket every region has on every chunk.
    ticket: Ticket,
    stays: BTreeMap<PlayerId, Stay>,
    /// The highest sequence number of each player's actions on blocks that a region
    /// reported as dealt with.
    dealt_with: BTreeMap<PlayerId, i32>,
    /// The actions that were passed on and have not been reported as dealt with.
    under_way: BTreeSet<(PlayerId, i32)>,
    /// How many players were let go, actions passed on, entries sent on with
    /// `NotMine`, and stays ended on a region's word.
    let_go: usize,
    passed_on: usize,
    sent_on: usize,
    ended: usize,
    /// Whether the store keeps the players' places, and what it keeps (ADR-0020).
    on: bool,
    places: Places,
    /// The attempt of the last join: no two connections of a run have one.
    attempts: u64,
    /// How many stays were found by the attempt their transfer carries, and how many
    /// `Ended` entries regions made.
    by_attempt: usize,
    told_ended: usize,
}

impl Cluster {
    /// Regions pinned to `areas`, numbered in their order, of which region 0 is the
    /// home region, with a ticket of the kind `ticket` on every chunk of the grid.
    fn new(areas: &[ChunkArea], ticket: Ticket, on: bool) -> Self {
        let mut grants = Grants {
            pinned: Vec::new(),
            granted: BTreeMap::new(),
            next: areas.len() as u32,
        };
        let mut sites = BTreeMap::new();
        for (index, area) in areas.iter().enumerate() {
            let id = RegionId(index as u32);
            grants.pinned.push((*area, id));
            let holdings = Holdings {
                held: Vec::new(),
                pinned: vec![*area],
            };
            let entity_ids = EntityIds::block(3 + index as u32).expect("the block exists");
            let mut site = Site {
                region: Region::new(run_config(on), entity_ids, holdings),
                next: TickInputs::default(),
                holding: false,
            };
            Self::hello(&mut site, ticket);
            sites.insert(id, site);
        }
        let mut cluster = Self {
            grants,
            sites,
            home: REGION_A,
            absorbed: BTreeMap::new(),
            storage: BTreeMap::new(),
            ticket,
            stays: BTreeMap::new(),
            dealt_with: BTreeMap::new(),
            under_way: BTreeSet::new(),
            let_go: 0,
            passed_on: 0,
            sent_on: 0,
            ended: 0,
            on,
            places: Places::default(),
            attempts: 0,
            by_attempt: 0,
            told_ended: 0,
        };
        cluster.settle();
        cluster
    }

    /// The edges say hello to a region that has begun anew: they are there, and every
    /// subscription begins anew. What the store and storage were about to tell the
    /// region is not told it: it asks again, and names its stays again, to which the
    /// store says again who may enter and who is dead (ADR-0020, section 9).
    fn hello(site: &mut Site, ticket: Ticket) {
        let next = &mut site.next;
        next.edges.push(started(E, START));
        next.edges.push(started(F, START));
        next.tickets_added = grid().into_iter().map(|chunk| (chunk, ticket)).collect();
        next.tickets_removed.clear();
        next.granted.clear();
        next.foreign.clear();
        next.chunks_loaded.clear();
        next.entered.clear();
        next.dead.clear();
        site.holding = true;
    }

    /// The living region that `region` is or went into (rule 39).
    fn living(&self, mut region: RegionId) -> RegionId {
        while let Some(survivor) = self.absorbed.get(&region) {
            region = *survivor;
        }
        assert!(self.sites.contains_key(&region), "{region} is no region");
        region
    }

    fn site(&mut self, region: RegionId) -> &mut Site {
        self.sites.get_mut(&region).expect("the region lives")
    }

    /// A player enters the world, in the home region, on a connection of their own:
    /// returns the join, which names an attempt no other join of the run names.
    fn join(&mut self, id: PlayerId, edge: EdgeId) -> PlayerChange {
        self.attempts += 1;
        let stay = Stay {
            edge,
            attempt: self.attempts,
            entity: None,
            site: self.home,
            made: Vec::new(),
        };
        let join = PlayerJoin {
            player: id,
            name: format!("player-{}", id.0.as_u128()),
            attempt: stay.attempt,
        };
        self.stays.insert(id, stay);
        let home = self.home;
        let change = PlayerChange::Join(edge, join);
        self.site(home).next.change(change.clone());
        change
    }

    /// A player's connection ends: the edge tells the region it takes them to be in,
    /// names the stay, and has given it up. Returns the leave, which names the entity
    /// if the edge has been told one, and else the attempt of the join (ADR-0020,
    /// section 4.4).
    fn leave(&mut self, id: PlayerId) -> PlayerChange {
        let stay = self.stays.remove(&id).expect("the player has a stay");
        let attempt = stay.entity.is_none().then_some(stay.attempt);
        let change = PlayerChange::Leave(stay.edge, id, stay.entity, attempt);
        self.site(stay.site).next.change(change.clone());
        change
    }

    /// A player does something: the edge numbers it and sends it to the region it
    /// takes them to be in, naming the stay.
    fn act(&mut self, id: PlayerId, input: PlayerInput) -> (EdgeId, EntityId, u64) {
        let stay = self.stays.get_mut(&id).expect("the player has a stay");
        let entity = stay.entity.expect("the player has been told their entity");
        stay.made.push(input.clone());
        let number = stay.made.len() as u64;
        let (edge, site) = (stay.edge, stay.site);
        self.site(site).next.input(edge, id, entity, number, input);
        (edge, entity, number)
    }

    /// Passes a stay on to `to`, with everything the player did that the region they
    /// come from has not applied (ADR-0012, rules 18 and 20). A stay the edge has given
    /// up arrives nowhere, and its entity is discarded where it was seen last.
    ///
    /// The stay is the edge's if the edge has it with that entity, or has the player
    /// with no entity yet, under the region the word comes from, on the connection
    /// whose attempt the transfer carries (ADR-0020, section 4.3, path 3).
    fn arrive(&mut self, from: RegionId, id: PlayerId, transfer: &PlayerTransfer, to: RegionId) {
        let to = self.living(to);
        assert_ne!(to, from, "{from} sent {id:?} on to itself");
        let next = &mut self.sites.get_mut(&to).expect("it lives").next;
        let theirs = |stay: &Stay| {
            stay.entity == Some(transfer.entity_id)
                || (stay.entity.is_none()
                    && stay.site == from
                    && transfer.attempt == Some(stay.attempt))
        };
        match self.stays.get_mut(&id) {
            Some(stay) if theirs(stay) => {
                if stay.entity.is_none() {
                    stay.entity = Some(transfer.entity_id);
                    self.by_attempt += 1;
                }
                stay.site = to;
                next.change(PlayerChange::Arrive(stay.edge, id, transfer.clone()));
                for (index, input) in stay.made.iter().enumerate() {
                    let number = index as u64 + 1;
                    if number > transfer.last_input {
                        next.input(stay.edge, id, transfer.entity_id, number, input.clone());
                    }
                }
            }
            _ => next.change(PlayerChange::Discard {
                entity: transfer.entity_id,
                chunk: chunk_of(transfer.pose.position),
            }),
        }
    }

    /// An action of `id` was dealt with, by the region they are in or by another.
    fn done(&mut self, id: PlayerId, sequence: i32) {
        self.under_way.remove(&(id, sequence));
        let highest = self.dealt_with.entry(id).or_insert(sequence);
        *highest = (*highest).max(sequence);
    }

    /// One tick of every region.
    fn tick(&mut self) {
        let ids: Vec<RegionId> = self.sites.keys().copied().collect();
        let mut outputs = Vec::new();
        for id in &ids {
            let site = self.site(*id);
            let inputs = if site.holding {
                site.answers()
            } else {
                std::mem::take(&mut site.next)
            };
            outputs.push((*id, tick(&mut site.region, &inputs)));
        }
        for (from, output) in &outputs {
            self.handle(*from, output);
        }
        for site in self.sites.values_mut() {
            if site.holding && site.answered() {
                site.holding = false;
            }
        }

        // What a region takes for its own, the store has granted it; no region changes
        // a block of a chunk that is not its own; and no entity is in two regions
        // (section 2.5).
        let mut entities = BTreeSet::new();
        for (id, site) in &self.sites {
            for chunk in grid() {
                if site.region.knowledge(chunk) == Knowledge::Held {
                    assert_eq!(self.grants.holder(chunk), Some(*id), "{chunk:?}");
                }
            }
            for player in site.region.state().players.values() {
                assert!(
                    entities.insert(player.entity_id),
                    "{:?} is in two regions",
                    player.entity_id
                );
            }
        }
        for (from, output) in &outputs {
            for (block, _) in block_changes(output) {
                assert_eq!(self.grants.holder(block.chunk()), Some(*from), "{block:?}");
            }
        }
    }

    /// What the store, storage and the edges make of one tick of the region `from`.
    fn handle(&mut self, from: RegionId, output: &TickOutput) {
        let (granted, foreign) = self.grants.answer(from, &output.claims);
        assert!(
            output.returns.is_empty(),
            "{from} gave back {:?}, which someone watches",
            output.returns
        );
        let deliveries: Vec<(ChunkPos, Chunk)> = output
            .chunk_requests
            .iter()
            .map(|chunk| {
                let stored = self.storage.get(chunk).cloned();
                (*chunk, stored.unwrap_or_else(stone_chunk))
            })
            .collect();
        let site = self.site(from);
        site.next.granted.extend(granted);
        site.next.foreign.extend(foreign);
        site.next.chunks_loaded.extend(deliveries);
        for (edge, number, _) in &output.durable {
            site.next.edges.push(EdgeEvent::Confirmed {
                edge: *edge,
                number: *number,
            });
        }

        for (id, sequence) in acknowledged(output) {
            self.done(id, sequence);
        }
        // The commit: the store takes the tick's notes, answers the region, and tells
        // every region of a floor it raised (ADR-0020, sections 3 to 5).
        if !self.on {
            assert!(output.stays.is_empty(), "{from} named {:?}", output.stays);
        }
        let (home, grants) = (self.home, &self.grants);
        let holder = |chunk| grants.holder(chunk).filter(|holder| *holder != home);
        let (enter, dead, all) = self.places.take(from == home, &output.stays, holder);
        let site = self.site(from);
        site.next.entered.extend(enter);
        site.next.dead.extend(dead);
        for site in self.sites.values_mut() {
            site.next.dead.extend(all.iter().copied());
        }

        for (id, event) in &output.player_events {
            if let PlayerEvent::Spawned {
                attempt, entity_id, ..
            } = event
            {
                assert_eq!(from, self.home, "only the home region is joined");
                // Only for the connection whose join it answers (ADR-0020, section
                // 4.1).
                if let Some(stay) = self.stays.get_mut(id)
                    && stay.entity.is_none()
                    && stay.attempt == *attempt
                {
                    stay.entity = Some(*entity_id);
                }
            }
        }
        for (edge, _, entry) in &output.durable {
            match entry {
                Durable::Departed {
                    player,
                    transfer,
                    to,
                } => {
                    self.let_go += 1;
                    self.arrive(from, *player, transfer, *to);
                }
                Durable::NotMine {
                    what: Misdirected::Arrival { player, transfer },
                    holder,
                } => {
                    self.sent_on += 1;
                    self.arrive(from, *player, transfer, *holder);
                }
                Durable::Remote { action, to } => {
                    self.passed_on += 1;
                    self.under_way.insert((action.player, action.sequence));
                    // Without a region named it is for the region that serves the edge
                    // the chunk: the one that holds it, which the edge has asked for
                    // the chunk, so that the action waits there until it is loaded.
                    let chunk = action.step.concerns().chunk();
                    let to = match to {
                        Some(named) => self.living(*named),
                        None => self.grants.holder(chunk).expect("it is somebody's"),
                    };
                    assert_ne!(to, from, "{from} passed {action:?} on to itself");
                    let next = &mut self.site(to).next;
                    next.remote_actions.push((*edge, action.clone()));
                }
                Durable::NotMine {
                    what: Misdirected::Remote(action),
                    holder,
                } => {
                    self.sent_on += 1;
                    let to = self.living(*holder);
                    assert_ne!(to, from, "{from} sent {action:?} on to itself");
                    let next = &mut self.site(to).next;
                    next.remote_actions.push((*edge, action.clone()));
                }
                Durable::RemoteDone {
                    player, sequence, ..
                } => self.done(*player, *sequence),
                // A stay that a later one replaced. No run has a player connect while
                // they are connected, so it is never the stay the edge has.
                Durable::Ended {
                    player,
                    entity,
                    attempt,
                } => {
                    assert!(self.on, "{from} made {entry:?}");
                    let meant = self.stays.get(player).is_some_and(|stay| {
                        stay.entity == Some(*entity)
                            || (stay.entity.is_none() && Some(stay.attempt) == *attempt)
                    });
                    assert!(!meant, "{from} ended the stay the edge has: {entry:?}");
                    self.told_ended += 1;
                }
                other => panic!("{from} made {other:?} in a tick"),
            }
        }
    }

    /// The chunks of the grid the region holds, as it says itself.
    fn held_by(&self, region: RegionId) -> Vec<ChunkPos> {
        let site = &self.sites[&region];
        grid()
            .into_iter()
            .filter(|chunk| site.region.knowledge(*chunk) == Knowledge::Held)
            .collect()
    }

    /// The region `survivor` absorbs the region `absorbed`, as of their last ticks, with
    /// whatever is on its way to either. The region is held to section 2.3 and 2.6 on
    /// the way: the plan changes nothing and is what the record says, and what the
    /// region is afterwards is what a restore makes.
    fn merge(&mut self, survivor: RegionId, absorbed: RegionId) {
        assert_ne!(absorbed, self.home, "the home region is never absorbed");
        let gone = self.sites.remove(&absorbed).expect("the region lives");
        let theirs = gone.region.state();
        for chunk in grid() {
            if let Some(blocks) = gone.region.chunk(chunk) {
                self.storage.insert(chunk, blocks.clone());
            }
        }
        let (mut chunks, areas) = self.grants.absorb(survivor, absorbed);
        let mut held = self.held_by(survivor);
        let pinned = self.grants.areas_of(survivor);
        let (ticket, on) = (self.ticket, self.on);

        let site = self.site(survivor);
        let before = site.region.clone();
        let state = site.region.absorb(absorbed, &theirs);
        assert_eq!(site.region, before, "`absorb` changed the region");
        let expected = absorbed_as_the_record_says(&before.state(), absorbed, &theirs);
        assert_eq!(state, expected);

        // With the grants that moved, what the store had granted the survivor in
        // answer to claims that no tick has been told of.
        chunks.extend(std::mem::take(&mut site.next.granted));
        let loaded = site.region.take_absorbed(state.clone(), &chunks, &areas);
        held.extend(&chunks);
        let holdings = Holdings { held, pinned };
        let restored = Region::restore(run_config(on), state.clone(), holdings);
        assert_eq!(site.region, restored, "after the merge");
        assert_eq!(loaded.len(), before.loaded_chunk_count());
        assert!(loaded.is_sorted_by_key(|(chunk, _)| *chunk));

        // The edges: hellos, and every entry of the welcome read and confirmed.
        Self::hello(site, ticket);
        for (edge, known) in &state.edges {
            let number = known.sent;
            site.next.edges.push(EdgeEvent::Confirmed {
                edge: *edge,
                number,
            });
        }
        // What they kept for the absorbed region goes to the survivor, in its order,
        // behind what they kept for the survivor.
        for change in gone.next.player_changes {
            site.next.change(change);
        }
        site.next.inputs.extend(gone.next.inputs);
        site.next.remote_actions.extend(gone.next.remote_actions);
        // The presence answers: a stay the edge has is the survivor's, and one it has
        // given up is ended.
        for (id, player) in &state.players {
            match self.stays.get_mut(id) {
                Some(stay) if stay.entity == Some(player.entity_id) => stay.site = survivor,
                _ => {
                    self.ended += 1;
                    let change = leave(player.edge, *id, Some(player.entity_id));
                    let site = self.sites.get_mut(&survivor).expect("it lives");
                    site.next.change(change);
                }
            }
        }
        // The players the edge had under the absorbed region and that are on their way
        // there, it has under the survivor.
        for stay in self.stays.values_mut() {
            if stay.site == absorbed {
                stay.site = survivor;
            }
        }
        self.storage.extend(loaded);
        self.absorbed.insert(absorbed, survivor);
    }

    /// The players of `region` who stand in `named` are split off, if that is a split.
    /// Returns the new region. The region is held to sections 2.4 and 2.6 on the way.
    fn split(&mut self, region: RegionId, named: &[ChunkPos]) -> Result<RegionId, NoSplit> {
        let part = RegionId(self.grants.next);
        let held = self.held_by(region);
        let pinned = self.grants.areas_of(region);
        let (ticket, on) = (self.ticket, self.on);

        let site = self.site(region);
        let before = site.region.clone();
        // What the store has granted the region in answer to claims that no tick has
        // been told of counts as held, for the plan as for what the region is
        // afterwards (ADR-0017, section 3.6.1). The answers stay where they are if
        // the split is off.
        let granted = site.next.granted.clone();
        let split = site.region.split(named, part, &granted);
        assert_eq!(site.region, before, "`split` changed the region");
        let as_a_set: BTreeSet<ChunkPos> = held.iter().chain(&granted).copied().collect();
        let expected =
            split_as_the_record_says(&before.state(), &as_a_set, !pinned.is_empty(), named, part);
        assert_eq!(split, expected);
        let splitting = split?;

        site.next.granted.clear();
        let (kept, made) = site.region.take_split(splitting.clone(), &granted);
        let stays: Vec<ChunkPos> = as_a_set
            .iter()
            .copied()
            .filter(|chunk| !splitting.chunks.contains(chunk))
            .collect();
        let holdings = Holdings {
            held: stays,
            pinned,
        };
        let restored = Region::restore(run_config(on), splitting.state.clone(), holdings);
        assert_eq!(site.region, restored, "the region after the split");
        let holdings = Holdings {
            held: splitting.chunks.clone(),
            pinned: Vec::new(),
        };
        let restored = Region::restore(run_config(on), splitting.part.clone(), holdings);
        assert_eq!(made.region, restored, "the part");
        assert_eq!(
            kept.len() + made.chunks.len(),
            before.loaded_chunk_count(),
            "a loaded chunk was lost"
        );
        for (chunk, blocks) in kept.iter().chain(&made.chunks) {
            assert_eq!(before.chunk(*chunk), Some(blocks), "{chunk:?}");
        }
        assert!(
            kept.iter()
                .all(|(chunk, _)| !splitting.chunks.contains(chunk))
        );
        let of_the_part = |(chunk, _): &(ChunkPos, Chunk)| splitting.chunks.contains(chunk);
        assert!(made.chunks.iter().all(of_the_part));
        assert!(kept.is_sorted_by_key(|(chunk, _)| *chunk));
        assert!(made.chunks.is_sorted_by_key(|(chunk, _)| *chunk));

        // The edges: hellos to both, and the entries of the welcome read and confirmed.
        Self::hello(site, ticket);
        for (edge, known) in &splitting.state.edges {
            let number = known.sent;
            site.next.edges.push(EdgeEvent::Confirmed {
                edge: *edge,
                number,
            });
        }
        let mut new = Site {
            region: made.region,
            next: TickInputs::default(),
            holding: false,
        };
        Self::hello(&mut new, ticket);
        // Each stay a `SplitOff` names is the part's, with every input of it the edge
        // still keeps; the split region is sent those again as well, and passes them
        // over. A stay the edge has given up is ended when the part says it has it.
        let kept_inputs = site.next.inputs.clone();
        for (id, player) in &splitting.part.players {
            let entry = splitting.state.edges[&player.edge]
                .outbox
                .values()
                .next_back()
                .expect("the edge of a player who went was told");
            let Durable::SplitOff { region, players } = entry else {
                panic!("the last entry for {:?} is {entry:?}", player.edge);
            };
            assert_eq!(*region, part);
            assert!(players.contains(&(*id, player.entity_id, player.attempt)));
            match self.stays.get_mut(id) {
                Some(stay) if stay.entity == Some(player.entity_id) => {
                    stay.site = part;
                    let theirs = kept_inputs.iter().filter(|(_, actor, entity, ..)| {
                        actor == id && *entity == player.entity_id
                    });
                    new.next.inputs.extend(theirs.cloned());
                }
                _ => {
                    self.ended += 1;
                    new.next
                        .change(leave(player.edge, *id, Some(player.entity_id)));
                }
            }
        }
        self.storage.extend(kept);
        self.storage.extend(made.chunks);
        self.grants.split(region, part, &splitting.chunks);
        self.sites.insert(part, new);
        Ok(part)
    }

    /// Whether nothing is on its way: no answer of the store, no chunk, no player and
    /// no action, and no hello unanswered.
    fn settled(&self) -> bool {
        self.sites.values().all(Site::idle)
    }

    /// Ticks until nothing is on its way, and returns how many ticks that took. What
    /// crosses a boundary takes its two ticks, a hello a few, and a player or an action
    /// is passed on at most as many times as there are regions: it ends soon.
    fn settle(&mut self) -> usize {
        for ticks in 0..40 {
            if self.settled() {
                return ticks;
            }
            self.tick();
        }
        panic!("what the regions pass on does not come to an end");
    }

    /// The region that has `id`, if one has.
    fn region_of(&self, id: PlayerId) -> Option<RegionId> {
        let mut found = None;
        for (region, site) in &self.sites {
            if site.region.player(id).is_some() {
                assert_eq!(found, None, "{id:?} is in two regions");
                found = Some(*region);
            }
        }
        found
    }

    /// Whether nothing of `id` waits anywhere: every input the edge sent for them was
    /// taken by the region that has them, and they are on their way nowhere.
    fn at_rest(&self, id: PlayerId) -> bool {
        self.sites.values().all(|site| {
            let waiting = site.next.inputs.iter().any(|(_, actor, ..)| *actor == id);
            let coming = site.next.player_changes.iter().any(|change| match change {
                PlayerChange::Join(_, join) => join.player == id,
                PlayerChange::Arrive(_, player, _) | PlayerChange::Leave(_, player, _, _) => {
                    *player == id
                }
                PlayerChange::Discard { .. } => false,
            });
            !waiting && !coming
        })
    }

    /// Whether everything `id` did was taken by the region that has them, and every
    /// action of theirs on blocks has been dealt with.
    fn unhurried(&self, id: PlayerId) -> bool {
        self.at_rest(id) && !self.under_way.iter().any(|(actor, _)| *actor == id)
    }
}

/// One region that holds everything and never merges or splits: what a [`Cluster`] is
/// compared with. It is given the same joins, inputs and leaves through the same
/// edges, each in the tick after it was made.
struct One {
    region: Region,
    next: TickInputs,
    dealt_with: BTreeMap<PlayerId, i32>,
    /// The store of this world, which answers the one region's notes.
    places: Places,
}

impl One {
    fn new(on: bool) -> Self {
        let holdings = Holdings {
            held: Vec::new(),
            pinned: vec![ChunkArea::EVERYWHERE],
        };
        let mut next = add(&grid().into_iter().map(viewer).collect::<Vec<_>>());
        next.edges = vec![started(E, START), started(F, START)];
        let mut one = Self {
            region: Region::new(run_config(on), ids(), holdings),
            next,
            dealt_with: BTreeMap::new(),
            places: Places::default(),
        };
        for _ in 0..3 {
            one.tick();
        }
        assert_eq!(one.region.loaded_chunk_count(), grid().len());
        one
    }

    fn tick(&mut self) {
        let inputs = std::mem::take(&mut self.next);
        let output = tick(&mut self.region, &inputs);
        assert!(
            output.durable.is_empty(),
            "the one region passed something on: {:?}",
            output.durable
        );
        self.next.granted = output.claims.clone();
        self.next.chunks_loaded = delivered(&output.chunk_requests).chunks_loaded;
        // The one region holds every place itself.
        let (enter, dead, all) = self.places.take(true, &output.stays, |_| None);
        self.next.entered = enter;
        self.next.dead = dead.into_iter().chain(all).collect();
        for (id, sequence) in acknowledged(&output) {
            let highest = self.dealt_with.entry(id).or_insert(sequence);
            *highest = (*highest).max(sequence);
        }
    }
}

/// The blocks around the top of the stone in which two chunks at `position` differ,
/// each with what the first has there and what the second has.
fn differences(
    position: ChunkPos,
    one: &Chunk,
    other: &Chunk,
) -> Vec<(BlockPos, Option<BlockState>, Option<BlockState>)> {
    let mut found = Vec::new();
    for x in 0..16 {
        for z in 0..16 {
            for y in 56..72 {
                let (here, there) = (one.get(x, y, z), other.get(x, y, z));
                if here != there {
                    let block =
                        BlockPos::new(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                    found.push((block, here, there));
                }
            }
        }
    }
    found
}

/// A cluster and the one region beside it, given the same.
struct Pair {
    cluster: Cluster,
    one: One,
    /// What was done, step by step, for a run that fails.
    steps: Vec<String>,
    /// What the run is called when it fails.
    name: String,
    /// How often the two were compared.
    compared: usize,
}

impl Drop for Pair {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("{}, whose steps were:", self.name);
            for step in &self.steps {
                eprintln!("  {step}");
            }
        }
    }
}

impl Pair {
    fn new(name: String, areas: &[ChunkArea], ticket: Ticket) -> Self {
        Self::with(name, areas, ticket, false)
    }

    /// The pair whose regions are made for a store that keeps the players' places, or
    /// that does not.
    fn with(name: String, areas: &[ChunkArea], ticket: Ticket, on: bool) -> Self {
        Self {
            cluster: Cluster::new(areas, ticket, on),
            one: One::new(on),
            steps: Vec::new(),
            name,
            compared: 0,
        }
    }

    fn join(&mut self, id: PlayerId, edge: EdgeId) {
        self.steps
            .push(format!("player {} joins through {edge:?}", id.0.as_u128()));
        let join = self.cluster.join(id, edge);
        self.one.next.change(join);
    }

    fn leave(&mut self, id: PlayerId) {
        let stay = &self.cluster.stays[&id];
        self.steps.push(format!(
            "player {} leaves, having {:?} in {}",
            id.0.as_u128(),
            stay.entity,
            stay.site
        ));
        let leave = self.cluster.leave(id);
        self.one.next.change(leave);
    }

    fn act(&mut self, id: PlayerId, input: PlayerInput) {
        let at = self.cluster.stays[&id].site;
        self.steps
            .push(format!("player {} in {at}: {input:?}", id.0.as_u128()));
        let (edge, entity, number) = self.cluster.act(id, input.clone());
        self.one.next.input(edge, id, entity, number, input);
    }

    fn merge(&mut self, survivor: RegionId, absorbed: RegionId) {
        self.steps.push(format!("{survivor} absorbs {absorbed}"));
        self.cluster.merge(survivor, absorbed);
    }

    fn split(&mut self, region: RegionId, named: &[ChunkPos]) -> Result<RegionId, NoSplit> {
        let split = self.cluster.split(region, named);
        self.steps
            .push(format!("{region} is split at {named:?}: {split:?}"));
        split
    }

    /// One tick of every region of both worlds, and a comparison if nothing is on its
    /// way afterwards.
    fn tick(&mut self) {
        self.steps.push("tick".to_owned());
        self.cluster.tick();
        self.one.tick();
        if self.cluster.settled() {
            self.compare();
        }
    }

    /// Ticks until nothing is on its way in the cluster, and compares.
    fn settle(&mut self) {
        for _ in 0..40 {
            if self.cluster.settled() && self.one.next == TickInputs::default() {
                self.compare();
                return;
            }
            self.tick();
        }
        panic!("what the regions pass on does not come to an end");
    }

    /// The players, where they are and what they hold, and every block of the grid are
    /// the same in the two worlds (section 2.6). Not compared: what only one region
    /// counts, its own acknowledgements.
    fn compare(&mut self) {
        self.compared += 1;
        let cluster = &self.cluster;
        let reference = self.one.region.state();
        let mut found: BTreeMap<PlayerId, PlayerState> = BTreeMap::new();
        for (region, site) in &cluster.sites {
            for (id, player) in site.region.state().players {
                let chunk = chunk_of(player.pose.position);
                assert_eq!(
                    cluster.grants.holder(chunk),
                    Some(*region),
                    "{id:?} stands in {chunk:?} and is {region}'s"
                );
                // The store has the stay, how often it was handed on, and its place.
                if cluster.on {
                    let kept = &cluster.places.records[&id];
                    assert_eq!((kept.stay, kept.hops), (player.entity_id, player.hops));
                    assert_eq!(kept.place, Some(place_of(&player)), "{id:?}");
                }
                let stay = cluster.stays.get(&id).expect("the edge has the stay");
                assert_eq!(
                    (stay.site, stay.entity),
                    (*region, Some(player.entity_id)),
                    "where the edge takes {id:?} to be"
                );
                // How often a player was handed on only several regions count.
                let plain = PlayerState {
                    handled: None,
                    hops: 0,
                    ..player
                };
                assert!(
                    found.insert(id, plain).is_none(),
                    "{id:?} is in two regions"
                );
            }
        }
        let expected: BTreeMap<PlayerId, PlayerState> = reference
            .players
            .into_iter()
            .map(|(id, player)| {
                let plain = PlayerState {
                    handled: None,
                    hops: 0,
                    ..player
                };
                (id, plain)
            })
            .collect();
        assert_eq!(found, expected, "the players");

        for chunk in grid() {
            let holder = cluster.grants.holder(chunk).expect("it is somebody's");
            let region = &cluster.sites[&holder].region;
            assert_eq!(region.knowledge(chunk), Knowledge::Held, "{chunk:?}");
            assert!(region.chunk(chunk).is_some(), "{chunk:?} is not loaded");
            let here = region.chunk(chunk).expect("it is loaded");
            let there = self.one.region.chunk(chunk).expect("it is loaded");
            assert!(
                here == there,
                "the blocks of {chunk:?}, which {holder} holds, and of the one region: {:?}",
                differences(chunk, here, there)
            );
        }
        assert!(cluster.under_way.is_empty(), "{:?}", cluster.under_way);
        assert_eq!(
            cluster.dealt_with, self.one.dealt_with,
            "what was dealt with"
        );
    }
}

/// The seeds of the generated runs: five, or as many as `CLUSTINE_RESHAPE_SEEDS` says,
/// or the one `CLUSTINE_RESHAPE_SEED` names, to see a run that failed again.
fn seeds() -> Vec<u64> {
    if let Ok(seed) = std::env::var("CLUSTINE_RESHAPE_SEED") {
        return vec![seed.parse().expect("CLUSTINE_RESHAPE_SEED is a number")];
    }
    let count = match std::env::var("CLUSTINE_RESHAPE_SEEDS") {
        Ok(count) => count.parse().expect("CLUSTINE_RESHAPE_SEEDS is a number"),
        Err(_) => 5,
    };
    (1..=count).collect()
}

/// What a generated run does to its regions.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reshaping {
    /// Region 0 absorbs region 1, once, at the round named.
    MergeAt(usize),
    /// Regions are split now and then, up to four of them.
    Splits,
    /// The one region is split, and absorbs its part again, over and over.
    SplitAndAbsorb,
    /// Regions are split and absorb each other in any order.
    Anything,
}

/// A run made up by a generator: up to six players who join through either edge, walk
/// across the grid, break and place blocks, change what they hold, leave and join
/// again, while their regions merge and split.
///
/// Each player walks along a row of blocks of their own and acts on the two rows
/// beside it, so that what one of them does to a block never depends on when another
/// did something: a block action across a boundary takes its two ticks, and the one
/// region has none. For the same reason a player acts on blocks only when everything
/// they did before has been dealt with.
struct Play {
    pair: Pair,
    random: Random,
    /// Where each player stands if every step so far was taken: from there they act.
    positions: BTreeMap<PlayerId, Vec3>,
    /// The number of the last action on a block; it goes up through every stay.
    sequence: i32,
    merges: usize,
    splits: usize,
    /// How many splits were off.
    off: usize,
}

impl Play {
    /// The run in a world whose store keeps the players' places, or does not.
    fn with(test: &str, seed: u64, areas: &[ChunkArea], on: bool) -> Self {
        let ticket = [Ticket::Viewer, Ticket::Guest][(seed % 2) as usize];
        let places = if on { "kept" } else { "not kept" };
        let name = format!(
            "{test}, seed {seed} (CLUSTINE_RESHAPE_SEED={seed}), {ticket:?}, places {places}"
        );
        Self {
            pair: Pair::with(name, areas, ticket, on),
            random: Random(0x2545_F491_4F6C_DD1D ^ (seed << 20) ^ seed),
            positions: BTreeMap::new(),
            sequence: 0,
            merges: 0,
            splits: 0,
            off: 0,
        }
    }

    /// The row of blocks player `n` walks along.
    fn row(n: u128) -> i32 {
        5 * (n as i32 - 1) + 2
    }

    /// Something player `n` does.
    fn input(&mut self, n: u128) -> PlayerInput {
        let id = player(n);
        let row = Self::row(n);
        let position = self.positions[&id];
        let random = &mut self.random;
        let kind = random.below(10);
        if kind < 4 {
            // Every other time to within a block and a half of a chunk border, from
            // where the blocks of both chunks are within reach.
            let x = if random.once_in(2) {
                let border = f64::from(random.below(6) as i32 - 2) * 16.0;
                border + random.below(30) as f64 / 10.0 - 1.5
            } else {
                random.below(1100) as f64 / 10.0 - 40.0
            };
            let z = f64::from(row) + 0.5;
            self.positions.insert(id, Vec3::new(x, 64.0, z));
            return walk(x, z);
        }
        if kind == 9 {
            let slot = random.below(3) as u8;
            if random.once_in(2) {
                return PlayerInput::SelectSlot { slot };
            }
            let stack = match random.below(4) {
                0 => None,
                1 => Some(ItemStack {
                    item: items::STONE,
                    count: 1 + random.below(64) as i32,
                }),
                _ => hotbar()[0],
            };
            return PlayerInput::SetHotbarSlot { slot, stack };
        }
        // A block of one of the two rows beside the player's, at the top of the stone
        // or on it, near where they stand.
        let x = position.x.floor() as i32 + random.below(5) as i32 - 2;
        let z = row + random.pick(&[-1, 1]);
        let target = BlockPos::new(x, 63 + random.below(2) as i32, z);
        self.sequence += 1;
        if kind < 7 {
            return dig(target, self.sequence);
        }
        let (against, face) = match random.below(3) {
            0 => (target.offset(0, -1, 0), Face::Top),
            1 => (target.offset(-1, 0, 0), Face::East),
            _ => (target.offset(1, 0, 0), Face::West),
        };
        assert_eq!(face.neighbour(against), target);
        use_on(against, face, self.sequence)
    }

    /// What the players do in one round.
    fn players(&mut self) {
        for n in 1..=6 {
            let id = player(n);
            let told = self.pair.cluster.stays.get(&id).map(|stay| stay.entity);
            match told {
                None => {
                    if self.random.once_in(8) {
                        let edge = if n % 2 == 0 { E } else { F };
                        self.pair.join(id, edge);
                        // At the spawn point; or, where the store keeps the places,
                        // where they were when they left, which is where every step
                        // they made took them: they leave when nothing of theirs
                        // waits.
                        if self.pair.cluster.on {
                            self.positions.entry(id).or_insert(ENTRANCE);
                        } else {
                            self.positions.insert(id, ENTRANCE);
                        }
                    }
                }
                // The edge passes on nothing of a player it has not told their entity.
                // Now and then such a player gives up: the leave names the attempt of
                // the join.
                Some(None) => {
                    if self.random.once_in(12) {
                        self.pair.leave(id);
                    }
                }
                Some(Some(_)) => {
                    // A leave in the tick of an input before it takes the input with it
                    // (a tick applies changes first), and the one region is given both
                    // a tick apart; so players leave when nothing of theirs waits.
                    if self.random.once_in(50) && self.pair.cluster.at_rest(id) {
                        self.pair.leave(id);
                        continue;
                    }
                    if self.random.once_in(2) {
                        for _ in 0..=self.random.below(2) {
                            let input = self.input(n);
                            let on_blocks = matches!(
                                input,
                                PlayerInput::Dig { .. } | PlayerInput::UseItemOn { .. }
                            );
                            // What a player does to a block across a boundary takes
                            // its two ticks, and more by way of a region that believes
                            // what is no longer so; what they do next to a block of
                            // the chunk they stand in takes none. So that the second
                            // never overtakes the first, a player acts on blocks when
                            // nothing of theirs is under way.
                            if on_blocks && !self.pair.cluster.unhurried(id) {
                                continue;
                            }
                            self.pair.act(id, input);
                        }
                    }
                }
            }
        }
    }

    /// Splits a region off one of the regions, around one of its players if it has
    /// any: the chunk they stand in, with a margin now and then, and now and then a
    /// chunk that has nothing to do with it.
    fn split(&mut self) {
        let live: Vec<RegionId> = self.pair.cluster.sites.keys().copied().collect();
        if live.len() >= 4 {
            return;
        }
        let region = self.random.pick(&live);
        let state = self.pair.cluster.sites[&region].region.state();
        let standing: Vec<ChunkPos> = state
            .players
            .values()
            .map(|player| chunk_of(player.pose.position))
            .collect();
        let mut named = Vec::new();
        if !standing.is_empty() {
            let chunk = self.random.pick(&standing);
            named.push(chunk);
            if self.random.once_in(2) {
                named.push(ChunkPos::new(chunk.x - 1, chunk.z));
                named.push(ChunkPos::new(chunk.x + 1, chunk.z));
            }
        }
        if self.random.once_in(4) {
            named.push(self.random.pick(&grid()));
        }
        match self.pair.split(region, &named) {
            Ok(_) => self.splits += 1,
            Err(_) => self.off += 1,
        }
    }

    /// Has one of the regions absorb another, which is not the home region.
    fn merge(&mut self) {
        let live: Vec<RegionId> = self.pair.cluster.sites.keys().copied().collect();
        let survivor = self.random.pick(&live);
        let home = self.pair.cluster.home;
        let others: Vec<RegionId> = live
            .into_iter()
            .filter(|region| *region != survivor && *region != home)
            .collect();
        if others.is_empty() {
            return;
        }
        let absorbed = self.random.pick(&others);
        self.pair.merge(survivor, absorbed);
        self.merges += 1;
    }

    /// Plays `rounds` rounds: in each the players do something, the regions are
    /// reshaped or not, with whatever is on its way, and every region ticks once. The
    /// worlds are compared after every tick at which nothing is on its way, and every
    /// forty rounds the run waits for that.
    fn run(&mut self, reshaping: Reshaping, rounds: usize) {
        for round in 0..rounds {
            self.players();
            match reshaping {
                Reshaping::MergeAt(at) => {
                    if round == at {
                        self.pair.merge(REGION_A, REGION_B);
                        self.merges += 1;
                    }
                }
                Reshaping::Splits => {
                    if self.random.once_in(25) {
                        self.split();
                    }
                }
                Reshaping::SplitAndAbsorb => {
                    if self.random.once_in(15) {
                        let live: Vec<RegionId> = self.pair.cluster.sites.keys().copied().collect();
                        match live.as_slice() {
                            [_] => self.split(),
                            [home, part] => {
                                self.pair.merge(*home, *part);
                                self.merges += 1;
                            }
                            _ => unreachable!("it is split only when it is one"),
                        }
                    }
                }
                Reshaping::Anything => {
                    if self.random.once_in(10) {
                        // Now and then two at once, so that a region is absorbed or
                        // split again before it has ticked: a part that no edge has
                        // said hello to, a survivor that has claimed nothing yet.
                        for _ in 0..=self.random.below(3) / 2 {
                            if self.random.once_in(2) {
                                self.split();
                            } else {
                                self.merge();
                            }
                        }
                    }
                }
            }
            self.pair.tick();
            if round % 40 == 39 {
                self.pair.settle();
            }
        }
        self.pair.settle();
    }
}

/// What several runs did, to see that they were about something.
#[derive(Debug, Default)]
struct Tally {
    merges: usize,
    splits: usize,
    off: usize,
    compared: usize,
    let_go: usize,
    passed_on: usize,
    sent_on: usize,
    ended: usize,
    dealt_with: usize,
    /// In the runs whose store keeps the places: how many joins were answered with a
    /// place, how many stays were found by their attempt, how many notes the store
    /// answered `Dead`, and how many `Ended` entries regions made.
    kept: usize,
    by_attempt: usize,
    dead: usize,
    told_ended: usize,
}

/// Every run of a test: each seed in a world whose store does not keep the players'
/// places, as before ADR-0020, and in one whose store does.
fn runs() -> Vec<(bool, u64)> {
    let both = |seed| [(false, seed), (true, seed)];
    seeds().into_iter().flat_map(both).collect()
}

impl Tally {
    fn note(&mut self, play: &Play) {
        let cluster = &play.pair.cluster;
        self.merges += play.merges;
        self.splits += play.splits;
        self.off += play.off;
        self.compared += play.pair.compared;
        self.let_go += cluster.let_go;
        self.passed_on += cluster.passed_on;
        self.sent_on += cluster.sent_on;
        self.ended += cluster.ended;
        self.dealt_with += play.sequence as usize;
        self.kept += cluster.places.kept;
        self.by_attempt += cluster.by_attempt;
        self.dead += cluster.places.dead;
        self.told_ended += cluster.told_ended;
    }
}

#[test]
fn two_regions_on_stripes_of_which_one_absorbs_the_other_show_what_one_region_shows() {
    // S35. The merge comes at some tick, with players and actions on their way between
    // the two and to either; the test gives the survivor the chunks and the area as
    // the store would, and its tickets again as its edges' hellos do.
    let mut tally = Tally::default();
    for (on, seed) in runs() {
        let test = "two regions on stripes that merge";
        let mut play = Play::with(test, seed, &[WESTERN, EASTERN], on);
        let at = 60 + (seed as usize * 7) % 50;
        play.run(Reshaping::MergeAt(at), 280);

        // One region is left, which holds the whole grid by its two areas, and the
        // worlds stay the same.
        let cluster = &play.pair.cluster;
        assert_eq!(cluster.sites.len(), 1);
        assert_eq!(cluster.held_by(REGION_A), grid());
        assert_eq!(cluster.grants.areas_of(REGION_A), [WESTERN, EASTERN]);
        tally.note(&play);
    }
    let runs = runs().len();
    assert_eq!(tally.merges, runs);
    assert!(tally.compared >= 60 * runs, "{tally:?}");
    assert!(
        tally.let_go >= 10 * runs && tally.passed_on >= 3 * runs,
        "{tally:?}"
    );
}

#[test]
fn a_region_that_is_split_and_run_as_two_or_more_shows_what_one_region_shows() {
    // S36: one region pinned to the whole world, of which regions are split off around
    // its players, and of those again.
    let mut tally = Tally::default();
    for (on, seed) in runs() {
        let test = "a region that is split";
        let mut play = Play::with(test, seed, &[ChunkArea::EVERYWHERE], on);
        play.run(Reshaping::Splits, 320);
        tally.note(&play);
    }
    let runs = runs().len();
    assert!(tally.splits >= 2 * runs, "{tally:?}");
    assert!(tally.compared >= 60 * runs, "{tally:?}");
    assert!(
        tally.let_go >= 10 * runs && tally.passed_on >= 10 * runs,
        "{tally:?}"
    );
}

#[test]
fn a_region_that_is_split_and_absorbs_its_part_again_is_as_one_that_never_was() {
    // S36, its second part: the players, the blocks and the held chunks.
    let mut tally = Tally::default();
    for (on, seed) in runs() {
        let test = "a region that absorbs its part again";
        let mut play = Play::with(test, seed, &[ChunkArea::EVERYWHERE], on);
        play.run(Reshaping::SplitAndAbsorb, 320);
        let parts: Vec<RegionId> = play.pair.cluster.sites.keys().copied().skip(1).collect();
        for part in parts {
            play.pair.merge(REGION_A, part);
        }
        play.pair.settle();

        let cluster = &play.pair.cluster;
        assert_eq!(cluster.sites.len(), 1);
        assert_eq!(cluster.held_by(REGION_A), grid());
        let never = &play.pair.one.region;
        let region = &cluster.sites[&REGION_A].region;
        for chunk in grid() {
            assert_eq!(never.knowledge(chunk), Knowledge::Held);
            assert_eq!(region.chunk(chunk), never.chunk(chunk));
        }
        assert_eq!(region.held_chunk_count(), never.held_chunk_count());
        assert_eq!(region.state().players.len(), never.state().players.len());
        assert_eq!(region.state().next_entity_id, never.state().next_entity_id);
        tally.note(&play);
    }
    let runs = runs().len();
    assert!(
        tally.splits >= 3 * runs && tally.merges >= 3 * runs,
        "{tally:?}"
    );
    assert!(tally.compared >= 60 * runs, "{tally:?}");
}

#[test]
fn regions_that_split_and_absorb_each_other_in_any_order_show_what_one_region_shows() {
    // Two regions on stripes or one that holds everything, and from there whatever the
    // generator comes to: parts of parts, a part absorbed by the neighbour of the
    // region it was split from, the home region absorbing everything.
    let mut tally = Tally::default();
    for (on, seed) in runs() {
        let test = "regions that merge and split";
        let both: &[ChunkArea] = &[WESTERN, EASTERN];
        let areas = if seed % 4 < 2 {
            both
        } else {
            &[ChunkArea::EVERYWHERE]
        };
        let mut play = Play::with(test, seed, areas, on);
        play.run(Reshaping::Anything, 480);
        tally.note(&play);
    }
    // With the places kept, players came back in place, and in a region that was not
    // the home region as well: let go to it without being placed, and found by the
    // attempt of their join.
    assert!(tally.kept >= seeds().len(), "{tally:?}");
    assert!(tally.by_attempt >= 1, "{tally:?}");
    let runs = runs().len();
    assert!(
        tally.splits >= 4 * runs && tally.merges >= 4 * runs,
        "{tally:?}"
    );
    assert!(tally.compared >= 60 * runs, "{tally:?}");
    assert!(
        tally.let_go >= 10 * runs && tally.passed_on >= 10 * runs,
        "{tally:?}"
    );
    assert!(tally.dealt_with >= 300 * runs, "{tally:?}");
}

// ---------------------------------------------------------------------------------------
// Section 2.2 from the split to the end of it
// ---------------------------------------------------------------------------------------

#[test]
fn a_chunk_that_came_back_to_a_pinned_region_without_a_word_takes_in_who_is_sent_there() {
    // Section 2.2 from the split to the end of it, with every answer of the store as
    // its table gives it: a part is split off a pinned region, gives a chunk back, and
    // the chunk is the pinned region's again, which nobody tells it.
    let (mut region, splitting, _, mut part) = taken(true);
    let mut grants = Grants {
        pinned: vec![(ChunkArea::EVERYWHERE, REGION_A)],
        granted: BTreeMap::new(),
        next: PART.0,
    };
    grants.split(REGION_A, PART, &splitting.chunks);
    assert_eq!(grants.holder(THIRD), Some(PART));

    // The region comes to believe that the part holds `THIRD`: player 3 sees it.
    let output = tick(&mut region, &add(&[viewer(THIRD)]));
    let (given, foreign) = grants.answer(REGION_A, &output.claims);
    assert_eq!((given, foreign.clone()), (vec![], vec![(THIRD, PART)]));
    tick(
        &mut region,
        &TickInputs {
            foreign,
            ..TickInputs::default()
        },
    );
    assert_eq!(region.knowledge(THIRD), Knowledge::Foreign(PART));

    // The part's only player walks back into the region.
    let output = tick(
        &mut part.region,
        &single_input(E, player(2), entity(2), 3, walk_to(middle(SECOND))),
    );
    let (_, foreign) = grants.answer(PART, &output.claims);
    let left = tick(
        &mut part.region,
        &TickInputs {
            foreign,
            ..TickInputs::default()
        },
    );
    let [
        (
            E,
            1,
            Durable::Departed {
                transfer,
                to: REGION_A,
                ..
            },
        ),
    ] = left.durable.as_slice()
    else {
        panic!("the part made {:?}", left.durable);
    };
    tick(&mut region, &arrive(E, player(2), transfer.clone()));
    assert!(region.player(player(2)).is_some());

    // Nothing uses the part's chunks any more, and when their time has come it gives
    // them back. By the store's table `THIRD` is the pinned region's from then on.
    let mut returned = Vec::new();
    for _ in 0..=LONG {
        let output = idle(&mut part.region);
        for chunk in &output.returns {
            assert_eq!(grants.granted.remove(chunk), Some(PART));
        }
        returned.extend(output.returns);
    }
    assert_eq!(returned, [THIRD, OUTPOST]);
    assert_eq!(part.region.held_chunk_count(), 0);
    assert_eq!(grants.holder(THIRD), Some(REGION_A));
    assert_eq!(
        region.knowledge(THIRD),
        Knowledge::Foreign(PART),
        "nobody told it"
    );

    // Player 3 steps into `THIRD`, and is let go to the part, as the region believes.
    let output = tick(
        &mut region,
        &single_input(F, player(3), entity(3), 2, walk_to(middle(THIRD))),
    );
    let [(F, 1, Durable::Departed { transfer, to, .. })] = output.durable.as_slice() else {
        panic!("the region made {:?}", output.durable);
    };
    assert_eq!(*to, PART);

    // The part knows nothing of the chunk: it takes them in and asks, and the store
    // says that the chunk is the region's.
    let mut inputs = arrive(F, player(3), transfer.clone());
    inputs.edges = vec![started(F, 10)];
    let output = tick(&mut part.region, &inputs);
    assert!(part.region.player(player(3)).is_some());
    let (given, foreign) = grants.answer(PART, &output.claims);
    assert_eq!((given, foreign.clone()), (vec![], vec![(THIRD, REGION_A)]));
    let output = tick(
        &mut part.region,
        &TickInputs {
            foreign,
            ..TickInputs::default()
        },
    );
    let [(F, 1, Durable::Departed { transfer, to, .. })] = output.durable.as_slice() else {
        panic!("the part made {:?}", output.durable);
    };
    assert_eq!(*to, REGION_A);

    // The region still believes the part to hold it. Outside its areas it would send
    // the player on again, and the two would pass them back and forth for ever; in its
    // own area it takes them in, drops the belief and asks, and is granted the chunk.
    let output = tick(&mut region, &arrive(F, player(3), transfer.clone()));
    assert!(output.durable.is_empty(), "{:?}", output.durable);
    assert_eq!(state_of(&region, player(3)), taken_in(transfer, F));
    assert_eq!(output.claims, [THIRD]);
    let (given, foreign) = grants.answer(REGION_A, &output.claims);
    assert_eq!((given.clone(), foreign), (vec![THIRD], vec![]));
    let output = tick(&mut region, &granted(&given));
    assert!(output.durable.is_empty());
    assert_eq!(output.chunk_requests, [THIRD]);
    assert_eq!(region.knowledge(THIRD), Knowledge::Held);
    assert_eq!(region.player_count(), 3);
}

// ---------------------------------------------------------------------------------------
// The same, in the cases section 2.1 gives for the stays
// ---------------------------------------------------------------------------------------

/// The player's stay as the region that has it says, and that region.
fn whereabouts(pair: &Pair, id: PlayerId) -> (RegionId, PlayerState) {
    let region = pair.cluster.region_of(id).expect("a region has the player");
    (region, state_of(&pair.cluster.sites[&region].region, id))
}

#[test]
fn a_player_on_their_way_to_the_absorbed_region_is_taken_in_by_the_survivor() {
    // Rule 48: an arrival that the survivor itself let go to the absorbed region before
    // the merge comes back to it and is taken in, with what the player did behind it.
    let name = "a player on their way at a merge".to_owned();
    let mut pair = Pair::new(name, &[WESTERN, EASTERN], Ticket::Viewer);
    pair.join(player(1), E);
    pair.settle();
    let (_, joined) = whereabouts(&pair, player(1));

    pair.act(player(1), walk(20.5, 7.5));
    pair.tick();
    assert_eq!(pair.cluster.region_of(player(1)), None, "they were let go");
    assert_eq!(pair.cluster.stays[&player(1)].site, REGION_B);
    pair.act(player(1), dig(BlockPos::new(20, 63, 8), 1));

    pair.merge(REGION_A, REGION_B);
    assert_eq!(pair.cluster.stays[&player(1)].site, REGION_A);
    pair.act(player(1), dig(BlockPos::new(21, 63, 8), 2));
    pair.settle();

    let (region, state) = whereabouts(&pair, player(1));
    assert_eq!(region, REGION_A);
    assert_eq!(
        (state.entity_id, state.pose.position.x, state.last_input),
        (joined.entity_id, 20.5, 3)
    );
    let survivor = &pair.cluster.sites[&REGION_A].region;
    assert_eq!(block(survivor, BlockPos::new(20, 63, 8)), Some(blocks::AIR));
    assert_eq!(block(survivor, BlockPos::new(21, 63, 8)), Some(blocks::AIR));
    assert_eq!(pair.cluster.let_go, 1);
}

#[test]
fn a_player_who_left_the_absorbed_region_and_joined_again_at_home_has_one_stay_after_the_merge() {
    // Section 2.1, the shortest case for the join: the leave waits for a region that
    // takes nothing any more; the player joins again at home, which is absorbing that
    // region, and the join is taken after the merge by a region that has the player
    // from it under the same edge. Behind the join comes the leave for the old stay.
    for join_first in [true, false] {
        let name = format!("a player who joins again at a merge, {join_first}");
        let mut pair = Pair::new(name, &[WESTERN, EASTERN], Ticket::Viewer);
        pair.join(player(1), E);
        pair.join(player(2), E);
        pair.settle();
        pair.act(player(1), walk(40.5, 7.5));
        pair.act(player(1), dig(BlockPos::new(40, 63, 8), 1));
        pair.settle();
        let (region, old) = whereabouts(&pair, player(1));
        assert_eq!(region, REGION_B);

        pair.leave(player(1));
        if join_first {
            pair.join(player(1), E);
            pair.merge(REGION_A, REGION_B);
        } else {
            pair.merge(REGION_A, REGION_B);
            pair.join(player(1), E);
        }
        // The merged region has the old stay until its next tick.
        let merged = &pair.cluster.sites[&REGION_A].region;
        assert_eq!(state_of(merged, player(1)), old);
        pair.settle();

        let (region, new) = whereabouts(&pair, player(1));
        assert_eq!(region, REGION_A);
        assert!(new.entity_id > old.entity_id);
        assert_eq!((new.pose.position, new.last_input), (ENTRANCE, 0));
        let merged = &pair.cluster.sites[&REGION_A].region;
        assert!(merged.entity(old.entity_id).is_none());
        assert_eq!(merged.player_count(), 2);

        // And the new stay goes on like any.
        pair.act(player(1), walk(12.5, 7.5));
        pair.settle();
        assert_eq!(whereabouts(&pair, player(1)).1.last_input, 1);
    }
}

#[test]
fn a_player_who_left_before_their_region_was_split_and_joined_again_has_one_stay() {
    // Section 2.1, the shortest cases for the arrival and the leave: a part is split
    // off with a player who has left since; the leave went to the split region, which
    // no longer has them; they join again and walk into the part. The part is told of
    // the old stay's end by its entity, before or after the new stay arrives there.
    let name = "a player who joins again at a split".to_owned();
    let mut pair = Pair::new(name, &[ChunkArea::EVERYWHERE], Ticket::Viewer);
    pair.join(player(1), E);
    pair.join(player(2), F);
    pair.settle();
    pair.act(player(1), walk(40.5, 7.5));
    pair.settle();
    let (_, old) = whereabouts(&pair, player(1));

    pair.leave(player(1));
    let part = pair
        .split(REGION_A, &[SECOND])
        .expect("player 1 is still there, and goes");
    let made = &pair.cluster.sites[&part].region;
    // As they were, and handed on once more by the split (ADR-0020, section 9).
    let old = PlayerState {
        hops: old.hops + 1,
        ..old
    };
    assert_eq!(state_of(made, player(1)), old);
    assert_eq!(pair.cluster.ended, 1, "the edge ends the stay the part has");

    pair.join(player(1), E);
    pair.settle();
    let (region, new) = whereabouts(&pair, player(1));
    assert_eq!(region, REGION_A);
    assert!(new.entity_id > old.entity_id);
    assert!(pair.cluster.sites[&part].region.player(player(1)).is_none());

    pair.act(player(1), walk(41.5, 7.5));
    pair.act(player(1), dig(BlockPos::new(41, 63, 8), 1));
    pair.settle();
    let (region, there) = whereabouts(&pair, player(1));
    assert_eq!(region, part);
    assert_eq!((there.entity_id, there.last_input), (new.entity_id, 2));
    let made = &pair.cluster.sites[&part].region;
    assert_eq!(block(made, BlockPos::new(41, 63, 8)), Some(blocks::AIR));
}

#[test]
fn a_third_region_that_still_names_the_absorbed_region_reaches_the_survivor() {
    // Rule 39: regions other than the survivor go on naming an absorbed region for as
    // long as they believe it, and the edge takes the name for the survivor's. Three
    // stripes; the middle one is absorbed by the western, and the eastern still
    // believes the middle one to hold the chunks next to its own.
    let middle = ChunkArea {
        min_x: Some(1),
        max_x: Some(3),
    };
    let eastern = ChunkArea {
        min_x: Some(3),
        max_x: None,
    };
    let name = "a third region at a merge".to_owned();
    let mut pair = Pair::new(name, &[WESTERN, middle, eastern], Ticket::Viewer);
    let third = RegionId(2);
    pair.join(player(1), E);
    pair.settle();
    pair.act(player(1), walk(49.5, 7.5));
    pair.settle();
    assert_eq!(pair.cluster.region_of(player(1)), Some(third));
    let believed = pair.cluster.sites[&third].region.knowledge(SECOND);
    assert_eq!(believed, Knowledge::Foreign(REGION_B));

    pair.merge(REGION_A, REGION_B);
    // A block of the absorbed region broken from the third, and then a step across.
    pair.act(player(1), dig(BlockPos::new(47, 63, 8), 1));
    pair.act(player(1), walk(46.5, 7.5));
    pair.act(player(1), dig(BlockPos::new(45, 63, 8), 2));
    pair.settle();

    let believed = pair.cluster.sites[&third].region.knowledge(SECOND);
    assert_eq!(believed, Knowledge::Foreign(REGION_B), "nothing told it");
    let (region, state) = whereabouts(&pair, player(1));
    assert_eq!(region, REGION_A);
    assert_eq!(state.last_input, 4);
    let survivor = &pair.cluster.sites[&REGION_A].region;
    assert_eq!(block(survivor, BlockPos::new(47, 63, 8)), Some(blocks::AIR));
    assert_eq!(block(survivor, BlockPos::new(45, 63, 8)), Some(blocks::AIR));
}

#[test]
fn the_same_run_twice_gives_byte_identical_states_and_identical_chunks() {
    // Everything is deterministic, the merges and splits among it: two runs with one
    // seed leave every region the same, to the byte of its serialised state.
    for (on, seed) in [(false, 1), (false, 2), (true, 1), (true, 2)] {
        let runs: Vec<Play> = (0..2)
            .map(|_| {
                let test = "the same run twice";
                let mut play = Play::with(test, seed, &[WESTERN, EASTERN], on);
                play.run(Reshaping::Anything, 200);
                play
            })
            .collect();
        let (one, other) = (&runs[0].pair.cluster, &runs[1].pair.cluster);
        assert_eq!(
            one.sites.keys().collect::<Vec<_>>(),
            other.sites.keys().collect::<Vec<_>>()
        );
        for (id, site) in &one.sites {
            let again = &other.sites[id];
            assert_eq!(bytes(&site.region.state()), bytes(&again.region.state()));
            assert_eq!(site.region, again.region, "{id}");
        }
        assert_eq!(runs[0].pair.steps, runs[1].pair.steps);
        assert!(runs[0].merges + runs[0].splits >= 5);
    }
}
