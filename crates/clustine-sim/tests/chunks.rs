//! Tests of a region on the chunks the world store grants it, written from sections 1 to
//! 3 and the scenarios S1 to S15, S17 and S18 of
//! `docs/adr/0012-the-tick-on-chunks.md` and the public API alone, by someone who has
//! not read how the region does it.
//!
//! The tests play the store, which answers a tick's claims in a later tick, and the
//! edges, which hold tickets. Every tick goes through [`World::tick`]. It keeps a
//! [`Model`] beside the region, which is what section 1.3 and steps 2 and 9 of section
//! 2.1 say a region knows of chunks, and holds the region to it after every tick: what
//! it knows of every chunk a test has named, its claims, its returns and what it asks of
//! storage. It also checks the five statements at the end of section 1.3 on what the
//! region itself says, and that the delta turns the state before into the state after.
//!
//! Since `docs/adr/0014-merging-and-splitting.md` the model also drops what a region
//! believes of a chunk of its own pinned areas when a player or an action is sent to it
//! for the chunk (section 2.2 there), for which [`World::tick`] follows who is in the
//! region through a tick's edge events and player changes (section 2.1 there).
//!
//! Beside the scenarios there are runs made up by a generator, of one region
//! ([`Wander`]) and of three with one store ([`Cluster`]), which are held to the same
//! in every tick. A test that is marked `ignore` with "finding" says what the record
//! says and fails: it stays as it is until the region or the record is changed.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks, items};
use clustine_sim::api::{
    Face, HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent, RemoteAction, RemoteStep,
};
use clustine_sim::{
    Durable, EdgeEvent, Holdings, Knowledge, Misdirected, PlayerChange, PlayerEvent, PlayerJoin,
    PlayerState, PlayerTransfer, Region, RegionConfig, RegionState, StateDelta, TickInputs,
    TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Section, Vec3,
};
use uuid::Uuid;

const E: EdgeId = EdgeId(1);
const F: EdgeId = EdgeId(2);

/// The chunk players enter the world in, and the chunks around it. On stripes the line
/// runs between `HOME` and `EAST`: all but `EAST` are of the western stripe.
const HOME: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);
const WEST: ChunkPos = ChunkPos::new(-1, 0);
const NORTH: ChunkPos = ChunkPos::new(0, -1);
const SOUTH: ChunkPos = ChunkPos::new(0, 1);

/// The regions of the stripes, as the world store numbers them from west to east, and a
/// third that holds what a test says it does.
const REGION_A: RegionId = RegionId(0);
const REGION_B: RegionId = RegionId(1);
const OTHER: RegionId = RegionId(7);

/// Where players enter the world: on the stone of `HOME`, two blocks from `EAST`.
const SPAWN: Vec3 = Vec3::new(14.5, 64.0, 8.5);

/// Blocks at the top of the stone: one near the spawn point, and one either side of the
/// line between `HOME` and `EAST`.
const OWN_BLOCK: BlockPos = BlockPos::new(14, 63, 8);
const BORDER_BLOCK_A: BlockPos = BlockPos::new(15, 63, 8);
const BORDER_BLOCK_B: BlockPos = BlockPos::new(16, 63, 8);

/// The western stripe, which region 0 is pinned to, and the rest, which region 1 is.
const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

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

fn unbelieve(chunks: &[(ChunkPos, RegionId)]) -> TickInputs {
    TickInputs {
        unbelieve: chunks.to_vec(),
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

fn join(edge: EdgeId, id: PlayerId) -> PlayerChange {
    PlayerChange::Join(
        edge,
        PlayerJoin {
            player: id,
            name: format!("player-{}", id.0.as_u128()),
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

/// A step to the point with these x and z coordinates, on the stone.
fn walk(x: f64, z: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, 64.0, z)),
        rotation: None,
        on_ground: true,
    }
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

fn remote(id: PlayerId, sequence: i32, step: RemoteStep) -> RemoteAction {
    RemoteAction {
        player: id,
        sequence,
        step,
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
    }
}

/// A player of the region as the region would let them go now.
fn transfer_of(state: &PlayerState) -> PlayerTransfer {
    PlayerTransfer {
        entity_id: state.entity_id,
        name: state.name.clone(),
        pose: state.pose,
        hotbar: state.hotbar,
        selected_slot: state.selected_slot,
        last_input: state.last_input,
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

// ---------------------------------------------------------------------------------------
// What the record says a region knows of chunks
// ---------------------------------------------------------------------------------------

/// Section 1 of the record, and steps 2 and 9 of its section 2.1, as a second
/// implementation that knows nothing of players but the chunks they stand in: what a
/// region holds, has asked and believes, its tickets, and what is loaded or asked of
/// storage.
#[derive(Debug, Clone)]
struct Model {
    pinned: Vec<ChunkArea>,
    /// The chunk of the spawn point, which is never given back.
    home: ChunkPos,
    return_after: u64,
    /// The chunks held, each with the first of the unbroken run of ticks at whose end
    /// nothing used it; `None` if something used it at the end of the last tick, or if
    /// no tick has ended since it was granted or the region was made.
    held: BTreeMap<ChunkPos, Option<u64>>,
    asked: BTreeSet<ChunkPos>,
    foreign: BTreeMap<ChunkPos, RegionId>,
    viewers: BTreeMap<ChunkPos, u32>,
    guests: BTreeMap<ChunkPos, u32>,
    loaded: BTreeSet<ChunkPos>,
    /// Asked of storage and not delivered.
    requested: BTreeSet<ChunkPos>,
}

impl Model {
    /// A region that was made or restored just now: it holds what the store says and
    /// knows nothing else (section 1.3, the first row, and section 1.4).
    fn new(config: &RegionConfig, holdings: &Holdings) -> Self {
        Self {
            pinned: holdings.pinned.clone(),
            home: chunk_of(config.spawn),
            return_after: config.return_after,
            held: holdings.held.iter().map(|chunk| (*chunk, None)).collect(),
            asked: BTreeSet::new(),
            foreign: BTreeMap::new(),
            viewers: BTreeMap::new(),
            guests: BTreeMap::new(),
            loaded: BTreeSet::new(),
            requested: BTreeSet::new(),
        }
    }

    fn knowledge(&self, chunk: ChunkPos) -> Knowledge {
        if self.held.contains_key(&chunk) {
            Knowledge::Held
        } else if self.asked.contains(&chunk) {
            Knowledge::Asked
        } else if let Some(region) = self.foreign.get(&chunk) {
            Knowledge::Foreign(*region)
        } else {
            Knowledge::Unknown
        }
    }

    fn viewers(&self, chunk: ChunkPos) -> u32 {
        self.viewers.get(&chunk).copied().unwrap_or(0)
    }

    fn guests(&self, chunk: ChunkPos) -> u32 {
        self.guests.get(&chunk).copied().unwrap_or(0)
    }

    fn tickets(&self, chunk: ChunkPos) -> u32 {
        self.viewers(chunk) + self.guests(chunk)
    }

    fn is_pinned(&self, chunk: ChunkPos) -> bool {
        self.pinned.iter().any(|area| area.contains(chunk))
    }

    /// Section 1.2: a need, or a guest's ticket in a pinned area.
    fn wants(&self, chunk: ChunkPos, standing: &BTreeSet<ChunkPos>) -> bool {
        standing.contains(&chunk)
            || self.viewers(chunk) > 0
            || (self.is_pinned(chunk) && self.guests(chunk) > 0)
    }

    /// Section 1.2: a player in it, or a ticket of either kind.
    fn uses(&self, chunk: ChunkPos, standing: &BTreeSet<ChunkPos>) -> bool {
        standing.contains(&chunk) || self.tickets(chunk) > 0
    }

    /// Section 1.2: never given back.
    fn keeps(&self, chunk: ChunkPos) -> bool {
        self.is_pinned(chunk) || chunk == self.home
    }

    /// Step 2 of a tick. Returns what the tick asks of storage.
    fn start(&mut self, inputs: &TickInputs) -> Vec<ChunkPos> {
        for chunk in &inputs.granted {
            self.asked.remove(chunk);
            self.foreign.remove(chunk);
            self.held.entry(*chunk).or_insert(None);
        }
        for (chunk, region) in &inputs.foreign {
            if !self.held.contains_key(chunk) {
                self.asked.remove(chunk);
                self.foreign.insert(*chunk, *region);
            }
        }
        for (chunk, region) in &inputs.unbelieve {
            if self.foreign.get(chunk) == Some(region) {
                self.foreign.remove(chunk);
            }
        }
        for (chunk, kind) in &inputs.tickets_added {
            let counts = match kind {
                Ticket::Viewer => &mut self.viewers,
                Ticket::Guest => &mut self.guests,
            };
            *counts.entry(*chunk).or_default() += 1;
        }
        for (chunk, kind) in &inputs.tickets_removed {
            let counts = match kind {
                Ticket::Viewer => &mut self.viewers,
                Ticket::Guest => &mut self.guests,
            };
            // A ticket of a kind that was never counted on the chunk is not released
            // ("Found while building", 8).
            let Some(count) = counts.get_mut(chunk) else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                counts.remove(chunk);
            }
            if self.tickets(*chunk) == 0 {
                self.loaded.remove(chunk);
                self.requested.remove(chunk);
            }
        }
        for (chunk, _) in &inputs.chunks_loaded {
            // What was asked of storage is held and has a ticket: it stops being asked
            // of storage with its last ticket, and a chunk with a ticket is not given
            // back.
            if self.requested.remove(chunk) {
                self.loaded.insert(*chunk);
            }
        }
        let requests: Vec<ChunkPos> = self
            .held
            .keys()
            .filter(|chunk| {
                self.tickets(**chunk) > 0
                    && !self.loaded.contains(chunk)
                    && !self.requested.contains(chunk)
            })
            .copied()
            .collect();
        self.requested.extend(&requests);
        requests
    }

    /// Step 9 of tick `tick`, at whose end the region's players stand in `standing`.
    /// Returns the tick's returns and its claims.
    fn finish(
        &mut self,
        tick: u64,
        standing: &BTreeSet<ChunkPos>,
    ) -> (Vec<ChunkPos>, Vec<ChunkPos>) {
        let mut returns = Vec::new();
        for chunk in self.held.keys().copied().collect::<Vec<_>>() {
            if self.uses(chunk, standing) {
                self.held.insert(chunk, None);
                continue;
            }
            let unused_since = *self
                .held
                .get_mut(&chunk)
                .expect("the chunk is held")
                .get_or_insert(tick);
            if !self.keeps(chunk) && tick - unused_since >= self.return_after {
                returns.push(chunk);
            }
        }
        for chunk in &returns {
            self.held.remove(chunk);
        }

        let unwanted: Vec<ChunkPos> = self
            .foreign
            .keys()
            .filter(|chunk| !self.wants(**chunk, standing))
            .copied()
            .collect();
        for chunk in unwanted {
            self.foreign.remove(&chunk);
        }

        let candidates: BTreeSet<ChunkPos> = standing
            .iter()
            .chain(self.viewers.keys())
            .chain(self.guests.keys())
            .copied()
            .collect();
        let claims: Vec<ChunkPos> = candidates
            .into_iter()
            .filter(|chunk| {
                self.wants(*chunk, standing) && self.knowledge(*chunk) == Knowledge::Unknown
            })
            .collect();
        self.asked.extend(&claims);
        (returns, claims)
    }

    /// ADR-0014, section 2.2, between steps 2 and 9 of a tick that begins with the
    /// state `before`: where the region would have sent an arrival or a remote action
    /// on to the region it believes to hold the chunk, and the chunk is of its own
    /// pinned areas, it drops the belief instead and goes on as if it knew nothing of
    /// the chunk.
    ///
    /// An arrival gets that far if it comes through an edge the region knows and the
    /// region does not have the player with that entity id or a higher one; an action,
    /// if it comes through an edge the region knows. So this follows who is in the
    /// region through the tick's edge events and player changes, by section 2.1 there.
    fn doubt(&mut self, before: &RegionState, inputs: &TickInputs) {
        let mut edges: BTreeMap<EdgeId, u64> = before
            .edges
            .iter()
            .map(|(edge, state)| (*edge, state.start))
            .collect();
        // Each player's edge and entity.
        let mut stays: BTreeMap<PlayerId, (EdgeId, EntityId)> = before
            .players
            .iter()
            .map(|(id, state)| (*id, (state.edge, state.entity_id)))
            .collect();
        for event in &inputs.edges {
            match event {
                EdgeEvent::Started { edge, start } => {
                    if edges.get(edge).is_none_or(|known| known < start) {
                        stays.retain(|_, (of, _)| of != edge);
                        edges.insert(*edge, *start);
                    }
                }
                EdgeEvent::Gone { edge } => {
                    edges.remove(edge);
                    stays.retain(|_, (of, _)| of != edge);
                }
                EdgeEvent::Confirmed { .. } => {}
            }
        }
        let mut next_entity = before.next_entity_id;
        for change in &inputs.player_changes {
            match change {
                PlayerChange::Join(edge, join) if edges.contains_key(edge) => {
                    stays.remove(&join.player);
                    if before.entity_ids.contains(next_entity) {
                        stays.insert(join.player, (*edge, next_entity));
                        next_entity.0 += 1;
                    }
                }
                PlayerChange::Leave(edge, id, entity) => {
                    let ended = |(of, has): &(EdgeId, EntityId)| {
                        of == edge && entity.is_none_or(|named| named == *has)
                    };
                    if stays.get(id).is_some_and(ended) {
                        stays.remove(id);
                    }
                }
                PlayerChange::Arrive(edge, id, transfer) if edges.contains_key(edge) => {
                    let entity = transfer.entity_id;
                    if stays.get(id).is_some_and(|(_, has)| *has >= entity) {
                        continue;
                    }
                    stays.remove(id);
                    let chunk = chunk_of(transfer.pose.position);
                    if self.is_pinned(chunk) {
                        self.foreign.remove(&chunk);
                    }
                    // Sent on where the chunk is still believed another's.
                    if !self.foreign.contains_key(&chunk) {
                        stays.insert(*id, (*edge, entity));
                    }
                }
                PlayerChange::Join(..)
                | PlayerChange::Arrive(..)
                | PlayerChange::Discard { .. } => {}
            }
        }
        for (edge, action) in &inputs.remote_actions {
            let chunk = action.step.concerns().chunk();
            if edges.contains_key(edge) && self.is_pinned(chunk) {
                self.foreign.remove(&chunk);
            }
        }
    }
}

/// A region with the [`Model`] of what it should know beside it.
struct World {
    region: Region,
    model: Model,
    /// Every chunk a tick was given or gave back, or a player stood in: the chunks of
    /// which the region and the model are compared.
    seen: BTreeSet<ChunkPos>,
    /// The model as it was while the last tick ran, between its steps 2 and 9: what the
    /// region knew of chunks when it judged what players did and let players go, with
    /// the beliefs gone that the tick dropped for a player or an action
    /// ([`Model::doubt`]).
    during: Model,
}

impl World {
    fn new(config: RegionConfig, entity_ids: EntityIds, holdings: Holdings) -> Self {
        let model = Model::new(&config, &holdings);
        let seen = holdings.held.iter().copied().collect();
        let world = Self {
            region: Region::new(config, entity_ids, holdings),
            during: model.clone(),
            model,
            seen,
        };
        world.compare();
        world
    }

    /// The region as another owner has it, who was given `state` and `holdings` by the
    /// store.
    fn restore(config: RegionConfig, state: RegionState, holdings: Holdings) -> Self {
        let model = Model::new(&config, &holdings);
        let mut seen: BTreeSet<ChunkPos> = holdings.held.iter().copied().collect();
        seen.extend(
            state
                .players
                .values()
                .map(|player| chunk_of(player.pose.position)),
        );
        let world = Self {
            region: Region::restore(config, state, holdings),
            during: model.clone(),
            model,
            seen,
        };
        world.compare();
        world
    }

    /// Region 0 of the stripes, new: pinned to the western stripe and granted nothing.
    fn pinned(return_after: u64) -> Self {
        Self::new(
            config(return_after),
            ids(),
            Holdings {
                held: Vec::new(),
                pinned: vec![WESTERN],
            },
        )
    }

    /// A new region on open land: pinned to nothing, and granted `held`.
    fn open(return_after: u64, held: &[ChunkPos]) -> Self {
        Self::new(
            config(return_after),
            ids(),
            Holdings {
                held: held.to_vec(),
                pinned: Vec::new(),
            },
        )
    }

    fn knowledge(&self, chunk: ChunkPos) -> Knowledge {
        self.region.knowledge(chunk)
    }

    /// What the region has of the block at `position`, if its chunk is loaded.
    fn block(&self, position: BlockPos) -> Option<BlockState> {
        let (x, z) = position.in_chunk();
        self.region
            .chunk(position.chunk())
            .and_then(|chunk| chunk.get(x, position.y, z))
    }

    fn state_of(&self, id: PlayerId) -> PlayerState {
        self.region
            .player_state(id)
            .expect("the player is in the region")
    }

    /// The chunks the region's players stand in.
    fn standing(&self) -> BTreeSet<ChunkPos> {
        self.region
            .state()
            .players
            .values()
            .map(|player| chunk_of(player.pose.position))
            .collect()
    }

    /// The region knows of every chunk a test has named what the model does, and has
    /// loaded what the model has.
    fn compare(&self) {
        let tick = self.region.tick_number();
        for chunk in &self.seen {
            assert_eq!(
                self.region.knowledge(*chunk),
                self.model.knowledge(*chunk),
                "what the region knows of {chunk:?} after tick {tick}"
            );
            assert_eq!(
                self.region.chunk(*chunk).is_some(),
                self.model.loaded.contains(chunk),
                "whether {chunk:?} is loaded after tick {tick}"
            );
        }
        assert_eq!(
            self.region.loaded_chunk_count(),
            self.model.loaded.len(),
            "how many chunks are loaded after tick {tick}"
        );
        assert_eq!(
            self.region.held_chunk_count(),
            self.model.held.len(),
            "how many chunks are held after tick {tick}"
        );
    }

    /// Notes every chunk the inputs name.
    fn see(&mut self, inputs: &TickInputs) {
        let tickets = inputs.tickets_added.iter().chain(&inputs.tickets_removed);
        self.seen.extend(tickets.map(|(chunk, _)| *chunk));
        self.seen
            .extend(inputs.chunks_loaded.iter().map(|(chunk, _)| *chunk));
        self.seen.extend(&inputs.granted);
        let said = inputs.foreign.iter().chain(&inputs.unbelieve);
        self.seen.extend(said.map(|(chunk, _)| *chunk));
        for change in &inputs.player_changes {
            match change {
                PlayerChange::Arrive(_, _, transfer) => {
                    self.seen.insert(chunk_of(transfer.pose.position));
                }
                PlayerChange::Discard { chunk, .. } => {
                    self.seen.insert(*chunk);
                }
                PlayerChange::Join(..) | PlayerChange::Leave(..) => {}
            }
        }
        for (_, _, _, _, input) in &inputs.inputs {
            match input {
                PlayerInput::Move {
                    position: Some(position),
                    ..
                } => {
                    self.seen.insert(chunk_of(*position));
                }
                PlayerInput::Dig { position, .. } => {
                    self.seen.insert(position.chunk());
                }
                PlayerInput::UseItemOn { position, face, .. } => {
                    self.seen.insert(position.chunk());
                    self.seen.insert(face.neighbour(*position).chunk());
                }
                _ => {}
            }
        }
        for (_, action) in &inputs.remote_actions {
            self.seen.insert(action.step.concerns().chunk());
            if let RemoteStep::PlaceAgainst { target, .. } = &action.step {
                self.seen.insert(target.chunk());
            }
        }
    }

    /// Advances the region by one tick and holds it to what the record says of every
    /// tick.
    fn tick(&mut self, inputs: &TickInputs) -> TickOutput {
        let before = self.region.state();
        self.see(inputs);
        self.seen.extend(self.standing());

        let requests = self.model.start(inputs);
        self.model.doubt(&before, inputs);
        self.during = self.model.clone();
        let output = self.region.tick(inputs);
        let after = self.region.state();
        let tick = output.tick;
        check_state(&before, &output, &after);
        assert_eq!(self.region.tick_number(), tick);

        // Section 2.1, step 8: a player is let go to the region believed to hold the
        // chunk they stand in, and nobody is left standing in such a chunk.
        for (_, _, entry) in &output.durable {
            if let Durable::Departed { transfer, to, .. } = entry {
                let chunk = chunk_of(transfer.pose.position);
                self.seen.insert(chunk);
                assert_eq!(
                    self.during.foreign.get(&chunk),
                    Some(to),
                    "tick {tick}: a player was let go from {chunk:?} to {to}"
                );
            }
        }
        let standing = self.standing();
        for chunk in &standing {
            assert!(
                !self.during.foreign.contains_key(chunk),
                "tick {tick}: a player was left in {chunk:?}, which is another region's"
            );
        }
        // A block is changed only in a loaded chunk, and a loaded chunk is held (section
        // 4.2).
        for (block, _) in block_changes(&output) {
            assert!(
                self.during.loaded.contains(&block.chunk()),
                "tick {tick}: {block:?} changed in a chunk that is not loaded"
            );
        }
        self.seen.extend(&standing);
        self.seen.extend(&output.claims);
        self.seen.extend(&output.returns);
        self.seen.extend(&output.chunk_requests);

        let (returns, claims) = self.model.finish(tick, &standing);
        assert_eq!(
            output.chunk_requests, requests,
            "tick {tick}: what is asked of storage"
        );
        assert_eq!(output.returns, returns, "tick {tick}: what is given back");
        assert_eq!(output.claims, claims, "tick {tick}: what is claimed");
        self.compare();

        // The statements at the end of section 1.3, on what the region itself says. The
        // first, that no chunk is in two collections, cannot be seen from outside:
        // `Region::knowledge` gives one answer.
        for chunk in &self.seen {
            let knowledge = self.region.knowledge(*chunk);
            let wanted = self.model.wants(*chunk, &standing);
            if wanted {
                assert_ne!(
                    knowledge,
                    Knowledge::Unknown,
                    "tick {tick}: {chunk:?} is wanted and unknown"
                );
            }
            if matches!(knowledge, Knowledge::Foreign(_)) {
                assert!(
                    wanted,
                    "tick {tick}: {chunk:?} is believed another's and not wanted"
                );
            }
            if self.region.chunk(*chunk).is_some() {
                assert_eq!(
                    knowledge,
                    Knowledge::Held,
                    "tick {tick}: {chunk:?} is loaded"
                );
                assert!(
                    self.model.tickets(*chunk) > 0,
                    "tick {tick}: {chunk:?} is loaded without a ticket"
                );
            }
        }
        for chunk in &output.chunk_requests {
            assert_eq!(self.region.knowledge(*chunk), Knowledge::Held);
            assert!(self.model.tickets(*chunk) > 0);
        }
        for chunk in &output.returns {
            assert_eq!(self.model.tickets(*chunk), 0, "tick {tick}: {chunk:?}");
            assert!(!standing.contains(chunk), "tick {tick}: {chunk:?}");
            assert!(
                self.region.chunk(*chunk).is_none(),
                "tick {tick}: {chunk:?}"
            );
            assert_eq!(self.region.knowledge(*chunk), Knowledge::Unknown);
            assert!(!self.model.keeps(*chunk), "tick {tick}: {chunk:?} is kept");
            assert!(!output.claims.contains(chunk), "tick {tick}: {chunk:?}");
        }
        for chunk in &output.claims {
            assert_eq!(self.region.knowledge(*chunk), Knowledge::Asked);
        }
        assert!(
            output.claims.is_sorted(),
            "tick {tick}: {:?}",
            output.claims
        );
        assert!(output.returns.is_sorted(), "tick {tick}");
        assert!(output.chunk_requests.is_sorted(), "tick {tick}");

        // Section 4.8: where the players are.
        let mut crowds: BTreeMap<ChunkPos, u32> = BTreeMap::new();
        for player in after.players.values() {
            *crowds.entry(chunk_of(player.pose.position)).or_default() += 1;
        }
        assert_eq!(
            self.region.crowds(),
            crowds.into_iter().collect::<Vec<_>>(),
            "tick {tick}"
        );
        output
    }

    /// A tick in which nothing happens.
    fn idle(&mut self) -> TickOutput {
        self.tick(&TickInputs::default())
    }

    /// Has the region come to hold `chunk` and load it, in three ticks: a viewer's
    /// ticket, for which it asks; the store's grant, with which it asks storage; and
    /// what storage delivers.
    fn hold(&mut self, chunk: ChunkPos) {
        let output = self.tick(&add(&[viewer(chunk)]));
        assert_eq!(output.claims, [chunk]);
        let output = self.tick(&granted(&[chunk]));
        assert_eq!(output.chunk_requests, [chunk]);
        self.tick(&delivered(&[chunk]));
        assert_eq!(self.knowledge(chunk), Knowledge::Held);
        assert!(self.region.chunk(chunk).is_some());
    }

    /// Has the region learn that `chunk` is `holder`'s, in two ticks: a viewer's ticket,
    /// for which it asks, then what the store answers. It believes so while the ticket
    /// is there.
    fn learn(&mut self, chunk: ChunkPos, holder: RegionId) {
        self.ask(chunk);
        self.tick(&foreign(&[(chunk, holder)]));
        assert_eq!(self.knowledge(chunk), Knowledge::Foreign(holder));
    }

    /// Has the region ask for `chunk`, for a viewer's ticket, and leaves it unanswered.
    fn ask(&mut self, chunk: ChunkPos) {
        let output = self.tick(&add(&[viewer(chunk)]));
        assert_eq!(output.claims, [chunk]);
        assert_eq!(self.knowledge(chunk), Knowledge::Asked);
    }
}

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
    // go in this tick: they are told what became of their actions before they are told
    // that they are another region's (ADR-0008, section 4).
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

/// The world store as far as chunks go (ADR-0011, sections 1.3, 3.2 and 3.3): who holds
/// a chunk is the region it is granted to, else the region pinned to an area with it,
/// else nobody.
#[derive(Debug, Clone, Default)]
struct Grants {
    pinned: Vec<(ChunkArea, RegionId)>,
    granted: BTreeMap<ChunkPos, RegionId>,
}

impl Grants {
    /// The stripes: region 0 pinned to the western one and region 1 to the rest.
    fn stripes() -> Self {
        Self {
            pinned: vec![(WESTERN, REGION_A), (EASTERN, REGION_B)],
            granted: BTreeMap::new(),
        }
    }

    fn holder(&self, chunk: ChunkPos) -> Option<RegionId> {
        self.granted.get(&chunk).copied().or_else(|| {
            self.pinned
                .iter()
                .find(|(area, _)| area.contains(chunk))
                .map(|(_, region)| *region)
        })
    }

    /// Answers a claim of `region` as the store would: `granted` and `foreign`, each in
    /// the order of the claims. A chunk nobody holds is the region's from now on.
    fn answer(
        &mut self,
        region: RegionId,
        claims: &[ChunkPos],
    ) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
        let mut granted = Vec::new();
        let mut foreign = Vec::new();
        for chunk in claims {
            match self.holder(*chunk) {
                Some(holder) if holder != region => foreign.push((*chunk, holder)),
                Some(_) => granted.push(*chunk),
                None => {
                    self.granted.insert(*chunk, region);
                    granted.push(*chunk);
                }
            }
        }
        (granted, foreign)
    }

    /// Frees what `region` gives back. The store would leave out a chunk the region has
    /// no grant for, with a warning; a region that returns one has made a mistake.
    fn take_back(&mut self, region: RegionId, returns: &[ChunkPos]) {
        for chunk in returns {
            assert_eq!(
                self.granted.remove(chunk),
                Some(region),
                "{region} gave back {chunk:?}, which it was not granted"
            );
        }
    }
}

/// Region 0 of the stripes with one chunk of each kind around its spawn point: `HOME`
/// held and loaded, `EAST` believed region 1's and `SOUTH` asked for, each for a
/// viewer's ticket, and `NORTH`, of which it knows nothing. It knows the edges `E` and
/// `F`, and player 1 has joined through `E`.
fn at_the_line() -> World {
    let mut world = World::pinned(0);
    world.hold(HOME);
    world.learn(EAST, REGION_B);
    world.ask(SOUTH);
    world.tick(&edges(vec![started(E, 10), started(F, 10)]));
    let output = world.tick(&changes(vec![join(E, player(1))]));
    assert!(matches!(
        output.player_events.as_slice(),
        [(id, PlayerEvent::Spawned { .. })] if *id == player(1)
    ));
    assert_eq!(world.knowledge(NORTH), Knowledge::Unknown);
    world
}

/// A region on open land that holds `HOME`, loaded for a viewer's ticket, and nothing
/// else. It knows the edges `E` and `F`, and player 1 has joined through `E`.
fn on_open_land(return_after: u64) -> World {
    let mut world = World::open(return_after, &[]);
    world.hold(HOME);
    world.tick(&edges(vec![started(E, 10), started(F, 10)]));
    world.tick(&changes(vec![join(E, player(1))]));
    assert_eq!(world.state_of(player(1)).pose.position, SPAWN);
    world
}

// ---------------------------------------------------------------------------------------
// S1: a pinned region claims the chunks of its area
// ---------------------------------------------------------------------------------------

#[test]
fn a_new_region_pinned_to_an_area_holds_nothing_of_it() {
    let mut world = World::pinned(0);
    let chunks = [HOME, WEST, NORTH, SOUTH, ChunkPos::new(-40, 12)];
    for chunk in chunks {
        assert_eq!(world.knowledge(chunk), Knowledge::Unknown);
    }
    assert_eq!(world.region.held_chunk_count(), 0);

    // Nothing asks, so nothing is asked for.
    for _ in 0..3 {
        let output = world.idle();
        assert!(output.claims.is_empty() && output.returns.is_empty());
        assert!(output.chunk_requests.is_empty());
    }
    for chunk in chunks {
        assert_eq!(world.knowledge(chunk), Knowledge::Unknown);
    }
}

#[test]
fn a_viewers_ticket_claims_a_chunk_of_the_pinned_area_once_and_its_grant_is_asked_of_storage_at_once()
 {
    for return_after in [0, 1, 5] {
        let mut world = World::pinned(return_after);
        let output = world.tick(&add(&[viewer(WEST)]));
        assert_eq!(output.claims, [WEST]);
        assert!(output.chunk_requests.is_empty());
        assert_eq!(world.knowledge(WEST), Knowledge::Asked);

        // While no answer has come it is not claimed again, also not for a second
        // ticket.
        for _ in 0..4 {
            let output = world.idle();
            assert!(output.claims.is_empty());
            assert_eq!(world.knowledge(WEST), Knowledge::Asked);
        }
        let output = world.tick(&add(&[viewer(WEST)]));
        assert!(output.claims.is_empty());

        let output = world.tick(&granted(&[WEST]));
        assert_eq!(world.knowledge(WEST), Knowledge::Held);
        assert_eq!(
            output.chunk_requests,
            [WEST],
            "asked of storage in the tick"
        );
        assert!(output.claims.is_empty());
        world.tick(&delivered(&[WEST]));
        assert!(world.region.chunk(WEST).is_some());

        // A chunk of a pinned area is never given back: not while it is watched, and
        // not long after the last look.
        for _ in 0..3 {
            assert!(world.idle().returns.is_empty());
        }
        world.tick(&remove(&[viewer(WEST), viewer(WEST)]));
        for _ in 0..return_after + 10 {
            let output = world.idle();
            assert!(output.returns.is_empty());
            assert_eq!(world.knowledge(WEST), Knowledge::Held);
        }
    }
}

#[test]
fn the_claims_of_a_tick_are_in_ascending_order_whatever_the_order_of_the_tickets() {
    let mut world = World::pinned(0);
    let far = ChunkPos::new(-3, 5);
    let output = world.tick(&add(&[
        viewer(SOUTH),
        guest(HOME),
        viewer(far),
        viewer(NORTH),
        guest(WEST),
        viewer(HOME),
    ]));
    assert_eq!(output.claims, [far, WEST, NORTH, HOME, SOUTH]);

    // And so is what is asked of storage, whatever the order of the grants.
    let output = world.tick(&granted(&[SOUTH, HOME, far, NORTH, WEST]));
    assert_eq!(output.chunk_requests, [far, WEST, NORTH, HOME, SOUTH]);
}

#[test]
fn a_chunk_stays_asked_until_it_is_answered_also_when_nothing_wants_it_any_more() {
    // A chunk leaves `Asked` only by an answer (section 1.3).
    let mut world = World::pinned(0);
    world.ask(WEST);
    world.tick(&remove(&[viewer(WEST)]));
    for _ in 0..5 {
        let output = world.idle();
        assert_eq!(world.knowledge(WEST), Knowledge::Asked);
        assert!(output.claims.is_empty());
    }
    // A ticket that comes back finds the claim under way and makes no second one.
    let output = world.tick(&add(&[viewer(WEST)]));
    assert!(output.claims.is_empty());
    assert_eq!(world.knowledge(WEST), Knowledge::Asked);
}

// ---------------------------------------------------------------------------------------
// S2: a guest's ticket
// ---------------------------------------------------------------------------------------

#[test]
fn a_guests_ticket_claims_an_unknown_chunk_of_a_pinned_area() {
    let mut world = World::pinned(0);
    let output = world.tick(&add(&[guest(WEST)]));
    assert_eq!(output.claims, [WEST]);
    assert_eq!(world.knowledge(WEST), Knowledge::Asked);
    assert!(world.idle().claims.is_empty());

    // The grant of a chunk that only a guest watches is asked of storage in its tick,
    // and what storage delivers is taken.
    let output = world.tick(&granted(&[WEST]));
    assert_eq!(output.chunk_requests, [WEST]);
    world.tick(&delivered(&[WEST]));
    assert!(world.region.chunk(WEST).is_some());
}

#[test]
fn a_guests_ticket_on_open_land_claims_nothing() {
    let mut world = World::open(0, &[]);
    let output = world.tick(&add(&[guest(EAST)]));
    assert!(output.claims.is_empty());
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    for _ in 0..3 {
        let output = world.idle();
        assert!(output.claims.is_empty() && output.chunk_requests.is_empty());
        assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    }
    // A second one changes nothing; a viewer's beside them makes the region ask.
    assert!(world.tick(&add(&[guest(EAST)])).claims.is_empty());
    let output = world.tick(&add(&[viewer(EAST)]));
    assert_eq!(output.claims, [EAST]);
}

#[test]
fn a_guests_ticket_outside_the_areas_a_region_is_pinned_to_claims_nothing() {
    // Region 0 of the stripes, and a chunk of the other stripe.
    let mut world = World::pinned(0);
    let output = world.tick(&add(&[guest(EAST), guest(WEST)]));
    assert_eq!(output.claims, [WEST], "only the chunk of its own area");
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    for _ in 0..3 {
        assert!(world.idle().claims.is_empty());
    }
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
}

#[test]
fn a_guests_ticket_in_a_pinned_area_keeps_what_the_region_believes_of_the_chunk() {
    // A chunk of the region's own area that the store has granted to another region,
    // as after a split. The guest's ticket wants it, so the belief stays.
    let mut world = World::pinned(0);
    let output = world.tick(&add(&[guest(WEST)]));
    assert_eq!(output.claims, [WEST]);
    world.tick(&foreign(&[(WEST, OTHER)]));
    for _ in 0..4 {
        let output = world.idle();
        assert_eq!(world.knowledge(WEST), Knowledge::Foreign(OTHER));
        assert!(output.claims.is_empty() && output.chunk_requests.is_empty());
    }
    world.tick(&remove(&[guest(WEST)]));
    assert_eq!(world.knowledge(WEST), Knowledge::Unknown);
    assert!(world.idle().claims.is_empty());
}

#[test]
fn a_guests_ticket_outside_the_pinned_areas_does_not_keep_a_belief() {
    for mut world in [World::open(0, &[]), World::pinned(0)] {
        world.learn(EAST, REGION_B);
        // A guest's ticket beside the viewer's changes nothing the region knows.
        let output = world.tick(&add(&[guest(EAST)]));
        assert!(output.claims.is_empty());
        assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));

        // The viewer's ticket goes and the guest's stays: the belief is not wanted.
        let output = world.tick(&remove(&[viewer(EAST)]));
        assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
        assert!(output.claims.is_empty());
        for _ in 0..3 {
            assert!(world.idle().claims.is_empty());
            assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
        }
    }
}

#[test]
fn an_answer_for_a_chunk_only_a_guest_watches_outside_the_pinned_areas_is_not_kept() {
    // The region asked for a viewer, who has gone; a guest's ticket is still there.
    let mut world = World::open(0, &[]);
    world.tick(&add(&[viewer(EAST), guest(EAST)]));
    world.tick(&remove(&[viewer(EAST)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    world.tick(&foreign(&[(EAST, REGION_B)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
}

#[test]
fn a_guests_ticket_claims_in_every_area_a_region_is_pinned_to_and_nowhere_else() {
    // A region that absorbed another is pinned to both their areas, which need not
    // touch (ADR-0011, section 2).
    let west = ChunkArea {
        min_x: None,
        max_x: Some(0),
    };
    let east = ChunkArea {
        min_x: Some(5),
        max_x: Some(7),
    };
    let holdings = Holdings {
        held: Vec::new(),
        pinned: vec![west, east],
    };
    let mut world = World::new(config(0), ids(), holdings);
    let [near, far, between, beyond] = [
        ChunkPos::new(-1, 0),
        ChunkPos::new(6, -2),
        ChunkPos::new(2, 0),
        ChunkPos::new(7, 0),
    ];
    let output = world.tick(&add(&[
        guest(far),
        guest(between),
        guest(beyond),
        guest(near),
    ]));
    assert_eq!(output.claims, [near, far]);

    // Granted, both are kept when the guests have gone; a chunk between the areas
    // that the region came to hold is given back.
    world.tick(&granted(&[near, far, between]));
    let output = world.tick(&remove(&[guest(far), guest(between), guest(near)]));
    assert_eq!(output.returns, [between]);
    for _ in 0..3 {
        assert!(world.idle().returns.is_empty());
    }
    assert_eq!(world.knowledge(near), Knowledge::Held);
    assert_eq!(world.knowledge(far), Knowledge::Held);
}

#[test]
fn a_grant_that_finds_only_a_guests_ticket_outside_the_pinned_areas_is_kept_and_loaded() {
    // Asked for a viewer who has gone since. The guest's ticket is no reason to claim,
    // and a reason to keep and to load what the region holds.
    let mut world = World::open(0, &[]);
    world.tick(&add(&[viewer(EAST), guest(EAST)]));
    world.tick(&remove(&[viewer(EAST)]));
    let output = world.tick(&granted(&[EAST]));
    assert_eq!(output.chunk_requests, [EAST]);
    assert!(output.returns.is_empty());
    world.tick(&delivered(&[EAST]));
    for _ in 0..4 {
        assert!(world.idle().returns.is_empty());
    }
    assert!(world.region.chunk(EAST).is_some());
}

#[test]
fn releasing_a_ticket_of_a_kind_that_is_not_on_a_chunk_takes_none_of_the_other_kind() {
    // "Found while building", 8.
    let mut world = World::open(0, &[]);
    world.learn(EAST, REGION_B);
    world.tick(&remove(&[guest(EAST)]));
    assert_eq!(
        world.knowledge(EAST),
        Knowledge::Foreign(REGION_B),
        "the viewer's ticket still wants it"
    );

    world.hold(WEST);
    world.tick(&TickInputs {
        tickets_added: vec![guest(WEST)],
        tickets_removed: vec![viewer(WEST)],
        ..TickInputs::default()
    });
    let output = world.tick(&remove(&[viewer(WEST), viewer(WEST)]));
    assert!(
        output.returns.is_empty(),
        "the guest's ticket still uses it"
    );
    assert!(world.region.chunk(WEST).is_some());
    // And the one release that there is a ticket for ends it.
    let output = world.tick(&remove(&[guest(WEST)]));
    assert_eq!(output.returns, [WEST]);
}

// ---------------------------------------------------------------------------------------
// S3: what the store calls another region's
// ---------------------------------------------------------------------------------------

#[test]
fn a_chunk_the_store_calls_anothers_is_believed_so_for_as_long_as_a_viewers_ticket_is_on_it() {
    for mut world in [World::pinned(0), World::open(0, &[])] {
        let output = world.tick(&add(&[viewer(EAST)]));
        assert_eq!(output.claims, [EAST]);
        let output = world.tick(&foreign(&[(EAST, REGION_B)]));
        assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));
        assert!(output.claims.is_empty() && output.chunk_requests.is_empty());

        for _ in 0..6 {
            let output = world.idle();
            assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));
            assert!(output.claims.is_empty(), "no further claim");
            assert!(output.chunk_requests.is_empty(), "and nothing is loaded");
        }

        // When the ticket goes, the belief goes at the end of that tick.
        let output = world.tick(&remove(&[viewer(EAST)]));
        assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
        assert!(output.claims.is_empty() && output.returns.is_empty());
        assert!(world.idle().claims.is_empty());

        // A new viewer's ticket claims it again.
        let output = world.tick(&add(&[viewer(EAST)]));
        assert_eq!(output.claims, [EAST]);
        assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    }
}

#[test]
fn a_belief_lasts_until_the_last_viewers_ticket_goes() {
    let mut world = World::pinned(0);
    world.tick(&add(&[viewer(EAST), viewer(EAST)]));
    world.tick(&foreign(&[(EAST, REGION_B)]));
    world.tick(&remove(&[viewer(EAST)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));

    // A ticket that goes and comes in one tick is counted before it is released, so
    // the chunk is wanted at the end of that tick as before.
    let output = world.tick(&TickInputs {
        tickets_added: vec![viewer(EAST)],
        tickets_removed: vec![viewer(EAST)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));
    assert!(output.claims.is_empty());

    world.tick(&remove(&[viewer(EAST)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
}

#[test]
fn a_later_answer_replaces_what_the_region_believed() {
    // `granted` and `foreign` are believed whatever the region knew.
    let mut world = World::open(0, &[]);
    world.learn(EAST, REGION_B);
    let output = world.tick(&foreign(&[(EAST, OTHER)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Foreign(OTHER));
    assert!(output.claims.is_empty());

    // And a grant makes a chunk it believed another's its own, loaded for the ticket.
    let output = world.tick(&granted(&[EAST]));
    assert_eq!(world.knowledge(EAST), Knowledge::Held);
    assert_eq!(output.chunk_requests, [EAST]);
}

#[test]
fn an_answer_in_the_tick_of_the_first_ticket_is_taken_and_nothing_is_claimed() {
    // The answer is step 2.2 of the tick and the ticket step 2.4: at the end the chunk
    // is wanted and believed another's.
    let mut world = World::pinned(0);
    let output = world.tick(&TickInputs {
        foreign: vec![(EAST, REGION_B)],
        tickets_added: vec![viewer(EAST)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));
    assert!(output.claims.is_empty());
}

// ---------------------------------------------------------------------------------------
// S4: answers for chunks nothing wants any more
// ---------------------------------------------------------------------------------------

#[test]
fn foreign_for_a_chunk_nothing_wants_any_more_leaves_it_unknown() {
    for mut world in [World::pinned(0), World::open(0, &[])] {
        world.ask(EAST);
        world.tick(&remove(&[viewer(EAST)]));
        assert_eq!(world.knowledge(EAST), Knowledge::Asked);

        let output = world.tick(&foreign(&[(EAST, REGION_B)]));
        assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
        assert!(output.claims.is_empty() && output.returns.is_empty());
        for _ in 0..3 {
            let output = world.idle();
            assert!(output.claims.is_empty());
            assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
        }
    }
}

#[test]
fn a_grant_for_a_chunk_nothing_wants_any_more_is_given_back_in_its_tick_on_open_land() {
    let mut world = World::open(0, &[]);
    world.ask(EAST);
    world.tick(&remove(&[viewer(EAST)]));

    let output = world.tick(&granted(&[EAST]));
    assert_eq!(output.returns, [EAST]);
    assert!(output.chunk_requests.is_empty() && output.claims.is_empty());
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    for _ in 0..3 {
        let output = world.idle();
        assert!(output.returns.is_empty() && output.claims.is_empty());
    }
}

#[test]
fn a_grant_for_a_chunk_nothing_wants_any_more_is_kept_in_a_pinned_area() {
    let mut world = World::pinned(0);
    world.ask(WEST);
    world.tick(&remove(&[viewer(WEST)]));

    let output = world.tick(&granted(&[WEST]));
    assert_eq!(world.knowledge(WEST), Knowledge::Held);
    assert!(output.returns.is_empty() && output.chunk_requests.is_empty());
    for _ in 0..5 {
        assert!(world.idle().returns.is_empty());
    }
    assert_eq!(world.knowledge(WEST), Knowledge::Held);
}

#[test]
fn a_grant_for_a_chunk_nothing_wants_any_more_is_held_for_the_time_before_a_return() {
    let mut world = World::open(3, &[]);
    world.ask(EAST);
    world.tick(&remove(&[viewer(EAST)]));

    let grant = world.tick(&granted(&[EAST]));
    assert!(grant.returns.is_empty());
    assert_eq!(world.knowledge(EAST), Knowledge::Held);
    // The tick of the grant is the first in which nothing used it: three ticks on.
    for _ in 0..2 {
        assert!(world.idle().returns.is_empty());
    }
    let output = world.idle();
    assert_eq!(output.tick, grant.tick + 3);
    assert_eq!(output.returns, [EAST]);
}

#[test]
fn answers_nobody_asked_for_are_believed_like_any_other() {
    // The store's word is taken whatever the region knew: of a chunk it never asked
    // for, nothing wanting it, a grant is given back at once on open land and a
    // `foreign` forgotten at once.
    let mut world = World::open(0, &[]);
    let output = world.tick(&granted(&[EAST]));
    assert_eq!(output.returns, [EAST]);
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    let output = world.tick(&foreign(&[(WEST, OTHER)]));
    assert_eq!(world.knowledge(WEST), Knowledge::Unknown);
    assert!(output.claims.is_empty());

    // With a viewer's ticket that came with it, the grant is held and loaded.
    let output = world.tick(&TickInputs {
        granted: vec![SOUTH],
        tickets_added: vec![viewer(SOUTH)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(SOUTH), Knowledge::Held);
    assert_eq!(output.chunk_requests, [SOUTH]);
    assert!(output.claims.is_empty() && output.returns.is_empty());
}

#[test]
fn a_chunk_granted_and_called_foreign_in_one_tick_is_held() {
    // Grants come first, and a region does not unlearn that it holds a chunk.
    let mut world = World::pinned(0);
    world.ask(WEST);
    world.tick(&TickInputs {
        granted: vec![WEST],
        foreign: vec![(WEST, OTHER)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(WEST), Knowledge::Held);
}

#[test]
fn foreign_for_a_chunk_the_region_holds_is_ignored() {
    let mut world = on_open_land(0);
    let output = world.tick(&foreign(&[(HOME, OTHER)]));
    assert_eq!(world.knowledge(HOME), Knowledge::Held);
    assert!(world.region.chunk(HOME).is_some(), "and it stays loaded");
    assert!(output.durable.is_empty(), "the player in it is not let go");
    assert!(world.region.player(player(1)).is_some());
    assert!(output.claims.is_empty() && output.returns.is_empty());
}

// ---------------------------------------------------------------------------------------
// S5: unbelieve
// ---------------------------------------------------------------------------------------

#[test]
fn unbelieve_naming_the_believed_region_asks_again_in_that_tick() {
    let mut world = World::pinned(0);
    world.learn(EAST, REGION_B);
    let output = world.tick(&unbelieve(&[(EAST, REGION_B)]));
    assert_eq!(output.claims, [EAST]);
    assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    assert!(world.idle().claims.is_empty());

    // The new answer is believed like the first.
    world.tick(&foreign(&[(EAST, OTHER)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Foreign(OTHER));
}

#[test]
fn unbelieve_asks_again_for_a_guest_in_a_pinned_area() {
    let mut world = World::pinned(0);
    world.tick(&add(&[guest(WEST)]));
    world.tick(&foreign(&[(WEST, OTHER)]));
    let output = world.tick(&unbelieve(&[(WEST, OTHER)]));
    assert_eq!(output.claims, [WEST]);
    assert_eq!(world.knowledge(WEST), Knowledge::Asked);
}

#[test]
fn unbelieve_naming_another_region_changes_nothing() {
    let mut world = World::pinned(0);
    world.learn(EAST, REGION_B);
    for doubted in [OTHER, REGION_A] {
        let output = world.tick(&unbelieve(&[(EAST, doubted)]));
        assert!(output.claims.is_empty());
        assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));
    }
}

#[test]
fn unbelieve_of_a_held_chunk_changes_nothing() {
    let mut world = on_open_land(0);
    let before = world.region.chunk(HOME).cloned();
    for doubted in [REGION_A, REGION_B, OTHER] {
        let output = world.tick(&unbelieve(&[(HOME, doubted)]));
        assert_eq!(world.knowledge(HOME), Knowledge::Held);
        assert!(output.claims.is_empty() && output.returns.is_empty());
        assert!(output.chunk_requests.is_empty() && output.durable.is_empty());
    }
    assert_eq!(world.region.chunk(HOME).cloned(), before);
    assert!(world.region.player(player(1)).is_some());
}

#[test]
fn unbelieve_of_a_chunk_whose_ticket_goes_in_the_same_tick_forgets_it_without_asking() {
    let mut world = World::pinned(0);
    world.learn(EAST, REGION_B);
    let output = world.tick(&TickInputs {
        unbelieve: vec![(EAST, REGION_B)],
        tickets_removed: vec![viewer(EAST)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    assert!(output.claims.is_empty());
    assert!(world.idle().claims.is_empty());
}

#[test]
fn unbelieve_of_a_chunk_that_is_asked_or_unknown_changes_nothing() {
    let mut world = World::pinned(0);
    world.ask(EAST);
    let output = world.tick(&unbelieve(&[(EAST, REGION_B), (NORTH, REGION_B)]));
    assert!(output.claims.is_empty(), "no second claim");
    assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    assert_eq!(world.knowledge(NORTH), Knowledge::Unknown);
}

#[test]
fn a_belief_that_has_changed_in_the_tick_of_the_doubt_is_left_alone() {
    // The store's answer is taken before the doubt (section 2.1, steps 2.2 and 2.3),
    // and the doubt names the region that was believed before it.
    let mut world = World::pinned(0);
    world.learn(EAST, REGION_B);
    let output = world.tick(&TickInputs {
        foreign: vec![(EAST, OTHER)],
        unbelieve: vec![(EAST, REGION_B)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(EAST), Knowledge::Foreign(OTHER));
    assert!(output.claims.is_empty());
}

#[test]
fn a_doubt_in_the_tick_of_an_answer_that_names_the_same_region_asks_again() {
    // By the order of section 2.1: the answer makes the chunk that region's, and the
    // doubt, which names it, makes the region ask once more.
    let mut world = World::pinned(0);
    world.ask(EAST);
    let output = world.tick(&TickInputs {
        foreign: vec![(EAST, REGION_B)],
        unbelieve: vec![(EAST, REGION_B)],
        ..TickInputs::default()
    });
    assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    assert_eq!(output.claims, [EAST]);
}

// ---------------------------------------------------------------------------------------
// S6: a player in a chunk the region has no answer for
// ---------------------------------------------------------------------------------------

#[test]
fn a_player_who_walks_into_an_unknown_chunk_stays_and_the_chunk_is_claimed_in_the_tick_of_the_step()
{
    let mut world = on_open_land(0);
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    let output = world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    assert_eq!(output.claims, [EAST]);
    assert!(output.durable.is_empty());
    assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    assert_eq!(world.state_of(player(1)).pose.position.x, 17.5);

    // What they do next is applied: a step on, and a block of their region's own chunk
    // broken from where they stand.
    let mut inputs = single_input(E, player(1), entity(1), 2, move_to(18.5));
    inputs.input(E, player(1), entity(1), 3, dig(BORDER_BLOCK_A, 1));
    let output = world.tick(&inputs);
    assert_eq!(acknowledged(&output), [(player(1), 1)]);
    assert_eq!(block_changes(&output), [(BORDER_BLOCK_A, blocks::AIR)]);
    assert!(output.claims.is_empty(), "the chunk is asked for once");
    let state = world.state_of(player(1));
    assert_eq!((state.pose.position.x, state.last_input), (18.5, 3));

    // The store grants the chunk: they stay, and so does the chunk while they do.
    let output = world.tick(&granted(&[EAST]));
    assert!(output.durable.is_empty());
    assert_eq!(world.knowledge(EAST), Knowledge::Held);
    for _ in 0..3 {
        let output = world.idle();
        assert!(output.returns.is_empty() && output.durable.is_empty());
    }
    world.tick(&single_input(E, player(1), entity(1), 4, move_to(19.5)));
    assert_eq!(world.state_of(player(1)).pose.position.x, 19.5);
    // Standing in it loads nothing: that is for the ticket of their edge.
    assert!(world.region.chunk(EAST).is_none());
}

#[test]
fn a_player_in_a_chunk_that_turns_out_to_be_anothers_is_let_go_in_the_tick_of_the_answer() {
    let mut world = on_open_land(0);
    world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    world.tick(&single_input(E, player(1), entity(1), 2, move_to(18.5)));
    assert!(world.idle().durable.is_empty(), "not before the answer");
    let before = world.state_of(player(1));
    assert_eq!((before.pose.position.x, before.last_input), (18.5, 2));
    let home = world.region.chunk(HOME).cloned();

    // The answer comes with more of the player: a step on, a block of the region's own
    // chunk, another slot in hand, a slot emptied, and a step back into the region's
    // chunk. None of it is applied.
    let mut inputs = foreign(&[(EAST, OTHER)]);
    inputs.input(E, player(1), entity(1), 3, move_to(19.5));
    inputs.input(E, player(1), entity(1), 4, dig(BORDER_BLOCK_A, 7));
    inputs.input(
        E,
        player(1),
        entity(1),
        5,
        PlayerInput::SelectSlot { slot: 3 },
    );
    let emptied = PlayerInput::SetHotbarSlot {
        slot: 0,
        stack: None,
    };
    inputs.input(E, player(1), entity(1), 6, emptied);
    inputs.input(E, player(1), entity(1), 7, move_to(14.5));
    let output = world.tick(&inputs);

    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Departed {
                player: player(1),
                transfer: transfer_of(&before),
                to: OTHER,
            }
        )]
    );
    assert!(world.region.player(player(1)).is_none());
    assert!(moved(&output).is_empty(), "the steps were not taken");
    assert!(acknowledged(&output).is_empty() && block_changes(&output).is_empty());
    assert_eq!(world.region.chunk(HOME).cloned(), home);
    assert!(
        removed(&output).is_empty(),
        "a departing entity is not reported removed"
    );
    // Nobody wants the chunk any more.
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    assert!(output.claims.is_empty());
}

#[test]
fn a_player_is_let_go_by_an_answer_that_comes_in_the_tick_after_the_step() {
    let mut world = on_open_land(0);
    world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    let before = world.state_of(player(1));
    let mut inputs = foreign(&[(EAST, REGION_B)]);
    inputs.input(E, player(1), entity(1), 2, move_to(12.5));
    let output = world.tick(&inputs);
    assert_eq!(
        entries(&output),
        [Durable::Departed {
            player: player(1),
            transfer: transfer_of(&before),
            to: REGION_B,
        }]
    );
    assert_eq!(before.last_input, 1);
}

#[test]
fn a_player_who_has_walked_out_again_before_the_answer_stays_whatever_it_is() {
    // Another's: nobody stands in it any more, so nobody is let go.
    let mut world = on_open_land(0);
    world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    world.tick(&single_input(E, player(1), entity(1), 2, move_to(14.5)));
    assert_eq!(world.knowledge(EAST), Knowledge::Asked);
    let output = world.tick(&foreign(&[(EAST, REGION_B)]));
    assert!(output.durable.is_empty());
    assert!(world.region.player(player(1)).is_some());
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);

    // Granted: it is the region's, and nothing uses it.
    let mut world = on_open_land(0);
    world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    world.tick(&single_input(E, player(1), entity(1), 2, move_to(14.5)));
    let output = world.tick(&granted(&[EAST]));
    assert_eq!(output.returns, [EAST]);
    assert!(world.region.player(player(1)).is_some());
}

#[test]
fn a_player_who_walks_on_into_a_second_unknown_chunk_has_both_claimed() {
    let mut world = on_open_land(0);
    let further = ChunkPos::new(2, 0);
    let output = world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    assert_eq!(output.claims, [EAST]);
    let output = world.tick(&single_input(E, player(1), entity(1), 2, move_to(33.5)));
    assert_eq!(output.claims, [further]);

    // The first is another's, and nobody stands in it: the player stays. The second is
    // another's too: they go.
    let output = world.tick(&foreign(&[(EAST, REGION_B)]));
    assert!(output.durable.is_empty());
    let before = world.state_of(player(1));
    let output = world.tick(&foreign(&[(further, OTHER)]));
    assert_eq!(
        entries(&output),
        [Durable::Departed {
            player: player(1),
            transfer: transfer_of(&before),
            to: OTHER,
        }]
    );
}

/// The entity ids region 1 of the stripes gives out.
fn eastern_ids() -> EntityIds {
    EntityIds::block(4).expect("block 4 exists")
}

/// Region 1 of the stripes. It has entity ids, and the chunk players enter the world in
/// is not of its area.
fn eastern() -> World {
    World::new(
        config(0),
        eastern_ids(),
        Holdings {
            held: Vec::new(),
            pinned: vec![EASTERN],
        },
    )
}

#[test]
fn a_player_who_joins_at_a_spawn_point_the_region_knows_nothing_of_stays_until_the_store_answers() {
    let mut world = eastern();
    world.tick(&edges(vec![started(E, 10)]));
    let output = world.tick(&changes(vec![join(E, player(1))]));
    assert_eq!(
        output.claims,
        [HOME],
        "the chunk they stand in is asked for"
    );
    assert!(output.durable.is_empty());
    let joined = world.state_of(player(1));
    assert_eq!(joined.pose.position, SPAWN);
    assert_eq!(world.knowledge(HOME), Knowledge::Asked);
    let output = world.idle();
    assert!(output.durable.is_empty() && output.claims.is_empty());

    // The store names the holder: they are let go to it in that tick, as they joined.
    let mut inputs = foreign(&[(HOME, REGION_A)]);
    inputs.input(E, player(1), joined.entity_id, 1, move_to(13.5));
    let output = world.tick(&inputs);
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Departed {
                player: player(1),
                transfer: transfer_of(&joined),
                to: REGION_A,
            }
        )]
    );
    assert_eq!(joined.last_input, 0);
    assert!(world.region.player(player(1)).is_none());
    assert_eq!(world.knowledge(HOME), Knowledge::Unknown);
}

#[test]
fn a_player_who_joins_at_a_spawn_point_the_region_believes_anothers_is_let_go_at_once() {
    let mut world = eastern();
    world.learn(HOME, REGION_A);
    world.tick(&edges(vec![started(E, 10)]));
    let mut inputs = changes(vec![join(E, player(1))]);
    // The first to enter the region gets the first id of its block.
    inputs.input(E, player(1), eastern_ids().first, 1, move_to(20.5));
    let output = world.tick(&inputs);
    match output.durable.as_slice() {
        [
            (
                E,
                1,
                Durable::Departed {
                    player: id,
                    transfer,
                    to: REGION_A,
                },
            ),
        ] => {
            assert_eq!(*id, player(1));
            assert_eq!(
                transfer.pose.position, SPAWN,
                "what they did is not applied"
            );
            assert_eq!(transfer.last_input, 0);
            assert_eq!(transfer.hotbar, hotbar());
            let block = EntityIds::block(4).expect("block 4 exists");
            assert_eq!(transfer.entity_id, block.first, "the next entity id");
        }
        other => panic!("expected one departure to region 0, got {other:?}"),
    }
    assert!(world.region.player(player(1)).is_none());
    assert_eq!(world.knowledge(HOME), Knowledge::Foreign(REGION_A));
}

#[test]
fn a_player_who_joins_the_home_region_before_it_has_asked_for_its_spawn_chunk_stays() {
    let mut world = World::pinned(0);
    world.tick(&edges(vec![started(E, 10)]));
    let output = world.tick(&changes(vec![join(E, player(1))]));
    assert_eq!(output.claims, [HOME]);
    let output = world.tick(&granted(&[HOME]));
    assert!(output.durable.is_empty());
    assert!(
        output.chunk_requests.is_empty(),
        "a player loads no chunk by standing in it"
    );
    assert_eq!(world.knowledge(HOME), Knowledge::Held);
    world.tick(&single_input(E, player(1), entity(1), 1, move_to(13.5)));
    assert_eq!(world.state_of(player(1)).pose.position.x, 13.5);
}

// ---------------------------------------------------------------------------------------
// S7: a step into a chunk the region believes another's
// ---------------------------------------------------------------------------------------

#[test]
fn a_player_who_steps_into_a_chunk_believed_anothers_is_let_go_in_that_tick_to_that_region() {
    let mut world = at_the_line();
    let before = world.state_of(player(1));
    let home = world.region.chunk(HOME).cloned();

    // A step within the region's own chunk and another slot in hand, the step across,
    // and behind it a block of the region's own chunk, a third slot and a step back,
    // which are for the next region to apply.
    let mut inputs = single_input(E, player(1), entity(1), 1, move_to(12.5));
    inputs.input(
        E,
        player(1),
        entity(1),
        2,
        PlayerInput::SelectSlot { slot: 2 },
    );
    inputs.input(E, player(1), entity(1), 3, move_to(17.5));
    inputs.input(E, player(1), entity(1), 4, dig(OWN_BLOCK, 1));
    inputs.input(
        E,
        player(1),
        entity(1),
        5,
        PlayerInput::SelectSlot { slot: 6 },
    );
    inputs.input(E, player(1), entity(1), 6, move_to(14.5));
    let output = world.tick(&inputs);

    let expected = PlayerTransfer {
        pose: Pose {
            position: Vec3::new(17.5, 64.0, SPAWN.z),
            on_ground: true,
            ..before.pose
        },
        selected_slot: 2,
        last_input: 3,
        ..transfer_of(&before)
    };
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Departed {
                player: player(1),
                transfer: expected,
                to: REGION_B,
            }
        )]
    );
    assert!(world.region.player(player(1)).is_none());
    assert!(acknowledged(&output).is_empty() && block_changes(&output).is_empty());
    assert_eq!(world.region.chunk(HOME).cloned(), home);
    assert!(removed(&output).is_empty());
    assert_eq!(
        world.knowledge(EAST),
        Knowledge::Foreign(REGION_B),
        "the viewer's ticket is still there"
    );
}

#[test]
fn a_player_is_let_go_to_the_region_the_store_named_last() {
    // The answer that changes the holder comes in the tick of the step.
    let mut world = at_the_line();
    let mut inputs = foreign(&[(EAST, OTHER)]);
    inputs.input(E, player(1), entity(1), 1, move_to(17.5));
    let output = world.tick(&inputs);
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::Departed { to: OTHER, .. })]
    ));
}

#[test]
fn a_player_who_steps_into_a_chunk_in_the_tick_it_is_called_anothers_is_let_go_in_that_tick() {
    // `SOUTH` was asked for a viewer's ticket; the answer and the step come together.
    let mut world = at_the_line();
    let mut inputs = foreign(&[(SOUTH, OTHER)]);
    inputs.input(E, player(1), entity(1), 1, walk(8.5, 20.5));
    inputs.input(E, player(1), entity(1), 2, walk(8.5, 8.5));
    let output = world.tick(&inputs);
    match output.durable.as_slice() {
        [(E, 1, Durable::Departed { transfer, to, .. })] => {
            assert_eq!(*to, OTHER);
            assert_eq!(transfer.pose.position, Vec3::new(8.5, 64.0, 20.5));
            assert_eq!(transfer.last_input, 1);
        }
        other => panic!("expected one departure, got {other:?}"),
    }
}

#[test]
fn a_player_who_steps_across_and_back_within_the_inputs_of_one_tick_is_let_go_at_the_first_step() {
    // What a player does while standing in a chunk believed another's is not applied,
    // so the step back is not, and they stand across at the end of the tick.
    let mut world = at_the_line();
    let mut inputs = single_input(E, player(1), entity(1), 1, move_to(16.5));
    inputs.input(E, player(1), entity(1), 2, move_to(14.5));
    let output = world.tick(&inputs);
    match output.durable.as_slice() {
        [(E, 1, Durable::Departed { transfer, to, .. })] => {
            assert_eq!(*to, REGION_B);
            assert_eq!(transfer.pose.position.x, 16.5);
            assert_eq!(transfer.last_input, 1);
        }
        other => panic!("expected one departure, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------
// S8: arrivals
// ---------------------------------------------------------------------------------------

const TRAVELLER: EntityId = EntityId(7_000_001);

/// Points in the four chunks of [`at_the_line`].
const IN_HOME: Vec3 = Vec3::new(12.5, 64.0, 8.5);
const IN_EAST: Vec3 = Vec3::new(20.5, 64.0, 8.5);
const IN_SOUTH: Vec3 = Vec3::new(8.5, 64.0, 20.5);
const IN_NORTH: Vec3 = Vec3::new(8.5, 64.0, -3.5);

fn arrive(edge: EdgeId, id: PlayerId, transfer: PlayerTransfer) -> TickInputs {
    changes(vec![PlayerChange::Arrive(edge, id, transfer)])
}

#[test]
fn an_arrival_for_a_held_chunk_is_taken_in() {
    let mut world = at_the_line();
    let output = world.tick(&arrive(F, player(5), transfer(TRAVELLER, 17, IN_HOME)));
    let state = world.state_of(player(5));
    assert_eq!(
        (state.entity_id, state.edge, state.last_input),
        (TRAVELLER, F, 17)
    );
    assert_eq!(state.pose.position, IN_HOME);
    assert_eq!(spawned(&output), [TRAVELLER]);
    assert!(output.durable.is_empty() && output.claims.is_empty());
}

#[test]
fn an_arrival_for_an_unknown_chunk_is_taken_in_and_the_chunk_claimed() {
    let mut world = at_the_line();
    let output = world.tick(&arrive(F, player(5), transfer(TRAVELLER, 17, IN_NORTH)));
    let state = world.state_of(player(5));
    assert_eq!(
        (state.entity_id, state.edge, state.last_input),
        (TRAVELLER, F, 17)
    );
    assert!(output.durable.is_empty());
    assert_eq!(output.claims, [NORTH]);
    assert_eq!(world.knowledge(NORTH), Knowledge::Asked);

    // They are the region's like any other player: what they do is applied.
    world.tick(&single_input(F, player(5), TRAVELLER, 18, walk(8.5, -4.5)));
    assert_eq!(world.state_of(player(5)).last_input, 18);
    // Once the store grants the chunk, as it does for a pinned region, they stay.
    let output = world.tick(&granted(&[NORTH]));
    assert!(output.durable.is_empty());
    assert!(world.region.player(player(5)).is_some());
}

#[test]
fn an_arrival_for_a_chunk_that_is_asked_for_is_taken_in_and_the_chunk_not_claimed_again() {
    let mut world = at_the_line();
    let output = world.tick(&arrive(F, player(5), transfer(TRAVELLER, 17, IN_SOUTH)));
    assert!(world.region.player(player(5)).is_some());
    assert!(output.durable.is_empty() && output.claims.is_empty());
    assert_eq!(world.knowledge(SOUTH), Knowledge::Asked);
}

#[test]
fn an_arrival_for_a_chunk_believed_anothers_goes_on_to_that_region() {
    let mut world = at_the_line();
    let before = world.region.state();
    let arriving = transfer(TRAVELLER, 17, IN_EAST);
    let output = world.tick(&arrive(F, player(5), arriving.clone()));

    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::NotMine {
                what: Misdirected::Arrival {
                    player: player(5),
                    transfer: arriving,
                },
                holder: REGION_B,
            }
        )]
    );
    assert_eq!(world.region.state().players, before.players);
    assert!(world.region.player(player(5)).is_none());
    assert!(output.events.is_empty(), "no entity is reported");
    assert!(output.player_events.is_empty() && output.claims.is_empty());
    assert!(
        world.region.edge(E).expect("E is known").outbox.is_empty(),
        "the entry is for the edge the player arrived through"
    );
}

#[test]
fn an_arrival_in_the_tick_its_chunk_is_called_anothers_goes_on_although_nothing_wants_the_chunk() {
    // Between a `foreign` and the end of its tick the chunk is believed that region's in
    // either case (section 1.3). `EAST` is outside the area the region is pinned to;
    // of a chunk of its own area it would drop the belief (the test below).
    let mut world = at_the_line();
    world.tick(&remove(&[viewer(EAST)]));
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    let mut inputs = foreign(&[(EAST, OTHER)]);
    inputs.change(PlayerChange::Arrive(
        E,
        player(5),
        transfer(TRAVELLER, 3, IN_EAST),
    ));
    let output = world.tick(&inputs);
    assert!(matches!(
        output.durable.as_slice(),
        [(
            E,
            1,
            Durable::NotMine {
                what: Misdirected::Arrival { .. },
                holder: OTHER,
            }
        )]
    ));
    assert!(world.region.player(player(5)).is_none());
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
}

#[test]
fn an_arrival_for_a_chunk_of_the_regions_own_area_believed_anothers_is_taken_in() {
    // ADR-0014, section 2.2. `SOUTH` is of the stripe the region is pinned to, and the
    // store calls it another's: a part that was split off holds it. Whoever sends a
    // player there since was told by the store that it is this region's again, so the
    // region drops what it believes and asks, in the tick of the answer
    let mut world = at_the_line();
    let mut inputs = foreign(&[(SOUTH, OTHER)]);
    inputs.change(PlayerChange::Arrive(
        E,
        player(5),
        transfer(TRAVELLER, 3, IN_SOUTH),
    ));
    let output = world.tick(&inputs);
    assert!(output.durable.is_empty(), "{:?}", output.durable);
    assert_eq!(world.state_of(player(5)).entity_id, TRAVELLER);
    assert_eq!(output.claims, [SOUTH]);
    assert_eq!(world.knowledge(SOUTH), Knowledge::Asked);

    // and in a later one.
    let mut world = at_the_line();
    world.tick(&foreign(&[(SOUTH, OTHER)]));
    assert_eq!(world.knowledge(SOUTH), Knowledge::Foreign(OTHER));
    let output = world.tick(&arrive(E, player(5), transfer(TRAVELLER, 3, IN_SOUTH)));
    assert!(output.durable.is_empty(), "{:?}", output.durable);
    assert!(world.region.player(player(5)).is_some());
    assert_eq!(output.claims, [SOUTH]);
    // What the store answers then is the truth of that moment: if the part still holds
    // the chunk, the player is let go to it.
    let output = world.tick(&foreign(&[(SOUTH, OTHER)]));
    assert!(matches!(
        entries(&output).as_slice(),
        [Durable::Departed { to: OTHER, .. }]
    ));

    // An arrival that is not taken in for another reason leaves the belief alone:
    // through an edge the region does not know, and of a stay that is no later than the
    // one the region has.
    let mut world = at_the_line();
    world.tick(&foreign(&[(SOUTH, OTHER)]));
    let own = world.state_of(player(1)).entity_id;
    world.tick(&arrive(
        EdgeId(99),
        player(5),
        transfer(TRAVELLER, 3, IN_SOUTH),
    ));
    world.tick(&arrive(E, player(1), transfer(own, 3, IN_SOUTH)));
    assert_eq!(world.knowledge(SOUTH), Knowledge::Foreign(OTHER));
}

#[test]
fn an_arrival_in_the_tick_a_belief_is_doubted_is_taken_in() {
    // The doubt is step 2.3 of the tick and the arrival step 4: the region knows
    // nothing of the chunk when the player comes, and asks because they stand in it.
    let mut world = at_the_line();
    let mut inputs = unbelieve(&[(EAST, REGION_B)]);
    inputs.change(PlayerChange::Arrive(
        F,
        player(5),
        transfer(TRAVELLER, 3, IN_EAST),
    ));
    let output = world.tick(&inputs);
    assert!(output.durable.is_empty());
    assert!(world.region.player(player(5)).is_some());
    assert_eq!(output.claims, [EAST]);
}

#[test]
fn an_arrival_of_an_earlier_stay_of_a_player_who_is_there_changes_nothing_whatever_the_chunk() {
    for position in [IN_HOME, IN_NORTH, IN_SOUTH, IN_EAST] {
        let mut world = at_the_line();
        let mut expected = world.region.state();
        let own = expected.players[&player(1)].entity_id;
        // Of two stays of a player the one with the lower entity id is the earlier
        // (ADR-0014, section 2.1).
        let earlier = EntityId(own.0 - 1);
        let output = world.tick(&arrive(F, player(1), transfer(earlier, 9, position)));
        let gone: Vec<EntityId> = removed(&output).iter().map(|(entity, _)| *entity).collect();
        assert_eq!(gone, [earlier], "the entity on its way is removed");
        assert!(output.durable.is_empty(), "at {position:?}");
        assert!(output.claims.is_empty(), "nobody came to stand there");
        expected.tick += 1;
        assert_eq!(world.region.state(), expected, "at {position:?}");

        // With the entity the player has, there is nothing to remove either.
        let output = world.tick(&arrive(F, player(1), transfer(own, 9, position)));
        assert!(output.events.is_empty() && output.durable.is_empty());
        expected.tick += 1;
        assert_eq!(world.region.state(), expected);
    }
}

#[test]
fn an_arrival_of_a_later_stay_takes_the_place_of_the_one_that_is_there() {
    // ADR-0014, section 2.1: `TRAVELLER` is above the entity the player has here.
    let stood = chunk_of(SPAWN);
    for (position, taken_in) in [
        (IN_HOME, true),
        (IN_NORTH, true),
        (IN_SOUTH, true),
        (IN_EAST, false),
    ] {
        let mut world = at_the_line();
        let own = world.state_of(player(1)).entity_id;
        assert!(own < TRAVELLER);
        let arriving = transfer(TRAVELLER, 9, position);
        let output = world.tick(&arrive(F, player(1), arriving.clone()));
        // The entity that was there is removed where it stood, and then it is an
        // arrival like any other: taken in, or sent on where the chunk is believed
        // another's.
        assert_eq!(removed(&output), [(own, stood)], "at {position:?}");
        if taken_in {
            let state = world.state_of(player(1));
            assert_eq!(
                (state.entity_id, state.edge, state.last_input),
                (TRAVELLER, F, 9)
            );
            assert_eq!(state.pose.position, position);
            assert_eq!(spawned(&output), [TRAVELLER]);
            assert!(output.durable.is_empty());
        } else {
            assert!(world.region.player(player(1)).is_none());
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
        }
    }
}

#[test]
fn an_arrival_through_an_unknown_edge_for_a_chunk_believed_anothers_makes_no_entry() {
    let mut world = at_the_line();
    let before = world.region.state();
    let output = world.tick(&arrive(
        EdgeId(99),
        player(5),
        transfer(TRAVELLER, 3, IN_EAST),
    ));
    assert!(output.durable.is_empty());
    assert_eq!(removed(&output), [(TRAVELLER, EAST)]);
    assert_eq!(world.region.state().edges, before.edges);
    assert_eq!(world.region.state().players, before.players);
}

#[test]
fn a_leave_behind_an_arrival_that_went_on_finds_nobody() {
    // "Found while building", 4: the player is not the region's for the rest of the
    // tick, and the entry stays in the outbox.
    let mut world = at_the_line();
    let arriving = transfer(TRAVELLER, 3, IN_EAST);
    let output = world.tick(&changes(vec![
        PlayerChange::Arrive(E, player(5), arriving.clone()),
        PlayerChange::Leave(E, player(5), None),
    ]));
    assert_eq!(
        entries(&output),
        [Durable::NotMine {
            what: Misdirected::Arrival {
                player: player(5),
                transfer: arriving,
            },
            holder: REGION_B,
        }]
    );
    assert!(output.events.is_empty());
}

/// [`at_the_line`] with an arrival of player 5 through `E` for `EAST`, which went on to
/// region 1: the first entry of `E`'s outbox.
fn sent_on() -> World {
    let mut world = at_the_line();
    let output = world.tick(&arrive(E, player(5), transfer(TRAVELLER, 3, IN_EAST)));
    assert!(matches!(
        output.durable.as_slice(),
        [(E, 1, Durable::NotMine { .. })]
    ));
    world
}

/// The ways an outbox is dropped: the edge starts anew, or stays away too long.
fn droppings() -> [EdgeEvent; 2] {
    [started(E, 20), EdgeEvent::Gone { edge: E }]
}

#[test]
fn dropping_an_outbox_reports_the_entity_of_an_arrival_that_went_on_removed_where_it_stood() {
    for event in droppings() {
        let mut world = sent_on();
        let own = world.state_of(player(1)).entity_id;
        let output = world.tick(&edges(vec![event.clone()]));
        let mut gone = removed(&output);
        gone.sort();
        let mut expected = vec![(own, HOME), (TRAVELLER, EAST)];
        expected.sort();
        assert_eq!(gone, expected, "after {event:?}");
        assert!(output.durable.is_empty());
        assert!(
            world
                .region
                .edge(E)
                .is_none_or(|edge| edge.outbox.is_empty())
        );
    }
}

#[test]
fn dropping_an_outbox_with_two_entries_for_one_entity_reports_it_removed_once() {
    for event in droppings() {
        // The arrival came twice and went on twice.
        let mut world = sent_on();
        let output = world.tick(&arrive(E, player(5), transfer(TRAVELLER, 3, IN_EAST)));
        assert!(matches!(
            output.durable.as_slice(),
            [(E, 2, Durable::NotMine { .. })]
        ));
        let output = world.tick(&edges(vec![event.clone()]));
        let times = removed(&output)
            .iter()
            .filter(|(entity, _)| *entity == TRAVELLER)
            .count();
        assert_eq!(times, 1, "after {event:?}");
        assert!(removed(&output).contains(&(TRAVELLER, EAST)));
    }
}

#[test]
fn dropping_an_outbox_does_not_report_an_entity_that_a_player_of_the_region_has() {
    for event in droppings() {
        for twice in [false, true] {
            // The player came back through another edge, into a chunk the region
            // holds, while the entry for their first arrival waits in `E`'s outbox.
            let mut world = sent_on();
            if twice {
                world.tick(&arrive(E, player(5), transfer(TRAVELLER, 3, IN_EAST)));
            }
            world.tick(&arrive(F, player(5), transfer(TRAVELLER, 3, IN_HOME)));
            assert_eq!(world.state_of(player(5)).entity_id, TRAVELLER);

            let output = world.tick(&edges(vec![event.clone()]));
            assert!(
                removed(&output)
                    .iter()
                    .all(|(entity, _)| *entity != TRAVELLER),
                "after {event:?}: a living entity of another edge's player is reported removed"
            );
            assert!(world.region.player(player(5)).is_some());
        }
    }
}

#[test]
fn dropping_an_outbox_reports_an_entity_that_came_back_through_the_same_edge_removed_once() {
    for event in droppings() {
        // Back through `E` itself: the reset removes them as one of `E`'s players, and
        // the entry for their first arrival names the same entity.
        let mut world = sent_on();
        world.tick(&arrive(E, player(5), transfer(TRAVELLER, 3, IN_HOME)));
        let output = world.tick(&edges(vec![event.clone()]));
        let times = removed(&output)
            .iter()
            .filter(|(entity, _)| *entity == TRAVELLER)
            .count();
        assert_eq!(times, 1, "after {event:?}");
        assert!(world.region.player(player(5)).is_none());
    }
}

#[test]
fn dropping_an_outbox_with_a_departure_and_an_arrival_that_went_on_reports_their_entity_once() {
    for event in droppings() {
        // Player 1 steps across and is let go; their arrival then reaches this region
        // again, for the chunk across the line, and goes on.
        let mut world = at_the_line();
        let own = world.state_of(player(1)).entity_id;
        let output = world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
        let [(E, 1, Durable::Departed { transfer, .. })] = output.durable.as_slice() else {
            panic!("expected a departure, got {:?}", output.durable);
        };
        let output = world.tick(&arrive(E, player(1), transfer.clone()));
        assert!(matches!(
            output.durable.as_slice(),
            [(E, 2, Durable::NotMine { .. })]
        ));

        let output = world.tick(&edges(vec![event.clone()]));
        assert_eq!(removed(&output), [(own, EAST)], "after {event:?}");
    }
}

#[test]
fn an_arrival_that_went_on_and_was_confirmed_is_not_reported_removed() {
    for event in droppings() {
        let mut world = sent_on();
        let own = world.state_of(player(1)).entity_id;
        world.tick(&edges(vec![EdgeEvent::Confirmed { edge: E, number: 1 }]));
        let output = world.tick(&edges(vec![event]));
        assert_eq!(removed(&output), [(own, HOME)]);
    }
}

#[test]
fn another_edges_reset_leaves_an_arrival_that_went_on_alone() {
    let mut world = sent_on();
    let output = world.tick(&edges(vec![started(F, 20)]));
    assert!(output.events.is_empty());
    let output = world.tick(&edges(vec![EdgeEvent::Gone { edge: F }]));
    assert!(output.events.is_empty());
    assert_eq!(world.region.edge(E).expect("E is known").outbox.len(), 1);
}

#[test]
fn dropping_an_outbox_reports_nothing_for_a_remote_action_that_went_on() {
    for event in droppings() {
        let mut world = at_the_line();
        let own = world.state_of(player(1)).entity_id;
        let step = RemoteStep::Break {
            position: BORDER_BLOCK_B,
        };
        let output = world.tick(&TickInputs {
            remote_actions: vec![(E, remote(player(9), 4, step))],
            ..TickInputs::default()
        });
        assert!(matches!(
            output.durable.as_slice(),
            [(E, 1, Durable::NotMine { .. })]
        ));
        let output = world.tick(&edges(vec![event]));
        assert_eq!(removed(&output), [(own, HOME)]);
    }
}

#[test]
fn a_restored_region_reports_the_entity_of_an_arrival_that_went_on_when_its_outbox_is_dropped() {
    // The entry is part of the state, so the next owner knows of the entity too.
    for event in droppings() {
        let original = sent_on();
        let own = original.state_of(player(1)).entity_id;
        let holdings = Holdings {
            held: Vec::new(),
            pinned: vec![WESTERN],
        };
        let mut world = World::restore(config(0), original.region.state(), holdings);
        let output = world.tick(&edges(vec![event]));
        let mut gone = removed(&output);
        gone.sort();
        let mut expected = vec![(own, HOME), (TRAVELLER, EAST)];
        expected.sort();
        assert_eq!(gone, expected);
    }
}

/// Every way two outboxes are dropped in one tick.
fn both_dropped() -> [[EdgeEvent; 2]; 4] {
    let gone = |edge| EdgeEvent::Gone { edge };
    [
        [started(E, 20), started(F, 20)],
        [started(F, 20), started(E, 20)],
        [gone(E), gone(F)],
        [gone(F), started(E, 20)],
    ]
}

fn times_removed(output: &TickOutput, entity: EntityId) -> usize {
    removed(output)
        .iter()
        .filter(|(named, _)| *named == entity)
        .count()
}

#[test]
fn two_outboxes_dropped_in_one_tick_report_an_entity_on_its_way_in_both_removed_once() {
    // The arrival reached the region through both edges, and went on from both. An
    // entity is reported if "it has not been reported in that tick" (section 2.2).
    for events in both_dropped() {
        let mut world = sent_on();
        let output = world.tick(&arrive(F, player(5), transfer(TRAVELLER, 3, IN_EAST)));
        assert!(matches!(
            output.durable.as_slice(),
            [(F, 1, Durable::NotMine { .. })]
        ));
        let output = world.tick(&edges(events.to_vec()));
        let gone = removed(&output);
        assert_eq!(
            times_removed(&output, TRAVELLER),
            1,
            "after {events:?}: {gone:?}"
        );
    }
}

#[test]
fn two_outboxes_dropped_in_one_tick_report_a_departed_entity_in_both_removed_once() {
    for events in both_dropped() {
        // Player 1 steps across and is let go through `E`, comes back through `F` and
        // steps across again: a departure of one entity in either outbox.
        let mut world = at_the_line();
        let own = world.state_of(player(1)).entity_id;
        let output = world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
        let [(E, 1, Durable::Departed { transfer, .. })] = output.durable.as_slice() else {
            panic!("expected a departure, got {:?}", output.durable);
        };
        let back = PlayerTransfer {
            pose: Pose::at(IN_HOME),
            ..transfer.clone()
        };
        world.tick(&arrive(F, player(1), back));
        let output = world.tick(&single_input(F, player(1), entity(1), 2, move_to(17.5)));
        assert!(matches!(
            output.durable.as_slice(),
            [(F, 1, Durable::Departed { .. })]
        ));

        let output = world.tick(&edges(events.to_vec()));
        let gone = removed(&output);
        assert_eq!(times_removed(&output, own), 1, "after {events:?}: {gone:?}");
    }
}

#[test]
fn an_entity_removed_with_its_players_edge_is_not_reported_again_for_another_outbox_of_the_tick() {
    // The player came back through `F` while the entry for their first arrival waits
    // in `E`'s outbox, and both edges are dropped in one tick, in either order: with
    // `F` their player goes, which reports the entity; `E`'s outbox names it as well.
    for events in both_dropped() {
        let mut world = sent_on();
        world.tick(&arrive(F, player(5), transfer(TRAVELLER, 3, IN_HOME)));
        let output = world.tick(&edges(events.to_vec()));
        let gone = removed(&output);
        assert_eq!(
            times_removed(&output, TRAVELLER),
            1,
            "after {events:?}: {gone:?}"
        );
        assert!(world.region.player(player(5)).is_none());
    }
}

#[test]
fn an_arrival_that_is_taken_in_and_steps_across_in_the_same_tick_is_let_go_in_it() {
    // What an edge sends again with an arrival is applied behind it, and a step of it
    // into a chunk believed another's lets the player go like any other.
    let mut world = at_the_line();
    let mut inputs = arrive(F, player(5), transfer(TRAVELLER, 17, IN_HOME));
    inputs.input(F, player(5), TRAVELLER, 18, move_to(13.5));
    inputs.input(F, player(5), TRAVELLER, 19, move_to(17.5));
    inputs.input(F, player(5), TRAVELLER, 20, move_to(13.5));
    let output = world.tick(&inputs);
    match output.durable.as_slice() {
        [(F, 1, Durable::Departed { transfer, to, .. })] => {
            assert_eq!(*to, REGION_B);
            assert_eq!(transfer.entity_id, TRAVELLER);
            assert_eq!(transfer.pose.position.x, 17.5);
            assert_eq!(transfer.last_input, 19);
        }
        other => panic!("expected one departure through F, got {other:?}"),
    }
    assert!(world.region.player(player(5)).is_none());
}

// ---------------------------------------------------------------------------------------
// S9: digging
// ---------------------------------------------------------------------------------------

/// Where a player of [`at_the_line`] stands to reach across into `SOUTH` and `NORTH`,
/// and a block of each within reach from there.
const BY_SOUTH: Vec3 = Vec3::new(8.5, 64.0, 14.5);
const BY_NORTH: Vec3 = Vec3::new(8.5, 64.0, 1.5);
const SOUTH_BLOCK: BlockPos = BlockPos::new(8, 63, 16);
const NORTH_BLOCK: BlockPos = BlockPos::new(8, 63, -1);
/// The blocks of `HOME` they are placed against, or into.
const BY_SOUTH_BLOCK: BlockPos = BlockPos::new(8, 63, 15);
const BY_NORTH_BLOCK: BlockPos = BlockPos::new(8, 63, 0);

/// Blocks of the four chunks that are far out of anyone's reach at the spawn point.
const FAR_BLOCKS: [BlockPos; 4] = [
    BlockPos::new(0, 63, 0),
    BlockPos::new(31, 63, 8),
    BlockPos::new(8, 63, 31),
    BlockPos::new(8, 63, -16),
];

fn break_at(position: BlockPos) -> RemoteStep {
    RemoteStep::Break { position }
}

#[test]
fn a_dig_at_a_block_of_a_chunk_believed_anothers_is_passed_on_with_that_region() {
    let mut world = at_the_line();
    let output = world.tick(&single_input(
        E,
        player(1),
        entity(1),
        1,
        dig(BORDER_BLOCK_B, 5),
    ));
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Remote {
                action: remote(player(1), 5, break_at(BORDER_BLOCK_B)),
                to: Some(REGION_B),
            }
        )]
    );
    assert!(acknowledged(&output).is_empty(), "not acknowledged here");
    assert!(output.events.is_empty());
    let state = world.state_of(player(1));
    assert_eq!((state.last_input, state.handled), (1, None));
    assert!(output.claims.is_empty());
}

#[test]
fn a_dig_at_a_block_of_a_chunk_the_region_has_asked_for_is_passed_on_without_a_region() {
    let mut world = at_the_line();
    world.tick(&single_input(
        E,
        player(1),
        entity(1),
        1,
        walk(BY_SOUTH.x, BY_SOUTH.z),
    ));
    let output = world.tick(&single_input(
        E,
        player(1),
        entity(1),
        2,
        dig(SOUTH_BLOCK, 5),
    ));
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Remote {
                action: remote(player(1), 5, break_at(SOUTH_BLOCK)),
                to: None,
            }
        )]
    );
    assert!(acknowledged(&output).is_empty() && output.events.is_empty());
    let state = world.state_of(player(1));
    assert_eq!((state.last_input, state.handled), (2, None));
    assert!(output.claims.is_empty());
    assert_eq!(world.knowledge(SOUTH), Knowledge::Asked);
}

#[test]
fn a_dig_at_a_block_of_an_unknown_chunk_is_passed_on_without_a_region_and_claims_nothing() {
    let mut world = at_the_line();
    // The step and the dig in one tick: the entry is of the tick of the input.
    let mut inputs = single_input(E, player(1), entity(1), 1, walk(BY_NORTH.x, BY_NORTH.z));
    inputs.input(E, player(1), entity(1), 2, dig(NORTH_BLOCK, 5));
    let output = world.tick(&inputs);
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Remote {
                action: remote(player(1), 5, break_at(NORTH_BLOCK)),
                to: None,
            }
        )]
    );
    assert!(acknowledged(&output).is_empty());
    assert!(block_changes(&output).is_empty());
    let state = world.state_of(player(1));
    assert_eq!((state.last_input, state.handled), (2, None));

    // A click is no reason to take a chunk.
    assert!(output.claims.is_empty());
    assert_eq!(world.knowledge(NORTH), Knowledge::Unknown);
    assert!(world.idle().claims.is_empty());
    assert_eq!(world.knowledge(NORTH), Knowledge::Unknown);
}

#[test]
fn a_dig_in_the_tick_its_chunk_is_called_anothers_names_that_region_although_nothing_wants_the_chunk()
 {
    // Between a `foreign` and the end of its tick the chunk is believed that region's in
    // either case.
    let mut world = at_the_line();
    world.tick(&single_input(
        E,
        player(1),
        entity(1),
        1,
        walk(BY_SOUTH.x, BY_SOUTH.z),
    ));
    world.tick(&remove(&[viewer(SOUTH)]));
    let mut inputs = foreign(&[(SOUTH, OTHER)]);
    inputs.input(E, player(1), entity(1), 2, dig(SOUTH_BLOCK, 5));
    let output = world.tick(&inputs);
    assert_eq!(
        entries(&output),
        [Durable::Remote {
            action: remote(player(1), 5, break_at(SOUTH_BLOCK)),
            to: Some(OTHER),
        }]
    );
    assert_eq!(world.knowledge(SOUTH), Knowledge::Unknown);
}

#[test]
fn a_dig_out_of_reach_is_acknowledged_and_nothing_else_whatever_the_chunk() {
    let mut world = at_the_line();
    let home = world.region.chunk(HOME).cloned();
    for (index, block) in FAR_BLOCKS.into_iter().enumerate() {
        let known = world.knowledge(block.chunk());
        let number = index as u64 + 1;
        let sequence = index as i32 + 20;
        let output = world.tick(&single_input(
            E,
            player(1),
            entity(1),
            number,
            dig(block, sequence),
        ));
        assert_eq!(acknowledged(&output), [(player(1), sequence)], "{block:?}");
        assert!(output.durable.is_empty(), "{block:?}");
        assert!(output.events.is_empty() && output.claims.is_empty());
        assert_eq!(world.knowledge(block.chunk()), known);
        assert_eq!(world.state_of(player(1)).last_input, number);
    }
    assert_eq!(world.region.chunk(HOME).cloned(), home);
}

#[test]
fn a_dig_at_a_block_of_a_held_chunk_that_is_not_loaded_is_acknowledged_without_effect() {
    let mut world = at_the_line();
    // `WEST` is held and nobody watches it; `NORTH` is held and still being loaded.
    world.ask(WEST);
    world.tick(&TickInputs {
        granted: vec![WEST],
        tickets_removed: vec![viewer(WEST)],
        ..TickInputs::default()
    });
    world.ask(NORTH);
    let output = world.tick(&granted(&[NORTH]));
    assert_eq!(output.chunk_requests, [NORTH]);
    assert_eq!(world.knowledge(WEST), Knowledge::Held);

    world.tick(&single_input(E, player(1), entity(1), 1, walk(1.5, 1.5)));
    let mut inputs = single_input(E, player(1), entity(1), 2, dig(BlockPos::new(-1, 63, 1), 8));
    inputs.input(E, player(1), entity(1), 3, dig(BlockPos::new(1, 63, -1), 9));
    let output = world.tick(&inputs);
    assert_eq!(acknowledged(&output).last(), Some(&(player(1), 9)));
    assert!(output.durable.is_empty() && output.events.is_empty());
}

// ---------------------------------------------------------------------------------------
// S10: placing
// ---------------------------------------------------------------------------------------

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

/// Has player 1 of [`at_the_line`], after a step to `from`, use the stone in their hand
/// on `face` of `position`, and returns the entries that made, having checked that
/// nothing was placed or acknowledged and no chunk claimed for it.
fn placed_from(from: Vec3, position: BlockPos, face: Face) -> Vec<(EdgeId, u64, Durable)> {
    let mut world = at_the_line();
    world.tick(&single_input(
        E,
        player(1),
        entity(1),
        1,
        walk(from.x, from.z),
    ));
    let home = world.region.chunk(HOME).cloned();
    let known: Vec<Knowledge> = [HOME, EAST, SOUTH, NORTH]
        .iter()
        .map(|chunk| world.knowledge(*chunk))
        .collect();

    let output = world.tick(&single_input(
        E,
        player(1),
        entity(1),
        2,
        use_on(position, face, 6),
    ));

    assert!(block_changes(&output).is_empty(), "nothing is placed");
    assert_eq!(world.region.chunk(HOME).cloned(), home);
    assert!(acknowledged(&output).is_empty(), "not acknowledged here");
    let state = world.state_of(player(1));
    assert_eq!((state.last_input, state.handled), (2, None));
    assert!(output.claims.is_empty());
    let after: Vec<Knowledge> = [HOME, EAST, SOUTH, NORTH]
        .iter()
        .map(|chunk| world.knowledge(*chunk))
        .collect();
    assert_eq!(after, known);
    output.durable
}

#[test]
fn placing_against_a_held_block_into_a_chunk_believed_anothers_is_passed_on_with_that_region() {
    assert_eq!(
        placed_from(SPAWN, BORDER_BLOCK_A, Face::East),
        vec![(
            E,
            1,
            Durable::Remote {
                action: remote(player(1), 6, place_at(BORDER_BLOCK_B, SPAWN)),
                to: Some(REGION_B),
            }
        )]
    );
}

#[test]
fn placing_against_a_held_block_into_an_unknown_or_asked_chunk_is_passed_on_without_a_region() {
    assert_eq!(
        placed_from(BY_NORTH, BY_NORTH_BLOCK, Face::North),
        vec![(
            E,
            1,
            Durable::Remote {
                action: remote(player(1), 6, place_at(NORTH_BLOCK, BY_NORTH)),
                to: None,
            }
        )]
    );
    assert_eq!(
        placed_from(BY_SOUTH, BY_SOUTH_BLOCK, Face::South),
        vec![(
            E,
            1,
            Durable::Remote {
                action: remote(player(1), 6, place_at(SOUTH_BLOCK, BY_SOUTH)),
                to: None,
            }
        )]
    );
}

#[test]
fn placing_against_a_block_of_a_chunk_believed_anothers_is_passed_on_with_that_region() {
    // Into a spot above it, which is that region's as well, and into one of the
    // region's own chunk: either way the block to place against is the first to ask.
    for (face, target) in [
        (Face::Top, BORDER_BLOCK_B.offset(0, 1, 0)),
        (Face::West, BORDER_BLOCK_A),
    ] {
        assert_eq!(
            placed_from(SPAWN, BORDER_BLOCK_B, face),
            vec![(
                E,
                1,
                Durable::Remote {
                    action: remote(player(1), 6, place_against(BORDER_BLOCK_B, target, SPAWN)),
                    to: Some(REGION_B),
                }
            )]
        );
    }
}

#[test]
fn placing_against_a_block_of_an_unknown_or_asked_chunk_is_passed_on_without_a_region() {
    for (from, against, face) in [
        (BY_NORTH, NORTH_BLOCK, Face::Top),
        (BY_NORTH, NORTH_BLOCK, Face::South),
        (BY_SOUTH, SOUTH_BLOCK, Face::Top),
        (BY_SOUTH, SOUTH_BLOCK, Face::North),
    ] {
        let target = face.neighbour(against);
        assert_eq!(
            placed_from(from, against, face),
            vec![(
                E,
                1,
                Durable::Remote {
                    action: remote(player(1), 6, place_against(against, target, from)),
                    to: None,
                }
            )]
        );
    }
}

#[test]
fn a_placement_without_a_block_in_hand_or_out_of_reach_is_acknowledged_whatever_the_chunks() {
    let mut world = at_the_line();
    // Out of reach, with the stone in hand.
    for (index, block) in FAR_BLOCKS.into_iter().enumerate() {
        let number = index as u64 + 1;
        let sequence = index as i32 + 30;
        let input = use_on(block, Face::Top, sequence);
        let output = world.tick(&single_input(E, player(1), entity(1), number, input));
        assert_eq!(acknowledged(&output), [(player(1), sequence)], "{block:?}");
        assert!(output.durable.is_empty() && output.events.is_empty());
        assert!(output.claims.is_empty());
    }
    // Within reach, with an empty hand.
    world.tick(&single_input(
        E,
        player(1),
        entity(1),
        10,
        PlayerInput::SelectSlot { slot: 4 },
    ));
    for (index, (block, face)) in [
        (BORDER_BLOCK_B, Face::Top),
        (BORDER_BLOCK_A, Face::East),
        (OWN_BLOCK, Face::Top),
    ]
    .into_iter()
    .enumerate()
    {
        let number = index as u64 + 11;
        let sequence = index as i32 + 40;
        let input = use_on(block, face, sequence);
        let output = world.tick(&single_input(E, player(1), entity(1), number, input));
        assert_eq!(acknowledged(&output), [(player(1), sequence)], "{block:?}");
        assert!(output.durable.is_empty() && output.events.is_empty());
    }
}

#[test]
fn a_placement_against_no_block_of_a_held_chunk_is_acknowledged_and_goes_nowhere() {
    // Against the air above the stone of the region's own chunk, into the chunk across
    // the line: there is nothing to place against, so nothing is left to ask.
    let mut world = at_the_line();
    let air = BORDER_BLOCK_A.offset(0, 1, 0);
    let output = world.tick(&single_input(
        E,
        player(1),
        entity(1),
        1,
        use_on(air, Face::East, 6),
    ));
    assert_eq!(acknowledged(&output), [(player(1), 6)]);
    assert!(output.durable.is_empty() && output.events.is_empty());
}

#[test]
fn a_placement_within_the_held_chunk_is_placed_and_acknowledged() {
    let mut world = at_the_line();
    let spot = BlockPos::new(12, 64, 8);
    let output = world.tick(&single_input(
        E,
        player(1),
        entity(1),
        1,
        use_on(spot.offset(0, -1, 0), Face::Top, 6),
    ));
    assert_eq!(block_changes(&output), [(spot, blocks::STONE)]);
    assert_eq!(acknowledged(&output), [(player(1), 6)]);
    assert!(output.durable.is_empty());
    assert_eq!(world.block(spot), Some(blocks::STONE));
}

// ---------------------------------------------------------------------------------------
// S11: what players of other regions do to blocks
// ---------------------------------------------------------------------------------------

/// Where the player of another region stands who does these things.
const ELSEWHERE: Vec3 = Vec3::new(18.5, 64.0, 8.5);

/// Gives [`at_the_line`] one remote action through `F`. Returns the world and what the
/// tick made.
fn remotely(action: RemoteAction) -> (World, TickOutput) {
    let mut world = at_the_line();
    let output = world.tick(&TickInputs {
        remote_actions: vec![(F, action)],
        ..TickInputs::default()
    });
    assert!(
        world.region.edge(E).expect("E is known").outbox.is_empty(),
        "the answer is for the edge the action came through"
    );
    assert!(output.claims.is_empty(), "an action is no reason to claim");
    assert!(acknowledged(&output).is_empty());
    (world, output)
}

#[test]
fn a_remote_action_about_a_held_chunk_is_taken() {
    let (world, output) = remotely(remote(player(9), 4, break_at(OWN_BLOCK)));
    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::RemoteDone {
                player: player(9),
                sequence: 4,
            }
        )]
    );
    assert_eq!(block_changes(&output), [(OWN_BLOCK, blocks::AIR)]);
    assert_eq!(world.block(OWN_BLOCK), Some(blocks::AIR));

    let spot = BlockPos::new(12, 64, 8);
    let (world, output) = remotely(remote(player(9), 5, place_at(spot, ELSEWHERE)));
    assert_eq!(
        entries(&output),
        [Durable::RemoteDone {
            player: player(9),
            sequence: 5,
        }]
    );
    assert_eq!(world.block(spot), Some(blocks::STONE));
}

/// The three steps an action can come as, about `block`.
fn steps_about(block: BlockPos) -> [RemoteStep; 3] {
    [
        break_at(block),
        place_against(block, block.offset(0, 1, 0), ELSEWHERE),
        place_at(block, ELSEWHERE),
    ]
}

#[test]
fn a_remote_action_about_a_chunk_believed_anothers_goes_on_to_that_region_as_it_came() {
    for step in steps_about(BORDER_BLOCK_B) {
        let action = remote(player(9), 4, step);
        let (world, output) = remotely(action.clone());
        assert_eq!(
            output.durable,
            vec![(
                F,
                1,
                Durable::NotMine {
                    what: Misdirected::Remote(action),
                    holder: REGION_B,
                }
            )]
        );
        assert!(output.events.is_empty());
        assert_eq!(world.knowledge(EAST), Knowledge::Foreign(REGION_B));
    }
}

#[test]
fn a_remote_action_about_an_asked_or_unknown_chunk_goes_on_without_a_region_as_it_came() {
    for (block, known) in [
        (SOUTH_BLOCK, Knowledge::Asked),
        (NORTH_BLOCK, Knowledge::Unknown),
    ] {
        for step in steps_about(block) {
            let action = remote(player(9), 4, step);
            let (mut world, output) = remotely(action.clone());
            assert_eq!(
                output.durable,
                vec![(F, 1, Durable::Remote { action, to: None })]
            );
            assert!(output.events.is_empty());
            // Nothing changes: the region knows of the chunk what it did.
            assert_eq!(world.knowledge(block.chunk()), known);
            assert!(world.idle().claims.is_empty());
            assert_eq!(world.knowledge(block.chunk()), known);
        }
    }
}

#[test]
fn a_remote_placement_against_a_held_block_into_a_held_spot_is_placed_and_done() {
    let against = BlockPos::new(12, 63, 8);
    let spot = against.offset(0, 1, 0);
    let (world, output) = remotely(remote(
        player(9),
        4,
        place_against(against, spot, ELSEWHERE),
    ));
    assert_eq!(
        output.durable,
        vec![(
            F,
            1,
            Durable::RemoteDone {
                player: player(9),
                sequence: 4,
            }
        )]
    );
    assert_eq!(block_changes(&output), [(spot, blocks::STONE)]);
    assert_eq!(world.block(spot), Some(blocks::STONE));
}

#[test]
fn a_remote_placement_against_a_held_block_goes_on_to_whoever_has_the_spot() {
    for (against, spot, to) in [
        (BORDER_BLOCK_A, BORDER_BLOCK_B, Some(REGION_B)),
        (BY_NORTH_BLOCK, NORTH_BLOCK, None),
        (BY_SOUTH_BLOCK, SOUTH_BLOCK, None),
    ] {
        let (world, output) = remotely(remote(
            player(9),
            4,
            place_against(against, spot, ELSEWHERE),
        ));
        assert_eq!(
            output.durable,
            vec![(
                F,
                1,
                Durable::Remote {
                    action: remote(player(9), 4, place_at(spot, ELSEWHERE)),
                    to,
                }
            )],
            "into {spot:?}"
        );
        assert!(output.events.is_empty(), "nothing is placed here");
        assert_eq!(world.block(against), Some(blocks::STONE));
    }
}

#[test]
fn a_remote_placement_against_no_block_of_a_held_chunk_is_done_and_goes_nowhere() {
    // The air above the stone at the line, with the spot across it.
    let air = BORDER_BLOCK_A.offset(0, 1, 0);
    let spot = BORDER_BLOCK_B.offset(0, 1, 0);
    let (_, output) = remotely(remote(player(9), 4, place_against(air, spot, ELSEWHERE)));
    assert_eq!(
        entries(&output),
        [Durable::RemoteDone {
            player: player(9),
            sequence: 4,
        }]
    );
    assert!(output.events.is_empty());
}

#[test]
fn a_remote_action_in_the_tick_its_chunk_is_called_anothers_goes_on_to_that_region() {
    // `EAST` is outside the area the region is pinned to, and nothing wants it any
    // more; of a chunk of its own area the region would drop the belief (the test
    // below).
    let mut world = at_the_line();
    world.tick(&remove(&[viewer(EAST)]));
    let action = remote(player(9), 4, break_at(BORDER_BLOCK_B));
    let output = world.tick(&TickInputs {
        foreign: vec![(EAST, OTHER)],
        remote_actions: vec![(F, action.clone())],
        ..TickInputs::default()
    });
    assert_eq!(
        entries(&output),
        [Durable::NotMine {
            what: Misdirected::Remote(action),
            holder: OTHER,
        }]
    );
}

#[test]
fn a_remote_action_about_a_chunk_of_the_regions_own_area_believed_anothers_goes_on_without_a_region()
 {
    // ADR-0014, section 2.2: the belief goes, and the action is answered as one about a
    // chunk the region knows nothing of. The viewer's ticket on `SOUTH` still wants the
    // chunk, so the region asks again.
    for step in steps_about(SOUTH_BLOCK) {
        let mut world = at_the_line();
        world.tick(&foreign(&[(SOUTH, OTHER)]));
        assert_eq!(world.knowledge(SOUTH), Knowledge::Foreign(OTHER));
        let action = remote(player(9), 4, step);
        let output = world.tick(&TickInputs {
            remote_actions: vec![(F, action.clone())],
            ..TickInputs::default()
        });
        assert_eq!(
            output.durable,
            vec![(F, 1, Durable::Remote { action, to: None })]
        );
        assert_eq!(output.claims, [SOUTH]);
        assert_eq!(world.knowledge(SOUTH), Knowledge::Asked);
    }

    // With nothing that wants the chunk it is unknown afterwards, and an action through
    // an edge the region does not know, which is not answered, changes nothing.
    let action = remote(player(9), 4, break_at(SOUTH_BLOCK));
    let mut world = at_the_line();
    let output = world.tick(&TickInputs {
        foreign: vec![(SOUTH, OTHER)],
        tickets_removed: vec![viewer(SOUTH)],
        remote_actions: vec![(F, action.clone())],
        ..TickInputs::default()
    });
    assert_eq!(
        entries(&output),
        [Durable::Remote {
            action: action.clone(),
            to: None
        }]
    );
    assert!(output.claims.is_empty());
    assert_eq!(world.knowledge(SOUTH), Knowledge::Unknown);

    let mut world = at_the_line();
    world.tick(&foreign(&[(SOUTH, OTHER)]));
    let output = world.tick(&TickInputs {
        remote_actions: vec![(EdgeId(99), action)],
        ..TickInputs::default()
    });
    assert!(output.durable.is_empty() && output.claims.is_empty());
    assert_eq!(world.knowledge(SOUTH), Knowledge::Foreign(OTHER));
}

// ---------------------------------------------------------------------------------------
// Section 2.5: the order of a tick's outbox entries
// ---------------------------------------------------------------------------------------

#[test]
fn the_outbox_entries_of_a_tick_are_in_the_order_of_the_steps_that_make_them() {
    // Entity ids for four players, so that a fifth is refused.
    let four = EntityIds {
        first: EntityId(500),
        end: EntityId(504),
    };
    let holdings = Holdings {
        held: Vec::new(),
        pinned: vec![WESTERN],
    };
    let mut world = World::new(config(0), four, holdings);
    world.hold(HOME);
    world.learn(EAST, REGION_B);
    world.tick(&edges(vec![started(E, 10), started(F, 10)]));
    world.tick(&changes(vec![
        join(E, player(1)),
        join(F, player(2)),
        join(E, player(3)),
        join(F, player(4)),
    ]));
    assert_eq!(world.region.player_count(), 4);

    // One tick with an entry of every kind for both edges, each kind given in an order
    // that is not that of the edges or of the players.
    let arriving_5 = transfer(EntityId(7_000_005), 0, IN_EAST);
    let arriving_7 = transfer(EntityId(7_000_007), 0, IN_EAST);
    let done = remote(player(20), 1, break_at(OWN_BLOCK));
    let theirs = remote(player(21), 2, break_at(BORDER_BLOCK_B));
    let nobodys = remote(player(22), 3, break_at(NORTH_BLOCK));
    let mut inputs = changes(vec![
        PlayerChange::Arrive(E, player(5), arriving_5.clone()),
        join(F, player(6)),
        PlayerChange::Arrive(F, player(7), arriving_7.clone()),
        join(E, player(8)),
    ]);
    inputs.remote_actions = vec![(F, done), (E, theirs.clone()), (F, nobodys.clone())];
    // The four entered in the order of their numbers and have the block's four ids.
    let entity = |n: i32| EntityId(four.first.0 + n - 1);
    inputs.input(F, player(4), entity(4), 1, move_to(17.5));
    inputs.input(E, player(1), entity(1), 1, dig(BORDER_BLOCK_B, 11));
    inputs.input(
        F,
        player(2),
        entity(2),
        1,
        use_on(BORDER_BLOCK_A, Face::East, 12),
    );
    inputs.input(E, player(3), entity(3), 1, move_to(18.5));
    inputs.input(E, player(3), entity(3), 2, dig(BORDER_BLOCK_B, 13));
    let output = world.tick(&inputs);

    let departed = |index: usize| match &output.durable[index].2 {
        Durable::Departed {
            player, transfer, ..
        } => (*player, transfer.clone()),
        other => panic!("entry {index} is {other:?}"),
    };
    assert_eq!(output.durable.len(), 11, "{:?}", output.durable);
    let (_, transfer_3) = departed(9);
    let (_, transfer_4) = departed(10);
    assert_eq!(
        output.durable,
        vec![
            // Step 4, in the order of the changes.
            (
                E,
                1,
                Durable::NotMine {
                    what: Misdirected::Arrival {
                        player: player(5),
                        transfer: arriving_5,
                    },
                    holder: REGION_B,
                }
            ),
            (F, 1, Durable::Refused { player: player(6) }),
            (
                F,
                2,
                Durable::NotMine {
                    what: Misdirected::Arrival {
                        player: player(7),
                        transfer: arriving_7,
                    },
                    holder: REGION_B,
                }
            ),
            (E, 2, Durable::Refused { player: player(8) }),
            // Step 5, one for one.
            (
                F,
                3,
                Durable::RemoteDone {
                    player: player(20),
                    sequence: 1,
                }
            ),
            (
                E,
                3,
                Durable::NotMine {
                    what: Misdirected::Remote(theirs),
                    holder: REGION_B,
                }
            ),
            (
                F,
                4,
                Durable::Remote {
                    action: nobodys,
                    to: None,
                }
            ),
            // Step 6, in the order of the inputs.
            (
                E,
                4,
                Durable::Remote {
                    action: remote(player(1), 11, break_at(BORDER_BLOCK_B)),
                    to: Some(REGION_B),
                }
            ),
            (
                F,
                5,
                Durable::Remote {
                    action: remote(player(2), 12, place_at(BORDER_BLOCK_B, SPAWN)),
                    to: Some(REGION_B),
                }
            ),
            // Step 8, in the order of the players, with the highest numbers.
            (
                E,
                5,
                Durable::Departed {
                    player: player(3),
                    transfer: transfer_3.clone(),
                    to: REGION_B,
                }
            ),
            (
                F,
                6,
                Durable::Departed {
                    player: player(4),
                    transfer: transfer_4,
                    to: REGION_B,
                }
            ),
        ]
    );
    assert_eq!(
        transfer_3.last_input, 1,
        "the dig behind the step is not applied"
    );
    assert_eq!(world.region.player_count(), 2);
}

// ---------------------------------------------------------------------------------------
// S12: giving chunks back
// ---------------------------------------------------------------------------------------

#[test]
fn on_open_land_a_held_chunk_that_nothing_uses_is_given_back_once_and_forgotten() {
    let mut world = World::open(0, &[HOME, EAST, WEST]);
    let output = world.idle();
    assert_eq!(output.returns, [WEST, EAST], "in ascending order");
    assert_eq!(world.knowledge(WEST), Knowledge::Unknown);
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    assert_eq!(world.region.held_chunk_count(), 1);
    for _ in 0..5 {
        let output = world.idle();
        assert!(output.returns.is_empty() && output.claims.is_empty());
    }

    // Returning forgets the chunk: needed again, it is claimed again.
    let output = world.tick(&add(&[viewer(EAST)]));
    assert_eq!(output.claims, [EAST]);
    assert!(output.returns.is_empty());
}

#[test]
fn a_chunk_with_a_player_in_it_or_a_ticket_of_either_kind_is_not_given_back() {
    let mut world = on_open_land(0);
    // `SOUTH` for a viewer's ticket, `WEST` for a guest's and `EAST` for the player.
    world.hold(SOUTH);
    world.hold(WEST);
    world.tick(&TickInputs {
        tickets_added: vec![guest(WEST)],
        tickets_removed: vec![viewer(WEST)],
        ..TickInputs::default()
    });
    let output = world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    assert_eq!(output.claims, [EAST]);
    world.tick(&granted(&[EAST]));

    for _ in 0..10 {
        assert!(world.idle().returns.is_empty());
    }
    for chunk in [HOME, EAST, WEST, SOUTH] {
        assert_eq!(world.knowledge(chunk), Knowledge::Held);
    }
    assert!(
        world.region.chunk(WEST).is_some(),
        "a guest's ticket keeps the chunk loaded as well"
    );

    // Each goes at the end of the tick in which its use ends.
    let output = world.tick(&remove(&[viewer(SOUTH)]));
    assert_eq!(output.returns, [SOUTH]);
    let output = world.tick(&remove(&[guest(WEST)]));
    assert_eq!(output.returns, [WEST]);
    let output = world.tick(&single_input(E, player(1), entity(1), 2, move_to(14.5)));
    assert_eq!(output.returns, [EAST]);
    for chunk in [EAST, WEST, SOUTH] {
        assert_eq!(world.knowledge(chunk), Knowledge::Unknown);
    }
}

#[test]
fn the_chunk_of_the_spawn_point_is_never_given_back() {
    for return_after in [0, 1, 5] {
        // Held from the start, and nothing ever uses it.
        let mut world = World::open(return_after, &[HOME]);
        for _ in 0..return_after + 12 {
            assert!(world.idle().returns.is_empty());
        }
        assert_eq!(world.knowledge(HOME), Knowledge::Held);

        // Used and then left: the player walks off and the ticket goes.
        let mut world = on_open_land(return_after);
        world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
        world.tick(&remove(&[viewer(HOME)]));
        world.tick(&granted(&[EAST]));
        for _ in 0..return_after + 12 {
            assert!(world.idle().returns.is_empty());
        }
        assert_eq!(world.knowledge(HOME), Knowledge::Held);
        assert!(world.region.chunk(HOME).is_none(), "but it is not loaded");
    }
}

#[test]
fn with_the_longest_time_before_a_return_nothing_is_ever_given_back() {
    let holdings = Holdings {
        held: vec![HOME, EAST],
        pinned: Vec::new(),
    };
    let mut world = World::restore(config(u64::MAX), after_a_hundred_ticks(), holdings);
    world.tick(&add(&[guest(EAST)]));
    world.tick(&remove(&[guest(EAST)]));
    for _ in 0..10 {
        assert!(world.idle().returns.is_empty());
    }
    assert_eq!(world.knowledge(EAST), Knowledge::Held);
}

/// In no tick.
const NEVER: [u64; 0] = [];

/// Ticks `world` idly up to and including tick `last`, and returns the numbers of the
/// ticks that gave `chunk` back.
fn given_back(world: &mut World, chunk: ChunkPos, last: u64) -> Vec<u64> {
    let mut ticks = Vec::new();
    while world.region.tick_number() < last {
        let output = world.idle();
        if output.returns.contains(&chunk) {
            ticks.push(output.tick);
        }
    }
    ticks
}

#[test]
fn a_chunk_is_given_back_return_after_ticks_after_the_first_tick_in_which_nothing_used_it() {
    // A new region: its first tick is the first in which nothing uses the chunk.
    for return_after in [1, 5] {
        let mut world = World::open(return_after, &[HOME, EAST]);
        assert_eq!(
            given_back(&mut world, EAST, 20),
            [1 + return_after],
            "with return_after {return_after}"
        );
        assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    }
}

#[test]
fn a_ticket_of_either_kind_for_one_tick_starts_the_time_before_a_return_anew() {
    for kind in [Ticket::Viewer, Ticket::Guest] {
        // There at the end of tick 3 alone: tick 4 is the first without use again.
        let mut world = World::open(5, &[HOME, EAST]);
        assert_eq!(given_back(&mut world, EAST, 2), NEVER);
        world.tick(&add(&[(EAST, kind)]));
        world.tick(&remove(&[(EAST, kind)]));
        assert_eq!(given_back(&mut world, EAST, 20), [9], "a {kind:?}'s ticket");

        // At the last moment: there at the end of tick 5, gone in tick 6.
        let mut world = World::open(5, &[HOME, EAST]);
        assert_eq!(given_back(&mut world, EAST, 4), NEVER);
        world.tick(&add(&[(EAST, kind)]));
        let output = world.tick(&remove(&[(EAST, kind)]));
        assert!(output.returns.is_empty());
        assert_eq!(
            given_back(&mut world, EAST, 20),
            [11],
            "a {kind:?}'s ticket"
        );
    }
}

#[test]
fn a_ticket_that_comes_and_goes_within_one_tick_is_no_use_at_the_end_of_any() {
    // "Nothing uses it at the end of that tick": the ticket of tick 3 was gone by then.
    let mut world = World::open(5, &[HOME, EAST]);
    assert_eq!(given_back(&mut world, EAST, 2), NEVER);
    world.tick(&TickInputs {
        tickets_added: vec![guest(EAST)],
        tickets_removed: vec![guest(EAST)],
        ..TickInputs::default()
    });
    assert_eq!(given_back(&mut world, EAST, 20), [6]);
}

#[test]
fn a_player_who_passes_through_starts_the_time_before_a_return_anew() {
    let mut world = on_open_land(5);
    let step = world.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    assert_eq!(step.claims, [EAST]);
    world.tick(&granted(&[EAST]));
    for _ in 0..8 {
        assert!(world.idle().returns.is_empty(), "while they stand in it");
    }
    let left = world.tick(&single_input(E, player(1), entity(1), 2, move_to(14.5)));
    assert!(left.returns.is_empty());
    assert_eq!(
        given_back(&mut world, EAST, left.tick + 20),
        [left.tick + 5]
    );
}

#[test]
fn the_time_before_a_return_starts_with_the_grant() {
    // Asked for long before, for a ticket that has gone: the ticks before the grant
    // count as ticks in which the chunk was used.
    let mut world = World::open(5, &[]);
    world.ask(EAST);
    world.tick(&remove(&[viewer(EAST)]));
    for _ in 0..9 {
        world.idle();
    }
    let grant = world.tick(&granted(&[EAST]));
    assert!(grant.returns.is_empty());
    assert_eq!(
        given_back(&mut world, EAST, grant.tick + 20),
        [grant.tick + 5]
    );

    // Granted with a ticket on it that goes two ticks later.
    let mut world = World::open(5, &[]);
    world.ask(EAST);
    world.tick(&granted(&[EAST]));
    world.idle();
    let gone = world.tick(&remove(&[viewer(EAST)]));
    assert_eq!(
        given_back(&mut world, EAST, gone.tick + 20),
        [gone.tick + 5]
    );
}

#[test]
fn a_grant_of_a_chunk_the_region_holds_already_changes_nothing() {
    // The record has no row for it. The store answers so when a region claims what it
    // holds (ADR-0011, section 3.2), and says that nothing about the chunk changes;
    // read here as: nor does the time it has been without use begin anew.
    let mut world = World::open(5, &[HOME, EAST]);
    assert_eq!(given_back(&mut world, EAST, 3), NEVER);
    let output = world.tick(&granted(&[EAST, HOME]));
    assert!(output.returns.is_empty() && output.chunk_requests.is_empty());
    assert_eq!(given_back(&mut world, EAST, 20), [6]);
    assert_eq!(world.knowledge(HOME), Knowledge::Held);
}

/// The state of a region that has run for a hundred ticks and has nobody in it.
fn after_a_hundred_ticks() -> RegionState {
    RegionState {
        tick: 100,
        ..RegionState::new(ids())
    }
}

#[test]
fn the_time_before_a_return_starts_anew_with_a_restore() {
    let holdings = Holdings {
        held: vec![HOME, EAST, WEST],
        pinned: Vec::new(),
    };
    let mut world = World::restore(config(5), after_a_hundred_ticks(), holdings.clone());
    assert_eq!(given_back(&mut world, EAST, 130), [106]);

    // A ticket that comes with an edge's hello within that time keeps the chunk, and
    // the others go when their time has come.
    let mut world = World::restore(config(5), after_a_hundred_ticks(), holdings.clone());
    assert_eq!(given_back(&mut world, EAST, 104), NEVER);
    world.tick(&add(&[guest(EAST)]));
    let output = world.idle();
    assert_eq!((output.tick, output.returns), (106, vec![WEST]));
    assert_eq!(given_back(&mut world, EAST, 130), NEVER);
    assert_eq!(world.knowledge(EAST), Knowledge::Held);

    // With no time to wait, everything nothing uses goes with the first tick.
    let mut world = World::restore(config(0), after_a_hundred_ticks(), holdings);
    let output = world.idle();
    assert_eq!((output.tick, output.returns), (101, vec![WEST, EAST]));
}

#[test]
fn a_chunk_is_never_given_back_and_claimed_in_one_tick() {
    // The last ticket goes and a new one comes in the same tick: it is used, so it
    // stays; nothing is given back and nothing claimed.
    let mut world = World::open(0, &[]);
    world.hold(EAST);
    let output = world.tick(&TickInputs {
        tickets_added: vec![guest(EAST)],
        tickets_removed: vec![viewer(EAST)],
        ..TickInputs::default()
    });
    assert!(output.returns.is_empty() && output.claims.is_empty());
    assert!(world.region.chunk(EAST).is_some());

    // Given back in one tick and wanted in the next: claimed then, not before.
    let output = world.tick(&remove(&[guest(EAST)]));
    assert_eq!(output.returns, [EAST]);
    assert!(output.claims.is_empty());
    let output = world.tick(&add(&[viewer(EAST)]));
    assert_eq!(output.claims, [EAST]);
    assert!(output.returns.is_empty());
}

// ---------------------------------------------------------------------------------------
// S13: loading
// ---------------------------------------------------------------------------------------

#[test]
fn a_ticket_of_either_kind_on_a_chunk_that_is_not_held_asks_storage_for_nothing() {
    // Believed another's, asked for and, on open land for a guest, unknown.
    let mut world = at_the_line();
    let output = world.tick(&add(&[guest(EAST), viewer(EAST), guest(SOUTH)]));
    assert!(output.chunk_requests.is_empty());
    let mut open = World::open(0, &[]);
    let output = open.tick(&add(&[guest(EAST)]));
    assert!(output.chunk_requests.is_empty());
    for _ in 0..4 {
        assert!(world.idle().chunk_requests.is_empty());
        assert!(open.idle().chunk_requests.is_empty());
    }

    // What storage delivers all the same is dropped.
    world.tick(&delivered(&[EAST, SOUTH]));
    open.tick(&delivered(&[EAST]));
    assert!(world.region.chunk(EAST).is_none() && world.region.chunk(SOUTH).is_none());
    assert!(open.region.chunk(EAST).is_none());
    assert_eq!(world.region.loaded_chunk_count(), 1, "the home chunk alone");
    assert_eq!(open.region.loaded_chunk_count(), 0);
}

#[test]
fn a_chunk_delivered_after_its_last_ticket_went_is_not_taken() {
    let mut world = World::pinned(0);
    world.ask(WEST);
    let output = world.tick(&granted(&[WEST]));
    assert_eq!(output.chunk_requests, [WEST]);
    world.tick(&remove(&[viewer(WEST)]));
    world.tick(&delivered(&[WEST]));
    assert!(world.region.chunk(WEST).is_none());
    assert_eq!(world.region.loaded_chunk_count(), 0);

    // A ticket that comes back asks storage again, and that answer is taken.
    let output = world.tick(&add(&[guest(WEST)]));
    assert_eq!(output.chunk_requests, [WEST]);
    assert!(world.idle().chunk_requests.is_empty(), "asked once");
    world.tick(&delivered(&[WEST]));
    assert!(world.region.chunk(WEST).is_some());
}

#[test]
fn a_chunk_delivered_in_the_tick_its_last_ticket_goes_is_not_taken() {
    let mut world = World::pinned(0);
    world.ask(WEST);
    world.tick(&granted(&[WEST]));
    world.tick(&TickInputs {
        tickets_removed: vec![viewer(WEST)],
        chunks_loaded: vec![(WEST, stone_chunk())],
        ..TickInputs::default()
    });
    assert!(world.region.chunk(WEST).is_none());
}

#[test]
fn a_loaded_chunk_is_dropped_with_its_last_ticket_and_so_is_in_no_returns() {
    // Without a time to wait, it is dropped at the start of the tick that gives it
    // back at its end.
    let mut world = World::open(0, &[]);
    world.hold(EAST);
    world.tick(&add(&[guest(EAST)]));
    world.tick(&remove(&[viewer(EAST)]));
    for _ in 0..3 {
        assert!(world.idle().returns.is_empty());
        assert!(world.region.chunk(EAST).is_some(), "loaded for the guest");
    }
    let output = world.tick(&remove(&[guest(EAST)]));
    assert_eq!(output.returns, [EAST]);
    assert!(world.region.chunk(EAST).is_none());

    // With one, it is held and not loaded meanwhile, and nothing is asked of storage.
    let mut world = World::open(3, &[]);
    world.hold(EAST);
    let gone = world.tick(&remove(&[viewer(EAST)]));
    assert!(world.region.chunk(EAST).is_none());
    assert_eq!(world.knowledge(EAST), Knowledge::Held);
    for _ in 0..2 {
        let output = world.idle();
        assert!(output.returns.is_empty() && output.chunk_requests.is_empty());
        assert!(world.region.chunk(EAST).is_none());
    }
    let output = world.idle();
    assert_eq!((output.tick, output.returns), (gone.tick + 3, vec![EAST]));
}

#[test]
fn what_storage_delivers_for_a_chunk_that_was_not_asked_of_it_is_dropped() {
    // The chunk is loaded and has been changed since; a second delivery of it, which
    // nothing asked for, must not put the stored chunk in its place.
    let mut world = on_open_land(0);
    world.tick(&single_input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 1)));
    assert_eq!(world.block(OWN_BLOCK), Some(blocks::AIR));
    world.tick(&delivered(&[HOME]));
    assert_eq!(world.block(OWN_BLOCK), Some(blocks::AIR));
}

// ---------------------------------------------------------------------------------------
// S14: a restore
// ---------------------------------------------------------------------------------------

#[test]
fn a_restored_region_knows_what_the_store_says_it_holds_and_nothing_else() {
    let original = at_the_line();
    assert_eq!(original.knowledge(EAST), Knowledge::Foreign(REGION_B));
    assert_eq!(original.knowledge(SOUTH), Knowledge::Asked);
    let state = original.region.state();
    let holdings = Holdings {
        held: vec![HOME, WEST],
        pinned: vec![WESTERN],
    };
    let mut world = World::restore(config(0), state.clone(), holdings);
    assert_eq!(world.region.state(), state);
    assert_eq!(world.knowledge(HOME), Knowledge::Held);
    assert_eq!(world.knowledge(WEST), Knowledge::Held);
    for chunk in [EAST, SOUTH, NORTH, ChunkPos::new(-7, 3)] {
        assert_eq!(world.knowledge(chunk), Knowledge::Unknown);
    }
    assert_eq!(world.region.held_chunk_count(), 2);
    assert_eq!(world.region.loaded_chunk_count(), 0);

    // Nothing is asked until the edges are back with their tickets; then everything
    // that was believed or asked before is asked again, and what is held is loaded.
    let output = world.idle();
    assert!(output.claims.is_empty() && output.returns.is_empty());
    assert!(output.chunk_requests.is_empty());
    let output = world.tick(&add(&[viewer(HOME), viewer(EAST), viewer(SOUTH)]));
    assert_eq!(output.claims, [SOUTH, EAST]);
    assert_eq!(output.chunk_requests, [HOME]);
}

#[test]
fn a_restored_region_given_the_same_tickets_and_answers_believes_the_same() {
    // Section 1.3: what a region believes follows from what it holds, its tickets and
    // the store's answers alone.
    let mut original = at_the_line();
    original.tick(&foreign(&[(SOUTH, OTHER)]));
    original.tick(&add(&[guest(WEST), guest(EAST)]));
    original.tick(&granted(&[WEST]));
    let chunks = [HOME, EAST, SOUTH, WEST, NORTH];
    let believed = chunks.map(|chunk| original.knowledge(chunk));
    assert_eq!(
        believed,
        [
            Knowledge::Held,
            Knowledge::Foreign(REGION_B),
            Knowledge::Foreign(OTHER),
            Knowledge::Held,
            Knowledge::Unknown,
        ]
    );

    // The store names no chunk of a pinned area as held: all of it is asked again.
    let holdings = Holdings {
        held: Vec::new(),
        pinned: vec![WESTERN],
    };
    let mut world = World::restore(config(0), original.region.state(), holdings);
    let output = world.tick(&add(&[
        viewer(HOME),
        viewer(EAST),
        viewer(SOUTH),
        guest(WEST),
        guest(EAST),
    ]));
    assert_eq!(output.claims, [WEST, HOME, SOUTH, EAST]);
    world.tick(&TickInputs {
        granted: vec![WEST, HOME],
        foreign: vec![(SOUTH, OTHER), (EAST, REGION_B)],
        ..TickInputs::default()
    });
    assert_eq!(chunks.map(|chunk| world.knowledge(chunk)), believed);
    assert_eq!(
        world.region.state().players,
        original.region.state().players
    );
}

/// The state of a region on open land whose player 1 has walked into `EAST`, and the
/// player as they are.
fn standing_in_east() -> (RegionState, PlayerState) {
    let mut original = on_open_land(0);
    original.tick(&single_input(E, player(1), entity(1), 1, move_to(17.5)));
    let state = original.region.state();
    let standing = state.players[&player(1)].clone();
    (state, standing)
}

#[test]
fn a_restored_player_in_a_chunk_the_store_does_not_name_stays_and_the_chunk_is_claimed() {
    let (state, standing) = standing_in_east();
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut world = World::restore(config(0), state, holdings);
    assert_eq!(world.knowledge(EAST), Knowledge::Unknown);
    assert_eq!(world.state_of(player(1)), standing);

    // The first tick claims the chunk, and what the player does in it is applied: the
    // region does not believe the chunk another's.
    let output = world.tick(&single_input(E, player(1), entity(1), 2, move_to(18.5)));
    assert_eq!(output.claims, [EAST]);
    assert!(output.durable.is_empty() && output.returns.is_empty());
    let state = world.state_of(player(1));
    assert_eq!((state.pose.position.x, state.last_input), (18.5, 2));
    assert!(world.idle().durable.is_empty());

    // The answer lets the player go or not, as it would have.
    let output = world.tick(&foreign(&[(EAST, REGION_B)]));
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::Departed {
                player: player(1),
                transfer: transfer_of(&state),
                to: REGION_B,
            }
        )]
    );
}

#[test]
fn a_restored_player_in_a_chunk_that_is_then_granted_stays() {
    let (state, _) = standing_in_east();
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut world = World::restore(config(0), state, holdings);
    let output = world.idle();
    assert_eq!(output.claims, [EAST]);
    let output = world.tick(&granted(&[EAST]));
    assert!(output.durable.is_empty() && output.returns.is_empty());
    assert!(world.region.player(player(1)).is_some());
    assert_eq!(world.knowledge(EAST), Knowledge::Held);
}

#[test]
fn a_restored_pinned_region_claims_the_chunk_its_player_stands_in_again() {
    // The store names only what it granted beyond the pinned areas: of a chunk of its
    // own area the region knows nothing until it has asked again.
    let original = at_the_line();
    let holdings = Holdings {
        held: Vec::new(),
        pinned: vec![WESTERN],
    };
    let mut world = World::restore(config(0), original.region.state(), holdings);
    assert_eq!(world.knowledge(HOME), Knowledge::Unknown);
    let output = world.idle();
    assert_eq!(output.claims, [HOME]);
    assert!(output.durable.is_empty());
    assert!(world.region.player(player(1)).is_some());
    let output = world.tick(&granted(&[HOME]));
    assert!(output.durable.is_empty());
    assert_eq!(world.knowledge(HOME), Knowledge::Held);
}

// ---------------------------------------------------------------------------------------
// S15: a region without entity ids
// ---------------------------------------------------------------------------------------

#[test]
fn a_region_with_the_empty_block_of_entity_ids_answers_every_join_with_refused() {
    // The region a split makes: `first` and `end` both 0, and pinned to nothing.
    let empty = EntityIds {
        first: EntityId(0),
        end: EntityId(0),
    };
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut world = World::new(config(0), empty, holdings);
    world.tick(&edges(vec![started(E, 10), started(F, 10)]));
    let output = world.tick(&changes(vec![join(E, player(1)), join(F, player(2))]));
    assert_eq!(
        output.durable,
        vec![
            (E, 1, Durable::Refused { player: player(1) }),
            (F, 1, Durable::Refused { player: player(2) }),
        ]
    );
    assert_eq!(world.region.player_count(), 0);
    assert!(output.events.is_empty() && output.player_events.is_empty());

    let output = world.tick(&changes(vec![join(E, player(1))]));
    assert_eq!(
        output.durable,
        vec![(E, 2, Durable::Refused { player: player(1) })]
    );
    assert_eq!(world.region.state().next_entity_id, EntityId(0));

    // A player who arrives brings their entity, and is taken in.
    world.tick(&arrive(E, player(5), transfer(TRAVELLER, 0, IN_HOME)));
    assert_eq!(world.state_of(player(5)).entity_id, TRAVELLER);
}

#[test]
fn a_join_that_is_refused_claims_nothing() {
    // Nobody came to stand at the spawn point, so its chunk is not needed.
    let empty = EntityIds {
        first: EntityId(0),
        end: EntityId(0),
    };
    let mut world = World::new(config(0), empty, Holdings::default());
    world.tick(&edges(vec![started(E, 10)]));
    let output = world.tick(&changes(vec![join(E, player(1))]));
    assert_eq!(entries(&output), [Durable::Refused { player: player(1) }]);
    assert!(output.claims.is_empty());
    assert_eq!(world.knowledge(HOME), Knowledge::Unknown);
}

// ---------------------------------------------------------------------------------------
// A made-up run, held to the record in every tick
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

    /// A point on the stone of one of the chunks of [`GRID`]: every other time one
    /// within a block and a half of a border between two of them, from where the
    /// blocks of both are within reach.
    fn point(&mut self) -> Vec3 {
        let x = if self.once_in(2) {
            let border = self.pick(&[0.0, 16.0, 32.0]);
            border + self.below(30) as f64 / 10.0 - 1.5
        } else {
            self.below(640) as f64 / 10.0 - 16.0
        };
        let z = self.below(320) as f64 / 10.0;
        Vec3::new(x, 64.0, z)
    }
}

/// The chunks a made-up run is about: two rows of four, with the line of the stripes
/// through the middle.
const GRID: [ChunkPos; 8] = [
    ChunkPos::new(-1, 0),
    ChunkPos::new(-1, 1),
    ChunkPos::new(0, 0),
    ChunkPos::new(0, 1),
    ChunkPos::new(1, 0),
    ChunkPos::new(1, 1),
    ChunkPos::new(2, 0),
    ChunkPos::new(2, 1),
];

/// Makes up the inputs of tick after tick for region 0, as its edges and the store would
/// give them: tickets of both kinds that come and go, the store's answers to the
/// region's claims a tick or more after they were made, a third region that takes and
/// leaves chunks, doubts, what storage delivers, edges that confirm, start anew and are
/// gone, four players who walk about, dig, are let go and arrive again through either
/// edge, and what players of other regions break.
struct Wander {
    random: Random,
    grants: Grants,
    /// Claims the store has not answered, each with the tick its answer goes into.
    claims: Vec<(u64, ChunkPos)>,
    /// What was asked of storage, each with the tick it is delivered in.
    loads: Vec<(u64, ChunkPos)>,
    /// The tickets the edges hold.
    tickets: Vec<(ChunkPos, Ticket)>,
    next_input: u64,
    next_sequence: i32,
    /// Players that were let go or sent on, who may arrive again.
    away: Vec<(PlayerId, PlayerTransfer)>,
    /// The start each edge has; none before the first tick.
    starts: BTreeMap<EdgeId, u64>,
    return_after: u64,
}

impl Wander {
    /// A run on the stripes, with one chunk of the region's own stripe granted to a
    /// third region, or on open land, where region 1 is pinned to the easternmost
    /// chunks, a third region holds one chunk and the rest is free.
    fn new(seed: u64, stripes: bool, return_after: u64) -> (World, Self) {
        let (world, mut grants) = if stripes {
            (World::pinned(return_after), Grants::stripes())
        } else {
            let far_east = ChunkArea {
                min_x: Some(2),
                max_x: None,
            };
            let mut grants = Grants {
                pinned: vec![(far_east, REGION_B)],
                granted: BTreeMap::new(),
            };
            grants.granted.insert(HOME, REGION_A);
            (World::open(return_after, &[HOME]), grants)
        };
        grants.granted.insert(ChunkPos::new(-1, 1), OTHER);
        let wander = Self {
            random: Random(0x9E37_79B9_7F4A_7C15 ^ seed),
            grants,
            claims: Vec::new(),
            loads: Vec::new(),
            tickets: Vec::new(),
            next_input: 0,
            next_sequence: 0,
            away: Vec::new(),
            starts: BTreeMap::new(),
            return_after,
        };
        (world, wander)
    }

    /// The region's owner is lost and another opens the region: with the state as it
    /// is, what the store says it holds, and nothing else. The edges' tickets are gone
    /// with their links, and so is what was asked of storage.
    fn restore(&mut self, world: World) -> World {
        // What the lost owner had asked and not heard, the store has answered all the
        // same: a grant among it is in what the region is told it holds.
        let asked: Vec<ChunkPos> = self.claims.drain(..).map(|(_, chunk)| chunk).collect();
        self.grants.answer(REGION_A, &asked);
        self.loads.clear();
        self.tickets.clear();
        let granted = self.grants.granted.iter();
        let pinned = self.grants.pinned.iter();
        let holdings = Holdings {
            held: granted
                .filter(|(_, region)| **region == REGION_A)
                .map(|(chunk, _)| *chunk)
                .collect(),
            pinned: pinned
                .filter(|(_, region)| *region == REGION_A)
                .map(|(area, _)| *area)
                .collect(),
        };
        let state = world.region.state();
        let mut restored = World::restore(config(self.return_after), state, holdings);
        restored.seen.extend(world.seen);
        restored.compare();
        restored
    }

    /// What comes next for the region, as of its state now.
    fn inputs(&mut self, world: &World) -> TickInputs {
        let tick = world.region.tick_number() + 1;
        let state = world.region.state();
        let random = &mut self.random;
        let mut inputs = TickInputs::default();

        if self.starts.is_empty() {
            for edge in [E, F] {
                self.starts.insert(edge, 10);
                inputs.edges.push(started(edge, 10));
            }
        } else {
            // Both outboxes can be dropped in one tick, and are now and then: an entity
            // on its way in both was once reported removed twice then
            // (`two_outboxes_dropped_in_one_tick_report_an_entity_on_its_way_in_both_removed_once`).
            let mut dropped = false;
            for edge in [E, F] {
                if let Some(known) = state.edges.get(&edge)
                    && random.once_in(4)
                {
                    let number = random.below(known.sent + 2);
                    inputs.edges.push(EdgeEvent::Confirmed { edge, number });
                }
                let start = self.starts.get_mut(&edge).expect("both edges have started");
                // The second goes with the first every other time.
                if (dropped && random.once_in(2)) || random.once_in(60) {
                    *start += 1;
                    inputs.edges.push(started(edge, *start));
                    dropped = true;
                } else if random.once_in(120) {
                    // Away too long, and back at once with the start it had.
                    inputs.edges.push(EdgeEvent::Gone { edge });
                    inputs.edges.push(started(edge, *start));
                    dropped = true;
                }
            }
        }

        // The store's answers that are due, in the order of the claims.
        let due: Vec<ChunkPos> = self
            .claims
            .iter()
            .filter(|(at, _)| *at <= tick)
            .map(|(_, chunk)| *chunk)
            .collect();
        self.claims.retain(|(at, _)| *at > tick);
        (inputs.granted, inputs.foreign) = self.grants.answer(REGION_A, &due);
        for (at, chunk) in &self.loads {
            if *at <= tick {
                inputs.chunks_loaded.push((*chunk, stone_chunk()));
            }
        }
        self.loads.retain(|(at, _)| *at > tick);

        // The third region takes a chunk nobody holds, or gives one back.
        if random.once_in(6) {
            let chunk = random.pick(&GRID);
            match self.grants.granted.get(&chunk).copied() {
                None if self.grants.holder(chunk).is_none() => {
                    self.grants.granted.insert(chunk, OTHER);
                }
                Some(OTHER) => {
                    self.grants.granted.remove(&chunk);
                }
                _ => {}
            }
        }
        // Word of the store that nobody asked for, about a chunk no claim is under way
        // for.
        if random.once_in(12) {
            let chunk = random.pick(&GRID);
            let asked = self.claims.iter().any(|(_, claimed)| *claimed == chunk);
            let said = inputs.foreign.iter().any(|(named, _)| *named == chunk);
            if let Some(holder) = self.grants.holder(chunk)
                && holder != REGION_A
                && !asked
                && !said
            {
                inputs.foreign.push((chunk, holder));
            }
        }
        if random.once_in(6) {
            let chunk = random.pick(&GRID);
            let doubted = random.pick(&[REGION_B, OTHER]);
            inputs.unbelieve.push((chunk, doubted));
        }

        for _ in 0..random.below(3) {
            let ticket = (
                random.pick(&GRID),
                random.pick(&[Ticket::Viewer, Ticket::Guest]),
            );
            self.tickets.push(ticket);
            inputs.tickets_added.push(ticket);
        }
        for _ in 0..random.below(3) {
            if !self.tickets.is_empty() {
                let index = random.below(self.tickets.len() as u64) as usize;
                inputs.tickets_removed.push(self.tickets.swap_remove(index));
            }
        }
        // Now and then an edge releases a ticket it does not hold.
        if random.once_in(30) {
            let ticket = (
                random.pick(&GRID),
                random.pick(&[Ticket::Viewer, Ticket::Guest]),
            );
            if !self.tickets.contains(&ticket) {
                inputs.tickets_removed.push(ticket);
            }
        }

        for n in 1..=4 {
            let id = player(n);
            let away = self.away.iter().position(|(away, _)| *away == id);
            if let Some(present) = state.players.get(&id) {
                // A few things one after another: steps, and blocks beside where they
                // stand by then, which can be across a chunk border, or far out of
                // reach.
                let mut position = present.pose.position;
                for _ in 0..random.below(4) {
                    let input = if random.once_in(3) {
                        let beside = if random.once_in(4) {
                            40
                        } else {
                            random.below(5) as i32 - 2
                        };
                        let x = position.x.floor() as i32 + beside;
                        self.next_sequence += 1;
                        let block = BlockPos::new(x, 63, position.z.floor() as i32);
                        dig(block, self.next_sequence)
                    } else {
                        position = random.point();
                        walk(position.x, position.z)
                    };
                    self.next_input += 1;
                    inputs.input(present.edge, id, present.entity_id, self.next_input, input);
                }
            } else if let Some(index) = away {
                if random.once_in(3) {
                    // Back where they left, or somewhere else entirely, through either
                    // edge.
                    let (_, mut transfer) = self.away.remove(index);
                    if random.once_in(2) {
                        transfer.pose.position = random.point();
                    }
                    let edge = random.pick(&[E, F]);
                    inputs.change(PlayerChange::Arrive(edge, id, transfer));
                }
            } else if random.once_in(2) {
                inputs.change(join(random.pick(&[E, F]), id));
            }
        }

        for _ in 0..random.below(2) {
            let block = BlockPos::new(random.below(64) as i32 - 16, 63, random.below(32) as i32);
            self.next_sequence += 1;
            let actor = player(20 + random.below(3) as u128);
            let action = remote(actor, self.next_sequence, break_at(block));
            inputs.remote_actions.push((random.pick(&[E, F]), action));
        }
        inputs
    }

    /// Takes note of what the tick asks of the store and of storage, and of the
    /// players it let go or sent on.
    fn note(&mut self, output: &TickOutput) {
        for chunk in &output.claims {
            let at = output.tick + 1 + self.random.below(3);
            self.claims.push((at, *chunk));
        }
        self.grants.take_back(REGION_A, &output.returns);
        for chunk in &output.chunk_requests {
            let at = output.tick + 1 + self.random.below(2);
            self.loads.push((at, *chunk));
        }
        for (_, _, entry) in &output.durable {
            if let Durable::Departed {
                player, transfer, ..
            }
            | Durable::NotMine {
                what: Misdirected::Arrival { player, transfer },
                ..
            } = entry
            {
                self.away.push((*player, transfer.clone()));
            }
        }
    }
}

/// An outbox entry a made-up tick is to make.
#[derive(Debug, PartialEq)]
enum Made {
    /// The entry as it is to be, for the edge.
    Entry(EdgeId, Durable),
    /// A departure, of which the made-up run does not know the whole transfer: the
    /// player, where they stood, the number of the last input applied to them, and the
    /// region they go to.
    LetGo(EdgeId, PlayerId, Vec3, u64, RegionId),
}

/// A player of the region as far as a made-up run moves them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Walker {
    position: Vec3,
    last_input: u64,
    edge: EdgeId,
    entity: EntityId,
}

/// The transfer of an entry that says a player is on their way.
fn on_their_way(entry: &Durable) -> Option<&PlayerTransfer> {
    match entry {
        Durable::Departed { transfer, .. }
        | Durable::NotMine {
            what: Misdirected::Arrival { transfer, .. },
            ..
        } => Some(transfer),
        _ => None,
    }
}

/// What a reset or a `Gone` of `edge` reports removed (ADR-0008, section 2, and section
/// 2.2 here): the edge's players where they stand, and then every entity on its way in
/// the outbox that is dropped, where it stood when it left, unless a player of the
/// region has it or it was reported already.
fn dropped(
    edge: EdgeId,
    here: &mut BTreeMap<PlayerId, Walker>,
    outbox: &BTreeMap<u64, Durable>,
    gone: &mut Vec<(EntityId, ChunkPos)>,
) {
    let theirs: Vec<PlayerId> = here
        .iter()
        .filter(|(_, walker)| walker.edge == edge)
        .map(|(id, _)| *id)
        .collect();
    for id in theirs {
        let walker = here.remove(&id).expect("the player is there");
        gone.push((walker.entity, chunk_of(walker.position)));
    }
    for transfer in outbox.values().filter_map(on_their_way) {
        let entity = transfer.entity_id;
        let reported = gone.iter().any(|(named, _)| *named == entity);
        let alive = here.values().any(|walker| walker.entity == entity);
        if !reported && !alive {
            gone.push((entity, chunk_of(transfer.pose.position)));
        }
    }
}

/// Sections 2.2 to 2.5 for a tick of a made-up run, whose players only join, arrive,
/// walk and dig, and whose remote actions only break: who is removed with an edge, who
/// is taken in, what of each player is applied, which entries are made in which order,
/// what is acknowledged, and who is let go with what. `during` is what the region knows
/// of chunks while the tick runs, without what it stopped believing of a chunk of its
/// own area because a player or an action came for it ([`Model::doubt`]): such an
/// arrival is taken in, and such an action goes on without a region.
fn check_made_up_tick(
    before: &RegionState,
    inputs: &TickInputs,
    during: &Model,
    output: &TickOutput,
    after: &RegionState,
) {
    let believed = |position: Vec3| during.foreign.get(&chunk_of(position)).copied();
    let mut here: BTreeMap<PlayerId, Walker> = before
        .players
        .iter()
        .map(|(id, state)| {
            let walker = Walker {
                position: state.pose.position,
                last_input: state.last_input,
                edge: state.edge,
                entity: state.entity_id,
            };
            (*id, walker)
        })
        .collect();
    // The start and the outbox of every edge the region knows.
    let mut edges: BTreeMap<EdgeId, (u64, BTreeMap<u64, Durable>)> = before
        .edges
        .iter()
        .map(|(edge, state)| (*edge, (state.start, state.outbox.clone())))
        .collect();
    let mut gone = Vec::new();
    let mut made = Vec::new();
    // One acknowledgement for each player, with the highest sequence number handled.
    let mut acknowledgements: BTreeMap<PlayerId, i32> = BTreeMap::new();
    let mut next_entity = before.next_entity_id;

    // Step 3.
    for event in &inputs.edges {
        match event {
            EdgeEvent::Confirmed { edge, number } => {
                if let Some((_, outbox)) = edges.get_mut(edge) {
                    outbox.retain(|entry, _| entry > number);
                }
            }
            EdgeEvent::Started { edge, start } => match edges.get(edge) {
                Some((known, _)) if known >= start => {}
                Some((_, outbox)) => {
                    dropped(*edge, &mut here, outbox, &mut gone);
                    edges.insert(*edge, (*start, BTreeMap::new()));
                }
                None => {
                    edges.insert(*edge, (*start, BTreeMap::new()));
                }
            },
            EdgeEvent::Gone { edge } => {
                if let Some((_, outbox)) = edges.remove(edge) {
                    dropped(*edge, &mut here, &outbox, &mut gone);
                }
            }
        }
    }

    // Step 4.
    for change in &inputs.player_changes {
        match change {
            PlayerChange::Join(edge, join) => {
                assert!(edges.contains_key(edge) && !here.contains_key(&join.player));
                let walker = Walker {
                    position: SPAWN,
                    last_input: 0,
                    edge: *edge,
                    entity: next_entity,
                };
                next_entity.0 += 1;
                here.insert(join.player, walker);
            }
            PlayerChange::Arrive(edge, id, transfer) => {
                assert!(edges.contains_key(edge) && !here.contains_key(id));
                let position = transfer.pose.position;
                match believed(position) {
                    Some(holder) => {
                        let what = Misdirected::Arrival {
                            player: *id,
                            transfer: transfer.clone(),
                        };
                        made.push(Made::Entry(*edge, Durable::NotMine { what, holder }));
                    }
                    None => {
                        let walker = Walker {
                            position,
                            last_input: transfer.last_input,
                            edge: *edge,
                            entity: transfer.entity_id,
                        };
                        here.insert(*id, walker);
                    }
                }
            }
            other => panic!("a made-up run has no {other:?}"),
        }
    }

    // Step 5.
    for (edge, action) in &inputs.remote_actions {
        assert!(edges.contains_key(edge));
        let entry = match during.knowledge(action.step.concerns().chunk()) {
            Knowledge::Held => Durable::RemoteDone {
                player: action.player,
                sequence: action.sequence,
            },
            Knowledge::Foreign(holder) => Durable::NotMine {
                what: Misdirected::Remote(action.clone()),
                holder,
            },
            Knowledge::Asked | Knowledge::Unknown => Durable::Remote {
                action: action.clone(),
                to: None,
            },
        };
        made.push(Made::Entry(*edge, entry));
    }

    // Step 6. An input is applied only to the stay it names (ADR-0014, section 2.1).
    for (edge, id, entity, number, input) in &inputs.inputs {
        let Some(walker) = here.get_mut(id) else {
            continue;
        };
        // Nothing is applied of a player who stands in a chunk believed another's.
        if walker.edge != *edge
            || walker.entity != *entity
            || believed(walker.position).is_some()
            || *number <= walker.last_input
        {
            continue;
        }
        match input {
            PlayerInput::Move {
                position: Some(to), ..
            } => walker.position = *to,
            PlayerInput::Dig { position, sequence } => {
                let away = (f64::from(position.x) + 0.5 - walker.position.x).abs();
                assert!(away <= 3.0 || away >= 30.0, "a dig {away} blocks away");
                let to = match during.knowledge(position.chunk()) {
                    Knowledge::Foreign(holder) => Some(Some(holder)),
                    Knowledge::Asked | Knowledge::Unknown => Some(None),
                    Knowledge::Held => None,
                };
                match to.filter(|_| away <= 3.0) {
                    Some(to) => {
                        let action = remote(*id, *sequence, break_at(*position));
                        made.push(Made::Entry(*edge, Durable::Remote { action, to }));
                    }
                    // Out of reach, or the region's own to break.
                    None => {
                        acknowledgements.insert(*id, *sequence);
                    }
                }
            }
            other => panic!("a made-up run has no {other:?}"),
        }
        walker.last_input = *number;
    }

    // Step 8.
    let mut stay = BTreeMap::new();
    for (id, walker) in here {
        match believed(walker.position) {
            Some(to) => {
                let Walker {
                    position,
                    last_input,
                    edge,
                    ..
                } = walker;
                made.push(Made::LetGo(edge, id, position, last_input, to));
            }
            None => {
                stay.insert(id, walker);
            }
        }
    }

    let tick = output.tick;
    let entries: Vec<Made> = output
        .durable
        .iter()
        .map(|(edge, _, entry)| match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } => Made::LetGo(
                *edge,
                *player,
                transfer.pose.position,
                transfer.last_input,
                *to,
            ),
            other => Made::Entry(*edge, other.clone()),
        })
        .collect();
    assert_eq!(entries, made, "tick {tick}: the entries made");
    let there: BTreeMap<PlayerId, Walker> = after
        .players
        .iter()
        .map(|(id, state)| {
            let walker = Walker {
                position: state.pose.position,
                last_input: state.last_input,
                edge: state.edge,
                entity: state.entity_id,
            };
            (*id, walker)
        })
        .collect();
    assert_eq!(there, stay, "tick {tick}: the players");
    let mut reported = removed(output);
    reported.sort();
    gone.sort();
    assert_eq!(reported, gone, "tick {tick}: the entities removed");
    // Also for a player who was let go behind the dig, in the same tick.
    let mut acknowledged = acknowledged(output);
    acknowledged.sort();
    let acknowledgements: Vec<(PlayerId, i32)> = acknowledgements.into_iter().collect();
    assert_eq!(acknowledged, acknowledgements, "tick {tick}");
}

/// What the made-up runs did, to make sure they did what is worth checking.
#[derive(Debug, Default)]
struct Tally {
    claims: usize,
    returns: usize,
    granted: usize,
    foreign: usize,
    doubts: usize,
    loads: usize,
    let_go: usize,
    sent_on: usize,
    arrivals: usize,
    passed_on_with_a_region: usize,
    passed_on_without: usize,
    done: usize,
    not_mine: usize,
    acknowledged: usize,
    blocks: usize,
    /// Entities that a reset or a `Gone` reported removed and that were no player's.
    orphans: usize,
}

impl Tally {
    fn note(&mut self, before: &RegionState, inputs: &TickInputs, output: &TickOutput) {
        self.claims += output.claims.len();
        self.returns += output.returns.len();
        self.granted += inputs.granted.len();
        self.foreign += inputs.foreign.len();
        self.doubts += inputs.unbelieve.len();
        self.loads += output.chunk_requests.len();
        self.acknowledged += acknowledged(output).len();
        self.blocks += block_changes(output).len();
        self.arrivals += spawned(output).len();
        for (_, _, entry) in &output.durable {
            match entry {
                Durable::Departed { .. } => self.let_go += 1,
                Durable::NotMine {
                    what: Misdirected::Arrival { .. },
                    ..
                } => self.sent_on += 1,
                Durable::NotMine { .. } => self.not_mine += 1,
                Durable::Remote { to: Some(_), .. } => self.passed_on_with_a_region += 1,
                Durable::Remote { to: None, .. } => self.passed_on_without += 1,
                Durable::RemoteDone { .. } => self.done += 1,
                other => panic!("a made-up run made {other:?}"),
            }
        }
        let players: BTreeSet<EntityId> = before
            .players
            .values()
            .map(|player| player.entity_id)
            .collect();
        self.orphans += removed(output)
            .iter()
            .filter(|(entity, _)| !players.contains(entity))
            .count();
    }

    fn assert_worth_it(&self) {
        let counts = [
            self.claims,
            self.returns,
            self.granted,
            self.foreign,
            self.doubts,
            self.loads,
            self.let_go,
            self.sent_on,
            self.arrivals,
            self.passed_on_with_a_region,
            self.passed_on_without,
            self.done,
            self.not_mine,
            self.acknowledged,
            self.blocks,
            self.orphans,
        ];
        assert!(counts.iter().all(|count| *count >= 30), "{self:?}");
    }
}

#[test]
fn a_made_up_run_keeps_to_the_record_in_every_tick() {
    // `World::tick` holds each tick to sections 1 and 2.1, and `check_made_up_tick` to
    // sections 2.2 to 2.5. Four times in a run the region is restored, and has to go on
    // from its state and what the store says it holds.
    let mut tally = Tally::default();
    for seed in 1..=12 {
        for stripes in [false, true] {
            let return_after = [0, 2, 5][seed as usize % 3];
            let (mut world, mut wander) = Wander::new(seed, stripes, return_after);
            for round in 0..300 {
                if round % 70 == 69 {
                    world = wander.restore(world);
                }
                let inputs = wander.inputs(&world);
                let before = world.region.state();
                let output = world.tick(&inputs);
                let after = world.region.state();
                check_made_up_tick(&before, &inputs, &world.during, &output, &after);
                wander.note(&output);
                tally.note(&before, &inputs, &output);
            }
        }
    }
    tally.assert_worth_it();
}

// ---------------------------------------------------------------------------------------
// S17: a tick is a function of the region and its inputs
// ---------------------------------------------------------------------------------------

/// The inputs of a run on the stripes with a bit of everything this record adds:
/// tickets of both kinds, claims that are granted and refused, a doubt, blocks either
/// side of the line and in chunks the region has no answer for, actions of other
/// regions' players, a hand-over in the tick of the step and one in the tick of the
/// answer, and arrivals that are taken in and sent on; and, of ADR-0014, an arrival for
/// a chunk of the region's own stripe that it believes another's.
fn script() -> Vec<TickInputs> {
    let mut script = vec![
        add(&[viewer(HOME), viewer(EAST), guest(WEST), viewer(SOUTH)]),
        TickInputs {
            granted: vec![WEST, HOME],
            foreign: vec![(EAST, REGION_B)],
            edges: vec![started(E, 10), started(F, 10)],
            ..TickInputs::default()
        },
        delivered(&[WEST, HOME]),
        changes(vec![
            join(E, player(1)),
            join(F, player(2)),
            join(E, player(3)),
        ]),
    ];
    let mut inputs = TickInputs::default();
    inputs.input(E, player(1), entity(1), 1, dig(OWN_BLOCK, 1));
    inputs.input(F, player(2), entity(2), 1, dig(BORDER_BLOCK_B, 1));
    inputs.input(E, player(3), entity(3), 1, walk(BY_SOUTH.x, BY_SOUTH.z));
    inputs.applied = vec![(E, 4), (F, 2)];
    script.push(inputs);

    let mut inputs = TickInputs {
        remote_actions: vec![
            (F, remote(player(9), 4, break_at(BORDER_BLOCK_A))),
            (E, remote(player(8), 2, break_at(BORDER_BLOCK_B))),
            (F, remote(player(9), 5, break_at(NORTH_BLOCK))),
        ],
        ..TickInputs::default()
    };
    inputs.input(E, player(3), entity(3), 2, dig(SOUTH_BLOCK, 1));
    inputs.input(
        E,
        player(3),
        entity(3),
        3,
        use_on(BY_SOUTH_BLOCK, Face::South, 2),
    );
    inputs.input(
        F,
        player(2),
        entity(2),
        2,
        use_on(BORDER_BLOCK_B, Face::Top, 2),
    );
    script.push(inputs);

    // Player 3 walks into the chunk that is asked for, and player 1 across the line.
    let mut inputs = single_input(E, player(3), entity(3), 4, walk(8.5, 20.5));
    inputs.input(E, player(1), entity(1), 2, move_to(17.5));
    inputs.input(E, player(1), entity(1), 3, dig(OWN_BLOCK, 2));
    script.push(inputs);
    script.push(edges(vec![EdgeEvent::Confirmed { edge: E, number: 2 }]));

    // The store's answer for it, with more of player 3 behind.
    let mut inputs = foreign(&[(SOUTH, OTHER)]);
    inputs.input(E, player(3), entity(3), 5, walk(8.5, 8.5));
    script.push(inputs);

    script.push(TickInputs {
        unbelieve: vec![(EAST, REGION_B), (SOUTH, REGION_B)],
        tickets_removed: vec![guest(WEST)],
        ..TickInputs::default()
    });
    script.push(changes(vec![
        PlayerChange::Arrive(F, player(5), transfer(TRAVELLER, 3, IN_NORTH)),
        PlayerChange::Arrive(E, player(6), transfer(EntityId(7_000_002), 1, IN_SOUTH)),
    ]));
    script.push(TickInputs {
        foreign: vec![(EAST, OTHER)],
        granted: vec![NORTH],
        tickets_added: vec![guest(NORTH), viewer(WEST)],
        ..TickInputs::default()
    });
    // `EAST` is beyond the stripe, so this arrival goes on; the one for `SOUTH`
    // above, which is of the stripe and believed another's, was taken in.
    let mut inputs = delivered(&[NORTH, WEST]);
    inputs.change(PlayerChange::Arrive(
        F,
        player(7),
        transfer(EntityId(7_000_003), 1, IN_EAST),
    ));
    script.push(inputs);
    script.push(single_input(
        F,
        player(5),
        TRAVELLER,
        4,
        dig(NORTH_BLOCK, 1),
    ));
    script.push(edges(vec![started(E, 20), EdgeEvent::Gone { edge: F }]));
    script.push(TickInputs::default());
    script
}

/// Gives `world` the inputs tick by tick, and returns what every tick gave back and the
/// bytes of the state after it.
fn run(mut world: World, inputs: &[TickInputs]) -> (Vec<TickOutput>, Vec<Vec<u8>>) {
    let mut outputs = Vec::new();
    let mut states = Vec::new();
    for inputs in inputs {
        outputs.push(world.tick(inputs));
        states.push(postcard::to_stdvec(&world.region.state()).expect("the state serialises"));
    }
    (outputs, states)
}

/// The inputs of a made-up run, with the store's answers as they were given.
fn recorded(seed: u64, stripes: bool, return_after: u64) -> Vec<TickInputs> {
    let (mut world, mut wander) = Wander::new(seed, stripes, return_after);
    let mut record = Vec::new();
    for _ in 0..200 {
        let inputs = wander.inputs(&world);
        let output = world.tick(&inputs);
        wander.note(&output);
        record.push(inputs);
    }
    record
}

/// The same inputs with every collection that names chunks built in the opposite
/// order.
fn turned_round(inputs: &TickInputs) -> TickInputs {
    let mut turned = inputs.clone();
    turned.tickets_added.reverse();
    turned.tickets_removed.reverse();
    turned.chunks_loaded.reverse();
    turned.granted.reverse();
    turned.foreign.reverse();
    turned.unbelieve.reverse();
    turned
}

#[test]
fn the_script_does_what_it_is_meant_to() {
    // So that the tests on it compare runs in which something happened.
    let (outputs, _) = run(World::pinned(0), &script());
    let all: Vec<Durable> = outputs.iter().flat_map(entries).collect();
    let has = |wanted: fn(&Durable) -> bool| all.iter().any(wanted);
    assert!(has(|entry| matches!(
        entry,
        Durable::Departed { to: REGION_B, .. }
    )));
    assert!(has(|entry| matches!(
        entry,
        Durable::Departed { to: OTHER, .. }
    )));
    assert!(has(|entry| matches!(
        entry,
        Durable::Remote { to: None, .. }
    )));
    assert!(has(|entry| matches!(
        entry,
        Durable::Remote { to: Some(_), .. }
    )));
    assert!(has(|entry| matches!(entry, Durable::RemoteDone { .. })));
    assert!(has(|entry| matches!(
        entry,
        Durable::NotMine {
            what: Misdirected::Remote(_),
            ..
        }
    )));
    assert!(has(|entry| matches!(
        entry,
        Durable::NotMine {
            what: Misdirected::Arrival { .. },
            ..
        }
    )));
    assert!(outputs.iter().any(|output| !output.claims.is_empty()));
    assert!(
        outputs
            .iter()
            .any(|output| !output.chunk_requests.is_empty())
    );
    assert!(
        outputs
            .iter()
            .any(|output| !block_changes(output).is_empty())
    );
}

#[test]
fn states_and_deltas_with_the_new_entries_survive_serialisation() {
    // The store keeps them as postcard bytes, and the next owner reads them.
    let mut world = World::pinned(0);
    let mut entries = 0;
    for inputs in script() {
        let output = world.tick(&inputs);
        let bytes = postcard::to_stdvec(&output.delta).expect("the delta serialises");
        let delta: StateDelta = postcard::from_bytes(&bytes).expect("reads back");
        assert_eq!(delta, output.delta);
        let state = world.region.state();
        let bytes = postcard::to_stdvec(&state).expect("the state serialises");
        let back: RegionState = postcard::from_bytes(&bytes).expect("reads back");
        assert_eq!(back, state);
        entries = entries.max(state.edges.values().map(|edge| edge.outbox.len()).sum());
    }
    assert!(
        entries >= 6,
        "the outboxes were full at some time: {entries}"
    );
}

#[test]
fn the_same_inputs_give_byte_identical_states_and_identical_outputs() {
    let script = script();
    assert_eq!(
        run(World::pinned(0), &script),
        run(World::pinned(0), &script)
    );

    // And runs with the store in the loop, replayed from their inputs alone, on open
    // land with chunks given back and on the stripes.
    for seed in 1..=4 {
        for stripes in [false, true] {
            let return_after = seed % 3;
            let record = recorded(seed, stripes, return_after);
            let world = || Wander::new(seed, stripes, return_after).0;
            let first = run(world(), &record);
            assert_eq!(first, run(world(), &record));
            let (outputs, _) = first;
            if !stripes {
                assert!(outputs.iter().any(|output| !output.returns.is_empty()));
            }
            assert!(outputs.iter().any(|output| !output.claims.is_empty()));
        }
    }
}

#[test]
fn the_order_in_which_a_ticks_inputs_name_chunks_changes_nothing() {
    // Tickets are counted, the store's answers are each about one chunk, and what a
    // tick claims, returns and asks of storage is in ascending order of the chunks: so
    // none of it depends on the order in which the runner happened to collect them.
    let script = script();
    let turned: Vec<TickInputs> = script.iter().map(turned_round).collect();
    assert_ne!(script, turned);
    assert_eq!(
        run(World::pinned(0), &script),
        run(World::pinned(0), &turned)
    );

    for seed in 1..=4 {
        for stripes in [false, true] {
            let return_after = seed % 3;
            let record = recorded(seed, stripes, return_after);
            let turned: Vec<TickInputs> = record.iter().map(turned_round).collect();
            assert_ne!(record, turned);
            let world = || Wander::new(seed, stripes, return_after).0;
            assert_eq!(run(world(), &record), run(world(), &turned));
        }
    }
}

// ---------------------------------------------------------------------------------------
// S18: two regions treat players and blocks as one region does
// ---------------------------------------------------------------------------------------

/// The subscriptions an edge has at a region.
type Tickets = Vec<(ChunkPos, Ticket)>;

/// One region of a [`Cluster`], with what its next tick is to be given.
struct Site {
    world: World,
    next: TickInputs,
}

/// What the regions of a cluster handed to the edge in one tick.
#[derive(Debug, Default)]
struct Passed {
    /// The players that were let go, each with the region they went to.
    departed: Vec<(PlayerId, RegionId)>,
    /// The region each action that was passed on named, if it named one.
    remote: Vec<Option<RegionId>>,
}

/// Regions side by side with one store and one edge `E` between them, which the
/// cluster plays: it answers each tick's claims into the next tick, delivers what is
/// asked of storage, and handles every outbox entry as section 5 of the record has an
/// edge do. A cluster of one region is what the others are compared with.
struct Cluster {
    grants: Grants,
    sites: Vec<Site>,
    /// The region players enter the world in.
    home: usize,
    /// The region the edge takes each player to be in.
    whereabouts: BTreeMap<PlayerId, usize>,
    /// The entity each player has, as the home region said when they spawned.
    entities: BTreeMap<PlayerId, EntityId>,
    /// Everything each player did, numbered from 1, to send again after a hand-over.
    made: BTreeMap<PlayerId, Vec<PlayerInput>>,
    /// The highest sequence number of each player's actions on blocks that a region
    /// reported as dealt with: an acknowledgement covers every action up to its own.
    dealt_with: BTreeMap<PlayerId, i32>,
    /// The actions that were passed on and have not been reported as dealt with.
    under_way: BTreeSet<(PlayerId, i32)>,
    /// How many actions the edge ended itself, having no region to send them to.
    ended: usize,
    /// How many chunks the regions gave back.
    returned: usize,
}

impl Cluster {
    /// Regions pinned to `areas`, numbered in their order, each with the tickets of
    /// `tickets` from its first tick on. Region 0 is the home region.
    fn new(areas: &[ChunkArea], tickets: &[&[(ChunkPos, Ticket)]]) -> Self {
        let regions = areas
            .iter()
            .zip(tickets)
            .map(|(area, tickets)| (vec![*area], tickets.to_vec()))
            .collect();
        Self::of(regions, 0, 0)
    }

    /// The division with a gap of ADR-0011, section 9: regions 0 and 1 pinned far to
    /// the west and to the east, and the home region, region 2, pinned to nothing and
    /// holding the home chunk alone. The chunks in between are free.
    fn with_a_gap(return_after: u64) -> Self {
        let regions = vec![
            (vec![FAR_WEST], Vec::new()),
            (vec![FAR_EAST], Vec::new()),
            (Vec::new(), vec![viewer(HOME)]),
        ];
        Self::of(regions, 2, return_after)
    }

    /// Regions with the areas each is pinned to and the tickets each has from its first
    /// tick on, numbered in their order.
    fn of(regions: Vec<(Vec<ChunkArea>, Tickets)>, home: usize, return_after: u64) -> Self {
        let mut grants = Grants::default();
        for (index, (areas, _)) in regions.iter().enumerate() {
            let region = RegionId(index as u32);
            grants
                .pinned
                .extend(areas.iter().map(|area| (*area, region)));
        }
        // A home region that is not pinned to the home chunk is granted it from the
        // start (ADR-0011, section 2).
        let granted_home = grants.holder(HOME).is_none();
        if granted_home {
            grants.granted.insert(HOME, RegionId(home as u32));
        }
        let mut sites = Vec::new();
        for (index, (areas, tickets)) in regions.into_iter().enumerate() {
            let held = if granted_home && index == home {
                vec![HOME]
            } else {
                Vec::new()
            };
            let holdings = Holdings {
                held,
                pinned: areas,
            };
            let entity_ids = EntityIds::block(3 + index as u32).expect("the block exists");
            sites.push(Site {
                world: World::new(config(return_after), entity_ids, holdings),
                next: TickInputs {
                    edges: vec![started(E, 10)],
                    tickets_added: tickets,
                    ..TickInputs::default()
                },
            });
        }
        let mut cluster = Self {
            grants,
            sites,
            home,
            whereabouts: BTreeMap::new(),
            entities: BTreeMap::new(),
            made: BTreeMap::new(),
            dealt_with: BTreeMap::new(),
            under_way: BTreeSet::new(),
            ended: 0,
            returned: 0,
        };
        cluster.settle();
        cluster
    }

    /// A player enters the world, in the home region.
    fn join(&mut self, id: PlayerId) {
        self.whereabouts.insert(id, self.home);
        self.sites[self.home].next.change(join(E, id));
    }

    /// Passes a player on to `to`, with everything they did that the region they come
    /// from has not applied (section 5.5, rules 18 and 20).
    fn arrive(&mut self, id: PlayerId, transfer: &PlayerTransfer, to: RegionId) {
        let site = to.0 as usize;
        self.whereabouts.insert(id, site);
        let next = &mut self.sites[site].next;
        next.change(PlayerChange::Arrive(E, id, transfer.clone()));
        let made = self.made.get(&id).map_or(&[][..], Vec::as_slice);
        for (index, input) in made.iter().enumerate() {
            let number = index as u64 + 1;
            if number > transfer.last_input {
                next.input(E, id, transfer.entity_id, number, input.clone());
            }
        }
    }

    /// An action of `id` was dealt with, by the region they are in or by another.
    fn done(&mut self, id: PlayerId, sequence: i32) {
        self.under_way.remove(&(id, sequence));
        let highest = self.dealt_with.entry(id).or_insert(sequence);
        *highest = (*highest).max(sequence);
    }

    /// The region an action without a region goes to: one that holds the chunk and
    /// knows so, and never the region the entry came from (section 5.6, rule 26, and
    /// "Found while building", 1).
    fn serving(&self, chunk: ChunkPos, from: usize) -> Option<usize> {
        self.sites.iter().enumerate().position(|(index, site)| {
            index != from && site.world.knowledge(chunk) == Knowledge::Held
        })
    }

    /// One tick of every region, in which the players do `doing`, in that order.
    fn tick(&mut self, doing: &[(PlayerId, PlayerInput)]) -> Passed {
        for (id, input) in doing {
            let made = self.made.entry(*id).or_default();
            made.push(input.clone());
            let number = made.len() as u64;
            let site = self.whereabouts[id];
            let next = &mut self.sites[site].next;
            next.input(E, *id, self.entities[id], number, input.clone());
        }
        let outputs: Vec<TickOutput> = self
            .sites
            .iter_mut()
            .map(|site| {
                let inputs = std::mem::take(&mut site.next);
                site.world.tick(&inputs)
            })
            .collect();

        let mut passed = Passed::default();
        for (from, output) in outputs.iter().enumerate() {
            let region = RegionId(from as u32);
            let (granted, foreign) = self.grants.answer(region, &output.claims);
            self.sites[from].next.granted.extend(granted);
            self.sites[from].next.foreign.extend(foreign);
            self.grants.take_back(region, &output.returns);
            self.returned += output.returns.len();
            for chunk in &output.chunk_requests {
                let delivery = (*chunk, stone_chunk());
                self.sites[from].next.chunks_loaded.push(delivery);
            }
            for (id, sequence) in acknowledged(output) {
                self.done(id, sequence);
            }
            for (id, event) in &output.player_events {
                if let PlayerEvent::Spawned { entity_id, .. } = event {
                    self.entities.insert(*id, *entity_id);
                }
            }
            for (edge, number, entry) in &output.durable {
                assert_eq!(*edge, E);
                self.sites[from].next.edges.push(EdgeEvent::Confirmed {
                    edge: E,
                    number: *number,
                });
                match entry {
                    Durable::Departed {
                        player,
                        transfer,
                        to,
                    } => {
                        assert_ne!(*to, region);
                        passed.departed.push((*player, *to));
                        self.arrive(*player, transfer, *to);
                    }
                    Durable::NotMine {
                        what: Misdirected::Arrival { player, transfer },
                        holder,
                    } => {
                        assert_ne!(*holder, region, "it is never the region that says it");
                        self.arrive(*player, transfer, *holder);
                    }
                    Durable::Remote { action, to } => {
                        passed.remote.push(*to);
                        assert_ne!(*to, Some(region));
                        self.under_way.insert((action.player, action.sequence));
                        let chunk = action.step.concerns().chunk();
                        let site = match to {
                            Some(named) => Some(named.0 as usize),
                            None => self.serving(chunk, from),
                        };
                        match site {
                            Some(site) => {
                                let next = &mut self.sites[site].next;
                                next.remote_actions.push((E, action.clone()));
                            }
                            None => {
                                self.ended += 1;
                                self.done(action.player, action.sequence);
                            }
                        }
                    }
                    Durable::NotMine {
                        what: Misdirected::Remote(action),
                        holder,
                    } => {
                        assert_ne!(*holder, region);
                        let next = &mut self.sites[holder.0 as usize].next;
                        next.remote_actions.push((E, action.clone()));
                    }
                    Durable::RemoteDone { player, sequence } => {
                        self.done(*player, *sequence);
                    }
                    other => panic!("{region} made {other:?}"),
                }
            }
        }

        // What a region takes for its own, the store has granted it; and no region
        // changes a block of a chunk it does not hold (section 4.2).
        for (index, site) in self.sites.iter().enumerate() {
            let region = Some(RegionId(index as u32));
            for chunk in &site.world.seen {
                if site.world.knowledge(*chunk) == Knowledge::Held {
                    assert_eq!(self.grants.holder(*chunk), region, "{chunk:?}");
                }
            }
            for (block, _) in block_changes(&outputs[index]) {
                assert_eq!(self.grants.holder(block.chunk()), region, "{block:?}");
            }
        }
        passed
    }

    /// Every player is in the region the edge takes them to be in and in no other,
    /// and no action is under way.
    fn assert_everyone_is_somewhere(&self) {
        for (id, expected) in &self.whereabouts {
            for (index, site) in self.sites.iter().enumerate() {
                assert_eq!(
                    site.world.region.player(*id).is_some(),
                    index == *expected,
                    "{id:?} and region {index}, of which {expected} is to have them"
                );
            }
        }
        assert!(self.under_way.is_empty(), "{:?}", self.under_way);
    }

    /// Whether nothing is on its way: no answer of the store, no chunk, no player and
    /// no action.
    fn settled(&self) -> bool {
        self.sites.iter().all(|site| {
            let next = &site.next;
            next.player_changes.is_empty()
                && next.inputs.is_empty()
                && next.remote_actions.is_empty()
                && next.granted.is_empty()
                && next.foreign.is_empty()
                && next.chunks_loaded.is_empty()
        })
    }

    /// Ticks until nothing is on its way, and returns what each of those ticks passed.
    /// A player or an action is passed on at most as many times as there are regions,
    /// each time with a tick or two for the store's answer, so it ends soon.
    fn settle(&mut self) -> Vec<Passed> {
        let mut passed = Vec::new();
        for _ in 0..20 {
            passed.push(self.tick(&[]));
            if self.settled() {
                return passed;
            }
        }
        panic!("what the regions pass on does not come to an end");
    }

    /// The chunk as the region that holds it has it.
    fn chunk(&self, chunk: ChunkPos) -> Chunk {
        let holder = self.grants.holder(chunk).expect("some region holds it");
        let site = &self.sites[holder.0 as usize];
        assert_eq!(site.world.knowledge(chunk), Knowledge::Held);
        site.world
            .region
            .chunk(chunk)
            .expect("the chunk is loaded")
            .clone()
    }

    fn block(&self, position: BlockPos) -> BlockState {
        let (x, z) = position.in_chunk();
        self.chunk(position.chunk())
            .get(x, position.y, z)
            .expect("the block is within the chunk")
    }

    /// The player as the region they are in has them, without what only that region
    /// counts: its own acknowledgements.
    fn player(&self, id: PlayerId) -> PlayerState {
        let site = &self.sites[self.whereabouts[&id]];
        PlayerState {
            handled: None,
            ..site.world.state_of(id)
        }
    }
}

/// What two players do at the line, step by step; the cluster is left to settle after
/// each. Player 1 and player 2 both begin at the spawn point, west of the line.
fn at_the_line_script() -> Vec<Vec<(PlayerId, PlayerInput)>> {
    let one = |input| vec![(player(1), input)];
    let two = |input| vec![(player(2), input)];
    vec![
        // From the west: a block across the line broken, put back, built upon, and a
        // block placed on this side against the one across.
        one(dig(BORDER_BLOCK_B, 1)),
        one(use_on(BORDER_BLOCK_A, Face::East, 2)),
        one(use_on(BORDER_BLOCK_B, Face::Top, 3)),
        one(use_on(BORDER_BLOCK_B.offset(0, 1, 0), Face::West, 4)),
        // Player 2 crosses, and does the same from the east.
        two(walk(17.5, 10.5)),
        two(dig(BlockPos::new(15, 63, 10), 1)),
        two(use_on(BlockPos::new(16, 63, 10), Face::West, 2)),
        two(use_on(BlockPos::new(15, 63, 11), Face::Top, 3)),
        // Player 1 crosses with a dig right behind the step, and comes back likewise.
        vec![
            (player(1), move_to(17.5)),
            (player(1), dig(BlockPos::new(17, 63, 9), 5)),
        ],
        vec![(player(1), move_to(14.5)), (player(1), dig(OWN_BLOCK, 6))],
        two(walk(13.5, 10.5)),
        two(dig(BlockPos::new(16, 63, 12), 4)),
        // Two actions of one player on one spot across the line in one tick, with one
        // of the other player on this side: placing into the stone fails, the dig
        // behind it does not.
        vec![
            (player(1), use_on(BlockPos::new(15, 63, 6), Face::East, 7)),
            (player(1), dig(BlockPos::new(16, 63, 6), 8)),
            (player(2), dig(BlockPos::new(15, 63, 12), 5)),
        ],
        // Out of reach across the line: dealt with where the player is.
        one(dig(BlockPos::new(30, 63, 8), 9)),
    ]
}

/// The steps of [`at_the_line_script`] in which a player walks across the line.
const CROSSINGS: [usize; 4] = [4, 8, 9, 10];

/// The blocks the script changes, as they are at its end.
fn at_the_line_blocks() -> Vec<(BlockPos, BlockState)> {
    vec![
        (BORDER_BLOCK_B, blocks::STONE),
        (BORDER_BLOCK_B.offset(0, 1, 0), blocks::STONE),
        (BORDER_BLOCK_A.offset(0, 1, 0), blocks::STONE),
        (BlockPos::new(15, 63, 10), blocks::STONE),
        (BlockPos::new(15, 64, 11), blocks::STONE),
        (BlockPos::new(17, 63, 9), blocks::AIR),
        (OWN_BLOCK, blocks::AIR),
        (BlockPos::new(16, 63, 12), blocks::AIR),
        (BlockPos::new(16, 63, 6), blocks::AIR),
        (BlockPos::new(15, 63, 12), blocks::AIR),
        (BlockPos::new(30, 63, 8), blocks::STONE),
    ]
}

/// Plays the script on `cluster` and returns, for each step, what the tick of the step
/// passed and what the ticks behind it did.
fn play(cluster: &mut Cluster) -> Vec<(Passed, Vec<Passed>)> {
    cluster.join(player(1));
    cluster.join(player(2));
    cluster.settle();
    at_the_line_script()
        .iter()
        .map(|step| (cluster.tick(step), cluster.settle()))
        .collect()
}

/// One region that holds everything, with the script played on it.
fn one_region() -> Cluster {
    let around: &[(ChunkPos, Ticket)] = &[viewer(HOME), viewer(EAST)];
    let mut cluster = Cluster::new(&[ChunkArea::EVERYWHERE], &[around]);
    for (step, behind) in play(&mut cluster) {
        let all = std::iter::once(&step).chain(&behind);
        for passed in all {
            assert!(passed.departed.is_empty() && passed.remote.is_empty());
        }
    }
    for (block, state) in at_the_line_blocks() {
        assert_eq!(cluster.block(block), state, "{block:?}");
    }
    cluster
}

/// Two regions end with the blocks and the players that one region ends with, and every
/// action was dealt with by some region.
fn assert_as_one(two: &Cluster, one: &Cluster) {
    for chunk in [HOME, EAST] {
        assert_eq!(
            two.chunk(chunk),
            one.chunk(chunk),
            "the blocks of {chunk:?}"
        );
    }
    for id in [player(1), player(2)] {
        assert_eq!(two.player(id), one.player(id));
    }
    assert_eq!(two.dealt_with, one.dealt_with);
    let last: BTreeMap<PlayerId, i32> = [(player(1), 9), (player(2), 5)].into();
    assert_eq!(one.dealt_with, last);
    assert!(two.under_way.is_empty(), "{:?}", two.under_way);
    assert_eq!(
        two.ended, 0,
        "no action was left without a region to take it"
    );
}

#[test]
fn two_regions_with_viewers_tickets_around_the_line_treat_players_and_blocks_as_one_does() {
    let one = one_region();
    let around: &[(ChunkPos, Ticket)] = &[viewer(HOME), viewer(EAST)];
    let mut two = Cluster::new(&[WESTERN, EASTERN], &[around, around]);
    assert_eq!(
        two.sites[0].world.knowledge(EAST),
        Knowledge::Foreign(REGION_B)
    );
    assert_eq!(
        two.sites[1].world.knowledge(HOME),
        Knowledge::Foreign(REGION_A)
    );

    let played = play(&mut two);
    assert_as_one(&two, &one);

    // Each region knew its neighbour: every action across the line named its region,
    // and every player was let go in the tick of the step.
    let mut remote = 0;
    for (index, (step, behind)) in played.iter().enumerate() {
        let all = std::iter::once(step).chain(behind);
        for passed in all {
            assert!(passed.remote.iter().all(Option::is_some), "step {index}");
            remote += passed.remote.len();
        }
        let later: usize = behind.iter().map(|passed| passed.departed.len()).sum();
        assert_eq!(later, 0, "step {index}");
        assert_eq!(
            step.departed.len(),
            usize::from(CROSSINGS.contains(&index)),
            "step {index}"
        );
    }
    assert!(remote >= 10, "{remote} actions went across the line");
}

#[test]
fn two_regions_that_know_nothing_of_each_other_end_with_the_same_blocks_and_players() {
    let one = one_region();
    // Each has its own chunk loaded and no ticket on the other's.
    let western: &[(ChunkPos, Ticket)] = &[viewer(HOME)];
    let eastern: &[(ChunkPos, Ticket)] = &[guest(EAST)];
    let mut two = Cluster::new(&[WESTERN, EASTERN], &[western, eastern]);
    assert_eq!(two.sites[0].world.knowledge(EAST), Knowledge::Unknown);
    assert_eq!(two.sites[1].world.knowledge(HOME), Knowledge::Unknown);

    let played = play(&mut two);
    assert_as_one(&two, &one);

    // Every action across the line was without a region, and every player was let go
    // when the store had answered: not in the tick of the step, and soon after.
    let mut remote = 0;
    for (index, (step, behind)) in played.iter().enumerate() {
        let all = std::iter::once(step).chain(behind);
        for passed in all {
            assert!(passed.remote.iter().all(Option::is_none), "step {index}");
            remote += passed.remote.len();
        }
        assert!(step.departed.is_empty(), "step {index}");
        let soon: usize = behind
            .iter()
            .take(2)
            .map(|passed| passed.departed.len())
            .sum();
        let later: usize = behind.iter().map(|passed| passed.departed.len()).sum();
        assert_eq!(
            soon,
            usize::from(CROSSINGS.contains(&index)),
            "step {index}"
        );
        assert_eq!(later, soon, "step {index}");
    }
    assert!(remote >= 10, "{remote} actions went across the line");
    // Nothing is left of what they believed for a moment.
    assert_eq!(two.sites[0].world.knowledge(EAST), Knowledge::Unknown);
    assert_eq!(two.sites[1].world.knowledge(HOME), Knowledge::Unknown);
}

/// The pinned areas of the division with a gap.
const FAR_WEST: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(-1),
};
const FAR_EAST: ChunkArea = ChunkArea {
    min_x: Some(2),
    max_x: None,
};

#[test]
fn in_a_made_up_run_of_three_regions_every_player_and_every_action_ends_in_one_place() {
    // Sections 2.2 and 2.4: no player goes round in circles, and every action ends.
    // Three regions with free chunks between them, which they take for their viewers
    // and players and give back; tickets of both kinds that come and go on every chunk
    // at every region, so that what the regions believe of each other goes stale; and
    // four players who walk across all of it and dig as they go. The cluster holds
    // every region to the record in every tick, and the store to its table.
    let chunks: Vec<ChunkPos> = (-3..=3)
        .flat_map(|x| [ChunkPos::new(x, 0), ChunkPos::new(x, 1)])
        .collect();
    let players: Vec<PlayerId> = (1..=4).map(player).collect();
    let mut let_go = 0;
    let mut passed_on = 0;
    let mut given_back = 0;
    let mut ended = 0;
    for seed in 1..=8 {
        let mut random = Random(0x2545_F491_4F6C_DD1D ^ seed);
        let mut cluster = Cluster::with_a_gap([0, 3][seed as usize % 2]);
        for id in &players {
            cluster.join(*id);
        }
        cluster.settle();
        cluster.assert_everyone_is_somewhere();

        let mut tickets: Vec<Vec<(ChunkPos, Ticket)>> = vec![Vec::new(); cluster.sites.len()];
        // Where each player stands if every step so far was taken: from there they dig.
        let mut positions: BTreeMap<PlayerId, Vec3> =
            players.iter().map(|id| (*id, SPAWN)).collect();
        let mut sequence = 0;
        for round in 0..400 {
            for (index, held) in tickets.iter_mut().enumerate() {
                let next = &mut cluster.sites[index].next;
                if random.once_in(3) {
                    let ticket = (
                        random.pick(&chunks),
                        random.pick(&[Ticket::Viewer, Ticket::Guest]),
                    );
                    held.push(ticket);
                    next.tickets_added.push(ticket);
                }
                if random.once_in(4) && !held.is_empty() {
                    let index = random.below(held.len() as u64) as usize;
                    next.tickets_removed.push(held.swap_remove(index));
                }
            }
            let mut doing = Vec::new();
            for id in &players {
                if !random.once_in(2) {
                    continue;
                }
                let position = positions.get_mut(id).expect("every player has one");
                if random.once_in(3) {
                    sequence += 1;
                    let x = position.x.floor() as i32 + random.below(5) as i32 - 2;
                    let block = BlockPos::new(x, 63, position.z.floor() as i32);
                    doing.push((*id, dig(block, sequence)));
                } else {
                    // Every other time to within a block and a half of a chunk border,
                    // from where the blocks of both chunks are within reach.
                    let x = if random.once_in(2) {
                        let border = f64::from(random.below(6) as i32 - 2) * 16.0;
                        border + random.below(30) as f64 / 10.0 - 1.5
                    } else {
                        random.below(1120) as f64 / 10.0 - 48.0
                    };
                    let z = random.below(320) as f64 / 10.0;
                    *position = Vec3::new(x, 64.0, z);
                    doing.push((*id, walk(x, z)));
                }
            }
            let passed = cluster.tick(&doing);
            let_go += passed.departed.len();
            passed_on += passed.remote.len();

            if round % 40 == 39 {
                for passed in cluster.settle() {
                    let_go += passed.departed.len();
                    passed_on += passed.remote.len();
                }
                cluster.assert_everyone_is_somewhere();
            }
        }
        given_back += cluster.returned;
        ended += cluster.ended;
    }
    // The run did what it is for: players changed regions, actions went from region to
    // region, chunks were given back, and some actions found no region to take them.
    let counts = [let_go, passed_on, given_back, ended];
    assert!(counts.iter().all(|count| *count >= 50), "{counts:?}");
}
