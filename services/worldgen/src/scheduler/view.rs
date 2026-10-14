//! The view of one feature run (ADR-0021, section 3): what each read is answered with
//! and what is recorded, `attempt` with its undoing, and the run's result.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use clustine_data::{BlockState, blocks};
use clustine_world::generation::{
    BlockBox, FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef,
    WorldgenHeightmap,
};
use clustine_world::{
    Biome, BlockEntity, BlockPos, Chunk, ChunkPos, InChunk, SECTION_SIZE, ScheduledTick,
};

use super::Recorded;
use super::lattice::{around, distance, index_in_three};
use super::output::{Changes, Output, RunResult, apply_blocks};
use crate::stages::{Stages, Terrain};

/// Terrain and starts that a run asks for beyond what it was handed: a read beyond the
/// margin, or the state of a piece of a start that its centre does not reference.
/// Both are functions of position, so fetching them keeps a run a function.
pub(crate) trait Fetch {
    fn terrain(&self, at: ChunkPos) -> Arc<Output>;
    fn starts(&self, source: ChunkPos) -> Arc<Output>;
}

/// What a line in the log says of the generator.
#[derive(Debug, Clone)]
pub(crate) struct Context {
    pub(crate) identity: String,
    pub(crate) version: u16,
    pub(crate) dimension: &'static str,
}

/// How many events a recorder keeps beside its counts.
const EVENTS: usize = 16;

/// One thing a recorder noted.
#[derive(Debug, Clone)]
struct Event {
    kind: &'static str,
    at: BlockPos,
    placing: Option<Placing>,
}

/// Six counts and the first sixteen events of a run. It never panics.
#[derive(Debug, Clone, Default)]
struct Recorder {
    counts: Recorded,
    events: Vec<Event>,
}

/// Which count an event goes to.
#[derive(Debug, Clone, Copy)]
enum Noted {
    FarRead,
    BeyondMargin,
    RefusedWrite,
    Dropped,
    FarPieceRead,
    FarPiece,
}

impl Recorder {
    fn note(&mut self, noted: Noted, at: BlockPos, placing: Option<Placing>) {
        let (count, kind) = match noted {
            Noted::FarRead => (&mut self.counts.far_reads, "a far read"),
            Noted::BeyondMargin => (&mut self.counts.beyond_margin, "a read beyond the margin"),
            Noted::RefusedWrite => (&mut self.counts.refused_writes, "a refused write"),
            Noted::Dropped => (&mut self.counts.dropped, "a dropped tick or mark"),
            Noted::FarPieceRead => (&mut self.counts.far_piece_reads, "a far piece read"),
            Noted::FarPiece => (&mut self.counts.far_pieces, "a far piece"),
        };
        *count = count.saturating_add(1);
        if self.events.len() < EVENTS {
            self.events.push(Event { kind, at, placing });
        }
    }

    fn anything(&self) -> bool {
        self.counts != Recorded::default()
    }

    /// The events, as a line in the log says them.
    fn events(&self) -> String {
        let events: Vec<String> = self
            .events
            .iter()
            .map(|event| {
                format!(
                    "{} at ({}, {}, {}) while placing {}",
                    event.kind,
                    event.at.x,
                    event.at.y,
                    event.at.z,
                    describe(event.placing)
                )
            })
            .collect();
        events.join("; ")
    }
}

/// What an attempt undoes, newest last.
#[derive(Debug)]
enum Undo {
    /// A block or a block entity of the live chunk `index` was overwritten. `touched`
    /// is whether the position had been written by this run before.
    Block {
        index: usize,
        at: InChunk,
        state: BlockState,
        entity: Option<BlockEntity>,
        touched: bool,
    },
    Tick {
        index: usize,
    },
    Mark {
        index: usize,
        at: InChunk,
    },
    Piece {
        piece: PieceKey,
        old: Option<PieceState>,
    },
}

/// Where a chunk lies from the centre of a run.
enum Place {
    /// In the three by three, at this index of the live chunks.
    Live(usize),
    /// In the ring around it, the rest of the five by five.
    Ring,
    Beyond,
}

struct View<'a> {
    centre: ChunkPos,
    min_y: i32,
    /// The height of the dimension in blocks.
    height: i32,
    /// The three by three as this run sees it now, in the order of `around`.
    live: Vec<Chunk>,
    /// The terrain of the five by five.
    terrain: &'a BTreeMap<ChunkPos, Arc<Output>>,
    fetch: &'a dyn Fetch,
    fetched_terrain: BTreeMap<ChunkPos, Arc<Output>>,
    fetched_starts: BTreeMap<ChunkPos, Arc<Output>>,
    references: Vec<StartRef>,
    /// `piece(P, c)`: what the runs this one sees left pieces in.
    seen_pieces: BTreeMap<PieceKey, PieceState>,
    /// What this run set.
    pieces: BTreeMap<PieceKey, PieceState>,
    touched: Vec<BTreeSet<InChunk>>,
    ticks: Vec<Vec<ScheduledTick>>,
    marks: Vec<BTreeSet<InChunk>>,
    recorder: Recorder,
    failures: usize,
    /// Kept only while an attempt is under way.
    undo: Vec<Undo>,
    attempts: usize,
    placing: Option<Placing>,
    context: &'a Context,
}

/// The run of `centre`: `decorate` on a view of the terrain within 2 (`terrain`, by
/// position) and of what the runs it sees left (`seen`, in ascending rank).
pub(crate) fn run<S: Stages>(
    stages: &S,
    centre: ChunkPos,
    terrain: &BTreeMap<ChunkPos, Arc<Output>>,
    seen: &[(ChunkPos, Arc<Output>)],
    fetch: &dyn Fetch,
    context: &Context,
) -> RunResult {
    let dimension = stages.dimension();
    // State(e, rank(centre)) for each e of the three by three: by L1 the runs that
    // changed e before this one are exactly the seen runs within 1 of e.
    let live: Vec<Chunk> = around(centre, 1)
        .map(|e| {
            let mut chunk = terrain_of(terrain, e).chunk.clone();
            for (d, run) in seen {
                if let Some(index) = index_in_three(*d, e) {
                    apply_blocks(&mut chunk, &run.run().changes[index]);
                }
            }
            chunk
        })
        .collect();
    let mut seen_pieces = BTreeMap::new();
    let mut tainted = false;
    for (_, run) in seen {
        let run = run.run();
        seen_pieces.extend(run.pieces.iter().map(|(key, state)| (*key, *state)));
        tainted |= run.tainted;
    }
    let mut view = View {
        centre,
        min_y: dimension.min_y,
        height: dimension.height as i32,
        live,
        terrain,
        fetch,
        fetched_terrain: BTreeMap::new(),
        fetched_starts: BTreeMap::new(),
        references: terrain_of(terrain, centre).references.clone(),
        seen_pieces,
        pieces: BTreeMap::new(),
        touched: vec![BTreeSet::new(); 9],
        ticks: vec![Vec::new(); 9],
        marks: vec![BTreeSet::new(); 9],
        recorder: Recorder::default(),
        failures: 0,
        undo: Vec::new(),
        attempts: 0,
        placing: None,
        context,
    };
    let decorated = catch_unwind(AssertUnwindSafe(|| stages.decorate(&mut view, centre)));
    if let Err(payload) = decorated {
        // Every attempt catches what panics inside it, so this panicked between two:
        // what the attempts before did is kept, and the run ends here.
        view.attempts = 0;
        view.undo.clear();
        view.placing = None;
        view.fail(None, &message(&*payload));
    }
    view.result(tainted)
}

fn terrain_of(terrain: &BTreeMap<ChunkPos, Arc<Output>>, at: ChunkPos) -> &Terrain {
    &terrain
        .get(&at)
        .expect("a run is handed the terrain within 2")
        .terrain()
        .terrain
}

/// The text of a panic's payload.
pub(crate) fn message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

/// What was being placed, as a line in the log says it.
fn describe(placing: Option<Placing>) -> String {
    match placing {
        None => "the run itself, between placements".to_owned(),
        Some(Placing::Feature(name)) => name.to_owned(),
        Some(Placing::Piece(key, name)) => format!(
            "{name}, piece {} of start {} of source chunk ({}, {})",
            key.piece, key.start.index, key.start.source.x, key.start.source.z
        ),
    }
}

/// Whether a piece's box spans chunks more than 2 apart.
fn spans_far(bounds: &BlockBox) -> bool {
    let (min, max) = (bounds.min.chunk(), bounds.max.chunk());
    i64::from(max.x) - i64::from(min.x) > 2 || i64::from(max.z) - i64::from(min.z) > 2
}

/// Whether a piece's box meets a chunk within 2 of `centre`.
fn meets_near(bounds: &BlockBox, centre: ChunkPos) -> bool {
    let (min, max) = (bounds.min.chunk(), bounds.max.chunk());
    let near = |low: i32, high: i32, at: i32| {
        i64::from(low) <= i64::from(at) + 2 && i64::from(high) >= i64::from(at) - 2
    };
    near(min.x, max.x, centre.x) && near(min.z, max.z, centre.z)
}

fn in_chunk(at: BlockPos) -> InChunk {
    let (x, z) = at.in_chunk();
    InChunk::new(x, at.y, z)
}

impl View<'_> {
    fn place(&self, chunk: ChunkPos) -> Place {
        match index_in_three(self.centre, chunk) {
            Some(index) => Place::Live(index),
            None if distance(self.centre, chunk) <= 2 => Place::Ring,
            None => Place::Beyond,
        }
    }

    fn within_height(&self, y: i32) -> bool {
        i64::from(y) >= i64::from(self.min_y)
            && i64::from(y) < i64::from(self.min_y) + i64::from(self.height)
    }

    fn note(&mut self, noted: Noted, at: BlockPos) {
        self.recorder.note(noted, at, self.placing);
    }

    /// The terrain of a chunk beyond the three by three, from what the run was handed
    /// or fetched for it. Records nothing: the caller does.
    fn frozen(&mut self, chunk: ChunkPos) -> &Terrain {
        if let Some(made) = self.terrain.get(&chunk) {
            return &made.terrain().terrain;
        }
        let fetch = self.fetch;
        &self
            .fetched_terrain
            .entry(chunk)
            .or_insert_with(|| fetch.terrain(chunk))
            .terrain()
            .terrain
    }

    /// The chunk a live read at `at` is answered from: the live one in the three by
    /// three, terrain's beyond, with the read recorded where section 3 says.
    fn read_chunk(&mut self, at: BlockPos) -> &Chunk {
        let chunk = at.chunk();
        match self.place(chunk) {
            Place::Live(index) => &self.live[index],
            Place::Ring => {
                self.note(Noted::FarRead, at);
                &self.frozen(chunk).chunk
            }
            Place::Beyond => {
                self.note(Noted::BeyondMargin, at);
                &self.frozen(chunk).chunk
            }
        }
    }

    /// The terrain a frozen read at `at` is answered from, a read beyond the five by
    /// five recorded.
    fn frozen_at(&mut self, at: BlockPos) -> &Terrain {
        let chunk = at.chunk();
        if let Place::Beyond = self.place(chunk) {
            self.note(Noted::BeyondMargin, at);
        }
        self.frozen(chunk)
    }

    /// The box and the state as made of a piece.
    fn made(&mut self, piece: PieceKey) -> (BlockBox, PieceState) {
        let source = piece.start.source;
        let known = self
            .terrain
            .values()
            .find_map(|made| made.terrain().starts.get(&source))
            .or_else(|| self.fetched_starts.get(&source))
            .cloned();
        let starts = match known {
            Some(starts) => starts,
            None => {
                let fetched = self.fetch.starts(source);
                self.fetched_starts.insert(source, Arc::clone(&fetched));
                fetched
            }
        };
        let made = starts
            .starts()
            .starts
            .get(usize::from(piece.start.index))
            .and_then(|start| start.pieces.get(usize::from(piece.piece)))
            .unwrap_or_else(|| panic!("no piece {piece:?} was made"));
        (made.bounds, made.state)
    }

    /// Keeps, while an attempt is under way, what puts a position back.
    fn keep_block(&mut self, index: usize, at: InChunk) {
        let touched = !self.touched[index].insert(at);
        if self.attempts > 0 {
            let chunk = &self.live[index];
            let state = chunk
                .get(usize::from(at.x), i32::from(at.y), usize::from(at.z))
                .expect("a position within the height");
            let entity = chunk.block_entities().get(&at).cloned();
            self.undo.push(Undo::Block {
                index,
                at,
                state,
                entity,
                touched,
            });
        }
    }

    fn undo_to(&mut self, mark: usize) {
        while self.undo.len() > mark {
            let undo = self.undo.pop().expect("longer than the mark");
            match undo {
                Undo::Block {
                    index,
                    at,
                    state,
                    entity,
                    touched,
                } => {
                    let chunk = &mut self.live[index];
                    chunk.set(usize::from(at.x), i32::from(at.y), usize::from(at.z), state);
                    if let Some(entity) = entity {
                        // It was there with this data before, beside the same others.
                        let _ = chunk.set_block_entity_data(at, entity.data);
                    }
                    if !touched {
                        self.touched[index].remove(&at);
                    }
                }
                Undo::Tick { index } => {
                    self.ticks[index].pop();
                }
                Undo::Mark { index, at } => {
                    self.marks[index].remove(&at);
                }
                Undo::Piece { piece, old } => match old {
                    Some(state) => {
                        self.pieces.insert(piece, state);
                    }
                    None => {
                        self.pieces.remove(&piece);
                    }
                },
            }
        }
    }

    fn fail(&mut self, placing: Option<Placing>, message: &str) {
        self.failures += 1;
        tracing::error!(
            identity = %self.context.identity,
            version = self.context.version,
            dimension = self.context.dimension,
            chunk = ?self.centre,
            placing = %describe(placing),
            message,
            "a placement failed and was undone"
        );
    }

    fn result(self, seen_tainted: bool) -> RunResult {
        if self.recorder.anything() {
            tracing::warn!(
                identity = %self.context.identity,
                version = self.context.version,
                dimension = self.context.dimension,
                chunk = ?self.centre,
                recorded = ?self.recorder.counts,
                events = %self.recorder.events(),
                "a run read or wrote beyond what it may"
            );
        }
        let changes = (0..9)
            .map(|index| {
                let chunk = &self.live[index];
                let blocks = self.touched[index]
                    .iter()
                    .map(|at| {
                        let state = chunk
                            .get(usize::from(at.x), i32::from(at.y), usize::from(at.z))
                            .expect("a written position is within the height");
                        (*at, state, chunk.block_entities().get(at).cloned())
                    })
                    .collect();
                Changes {
                    blocks,
                    ticks: self.ticks[index].clone(),
                    marks: self.marks[index].iter().copied().collect(),
                }
            })
            .collect();
        RunResult {
            changes,
            pieces: self.pieces,
            recorded: self.recorder.counts,
            failures: self.failures,
            tainted: seen_tainted || self.failures > 0,
        }
    }
}

impl Neighbourhood for View<'_> {
    fn centre(&self) -> ChunkPos {
        self.centre
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        if !self.within_height(at.y) {
            return blocks::VOID_AIR;
        }
        let (x, z) = at.in_chunk();
        self.read_chunk(at)
            .get(x, at.y, z)
            .expect("within the height of the dimension")
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        let Place::Live(index) = self.place(at.chunk()) else {
            self.note(Noted::RefusedWrite, at);
            return false;
        };
        if !self.within_height(at.y) {
            return false;
        }
        let position = in_chunk(at);
        self.keep_block(index, position);
        let (x, z) = at.in_chunk();
        self.live[index].set(x, at.y, z, state);
        true
    }

    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        if !self.within_height(at.y) {
            return None;
        }
        let position = in_chunk(at);
        self.read_chunk(at).block_entities().get(&position)
    }

    /// Gives the block entity at `at` the data of `entity`, or no data for `None`. A
    /// block entity is where its block keeps one, and nowhere else (the chunk keeps
    /// that true): so this is refused where the block keeps none, or one of another
    /// type, and where the data is more than a chunk may carry.
    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        let Place::Live(index) = self.place(at.chunk()) else {
            self.note(Noted::RefusedWrite, at);
            return false;
        };
        if !self.within_height(at.y) {
            return false;
        }
        let position = in_chunk(at);
        let Some(there) = self.live[index].block_entities().get(&position) else {
            return entity.is_none();
        };
        let data = match entity {
            Some(entity) if entity.kind != there.kind => return false,
            Some(entity) => entity.data,
            None => Vec::new(),
        };
        let touched = self.touched[index].contains(&position);
        let mark = self.undo.len();
        self.keep_block(index, position);
        if self.live[index]
            .set_block_entity_data(position, data)
            .is_err()
        {
            // Nothing was changed: what was kept to put it back is not needed.
            self.undo.truncate(mark);
            if !touched {
                self.touched[index].remove(&position);
            }
            return false;
        }
        true
    }

    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        let at = BlockPos::new(x, self.min_y, z);
        let min_y = self.min_y;
        let heights = self.read_chunk(at).heights();
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        let height = match map {
            FinalHeightmap::WorldSurface => heights.world_surface[column],
            FinalHeightmap::OceanFloor => heights.ocean_floor[column],
            FinalHeightmap::MotionBlocking => heights.motion_blocking[column],
            FinalHeightmap::MotionBlockingNoLeaves => heights.motion_blocking_no_leaves[column],
        };
        min_y + i32::from(height)
    }

    /// As [`height`](Self::height), the y above the highest block that counts.
    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
        let at = BlockPos::new(x, self.min_y, z);
        let min_y = self.min_y;
        let heights = &self.frozen_at(at).worldgen_heights;
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        let height = match map {
            WorldgenHeightmap::WorldSurface => heights.world_surface[column],
            WorldgenHeightmap::OceanFloor => heights.ocean_floor[column],
        };
        min_y + i32::from(height)
    }

    /// The biome at `at`; above or below the dimension, that of the nearest block
    /// within it, as the game clamps.
    fn biome(&mut self, at: BlockPos) -> Biome {
        let top = self.min_y + self.height - 1;
        let y = at.y.clamp(self.min_y, top);
        let min_y = self.min_y;
        let chunk = &self.frozen_at(at).chunk;
        let above_bottom = (y - min_y) as usize;
        let (x, z) = at.in_chunk();
        chunk.sections()[above_bottom / SECTION_SIZE].biome_at(x, above_bottom % SECTION_SIZE, z)
    }

    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
        let Place::Live(index) = self.place(at.chunk()) else {
            self.note(Noted::Dropped, at);
            return;
        };
        if !self.within_height(at.y) {
            return;
        }
        self.ticks[index].push(ScheduledTick {
            at: in_chunk(at),
            ..tick
        });
        if self.attempts > 0 {
            self.undo.push(Undo::Tick { index });
        }
    }

    fn mark(&mut self, at: BlockPos) {
        let Place::Live(index) = self.place(at.chunk()) else {
            self.note(Noted::Dropped, at);
            return;
        };
        if !self.within_height(at.y) {
            return;
        }
        let position = in_chunk(at);
        if self.marks[index].insert(position) && self.attempts > 0 {
            self.undo.push(Undo::Mark {
                index,
                at: position,
            });
        }
    }

    fn starts(&self) -> &[StartRef] {
        &self.references
    }

    fn piece(&mut self, piece: PieceKey) -> PieceState {
        let (bounds, made) = self.made(piece);
        if !meets_near(&bounds, self.centre) {
            self.note(Noted::FarPieceRead, bounds.min);
        }
        self.pieces
            .get(&piece)
            .or_else(|| self.seen_pieces.get(&piece))
            .copied()
            .unwrap_or(made)
    }

    fn set_piece(&mut self, piece: PieceKey, state: PieceState) {
        let (bounds, _) = self.made(piece);
        if spans_far(&bounds) {
            self.note(Noted::FarPiece, bounds.min);
        }
        let old = self.pieces.insert(piece, state);
        if self.attempts > 0 {
            self.undo.push(Undo::Piece { piece, old });
        }
    }

    fn attempt(&mut self, what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        let mark = self.undo.len();
        let recorder = self.recorder.clone();
        let failures = self.failures;
        let outer = self.placing.replace(what);
        self.attempts += 1;
        let placed = catch_unwind(AssertUnwindSafe(|| place(&mut *self)));
        self.attempts -= 1;
        self.placing = outer;
        let kept = match placed {
            Ok(()) => true,
            Err(payload) => {
                self.undo_to(mark);
                self.recorder = recorder;
                self.failures = failures;
                self.fail(Some(what), &message(&*payload));
                false
            }
        };
        if self.attempts == 0 {
            self.undo.clear();
        }
        kept
    }
}
