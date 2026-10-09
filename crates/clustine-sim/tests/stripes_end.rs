//! Tests of what section 3.6.1 of `docs/adr/0017-the-end-of-the-stripes.md` changes in
//! the simulation: the grants that wait when a split is worked out go by nearness.
//! Written from that section, from the scenarios S1 to S9 of its section 9.5a, from
//! section 2 of `docs/adr/0014-merging-and-splitting.md` and from the public API, by
//! someone who has not read how the region does it.
//!
//! The tests play the store and the edges, as those of `reshape.rs` do, and the helpers
//! that make regions, tick them and hold every tick to ADR-0008 are copied from there.
//! Two things are second implementations of what the record says, which the region is
//! held to: [`split_by_the_record`] is `Region::split` as section 3.6.1 has it, with
//! [`goes_by_the_record`] for `Sides::goes`; and [`Cluster`] is the cluster of
//! `reshape.rs`, an edge and a store around several regions that merge and split, in
//! which a region asks for chunks in the tick before it is split and the store's
//! grants wait when the split is worked out (S9).
//!
//! A grant waits when the store has granted a chunk and no tick has been told: here a
//! region ticks once with a viewer's ticket on a chunk it knows nothing of, which puts
//! the chunk among that tick's claims, and the chunk is handed to `split` and to
//! `take_split` without ever being among a tick's `granted`.
//!
//! The runs of the cluster are made up by a generator, five to a test. The environment
//! variable `CLUSTINE_RESHAPE_SEEDS` says how many instead, and `CLUSTINE_RESHAPE_SEED`
//! names the one to run; a run that fails prints its seed and its steps.
//!
//! No test here failed for something the region does otherwise than the record says:
//! there is no ignored test in this file.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks, items};
use clustine_sim::api::{Face, HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent};
use clustine_sim::{
    Durable, EdgeEvent, EdgeState, Holdings, Knowledge, Misdirected, NoSplit, PlayerChange,
    PlayerEvent, PlayerJoin, PlayerState, PlayerTransfer, Region, RegionConfig, RegionState, Sides,
    Splitting, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Section, Vec3,
};
use uuid::Uuid;

const E: EdgeId = EdgeId(1);
const F: EdgeId = EdgeId(2);

/// The chunk players enter the world in: the home chunk.
const HOME: ChunkPos = ChunkPos::new(0, 0);

/// Where `p` stands in the scenarios, 19 chunks east of the home chunk.
const P_AT: ChunkPos = ChunkPos::new(19, 0);

/// The region that is split, its neighbour on stripes, and the region a split makes.
const REGION_A: RegionId = RegionId(0);
const PART: RegionId = RegionId(12);

/// Where players enter the world: on the stone of `HOME`.
const SPAWN: Vec3 = Vec3::new(14.5, 64.0, 8.5);

/// The time before a return of the regions that are split by hand here: long, so that
/// a chunk nobody watches is still held when the split comes.
const LONG: u64 = 1000;

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

/// Viewers' subscriptions to `chunks` that begin.
fn looking_at(chunks: &[ChunkPos]) -> TickInputs {
    TickInputs {
        tickets_added: chunks.iter().copied().map(viewer).collect(),
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

/// Storage delivers the chunks, each a column of stone.
fn delivered(chunks: &[ChunkPos]) -> TickInputs {
    TickInputs {
        chunks_loaded: chunks.iter().map(|chunk| (*chunk, stone_chunk())).collect(),
        ..TickInputs::default()
    }
}

fn started(edge: EdgeId, start: u64) -> EdgeEvent {
    EdgeEvent::Started { edge, start }
}

fn join(edge: EdgeId, id: PlayerId) -> PlayerChange {
    PlayerChange::Join(
        edge,
        PlayerJoin {
            player: id,
            name: format!("player-{}", id.0.as_u128()),
        },
    )
}

fn leave(edge: EdgeId, id: PlayerId, entity: Option<EntityId>) -> PlayerChange {
    PlayerChange::Leave(edge, id, entity)
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
/// from 1.
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

fn state_of(region: &Region, id: PlayerId) -> PlayerState {
    region
        .player_state(id)
        .expect("the player is in the region")
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

/// The edges of [`with_players`], as the region knows them before the split: `E` has
/// an entry nobody has confirmed, so that what a split adds is numbered on.
fn edges_before_a_split() -> [(EdgeId, EdgeState); 2] {
    let refused = Durable::Refused { player: player(9) };
    [
        (E, edge_state(10, 3, 17, 4, &[(4, refused)])),
        (F, edge_state(11, 5, 6, 0, &[])),
    ]
}

/// A region as a restore makes it at tick 40 of a state with a player in the middle of
/// each chunk named: player `n` has `entity(n)`. Its home chunk is `HOME`, whether it
/// holds it or not, and it gives nothing back while a test lasts.
fn with_players(holdings: Holdings, players: &[(i32, EdgeId, ChunkPos)]) -> Region {
    let state = RegionState {
        tick: 40,
        entity_ids: ids(),
        next_entity_id: entity(20),
        players: players
            .iter()
            .map(|(n, edge, chunk)| (player(*n as u128), someone(entity(*n), *edge, *chunk)))
            .collect(),
        edges: edges_before_a_split().into_iter().collect(),
    };
    Region::restore(config(LONG), state, holdings)
}

/// The players of `state`, by the number [`with_players`] was given for each.
fn numbers_of(state: &RegionState) -> Vec<i32> {
    state
        .players
        .values()
        .map(|player| player.entity_id.0 - ids().first.0 + 1)
        .collect()
}

/// `chunks` in ascending order, each once.
fn sorted(chunks: impl IntoIterator<Item = ChunkPos>) -> Vec<ChunkPos> {
    let set: BTreeSet<ChunkPos> = chunks.into_iter().collect();
    set.into_iter().collect()
}

/// The seven by seven chunks around `centre`: what a viewer who stands there sees at a
/// view distance of 3.
fn around(centre: ChunkPos) -> Vec<ChunkPos> {
    (-3..=3)
        .flat_map(|x| (-3..=3).map(move |z| ChunkPos::new(centre.x + x, centre.z + z)))
        .collect()
}

/// The seven chunks `(x, -3)` to `(x, 3)`: the row that comes into the view of
/// somebody who walks east or west along z = 0.
fn row_at(x: i32) -> Vec<ChunkPos> {
    (-3..=3).map(|z| ChunkPos::new(x, z)).collect()
}

/// What the store says of a region on open land that holds `held`.
fn on_open_land(held: &[ChunkPos]) -> Holdings {
    Holdings {
        held: sorted(held.iter().copied()),
        pinned: Vec::new(),
    }
}

/// The chunks of `loaded`, in the order they are in.
fn positions(loaded: &[(ChunkPos, Chunk)]) -> Vec<ChunkPos> {
    loaded.iter().map(|(chunk, _)| *chunk).collect()
}

/// The region of S1: it holds the seven by seven chunks around `HOME` and around
/// `p_at`, all loaded for a viewer, with `s`, player 1, in `HOME` and `p`, player 2,
/// in `p_at`, both of `E`. In its last tick it has asked for `asked`, for a viewer,
/// and it has had no answer: what the store grants of them waits.
fn two_views(pinned: Vec<ChunkArea>, p_at: ChunkPos, asked: &[ChunkPos]) -> Region {
    let held = sorted(around(HOME).into_iter().chain(around(p_at)));
    let holdings = Holdings {
        held: held.clone(),
        pinned,
    };
    let mut region = with_players(holdings, &[(1, E, HOME), (2, E, p_at)]);
    let output = tick(&mut region, &looking_at(&held));
    assert_eq!(output.chunk_requests, held);
    assert!(output.claims.is_empty() && output.returns.is_empty());

    let mut inputs = delivered(&held);
    inputs.tickets_added = asked.iter().copied().map(viewer).collect();
    let output = tick(&mut region, &inputs);
    assert_eq!(output.claims, sorted(asked.iter().copied()));
    assert!(output.returns.is_empty() && output.durable.is_empty());
    assert_eq!(region.loaded_chunk_count(), held.len());
    assert_eq!(region.held_chunk_count(), held.len());
    for chunk in asked {
        assert_eq!(region.knowledge(*chunk), Knowledge::Asked, "{chunk:?}");
    }
    region
}

// ---------------------------------------------------------------------------------------
// The split as section 3.6.1 says it
// ---------------------------------------------------------------------------------------

/// How far two chunks are apart: in chunks along the longer of the two axes, worked
/// out in 64 bits.
fn apart(a: ChunkPos, b: ChunkPos) -> i64 {
    let along_x = (i64::from(a.x) - i64::from(b.x)).abs();
    let along_z = (i64::from(a.z) - i64::from(b.z)).abs();
    along_x.max(along_z)
}

/// `Sides::goes` as the record says it: nearer to a seed than to every chunk of
/// `staying`, a tie staying, and any chunk if nothing stays.
fn goes_by_the_record(seeds: &[ChunkPos], staying: &[ChunkPos], chunk: ChunkPos) -> bool {
    let nearest = |to: &[ChunkPos]| to.iter().map(|other| apart(chunk, *other)).min();
    match (nearest(seeds), nearest(staying)) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(to_seed), Some(to_stayer)) => to_seed < to_stayer,
    }
}

/// Section 3.6.1 of ADR-0017 over section 2.4 of ADR-0014, step for step: what `split`
/// gives for a region with `state` that holds `held` by what its ticks were told, has
/// been granted `granted` in answer to claims no tick was told of, is pinned to some
/// area or to none, and has its spawn point in `HOME`.
fn split_by_the_record(
    state: &RegionState,
    held: &[ChunkPos],
    granted: &[ChunkPos],
    pinned: bool,
    named: &[ChunkPos],
    part: RegionId,
) -> Result<Splitting, NoSplit> {
    // "The region holds" means held by its ticks or named in `granted`.
    let holds: BTreeSet<ChunkPos> = held.iter().chain(granted).copied().collect();
    let stands_in = |player: &PlayerState| chunk_of(player.pose.position);

    // 1. Seeds.
    let seeds: BTreeSet<ChunkPos> = named
        .iter()
        .copied()
        .filter(|chunk| holds.contains(chunk) && *chunk != HOME)
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

    // 2. Who goes, and where the stayers are.
    let goes = |player: &PlayerState| seeds.contains(&stands_in(player));
    let nobody_stays = state.players.values().all(goes);
    if nobody_stays && !holds.contains(&HOME) && !pinned {
        return Err(NoSplit::NothingStays);
    }
    let mut staying: BTreeSet<ChunkPos> = state
        .players
        .values()
        .filter(|player| !goes(player))
        .map(stands_in)
        .collect();
    if holds.contains(&HOME) {
        staying.insert(HOME);
    }
    let sides = Sides {
        seeds: seeds.iter().copied().collect(),
        staying: staying.iter().copied().collect(),
    };

    // 3. The chunks of the part: the rule there was, applied to the grants that wait.
    let chunks: Vec<ChunkPos> = holds
        .iter()
        .copied()
        .filter(|chunk| goes_by_the_record(&sides.seeds, &sides.staying, *chunk))
        .collect();

    // 4. The two states, as ADR-0014 has them.
    let tick = state.tick + 1;
    let mut ours = state.clone();
    ours.tick = tick;
    let mut theirs = RegionState::new(no_ids());
    theirs.tick = tick;
    let mut went: BTreeMap<EdgeId, Vec<(PlayerId, EntityId)>> = BTreeMap::new();
    for (id, player) in &state.players {
        if goes(player) {
            ours.players.remove(id);
            theirs.players.insert(*id, player.clone());
            went.entry(player.edge)
                .or_default()
                .push((*id, player.entity_id));
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
        sides,
    })
}

/// Holds a region that has taken `splitting` with `granted`, and the part, to section
/// 2.6 of ADR-0014 as section 3.6.1 leaves it: each is what `Region::restore` makes of
/// its state and of what the store grants it, which for the region is what it held
/// by its ticks (`held`) and `granted`, without the part's. `config` is what the
/// region was made with, which the part is restored with as well.
fn check_taken(
    region: &Region,
    part: &Region,
    splitting: &Splitting,
    held: &[ChunkPos],
    granted: &[ChunkPos],
    pinned: &[ChunkArea],
    config: &RegionConfig,
) {
    let stays: Vec<ChunkPos> = sorted(held.iter().chain(granted).copied())
        .into_iter()
        .filter(|chunk| !splitting.chunks.contains(chunk))
        .collect();
    let holdings = Holdings {
        held: stays.clone(),
        pinned: pinned.to_vec(),
    };
    let restored = Region::restore(config.clone(), splitting.state.clone(), holdings);
    assert_eq!(region, &restored, "the region after the split");
    let holdings = Holdings {
        held: splitting.chunks.clone(),
        pinned: Vec::new(),
    };
    let restored = Region::restore(config.clone(), splitting.part.clone(), holdings);
    assert_eq!(part, &restored, "the part");

    // No chunk is held by both, and none that either held or was granted is lost.
    for chunk in &splitting.chunks {
        assert_eq!(part.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(region.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
    }
    for chunk in &stays {
        assert_eq!(region.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(part.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
    }
    assert_eq!(region.held_chunk_count(), stays.len());
    assert_eq!(part.held_chunk_count(), splitting.chunks.len());
}

// ---------------------------------------------------------------------------------------
// S1 to S3: on which side a grant that waits lies
// ---------------------------------------------------------------------------------------

#[test]
fn grants_that_wait_on_the_parts_side_go_with_the_part() {
    // S1. The seven chunks at (23, ..) came into p's view in the last tick: the region
    // has asked for them, the store has granted them, and no tick has been told.
    let seven = row_at(23);
    let mut region = two_views(Vec::new(), P_AT, &seven);
    let before = region.clone();
    let held = sorted(around(HOME).into_iter().chain(around(P_AT)));

    let splitting = region.split(&[P_AT], PART, &seven).expect("p goes");
    assert_eq!(region, before, "`split` changed the region");
    let of_the_part = sorted(around(P_AT).into_iter().chain(seven.clone()));
    assert_eq!(of_the_part.len(), 56);
    assert_eq!(splitting.chunks, of_the_part);
    let sides = Sides {
        seeds: vec![P_AT],
        staying: vec![HOME],
    };
    assert_eq!(splitting.sides, sides);
    assert_eq!(numbers_of(&splitting.state), [1]);
    assert_eq!(numbers_of(&splitting.part), [2]);
    let by_the_record = split_by_the_record(&before.state(), &held, &seven, false, &[P_AT], PART);
    assert_eq!(Ok(splitting.clone()), by_the_record);

    // Taken with the same grants: the region knows nothing of any of the 56, the part
    // holds all of them, and each is what a restore makes of its state and holdings.
    let (kept, part) = region.take_split(splitting.clone(), &seven);
    for chunk in &of_the_part {
        assert_eq!(region.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
        assert_eq!(part.region.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
    }
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &seven,
        &[],
        &config(LONG),
    );
    assert_eq!(region.held_chunk_count(), 49);
    let restored = Region::restore(
        config(LONG),
        splitting.state.clone(),
        on_open_land(&around(HOME)),
    );
    assert_eq!(region, restored);
    let restored = Region::restore(
        config(LONG),
        splitting.part.clone(),
        on_open_land(&of_the_part),
    );
    assert_eq!(part.region, restored);

    // The chunks that were loaded are divided as the land is. The seven were never
    // loaded: whoever holds them reads them from the store.
    assert_eq!(positions(&kept), sorted(around(HOME)));
    assert_eq!(positions(&part.chunks), sorted(around(P_AT)));
    for (chunk, blocks) in kept.iter().chain(&part.chunks) {
        assert_eq!(before.chunk(*chunk), Some(blocks), "{chunk:?}");
    }
}

#[test]
fn after_such_a_split_the_region_claims_none_of_the_seven_and_the_part_serves_them() {
    // S1, on from it: what statement L of section 3.6 rests on in the simulation. The
    // part holds the row ahead of p, so a viewer's ticket for it makes the part ask
    // storage and nobody ask the store; the region, which has no ticket, asks nothing.
    let seven = row_at(23);
    let mut region = two_views(Vec::new(), P_AT, &seven);
    let splitting = region.split(&[P_AT], PART, &seven).expect("p goes");
    let (_, part) = region.take_split(splitting, &seven);
    let mut part = part.region;

    let output = tick(&mut part, &looking_at(&seven));
    assert!(output.claims.is_empty(), "{:?}", output.claims);
    assert_eq!(output.chunk_requests, seven);
    let output = tick(&mut region, &TickInputs::default());
    assert!(output.claims.is_empty(), "{:?}", output.claims);
    assert!(output.returns.is_empty(), "{:?}", output.returns);
}

#[test]
fn grants_that_wait_nearer_to_the_home_chunk_than_to_the_seed_stay() {
    // S2: (-4, 0) lies behind the home chunk, and (9, 0) is 9 from it and 10 from the
    // seed.
    let west = ChunkPos::new(-4, 0);
    let nine = ChunkPos::new(9, 0);
    let waiting = [west, nine];
    let mut region = two_views(Vec::new(), P_AT, &waiting);
    let before = region.clone();
    let held = sorted(around(HOME).into_iter().chain(around(P_AT)));

    let splitting = region.split(&[P_AT], PART, &waiting).expect("p goes");
    assert_eq!(region, before, "`split` changed the region");
    assert_eq!(splitting.chunks, sorted(around(P_AT)));
    assert!(!splitting.chunks.contains(&west) && !splitting.chunks.contains(&nine));
    assert!(!splitting.sides.goes(west) && !splitting.sides.goes(nine));
    let by_the_record = split_by_the_record(&before.state(), &held, &waiting, false, &[P_AT], PART);
    assert_eq!(Ok(splitting.clone()), by_the_record);

    let (_, part) = region.take_split(splitting.clone(), &waiting);
    for chunk in waiting {
        assert_eq!(region.knowledge(chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(
            part.region.knowledge(chunk),
            Knowledge::Unknown,
            "{chunk:?}"
        );
    }
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &waiting,
        &[],
        &config(LONG),
    );
    assert_eq!(region.held_chunk_count(), 51);
    assert_eq!(part.region.held_chunk_count(), 49);
}

#[test]
fn a_grant_that_waits_one_chunk_nearer_to_the_seed_than_to_the_home_chunk_goes() {
    // S2, its second half: (10, 0) is 10 from the home chunk and 9 from the seed.
    let ten = ChunkPos::new(10, 0);
    let mut region = two_views(Vec::new(), P_AT, &[ten]);
    let held = sorted(around(HOME).into_iter().chain(around(P_AT)));

    let splitting = region.split(&[P_AT], PART, &[ten]).expect("p goes");
    assert_eq!(
        splitting.chunks,
        sorted(around(P_AT).into_iter().chain([ten]))
    );
    assert!(splitting.sides.goes(ten));
    let (_, part) = region.take_split(splitting.clone(), &[ten]);
    assert_eq!(region.knowledge(ten), Knowledge::Unknown);
    assert_eq!(part.region.knowledge(ten), Knowledge::Held);
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &[ten],
        &[],
        &config(LONG),
    );
}

#[test]
fn a_grant_that_waits_as_near_to_the_seed_as_to_the_home_chunk_stays() {
    // S3: (10, 5) is ten chunks from (20, 0) and ten from (0, 0).
    let seed = ChunkPos::new(20, 0);
    let tie = ChunkPos::new(10, 5);
    let mut region = two_views(Vec::new(), seed, &[tie]);
    let held = sorted(around(HOME).into_iter().chain(around(seed)));

    let splitting = region.split(&[seed], PART, &[tie]).expect("p goes");
    let sides = Sides {
        seeds: vec![seed],
        staying: vec![HOME],
    };
    assert_eq!(splitting.sides, sides);
    assert!(!splitting.sides.goes(tie));
    assert_eq!(splitting.chunks, sorted(around(seed)));

    let (_, part) = region.take_split(splitting.clone(), &[tie]);
    assert_eq!(region.knowledge(tie), Knowledge::Held);
    assert_eq!(part.region.knowledge(tie), Knowledge::Unknown);
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &[tie],
        &[],
        &config(LONG),
    );
}

// ---------------------------------------------------------------------------------------
// S4: who stands in a chunk whose grant waits
// ---------------------------------------------------------------------------------------

/// A region that holds the seven by seven chunks around `HOME` and around `P_AT`, with
/// player 1 in `HOME` and the others where `others` says, each `(n, chunk)`. It has
/// ticked once, so that it has asked for every chunk somebody stands in that it does
/// not hold. Returns it with the chunks it holds by what its ticks were told.
fn with_somebody_ahead(others: &[(i32, ChunkPos)]) -> (Region, Vec<ChunkPos>) {
    let held = sorted(around(HOME).into_iter().chain(around(P_AT)));
    let mut players = vec![(1, E, HOME)];
    players.extend(others.iter().map(|(n, chunk)| (*n, E, *chunk)));
    let mut region = with_players(on_open_land(&held), &players);
    let output = tick(&mut region, &looking_at(&held));
    let ahead = others.iter().map(|(_, chunk)| *chunk);
    let asked = sorted(ahead.filter(|chunk| !held.contains(chunk)));
    assert_eq!(output.claims, asked);
    assert!(output.durable.is_empty(), "nobody is let go");
    (region, held)
}

#[test]
fn somebody_who_stands_in_a_named_chunk_whose_grant_waits_is_a_seed_and_goes() {
    // S4, and N15: p has walked into (23, 0), which the region asked for when p came
    // to stand there. The store has granted it; no tick has been told.
    let ahead = ChunkPos::new(23, 0);
    let (mut region, held) = with_somebody_ahead(&[(2, ahead)]);
    assert_eq!(region.knowledge(ahead), Knowledge::Asked);
    let before = region.clone();

    let splitting = region.split(&[ahead], PART, &[ahead]).expect("p goes");
    assert_eq!(region, before, "`split` changed the region");
    assert_eq!(numbers_of(&splitting.part), [2]);
    assert_eq!(numbers_of(&splitting.state), [1]);
    assert_eq!(splitting.sides.seeds, [ahead]);
    assert_eq!(splitting.sides.staying, [HOME]);
    assert!(splitting.chunks.contains(&ahead));
    // With it goes what the region held around (19, 0), which is nearer to p than to
    // the home chunk.
    assert_eq!(
        splitting.chunks,
        sorted(around(P_AT).into_iter().chain([ahead]))
    );
    let went = Durable::SplitOff {
        region: PART,
        players: vec![(player(2), entity(2))],
    };
    assert_eq!(splitting.state.edges[&E].outbox.get(&5), Some(&went));
    let by_the_record =
        split_by_the_record(&before.state(), &held, &[ahead], false, &[ahead], PART);
    assert_eq!(Ok(splitting.clone()), by_the_record);

    // The part holds the chunk its player stands in (section 3.6.5, the first of the
    // three), and so does not have to claim it.
    let (_, part) = region.take_split(splitting.clone(), &[ahead]);
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &[ahead],
        &[],
        &config(LONG),
    );
    let mut part = part.region;
    assert_eq!(part.knowledge(ahead), Knowledge::Held);
    assert_eq!(chunk_of(state_of(&part, player(2)).pose.position), ahead);
    let output = tick(&mut part, &TickInputs::default());
    assert!(output.claims.is_empty() && output.durable.is_empty());
}

#[test]
fn without_the_grant_somebody_who_stands_in_a_chunk_that_is_asked_for_is_no_seed() {
    // S4, its second half: the claim has not been granted, so the region does not hold
    // the chunk in either sense, and there is nobody to split off.
    let ahead = ChunkPos::new(23, 0);
    let (region, _) = with_somebody_ahead(&[(2, ahead)]);
    let before = region.clone();
    assert_eq!(region.split(&[ahead], PART, &[]), Err(NoSplit::Nobody));
    // Nor does a grant of another chunk make them one.
    let other = [ChunkPos::new(23, 1)];
    assert_eq!(region.split(&[ahead], PART, &other), Err(NoSplit::Nobody));
    assert_eq!(region, before);
}

#[test]
fn somebody_who_stays_in_a_chunk_whose_grant_waits_keeps_it_and_what_is_nearer_to_them() {
    // Section 3.6.1, items 2 and 3: where the stayers are goes by the same meaning of
    // "holds". Player 3 stands in (23, 0), whose grant waits, and is not named: they
    // stay, the chunk stays with them, and so does what is as near to them as to p.
    let ahead = ChunkPos::new(23, 0);
    let (mut region, held) = with_somebody_ahead(&[(2, P_AT), (3, ahead)]);
    let before = region.clone();

    let splitting = region.split(&[P_AT], PART, &[ahead]).expect("p goes");
    assert_eq!(numbers_of(&splitting.part), [2]);
    assert_eq!(numbers_of(&splitting.state), [1, 3]);
    assert_eq!(splitting.sides.seeds, [P_AT]);
    assert_eq!(splitting.sides.staying, [HOME, ahead]);
    assert!(!splitting.chunks.contains(&ahead));
    // (21, 0) is two chunks from either of them: a tie, which stays.
    assert!(!splitting.chunks.contains(&ChunkPos::new(21, 0)));
    assert!(splitting.chunks.contains(&ChunkPos::new(20, 0)));
    let by_the_record = split_by_the_record(&before.state(), &held, &[ahead], false, &[P_AT], PART);
    assert_eq!(Ok(splitting.clone()), by_the_record);

    let (_, part) = region.take_split(splitting.clone(), &[ahead]);
    assert_eq!(region.knowledge(ahead), Knowledge::Held);
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &[ahead],
        &[],
        &config(LONG),
    );
    // Nobody who stays is let go by the next tick: each stands in a chunk the region
    // holds.
    let output = tick(&mut region, &TickInputs::default());
    assert!(output.claims.is_empty() && output.durable.is_empty());
    assert_eq!(region.player_count(), 2);
}

// ---------------------------------------------------------------------------------------
// S5 and S6: a plan changes nothing, and neither does a grant of what is held
// ---------------------------------------------------------------------------------------

#[test]
fn a_region_that_plans_with_grants_that_wait_and_does_not_take_is_the_region_it_was() {
    // S5.
    let seven = row_at(23);
    let mut region = two_views(Vec::new(), P_AT, &seven);
    let mut never = region.clone();
    let state = bytes(&region.state());

    let plan = region.split(&[P_AT], PART, &seven);
    assert!(plan.is_ok());
    assert_eq!(region.split(&[P_AT], PART, &seven), plan);
    // A plan that is off as well.
    assert_eq!(region.split(&[HOME], PART, &seven), Err(NoSplit::Nobody));
    assert_eq!(region, never, "`split` changed the region");
    assert_eq!(bytes(&region.state()), state);
    for chunk in &seven {
        assert_eq!(region.knowledge(*chunk), Knowledge::Asked, "{chunk:?}");
    }

    // Its next tick, with those grants among its inputs, is the next tick of a region
    // that never planned, and so are the ticks after it.
    let p = state_of(&region, player(2));
    let s = state_of(&region, player(1));
    let mut first = granted(&seven);
    let step = walk_to(middle(ChunkPos::new(20, 0)));
    first.input(E, player(2), p.entity_id, p.last_input + 1, step);
    let after = [
        first,
        delivered(&seven),
        single_input(
            E,
            player(1),
            s.entity_id,
            s.last_input + 1,
            dig(BlockPos::new(9, 63, 8), 31),
        ),
        TickInputs::default(),
    ];
    for (index, inputs) in after.iter().enumerate() {
        let output = tick(&mut region, inputs);
        assert_eq!(output, tick(&mut never, inputs), "tick {index} after");
        assert_eq!(region, never, "tick {index} after");
        if index == 0 {
            // The grants are taken as ever: held, and asked of storage for the viewer.
            assert_eq!(output.chunk_requests, seven);
            assert!(output.claims.is_empty());
        }
        if index == 2 {
            assert_eq!(block_changes(&output).len(), 1);
        }
    }
    for chunk in &seven {
        assert_eq!(region.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert!(region.chunk(*chunk).is_some(), "{chunk:?} is loaded");
    }
    assert_eq!(
        chunk_of(state_of(&region, player(2)).pose.position),
        ChunkPos::new(20, 0)
    );
}

#[test]
fn a_chunk_among_the_grants_that_the_region_holds_already_changes_nothing() {
    // S6.
    let seven = row_at(23);
    let region = two_views(Vec::new(), P_AT, &seven);
    let plain = region.split(&[P_AT], PART, &seven);
    assert!(plain.is_ok());

    // One that goes, one that stays, both, and each twice.
    let goes = ChunkPos::new(19, 1);
    let stays = ChunkPos::new(0, 2);
    for more in [vec![goes], vec![stays], vec![goes, stays, goes, stays]] {
        let mut with_held = seven.clone();
        with_held.extend(&more);
        assert_eq!(region.split(&[P_AT], PART, &with_held), plain, "{more:?}");
        assert_eq!(
            region.split(&[P_AT], PART, &more),
            region.split(&[P_AT], PART, &[]),
            "{more:?} alone"
        );
    }
    // The seed itself, and the home chunk.
    let mut with_held = seven.clone();
    with_held.push(P_AT);
    assert_eq!(region.split(&[P_AT], PART, &with_held), plain, "the seed");
    let mut with_held = seven.clone();
    with_held.push(HOME);
    assert_eq!(
        region.split(&[P_AT], PART, &with_held),
        plain,
        "the home chunk"
    );

    // Taking it is the same as well: the region holds a chunk once.
    let splitting = plain.expect("p goes");
    let mut with_held = seven.clone();
    with_held.extend([goes, stays]);
    let (mut one, mut other) = (region.clone(), region.clone());
    let (kept, part) = one.take_split(splitting.clone(), &seven);
    let (kept_too, part_too) = other.take_split(splitting, &with_held);
    assert_eq!(one, other);
    assert_eq!(part.region, part_too.region);
    assert_eq!(kept, kept_too);
    assert_eq!(part.chunks, part_too.chunks);
}

// ---------------------------------------------------------------------------------------
// S7: a chunk of the region's own pinned area whose grant waits
// ---------------------------------------------------------------------------------------

/// The chunks west of x = 30, which the region of S7 is pinned to.
const WEST_OF_THIRTY: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(30),
};

#[test]
fn a_chunk_of_the_regions_own_area_whose_grant_waits_goes_and_the_region_stays_pinned() {
    // S7. The seven at (23, ..) are of the area and nearer to the seed; (-4, 0) is of
    // the area and nearer to the home chunk.
    let seven = row_at(23);
    let west = ChunkPos::new(-4, 0);
    let mut waiting = seven.clone();
    waiting.push(west);
    let mut region = two_views(vec![WEST_OF_THIRTY], P_AT, &waiting);
    let before = region.clone();
    let held = sorted(around(HOME).into_iter().chain(around(P_AT)));
    assert!(seven.iter().all(|chunk| region.pins(*chunk)));

    let splitting = region.split(&[P_AT], PART, &waiting).expect("p goes");
    assert_eq!(region, before, "`split` changed the region");
    assert_eq!(
        splitting.chunks,
        sorted(around(P_AT).into_iter().chain(seven.clone()))
    );
    let by_the_record = split_by_the_record(&before.state(), &held, &waiting, true, &[P_AT], PART);
    assert_eq!(Ok(splitting.clone()), by_the_record);

    let (_, part) = region.take_split(splitting.clone(), &waiting);
    let area = [WEST_OF_THIRTY];
    check_taken(
        &region,
        &part.region,
        &splitting,
        &held,
        &waiting,
        &area,
        &config(LONG),
    );
    assert_eq!(region.knowledge(west), Knowledge::Held);
    for chunk in &seven {
        assert_eq!(region.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
        assert!(
            region.pins(*chunk),
            "{chunk:?} is of the region's area still"
        );
        assert_eq!(part.region.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert!(!part.region.pins(*chunk), "the part is pinned to nothing");
    }

    // Pinned as before: a guest's ticket makes the region ask for a chunk of its area,
    // one that went among them, and for none beyond it (ADR-0012, section 1.2).
    let gone = ChunkPos::new(23, 0);
    let beyond = ChunkPos::new(31, 0);
    let inputs = TickInputs {
        tickets_added: vec![guest(gone), guest(beyond)],
        ..TickInputs::default()
    };
    let output = tick(&mut region, &inputs);
    assert_eq!(output.claims, [gone]);
}

// ---------------------------------------------------------------------------------------
// S8: `Sides::goes`
// ---------------------------------------------------------------------------------------

#[test]
fn a_seed_is_on_the_parts_side_and_no_chunk_where_somebody_stays_is() {
    // S8.
    let sides = Sides {
        seeds: vec![ChunkPos::new(-7, 12), ChunkPos::new(19, 0)],
        staying: vec![HOME, ChunkPos::new(3, -9), ChunkPos::new(19, 4)],
    };
    for seed in &sides.seeds {
        assert!(sides.goes(*seed), "{seed:?}");
    }
    for stays in &sides.staying {
        assert!(!sides.goes(*stays), "{stays:?}");
    }
    // One chunk nearer to a seed, and one nearer to where somebody stays.
    assert!(sides.goes(ChunkPos::new(19, 1)));
    assert!(!sides.goes(ChunkPos::new(19, 3)));
    // Along the longer axis: (9, 9) is nine from the home chunk and ten from (19, 0).
    assert!(!sides.goes(ChunkPos::new(9, 9)));
    // And (13, -3) is six from (19, 0) and seven from (19, 4), the nearest who stays.
    assert!(sides.goes(ChunkPos::new(13, -3)));
}

#[test]
fn a_chunk_as_near_to_a_seed_as_to_somebody_who_stays_is_not_on_the_parts_side() {
    // S8: a tie stays, along either axis and across them.
    let sides = Sides {
        seeds: vec![ChunkPos::new(20, 0)],
        staying: vec![HOME],
    };
    for tie in [
        ChunkPos::new(10, 0),
        ChunkPos::new(10, 5),
        ChunkPos::new(10, -10),
        ChunkPos::new(10, 10),
        ChunkPos::new(-30, 50),
        ChunkPos::new(50, 50),
    ] {
        assert_eq!(apart(tie, sides.seeds[0]), apart(tie, HOME), "{tie:?}");
        assert!(!sides.goes(tie), "{tie:?}");
    }
    assert!(sides.goes(ChunkPos::new(11, 5)));
    assert!(!sides.goes(ChunkPos::new(9, 5)));
    // As near to the nearest seed as to the nearest of those who stay, among several.
    let sides = Sides {
        seeds: vec![ChunkPos::new(20, 0), ChunkPos::new(40, 0)],
        staying: vec![HOME, ChunkPos::new(60, 0)],
    };
    assert!(!sides.goes(ChunkPos::new(10, 0)));
    assert!(!sides.goes(ChunkPos::new(50, 0)));
    assert!(sides.goes(ChunkPos::new(30, 0)));
}

#[test]
fn every_chunk_is_on_the_parts_side_if_nothing_stays() {
    // S8.
    let sides = Sides {
        seeds: vec![ChunkPos::new(19, 0)],
        staying: Vec::new(),
    };
    for chunk in [
        HOME,
        ChunkPos::new(19, 0),
        ChunkPos::new(-1000, 77),
        ChunkPos::new(i32::MIN, i32::MAX),
        ChunkPos::new(i32::MAX, i32::MIN),
    ] {
        assert!(sides.goes(chunk), "{chunk:?}");
    }
}

#[test]
fn the_sides_are_told_apart_at_the_ends_of_what_a_coordinate_can_be() {
    // S8. Two chunks can be further apart than 32 bits can say, along either axis.
    let (low, high) = (i32::MIN, i32::MAX);
    for flip in [false, true] {
        let at = |x: i32, z: i32| match flip {
            false => ChunkPos::new(x, z),
            true => ChunkPos::new(z, x),
        };
        // The seed at the eastern end, somebody staying at the western: the two are
        // 2^32 - 1 apart. (0, 0) is 2^31 - 1 from the seed and 2^31 from the stayer.
        let sides = Sides {
            seeds: vec![at(high, 0)],
            staying: vec![at(low, 0)],
        };
        assert!(sides.goes(at(high, 0)), "the seed, {flip}");
        assert!(!sides.goes(at(low, 0)), "the stayer, {flip}");
        assert!(sides.goes(at(0, 0)), "{flip}");
        assert!(!sides.goes(at(-1, 0)), "{flip}");
        assert!(sides.goes(at(high - 1, 5)), "{flip}");
        assert!(!sides.goes(at(low + 1, -5)), "{flip}");
        // Beside the seed along the other axis, at its far ends: the other axis is the
        // longer one for neither.
        assert!(sides.goes(at(high, high)), "{flip}");
        assert!(!sides.goes(at(low, low)), "{flip}");

        // A tie that 32 bits would not find: each is 2^31 - 1 from (0, 0).
        let sides = Sides {
            seeds: vec![at(high, 0)],
            staying: vec![at(low + 1, 0)],
        };
        assert!(!sides.goes(at(0, 0)), "the tie, {flip}");
        assert!(sides.goes(at(1, 0)), "{flip}");
        assert!(!sides.goes(at(-1, 0)), "{flip}");

        // The other way round: the seed at the western end.
        let sides = Sides {
            seeds: vec![at(low, low)],
            staying: vec![at(high, high)],
        };
        assert!(sides.goes(at(low, low)), "{flip}");
        assert!(!sides.goes(at(high, high)), "{flip}");
        assert!(sides.goes(at(-1, -1)), "{flip}");
        assert!(!sides.goes(at(0, 0)), "{flip}");
        // The corners in between are as far from the one as from the other.
        assert!(!sides.goes(at(low, high)), "{flip}");
        assert!(!sides.goes(at(high, low)), "{flip}");
    }
}

#[test]
fn made_up_sides_answer_as_the_record_says_near_and_far() {
    // S8, against the rule written out: several seeds and several who stay, on a grid
    // around them and at coordinates near the ends.
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let ends = [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX];
    for _ in 0..200 {
        let far = random.once_in(3);
        let somewhere = |random: &mut Random| {
            if far {
                ChunkPos::new(random.pick(&ends), random.pick(&ends))
            } else {
                ChunkPos::new(random.below(13) as i32 - 6, random.below(13) as i32 - 6)
            }
        };
        let seeds: Vec<ChunkPos> = (0..=random.below(3))
            .map(|_| somewhere(&mut random))
            .collect();
        let staying: Vec<ChunkPos> = (0..random.below(4))
            .map(|_| somewhere(&mut random))
            .filter(|chunk| !seeds.contains(chunk))
            .collect();
        let sides = Sides {
            seeds: sorted(seeds),
            staying: sorted(staying),
        };
        for _ in 0..60 {
            let chunk = somewhere(&mut random);
            assert_eq!(
                sides.goes(chunk),
                goes_by_the_record(&sides.seeds, &sides.staying, chunk),
                "{chunk:?} with {sides:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Section 3.6.1 as a whole, on made-up regions
// ---------------------------------------------------------------------------------------

#[test]
fn splitting_made_up_regions_with_grants_that_wait_gives_what_the_record_says() {
    // Regions on a small patch of land with up to six players, of whom some stand in
    // chunks the region does not hold. Each ticks once, asking for what its players
    // stand in and for what a viewer has come to see; the store grants some of that,
    // and the split is worked out before a tick is told.
    let mut random = Random(0x0DDB_1A5E_5BAD_5EED);
    let mut splits = 0;
    let mut waited = 0;
    let mut waited_and_went = 0;
    let mut seeds_by_a_grant = 0;
    for round in 0..400 {
        let somewhere = |random: &mut Random| {
            ChunkPos::new(random.below(9) as i32 - 3, random.below(5) as i32 - 2)
        };
        let pinned = random.once_in(3);
        let mut held: BTreeSet<ChunkPos> = (0..random.below(24))
            .map(|_| somewhere(&mut random))
            .collect();
        if !random.once_in(4) {
            held.insert(HOME);
        }
        let held: Vec<ChunkPos> = held.into_iter().collect();
        let players: Vec<(i32, EdgeId, ChunkPos)> = (1..=random.below(7) as i32)
            .map(|n| (n, random.pick(&[E, F]), somewhere(&mut random)))
            .collect();
        let areas = if pinned {
            vec![ChunkArea::EVERYWHERE]
        } else {
            Vec::new()
        };
        let holdings = Holdings {
            held: held.clone(),
            pinned: areas.clone(),
        };
        let mut region = with_players(holdings, &players);

        let seen: Vec<ChunkPos> = (0..random.below(8))
            .map(|_| somewhere(&mut random))
            .collect();
        let seen = sorted(seen);
        let output = tick(&mut region, &looking_at(&seen));
        assert!(output.returns.is_empty() && output.durable.is_empty());
        let stood_in = players.iter().map(|(_, _, chunk)| *chunk);
        let asked = sorted(
            seen.iter()
                .copied()
                .chain(stood_in)
                .filter(|chunk| !held.contains(chunk)),
        );
        assert_eq!(output.claims, asked, "round {round}");

        // What the store granted of it, with a chunk the region holds now and then.
        let mut waiting: Vec<ChunkPos> = asked
            .iter()
            .copied()
            .filter(|_| !random.once_in(3))
            .collect();
        if !held.is_empty() && random.once_in(4) {
            waiting.push(random.pick(&held));
        }
        // Said twice now and then, and in no order: each counts once.
        if !waiting.is_empty() && random.once_in(5) {
            waiting.push(waiting[0]);
        }
        if random.once_in(2) {
            waiting.reverse();
        }
        let mut named: Vec<ChunkPos> = Vec::new();
        for (_, _, chunk) in &players {
            if random.once_in(2) {
                named.push(*chunk);
            }
        }
        if random.once_in(3) {
            named.push(somewhere(&mut random));
        }

        let before = region.clone();
        let split = region.split(&named, PART, &waiting);
        assert_eq!(region, before, "`split` changed the region, round {round}");
        let by_the_record =
            split_by_the_record(&before.state(), &held, &waiting, pinned, &named, PART);
        assert_eq!(split, by_the_record, "round {round}");
        let Ok(splitting) = split else { continue };
        splits += 1;
        let new: Vec<ChunkPos> = waiting
            .iter()
            .copied()
            .filter(|chunk| !held.contains(chunk))
            .collect();
        waited += new.len();
        let went = |chunk: &&ChunkPos| splitting.chunks.contains(chunk);
        waited_and_went += new.iter().filter(went).count();
        let by_a_grant = |seed: &&ChunkPos| !held.contains(seed);
        seeds_by_a_grant += splitting.sides.seeds.iter().filter(by_a_grant).count();

        // Every seed is a chunk of the part (section 3.6.5, the first of the three),
        // and every chunk of the part is on its side.
        for seed in &splitting.sides.seeds {
            assert!(splitting.chunks.contains(seed), "{seed:?}, round {round}");
        }
        for chunk in &splitting.chunks {
            assert!(splitting.sides.goes(*chunk), "{chunk:?}, round {round}");
        }
        assert!(splitting.chunks.is_sorted());

        let (kept, part) = region.take_split(splitting.clone(), &waiting);
        check_taken(
            &region,
            &part.region,
            &splitting,
            &held,
            &waiting,
            &areas,
            &config(LONG),
        );
        assert!(
            kept.is_empty() && part.chunks.is_empty(),
            "nothing was loaded"
        );
        // Whoever went stands in a chunk the part holds.
        for player in splitting.part.players.values() {
            let stands_in = chunk_of(player.pose.position);
            assert_eq!(part.region.knowledge(stands_in), Knowledge::Held);
        }
    }
    // The rounds were about something.
    assert!(splits >= 100, "{splits} splits");
    assert!(waited >= 200, "{waited} grants waited");
    assert!(waited_and_went >= 50, "{waited_and_went} of them went");
    assert!(
        seeds_by_a_grant >= 20,
        "{seeds_by_a_grant} seeds by a grant"
    );
}

// ---------------------------------------------------------------------------------------
// S9: against one region, with a claim made in the tick before each split
// ---------------------------------------------------------------------------------------

/// The chunks the runs are about: two rows of eight. On stripes the line runs between
/// x = 0 and x = 1, through the middle.
fn grid() -> Vec<ChunkPos> {
    (-3..=4)
        .flat_map(|x| [ChunkPos::new(x, 0), ChunkPos::new(x, 1)])
        .collect()
}

/// The chunks beside the grid that a region asks for in the tick before it is split,
/// and that nobody walks into. Those in the west are of the area the home region is
/// pinned to; those in the east are beyond every area, and nobody's until a region
/// claims them.
fn outskirts() -> Vec<ChunkPos> {
    [-5, -4, 5, 6]
        .into_iter()
        .flat_map(|x| [ChunkPos::new(x, 0), ChunkPos::new(x, 1)])
        .collect()
}

/// The land the regions of the runs are pinned to between them: everything west of
/// x = 5, as one area or as two stripes with the line at x = 1.
const WHOLE: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(5),
};
const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: Some(5),
};

/// Where players enter the world in the runs: in `HOME`, in a row of blocks that nobody
/// builds in, so that a block is never placed where someone stands.
const ENTRANCE: Vec3 = Vec3::new(14.5, 64.0, 7.5);

/// The start every edge of the runs has.
const START: u64 = 10;

fn run_config() -> RegionConfig {
    RegionConfig {
        spawn: ENTRANCE,
        starting_hotbar: hotbar(),
        return_after: 0,
    }
}

/// The world store as far as chunks go (ADR-0011, sections 1.3, 2 and 3): who holds a
/// chunk is the region it is granted to, else the region pinned to an area with it,
/// else nobody.
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
    /// the order of the claims. A chunk that is nobody's is the region's from here on,
    /// by a grant that is made before it is answered.
    fn answer(
        &mut self,
        region: RegionId,
        claims: &[ChunkPos],
    ) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
        let mut granted = Vec::new();
        let mut foreign = Vec::new();
        for chunk in claims {
            match self.holder(*chunk) {
                None => {
                    self.granted.insert(*chunk, region);
                    granted.push(*chunk);
                }
                Some(holder) if holder == region => granted.push(*chunk),
                Some(holder) => foreign.push((*chunk, holder)),
            }
        }
        (granted, foreign)
    }

    /// A return: the chunks are nobody's again, or the region's that is pinned to an
    /// area with them.
    fn give_back(&mut self, region: RegionId, chunks: &[ChunkPos]) {
        for chunk in chunks {
            assert_eq!(self.granted.remove(chunk), Some(region), "{chunk:?}");
        }
    }

    /// `AbsorbCommit`: the absorbed region's grants are the survivor's, also those
    /// whose answer it never took, and its areas are appended to the survivor's.
    /// Returns the grants that moved and the areas, as the store's answer names them.
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
    /// that is split held them by grant, one whose answer waits among them, or by
    /// being pinned (section 3.6.1, "The store takes it as it is").
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
    /// (ADR-0012, section 4.5).
    holding: bool,
    /// Whether the region has a viewer's ticket on each of the [`outskirts`].
    looking_out: bool,
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

    /// Whether nothing waits for the region.
    fn idle(&self) -> bool {
        let next = &self.next;
        !self.holding
            && next.player_changes.is_empty()
            && next.inputs.is_empty()
            && next.remote_actions.is_empty()
            && next.granted.is_empty()
            && next.foreign.is_empty()
            && next.chunks_loaded.is_empty()
    }
}

/// A player's stay as the edge has it.
#[derive(Debug, Clone)]
struct Stay {
    edge: EdgeId,
    /// The entity, once the home region has said it.
    entity: Option<EntityId>,
    /// The region the edge takes the stay to be in.
    site: RegionId,
    /// Everything the player did in this stay, numbered from 1, to send again after a
    /// hand-over.
    made: Vec<PlayerInput>,
}

/// What the splits of a run were handed of grants that waited.
#[derive(Debug, Default, Clone, Copy)]
struct Waited {
    /// How many splits were worked out with at least one grant that no tick had been
    /// told of.
    splits: usize,
    /// How many such grants went with the part, and how many stayed.
    went: usize,
    stayed: usize,
}

/// The cluster of `reshape.rs`: regions with one store and the edges `E` and `F`
/// between them, which the cluster plays. It answers each tick's claims into the next
/// tick, delivers what is asked of storage as the regions left it, handles every
/// outbox entry as section 5 of ADR-0012 has an edge do, and merges and splits
/// regions, doing for each what section 8 of ADR-0014 has the runner, the store and an
/// edge do, as far as a test that reads every entry in the tick that makes it needs to.
///
/// What is new here: a region is given a viewer's ticket on each of the [`outskirts`]
/// and ticks once before it is split, so that the store's answers to that tick's
/// claims wait when the split is worked out, and the split is handed the grants among
/// them (ADR-0017, section 3.6.1). The world has land that is nobody's, and a region
/// gives back what nothing uses of what lies outside its areas.
///
/// Every region has a ticket of one kind on every chunk of [`grid`], so that whoever
/// holds a chunk of it has it loaded.
struct Cluster {
    grants: Grants,
    sites: BTreeMap<RegionId, Site>,
    /// The region players enter the world in.
    home: RegionId,
    /// The regions that were absorbed, each with the region it went into.
    absorbed: BTreeMap<RegionId, RegionId>,
    /// What the store has of each chunk that a region had loaded when it began anew or
    /// was absorbed: nothing was unsaved then.
    storage: BTreeMap<ChunkPos, Chunk>,
    /// The kind of ticket every region has on every chunk of the grid.
    ticket: Ticket,
    stays: BTreeMap<PlayerId, Stay>,
    /// The highest sequence number of each player's actions on blocks that a region
    /// reported as dealt with.
    dealt_with: BTreeMap<PlayerId, i32>,
    /// The actions that were passed on and have not been reported as dealt with.
    under_way: BTreeSet<(PlayerId, i32)>,
    /// How many players were let go and actions passed on.
    let_go: usize,
    passed_on: usize,
    waited: Waited,
}

impl Cluster {
    /// Regions pinned to `areas`, numbered in their order, of which region 0 is the
    /// home region, with a ticket of the kind `ticket` on every chunk of the grid.
    fn new(areas: &[ChunkArea], ticket: Ticket) -> Self {
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
                region: Region::new(run_config(), entity_ids, holdings),
                next: TickInputs::default(),
                holding: false,
                looking_out: false,
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
            waited: Waited::default(),
        };
        cluster.settle();
        cluster
    }

    /// The edges say hello to a region that has begun anew: they are there, and every
    /// subscription begins anew, to the grid alone. What the store and storage were
    /// about to tell the region is not told it: it asks again.
    fn hello(site: &mut Site, ticket: Ticket) {
        let next = &mut site.next;
        next.edges.push(started(E, START));
        next.edges.push(started(F, START));
        next.tickets_added = grid().into_iter().map(|chunk| (chunk, ticket)).collect();
        next.tickets_removed.clear();
        next.granted.clear();
        next.foreign.clear();
        next.chunks_loaded.clear();
        site.holding = true;
        site.looking_out = false;
    }

    /// An edge's viewer comes to see the [`outskirts`] through `region`, or, if it
    /// saw them already, looks away: the tick that takes it claims those of them that
    /// the region knows nothing of, or lets go of what it knew of them.
    fn look_out(&mut self, region: RegionId) {
        let site = self.site(region);
        let tickets = outskirts().into_iter().map(viewer);
        if site.looking_out {
            site.next.tickets_removed.extend(tickets);
        } else {
            site.next.tickets_added.extend(tickets);
        }
        site.looking_out = !site.looking_out;
    }

    /// The living region that `region` is or went into.
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

    /// A player enters the world, in the home region.
    fn join(&mut self, id: PlayerId, edge: EdgeId) {
        let stay = Stay {
            edge,
            entity: None,
            site: self.home,
            made: Vec::new(),
        };
        self.stays.insert(id, stay);
        let home = self.home;
        self.site(home).next.change(join(edge, id));
    }

    /// A player's connection ends: the edge tells the region it takes them to be in,
    /// names the stay, and has given it up.
    fn leave(&mut self, id: PlayerId) {
        let stay = self.stays.remove(&id).expect("the player has a stay");
        let change = leave(stay.edge, id, stay.entity);
        self.site(stay.site).next.change(change);
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
    /// come from has not applied. A stay the edge has given up arrives nowhere, and
    /// its entity is discarded where it was seen last.
    fn arrive(&mut self, from: RegionId, id: PlayerId, transfer: &PlayerTransfer, to: RegionId) {
        let to = self.living(to);
        assert_ne!(to, from, "{from} sent {id:?} on to itself");
        let next = &mut self.sites.get_mut(&to).expect("it lives").next;
        match self.stays.get_mut(&id) {
            Some(stay) if stay.entity == Some(transfer.entity_id) => {
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
        // a block of a chunk that is not its own; and no entity is in two regions.
        let mut entities = BTreeSet::new();
        for (id, site) in &self.sites {
            for chunk in grid().into_iter().chain(outskirts()) {
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
        // A return comes before a claim (ADR-0012, section 4.2). Nothing of the grid
        // is ever given back: someone watches it.
        for chunk in &output.returns {
            assert!(
                outskirts().contains(chunk),
                "{from} gave back {chunk:?}, which someone watches"
            );
        }
        self.grants.give_back(from, &output.returns);
        let (granted, foreign) = self.grants.answer(from, &output.claims);
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
        for (id, event) in &output.player_events {
            if let PlayerEvent::Spawned { entity_id, .. } = event {
                assert_eq!(from, self.home, "only the home region is joined");
                if let Some(stay) = self.stays.get_mut(id) {
                    stay.entity.get_or_insert(*entity_id);
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
                } => self.arrive(from, *player, transfer, *holder),
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
                    let to = self.living(*holder);
                    assert_ne!(to, from, "{from} sent {action:?} on to itself");
                    let next = &mut self.site(to).next;
                    next.remote_actions.push((*edge, action.clone()));
                }
                Durable::RemoteDone { player, sequence } => self.done(*player, *sequence),
                other => panic!("{from} made {other:?} in a tick"),
            }
        }
    }

    /// The chunks of the grid and of the outskirts the region holds by what its ticks
    /// were told, as it says itself.
    fn held_by(&self, region: RegionId) -> Vec<ChunkPos> {
        let site = &self.sites[&region];
        sorted(grid().into_iter().chain(outskirts()))
            .into_iter()
            .filter(|chunk| site.region.knowledge(*chunk) == Knowledge::Held)
            .collect()
    }

    /// The region `survivor` absorbs the region `absorbed`, as of their last ticks, with
    /// whatever is on its way to either. What the region is afterwards is what a
    /// restore makes of the merged state and of what the store grants it, the grants
    /// either region never took among them (section 3.6.6).
    fn merge(&mut self, survivor: RegionId, absorbed: RegionId) {
        assert_ne!(absorbed, self.home, "the home region is never absorbed");
        let gone = self.sites.remove(&absorbed).expect("the region lives");
        let theirs = gone.region.state();
        for chunk in grid().into_iter().chain(outskirts()) {
            if let Some(blocks) = gone.region.chunk(chunk) {
                self.storage.insert(chunk, blocks.clone());
            }
        }
        let (mut chunks, areas) = self.grants.absorb(survivor, absorbed);
        let mut held = self.held_by(survivor);
        let pinned = self.grants.areas_of(survivor);
        let ticket = self.ticket;

        let site = self.site(survivor);
        let before = site.region.clone();
        let state = site.region.absorb(absorbed, &theirs);
        assert_eq!(site.region, before, "`absorb` changed the region");

        // With the grants that moved, what the store had granted the survivor in
        // answer to claims that no tick has been told of.
        chunks.extend(std::mem::take(&mut site.next.granted));
        let loaded = site.region.take_absorbed(state.clone(), &chunks, &areas);
        held.extend(&chunks);
        let holdings = Holdings {
            held: sorted(held),
            pinned,
        };
        let restored = Region::restore(run_config(), state.clone(), holdings);
        assert_eq!(site.region, restored, "after the merge");

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

    /// The players of `region` who stand in `named` are split off, if that is a split,
    /// with the grants that wait handed to the split. Returns the new region. The
    /// region is held to section 3.6.1 of ADR-0017 and section 2.6 of ADR-0014 on the
    /// way.
    fn split(&mut self, region: RegionId, named: &[ChunkPos]) -> Result<RegionId, NoSplit> {
        let part = RegionId(self.grants.next);
        let held = self.held_by(region);
        let pinned = self.grants.areas_of(region);
        let ticket = self.ticket;
        let store = self.grants.clone();

        let site = self.site(region);
        let before = site.region.clone();
        // What the store has granted the region in answer to the claims of its last
        // tick. Each is the region's by the store's table, and none by its ticks.
        let waiting = site.next.granted.clone();
        for chunk in &waiting {
            assert_eq!(store.holder(*chunk), Some(region), "{chunk:?}");
            assert_eq!(before.knowledge(*chunk), Knowledge::Asked, "{chunk:?}");
        }
        let split = site.region.split(named, part, &waiting);
        assert_eq!(site.region, before, "`split` changed the region");
        let by_the_record = split_by_the_record(
            &before.state(),
            &held,
            &waiting,
            !pinned.is_empty(),
            named,
            part,
        );
        assert_eq!(split, by_the_record);
        // If the split is off, the answers stay where they are and the next tick
        // takes them.
        let splitting = split?;

        site.next.granted.clear();
        let (kept, made) = site.region.take_split(splitting.clone(), &waiting);
        check_taken(
            &site.region,
            &made.region,
            &splitting,
            &held,
            &waiting,
            &pinned,
            &run_config(),
        );
        assert_eq!(
            kept.len() + made.chunks.len(),
            before.loaded_chunk_count(),
            "a loaded chunk was lost"
        );
        for (chunk, blocks) in kept.iter().chain(&made.chunks) {
            assert_eq!(before.chunk(*chunk), Some(blocks), "{chunk:?}");
        }
        let of_the_part = |(chunk, _): &(ChunkPos, Chunk)| splitting.chunks.contains(chunk);
        assert!(!kept.iter().any(of_the_part));
        assert!(made.chunks.iter().all(of_the_part));
        let went = |chunk: &&ChunkPos| splitting.chunks.contains(chunk);
        let went = waiting.iter().filter(went).count();

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
            looking_out: false,
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
            assert!(players.contains(&(*id, player.entity_id)));
            match self.stays.get_mut(id) {
                Some(stay) if stay.entity == Some(player.entity_id) => {
                    stay.site = part;
                    let theirs = kept_inputs.iter().filter(|(_, actor, entity, ..)| {
                        actor == id && *entity == player.entity_id
                    });
                    new.next.inputs.extend(theirs.cloned());
                }
                _ => new
                    .next
                    .change(leave(player.edge, *id, Some(player.entity_id))),
            }
        }
        self.storage.extend(kept);
        self.storage.extend(made.chunks);
        self.grants.split(region, part, &splitting.chunks);
        self.sites.insert(part, new);
        if !waiting.is_empty() {
            self.waited.splits += 1;
            self.waited.went += went;
            self.waited.stayed += waiting.len() - went;
        }
        Ok(part)
    }

    /// Whether nothing is on its way: no answer of the store, no chunk, no player and
    /// no action, and no hello unanswered.
    fn settled(&self) -> bool {
        self.sites.values().all(Site::idle)
    }

    /// Ticks until nothing is on its way, and returns how many ticks that took.
    fn settle(&mut self) -> usize {
        for ticks in 0..40 {
            if self.settled() {
                return ticks;
            }
            self.tick();
        }
        panic!("what the regions pass on does not come to an end");
    }

    /// Whether nothing of `id` waits anywhere: every input the edge sent for them was
    /// taken by the region that has them, and they are on their way nowhere.
    fn at_rest(&self, id: PlayerId) -> bool {
        self.sites.values().all(|site| {
            let waiting = site.next.inputs.iter().any(|(_, actor, ..)| *actor == id);
            let coming = site.next.player_changes.iter().any(|change| match change {
                PlayerChange::Join(_, join) => join.player == id,
                PlayerChange::Arrive(_, player, _) | PlayerChange::Leave(_, player, _) => {
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
}

impl One {
    fn new() -> Self {
        let holdings = Holdings {
            held: Vec::new(),
            pinned: vec![ChunkArea::EVERYWHERE],
        };
        let mut next = looking_at(&grid());
        next.edges = vec![started(E, START), started(F, START)];
        let mut one = Self {
            region: Region::new(run_config(), ids(), holdings),
            next,
            dealt_with: BTreeMap::new(),
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
        Self {
            cluster: Cluster::new(areas, ticket),
            one: One::new(),
            steps: Vec::new(),
            name,
            compared: 0,
        }
    }

    fn join(&mut self, id: PlayerId, edge: EdgeId) {
        self.steps
            .push(format!("player {} joins through {edge:?}", id.0.as_u128()));
        self.cluster.join(id, edge);
        self.one.next.change(join(edge, id));
    }

    fn leave(&mut self, id: PlayerId) {
        let stay = &self.cluster.stays[&id];
        self.steps.push(format!(
            "player {} leaves, having {:?} in {}",
            id.0.as_u128(),
            stay.entity,
            stay.site
        ));
        self.one.next.change(leave(stay.edge, id, stay.entity));
        self.cluster.leave(id);
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

    fn look_out(&mut self, region: RegionId) {
        let looks = !self.cluster.sites[&region].looking_out;
        self.steps
            .push(format!("{region} is asked for the outskirts: {looks}"));
        self.cluster.look_out(region);
    }

    fn split(&mut self, region: RegionId, named: &[ChunkPos]) -> Result<RegionId, NoSplit> {
        let waiting = self.cluster.sites[&region].next.granted.clone();
        let split = self.cluster.split(region, named);
        self.steps.push(format!(
            "{region} is split at {named:?} with {waiting:?} granted: {split:?}"
        ));
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
    /// the same in the two worlds (ADR-0014, section 2.6). Not compared: what only one
    /// region counts, its own acknowledgements.
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
                let stay = cluster.stays.get(&id).expect("the edge has the stay");
                assert_eq!(
                    (stay.site, stay.entity),
                    (*region, Some(player.entity_id)),
                    "where the edge takes {id:?} to be"
                );
                let plain = PlayerState {
                    handled: None,
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
    /// Regions are split now and then, up to four of them.
    Splits,
    /// The one region is split, and absorbs its part again, over and over.
    SplitAndAbsorb,
    /// Regions are split and absorb each other in any order.
    Anything,
}

/// A run made up by a generator, as in `reshape.rs`: up to six players who join
/// through either edge, walk across the grid, break and place blocks, change what they
/// hold, leave and join again, while their regions merge and split.
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
}

impl Play {
    fn new(test: &str, seed: u64, areas: &[ChunkArea]) -> Self {
        let ticket = [Ticket::Viewer, Ticket::Guest][(seed % 2) as usize];
        let name = format!("{test}, seed {seed} (CLUSTINE_RESHAPE_SEED={seed}), {ticket:?}");
        Self {
            pair: Pair::new(name, areas, ticket),
            random: Random(0x2545_F491_4F6C_DD1D ^ (seed << 20) ^ seed),
            positions: BTreeMap::new(),
            sequence: 0,
            merges: 0,
            splits: 0,
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
                        self.positions.insert(id, ENTRANCE);
                    }
                }
                // The edge passes on nothing of a player it has not told their entity.
                Some(None) => {}
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
                            // So that a second action never overtakes the first across
                            // a boundary, a player acts on blocks when nothing of
                            // theirs is under way.
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
    ///
    /// **In the tick before**, an edge's viewer comes to see the outskirts through the
    /// region: that tick claims those of them the region knows nothing of, the store
    /// answers, and the split is worked out before a tick has been told (S9).
    fn split(&mut self) {
        let live: Vec<RegionId> = self.pair.cluster.sites.keys().copied().collect();
        if live.len() >= 4 {
            return;
        }
        let region = self.random.pick(&live);
        if self.pair.cluster.sites[&region].looking_out {
            // It looked there already, for a split that was off. It looks away first,
            // and lets go of what nothing else of it keeps.
            self.pair.look_out(region);
            self.pair.tick();
        }
        self.pair.look_out(region);
        self.pair.tick();

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
        if self.pair.split(region, &named).is_ok() {
            self.splits += 1;
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
    compared: usize,
    let_go: usize,
    passed_on: usize,
    dealt_with: usize,
    waited: Waited,
}

impl Tally {
    fn note(&mut self, play: &Play) {
        let cluster = &play.pair.cluster;
        self.merges += play.merges;
        self.splits += play.splits;
        self.compared += play.pair.compared;
        self.let_go += cluster.let_go;
        self.passed_on += cluster.passed_on;
        self.dealt_with += play.sequence as usize;
        self.waited.splits += cluster.waited.splits;
        self.waited.went += cluster.waited.went;
        self.waited.stayed += cluster.waited.stayed;
    }
}

#[test]
fn a_region_split_with_grants_that_wait_and_run_as_two_or_more_shows_what_one_region_shows() {
    // S9: the runs of ADR-0014's S36, one region pinned to everything west of x = 5
    // of which regions are split off around its players, and of those again, with a
    // claim made in the tick before each split and its grant handed to `split`.
    let mut tally = Tally::default();
    for seed in seeds() {
        let test = "a region that is split with grants that wait";
        let mut play = Play::new(test, seed, &[WHOLE]);
        play.run(Reshaping::Splits, 320);
        tally.note(&play);
    }
    let runs = seeds().len();
    assert!(tally.splits >= 2 * runs, "{tally:?}");
    assert!(tally.compared >= 60 * runs, "{tally:?}");
    assert!(
        tally.let_go >= 10 * runs && tally.passed_on >= 10 * runs,
        "{tally:?}"
    );
    assert!(tally.waited.splits >= 2 * runs, "{tally:?}");
    assert!(
        tally.waited.went >= runs && tally.waited.stayed >= runs,
        "{tally:?}"
    );
}

#[test]
fn a_region_split_with_grants_that_wait_that_absorbs_its_part_again_is_as_one_that_never_was() {
    // S9, on the second part of S36: the players, the blocks and the held chunks of
    // the grid.
    let mut tally = Tally::default();
    for seed in seeds() {
        let test = "a region that absorbs the part it was split with grants that wait";
        let mut play = Play::new(test, seed, &[WHOLE]);
        play.run(Reshaping::SplitAndAbsorb, 320);
        let parts: Vec<RegionId> = play.pair.cluster.sites.keys().copied().skip(1).collect();
        for part in parts {
            play.pair.merge(REGION_A, part);
        }
        play.pair.settle();

        let cluster = &play.pair.cluster;
        assert_eq!(cluster.sites.len(), 1);
        let never = &play.pair.one.region;
        let region = &cluster.sites[&REGION_A].region;
        for chunk in grid() {
            assert_eq!(region.knowledge(chunk), Knowledge::Held);
            assert_eq!(never.knowledge(chunk), Knowledge::Held);
            assert_eq!(region.chunk(chunk), never.chunk(chunk));
        }
        assert_eq!(region.state().players.len(), never.state().players.len());
        assert_eq!(region.state().next_entity_id, never.state().next_entity_id);
        tally.note(&play);
    }
    let runs = seeds().len();
    assert!(
        tally.splits >= 3 * runs && tally.merges >= 3 * runs,
        "{tally:?}"
    );
    assert!(tally.compared >= 60 * runs, "{tally:?}");
    assert!(tally.waited.splits >= 3 * runs, "{tally:?}");
    assert!(
        tally.waited.went >= runs && tally.waited.stayed >= runs,
        "{tally:?}"
    );
}

#[test]
fn regions_that_split_with_grants_that_wait_and_absorb_each_other_show_what_one_region_shows() {
    // S9, on whatever the generator comes to: two regions on stripes or one region,
    // parts of parts, a part absorbed by the neighbour of the region it was split
    // from, a survivor that is split while grants of the region it absorbed wait.
    let mut tally = Tally::default();
    for seed in seeds() {
        let test = "regions that merge and split with grants that wait";
        let both: &[ChunkArea] = &[WESTERN, EASTERN];
        let areas = if seed % 4 < 2 { both } else { &[WHOLE] };
        let mut play = Play::new(test, seed, areas);
        play.run(Reshaping::Anything, 480);
        tally.note(&play);
    }
    let runs = seeds().len();
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
    assert!(tally.waited.splits >= 4 * runs, "{tally:?}");
    assert!(
        tally.waited.went >= runs && tally.waited.stayed >= runs,
        "{tally:?}"
    );
}
