//! What the scheduler's tests share: toy stages, the lattice of
//! `docs/adr/0021-generation-is-a-function.md`, section 2, worked out here by hand, the
//! plain model of that section, and ways to ask a scheduler for many chunks at once.
//!
//! Written from the record, without the scheduler's code. Every test binary uses a
//! part of this, so what one of them does not use is not a fault.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

use clustine_data::{BLOCK_INFO, BlockState, DIMENSION_TYPES, DimensionType, blocks};
use clustine_world::generation::{
    BlockBox, Failed, FinalHeightmap, GeneratorSettings, Neighbourhood, PieceKey, PieceState,
    Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{
    BLOCKS_PER_SECTION, Biome, BlockEntity, BlockPos, Chunk, ChunkGenerator, ChunkPos, InChunk,
    ScheduledTick, Section, Ticked,
};
use clustine_worldgen::scheduler::{Options, Order, Pick, Scheduler};
use clustine_worldgen::stages::{
    PieceMade, Stages, Start, Starts, StartsNear, Terrain, WorldgenHeights,
};

/// The version every toy says it is; a lit chunk is marked with it.
pub const TOY_VERSION: u16 = 7;

pub fn overworld() -> &'static DimensionType {
    DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the overworld is a vanilla dimension type")
}

// The lattice.

/// The rank of a chunk, by the table of section 2.
pub fn rank(position: ChunkPos) -> u8 {
    const RANKS: [[u8; 3]; 3] = [[0, 1, 2], [7, 6, 3], [8, 5, 4]];
    RANKS[position.x.rem_euclid(3) as usize][position.z.rem_euclid(3) as usize]
}

/// The Chebyshev distance, which "within n" means throughout the record.
pub fn distance(a: ChunkPos, b: ChunkPos) -> i32 {
    (a.x - b.x).abs().max((a.z - b.z).abs())
}

/// The chunks within `n` of `centre`, ascending.
pub fn within(centre: ChunkPos, n: i32) -> impl Iterator<Item = ChunkPos> {
    (-n..=n).flat_map(move |dx| (-n..=n).map(move |dz| ChunkPos::new(centre.x + dx, centre.z + dz)))
}

/// Every chunk within `n` of a chunk of `set`.
pub fn dilate(set: &BTreeSet<ChunkPos>, n: i32) -> BTreeSet<ChunkPos> {
    set.iter()
        .flat_map(|position| within(*position, n))
        .collect()
}

/// The chunks of a rectangle, both corners included.
pub fn rectangle(min: ChunkPos, max: ChunkPos) -> BTreeSet<ChunkPos> {
    (min.x..=max.x)
        .flat_map(|x| (min.z..=max.z).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// What a chunk depends on, by the sets of section 2.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    pub runs: BTreeSet<ChunkPos>,
    pub terrain: BTreeSet<ChunkPos>,
    pub starts: BTreeSet<ChunkPos>,
}

impl Needs {
    fn of_runs(runs: BTreeSet<ChunkPos>) -> Self {
        let terrain = dilate(&runs, 2);
        let starts = dilate(&terrain, 8);
        Self {
            runs,
            terrain,
            starts,
        }
    }

    fn moved(&self, dx: i32, dz: i32) -> Self {
        let moved = |set: &BTreeSet<ChunkPos>| {
            set.iter()
                .map(|p| ChunkPos::new(p.x + dx, p.z + dz))
                .collect()
        };
        Self {
            runs: moved(&self.runs),
            terrain: moved(&self.terrain),
            starts: moved(&self.starts),
        }
    }

    pub fn union(mut self, other: &Self) -> Self {
        self.runs.extend(other.runs.iter().copied());
        self.terrain.extend(other.terrain.iter().copied());
        self.starts.extend(other.starts.iter().copied());
        self
    }
}

/// `runs(c)`: `c` and, with every chunk in it, every chunk within 2 of lower rank.
pub fn runs_of(centre: ChunkPos) -> BTreeSet<ChunkPos> {
    let mut runs = BTreeSet::from([centre]);
    let mut open = vec![centre];
    while let Some(run) = open.pop() {
        for seen in within(run, 2) {
            if rank(seen) < rank(run) && runs.insert(seen) {
                open.push(seen);
            }
        }
    }
    runs
}

/// The class of a chunk as a position in `0..3` squared, and how far the chunk is from
/// it. The lattice repeats every three chunks, so what a chunk needs is what its
/// class's representative needs, moved.
fn class(position: ChunkPos) -> (ChunkPos, i32, i32) {
    let representative = ChunkPos::new(position.x.rem_euclid(3), position.z.rem_euclid(3));
    (
        representative,
        position.x - representative.x,
        position.z - representative.z,
    )
}

fn by_class(make: impl Fn(ChunkPos) -> Needs) -> Vec<Needs> {
    (0..3)
        .flat_map(|x| (0..3).map(move |z| ChunkPos::new(x, z)))
        .map(make)
        .collect()
}

fn class_index(representative: ChunkPos) -> usize {
    (representative.x * 3 + representative.z) as usize
}

/// What `finished(e)` depends on: `R(e)`, `T(e)` and the starts within 8 of `T(e)`.
pub fn finished_needs(e: ChunkPos) -> Needs {
    static NEEDS: OnceLock<Vec<Needs>> = OnceLock::new();
    let needs = NEEDS.get_or_init(|| {
        by_class(|e| {
            let runs = within(e, 1).flat_map(runs_of).collect();
            Needs::of_runs(runs)
        })
    });
    let (representative, dx, dz) = class(e);
    needs[class_index(representative)].moved(dx, dz)
}

/// What the lit chunk at `p` depends on: that of the nine finished chunks within 1.
pub fn lit_needs(p: ChunkPos) -> Needs {
    let (representative, dx, dz) = class(p);
    lit_needs_by_class()[class_index(representative)].moved(dx, dz)
}

fn lit_needs_by_class() -> &'static [Needs] {
    static NEEDS: OnceLock<Vec<Needs>> = OnceLock::new();
    NEEDS.get_or_init(|| {
        by_class(|p| {
            let runs = within(p, 1).flat_map(|e| finished_needs(e).runs).collect();
            Needs::of_runs(runs)
        })
    })
}

/// Which of the sets a chunk depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Runs,
    Terrain,
    Starts,
}

/// Whether the lit chunk at `p` depends on the task of `kind` at `on`: what
/// [`lit_needs`] says, without making the sets.
pub fn lit_depends(p: ChunkPos, kind: Kind, on: ChunkPos) -> bool {
    let (representative, dx, dz) = class(p);
    let needs = &lit_needs_by_class()[class_index(representative)];
    let set = match kind {
        Kind::Runs => &needs.runs,
        Kind::Terrain => &needs.terrain,
        Kind::Starts => &needs.starts,
    };
    set.contains(&ChunkPos::new(on.x - dx, on.z - dz))
}

/// What the lit chunks of `lit` depend on together.
pub fn lit_needs_of(lit: &BTreeSet<ChunkPos>) -> Needs {
    lit.iter()
        .fold(Needs::default(), |all, p| all.union(&lit_needs(*p)))
}

/// The chunks an edge has a player view at distance `view_distance`: `view_area` of
/// `services/edge/src/fanout.rs`, written out again here.
pub fn view_area(centre: ChunkPos, view_distance: i32) -> BTreeSet<ChunkPos> {
    const BUFFER: i64 = 2;
    let reach = view_distance + BUFFER as i32;
    let limit = i64::from(view_distance) * i64::from(view_distance);
    let mut area = BTreeSet::new();
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let x = (i64::from(dx).abs() - BUFFER).max(0);
            let z = (i64::from(dz).abs() - BUFFER).max(0);
            if x * x + z * z < limit {
                area.insert(ChunkPos::new(centre.x + dx, centre.z + dz));
            }
        }
    }
    area
}

// Seeded randomness of the tests' own: no hash map, no clock.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// SplitMix64.
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A number in `low..high`.
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        low + self.below((high - low) as u64) as i32
    }
}

/// A number made from a seed, a position and what it is for.
pub fn mix(seed: u64, x: i32, z: i32, salt: u64) -> u64 {
    let mut rng = Rng::new(
        seed ^ (u64::from(x as u32) << 32)
            ^ u64::from(z as u32)
            ^ salt.wrapping_mul(0xd6e8_feb8_6659_fd93),
    );
    rng.next();
    rng.next()
}

pub fn wool(colour: u64) -> BlockState {
    BlockState(blocks::WHITE_WOOL.0 + (colour % 16) as u16)
}

pub fn chest_kind() -> u8 {
    let block = blocks::CHEST.block_id().expect("a chest is a block");
    BLOCK_INFO[block]
        .block_entity_type
        .expect("a chest keeps a block entity")
}

fn in_chunk(at: BlockPos) -> InChunk {
    let (x, z) = at.in_chunk();
    InChunk::new(x, at.y, z)
}

// The toy stages.

/// What the toy's runs do besides structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Test 1's toy features: they read blocks and final heights up to 16 blocks
    /// beyond the centre chunk, write up to 16 beyond, read worldgen heights and biomes
    /// up to 32 beyond, leave ticks and marks, place chests with data, and set the
    /// state of pieces whose boxes span chunks at most 2 apart, some twice in one run.
    Main,
    /// `Main`, and one live read at 17 blocks beyond the centre chunk.
    ReadsFar,
    /// `Main`, and a block, a block entity, a tick and a mark at 17 blocks beyond.
    WritesFar,
    /// Only one start, at (0, 0), with a piece that spans chunks 0 to 3 in x: each run
    /// that the piece meets writes what it reads of it, and the run of (0, 0) sets it.
    SetsFarPiece,
    /// Only one start, at (0, 0), with a piece in that chunk alone: every run writes
    /// what it reads of it, and the run of (0, 0) sets it.
    ReadsFarPiece,
}

/// What the toy does wrong at one chunk's run (test 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The placement "toy:faulty" writes blocks, a block entity, a tick, a mark and a
    /// piece's state, and panics.
    Panics,
    /// The placement "toy:faulty" is not made at all: what `Panics` must come to.
    DoesNothing,
    /// The run panics between its first placement and the rest.
    CutShort,
    /// The run returns after its first placement: what `CutShort` must come to.
    StopsEarly,
}

/// Where the fault's placements write in their chunk, at a height nothing else uses.
pub const FAULT_Y: i32 = 200;
pub const BEFORE_AT: (usize, usize) = (1, 1);
pub const AFTER_AT: (usize, usize) = (2, 2);
pub const FAULTY_AT: (usize, usize) = (3, 3);
pub const FAULTY_CHEST_AT: (usize, usize) = (4, 4);

/// Where the far piece toys write what they read, in the centre chunk.
pub const PIECE_WITNESS: (usize, i32, usize) = (5, 100, 5);
/// The flags of the far pieces as made, and as the run of (0, 0) sets them.
pub const FAR_MADE: u16 = 3;
pub const FAR_SET: u16 = 9;
pub const FAR_SOURCE: ChunkPos = ChunkPos::new(0, 0);

/// How many times the toy's stages were called to their end.
#[derive(Debug, Default)]
pub struct Calls {
    pub starts: AtomicU64,
    pub terrain: AtomicU64,
    pub decorate: AtomicU64,
    /// Calls of `terrain` or `starts` at the position where they panic, counted before
    /// the panic.
    pub at_panic: AtomicU64,
}

impl Calls {
    pub fn get(&self) -> (u64, u64, u64, u64) {
        (
            self.starts.load(Ordering::SeqCst),
            self.terrain.load(Ordering::SeqCst),
            self.decorate.load(Ordering::SeqCst),
            self.at_panic.load(Ordering::SeqCst),
        )
    }
}

/// Holds every stage call while it is shut, so that a test can keep tasks under way.
/// Each call that finds it shut says so on a channel before it waits.
pub struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
    entered: Mutex<mpsc::Sender<()>>,
}

impl Gate {
    pub fn new(open: bool) -> (Arc<Self>, mpsc::Receiver<()>) {
        let (entered, told) = mpsc::channel();
        let gate = Self {
            open: Mutex::new(open),
            changed: Condvar::new(),
            entered: Mutex::new(entered),
        };
        (Arc::new(gate), told)
    }

    pub fn set(&self, open: bool) {
        *self.open.lock().unwrap() = open;
        self.changed.notify_all();
    }

    fn pass(&self) {
        let mut open = self.open.lock().unwrap();
        if !*open {
            // The test may have stopped listening; that is no fault of the call.
            let _ = self.entered.lock().unwrap().send(());
        }
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
}

/// Toy stages: cheap, seeded, and a function of the seed and what they are handed.
pub struct Toy {
    pub seed: u64,
    pub mode: Mode,
    pub fault: Option<(ChunkPos, Fault)>,
    pub terrain_panics_at: Option<ChunkPos>,
    pub starts_panic_at: Option<ChunkPos>,
    pub gate: Option<Arc<Gate>>,
    pub calls: Arc<Calls>,
}

impl Toy {
    pub fn new(seed: u64, mode: Mode) -> Self {
        Self {
            seed,
            mode,
            fault: None,
            terrain_panics_at: None,
            starts_panic_at: None,
            gate: None,
            calls: Arc::new(Calls::default()),
        }
    }

    pub fn with_fault(mut self, at: ChunkPos, fault: Fault) -> Self {
        self.fault = Some((at, fault));
        self
    }

    pub fn with_terrain_panicking_at(mut self, at: ChunkPos) -> Self {
        self.terrain_panics_at = Some(at);
        self
    }

    pub fn with_starts_panicking_at(mut self, at: ChunkPos) -> Self {
        self.starts_panic_at = Some(at);
        self
    }

    pub fn with_gate(mut self, gate: Arc<Gate>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// The same toy with counts of its own and no gate.
    pub fn again(&self) -> Self {
        Self {
            seed: self.seed,
            mode: self.mode,
            fault: self.fault,
            terrain_panics_at: self.terrain_panics_at,
            starts_panic_at: self.starts_panic_at,
            gate: None,
            calls: Arc::new(Calls::default()),
        }
    }

    fn pass(&self) {
        if let Some(gate) = &self.gate {
            gate.pass();
        }
    }

    /// The starts of `source`, without counting or waiting.
    pub fn make_starts(&self, source: ChunkPos) -> Starts {
        let piece = |min: BlockPos, max: BlockPos, flags| PieceMade {
            bounds: BlockBox { min, max },
            state: PieceState {
                shift_y: 0,
                settled: false,
                flags,
            },
        };
        match self.mode {
            Mode::SetsFarPiece | Mode::ReadsFarPiece => {
                if source != FAR_SOURCE {
                    return Starts::default();
                }
                let max_x = if self.mode == Mode::SetsFarPiece {
                    63
                } else {
                    15
                };
                return Starts {
                    starts: vec![Start {
                        structure: "toy:far",
                        pieces: vec![piece(
                            BlockPos::new(0, -60, 0),
                            BlockPos::new(max_x, -40, 15),
                            FAR_MADE,
                        )],
                    }],
                };
            }
            Mode::Main | Mode::ReadsFar | Mode::WritesFar => {}
        }
        let mut rng = Rng::new(mix(self.seed, source.x, source.z, 1));
        let mut starts = Vec::new();
        if rng.below(5) == 0 {
            for _ in 0..1 + rng.below(2) {
                let pieces = (0..1 + rng.below(3))
                    .map(|_| {
                        // Boxes span chunks at most 2 apart, within 5 of the source.
                        let x = source.x + rng.range(-3, 4);
                        let z = source.z + rng.range(-3, 4);
                        let (wide, deep) = (rng.range(0, 3), rng.range(0, 3));
                        let min =
                            BlockPos::new(x * 16 + rng.range(0, 8), -60, z * 16 + rng.range(0, 8));
                        let max = BlockPos::new(
                            (x + wide) * 16 + 8 + rng.range(0, 8),
                            -40,
                            (z + deep) * 16 + 8 + rng.range(0, 8),
                        );
                        piece(min, max, rng.below(1 << 16) as u16)
                    })
                    .collect();
                starts.push(Start {
                    structure: "toy:scattered",
                    pieces,
                });
            }
        }
        if let Some((at, _)) = self.fault
            && at == source
        {
            // A piece that the faulty placement sets, which the runs on either side of
            // it read after.
            starts.push(Start {
                structure: "toy:faulty_piece",
                pieces: vec![piece(
                    BlockPos::new((at.x - 1) * 16 + 4, -60, at.z * 16 + 2),
                    BlockPos::new((at.x + 1) * 16 + 11, -40, at.z * 16 + 13),
                    5,
                )],
            });
        }
        Starts { starts }
    }

    /// The piece that the faulty placement sets.
    pub fn faulty_piece(&self) -> Option<PieceKey> {
        let (at, _) = self.fault?;
        let index = self.make_starts(at).starts.len() - 1;
        Some(PieceKey {
            start: StartRef {
                source: at,
                index: index as u16,
            },
            piece: 0,
        })
    }

    fn fault_here(&self, centre: ChunkPos) -> Option<Fault> {
        self.fault
            .and_then(|(at, fault)| (at == centre).then_some(fault))
    }

    /// Places blocks on the ground, reading what is around, as section 9's test 1 asks.
    fn scatter(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        let (base_x, base_z) = (centre.x * 16, centre.z * 16);
        const MAPS: [FinalHeightmap; 4] = [
            FinalHeightmap::WorldSurface,
            FinalHeightmap::OceanFloor,
            FinalHeightmap::MotionBlocking,
            FinalHeightmap::MotionBlockingNoLeaves,
        ];
        for index in 0..6 {
            let mut rng = Rng::new(mix(self.seed, centre.x, centre.z, 100 + index));
            view.attempt(Placing::Feature("toy:scatter"), &mut |v| {
                let x = base_x + rng.range(-16, 32);
                let z = base_z + rng.range(-16, 32);
                let map = MAPS[rng.below(4) as usize];
                let y = v.height(map, x, z);
                let at = BlockPos::new(x, y, z);
                let below = v.block(at.offset(0, -1, 0));
                let kept = v
                    .block_entity(at.offset(0, -1, 0))
                    .map(|entity| entity.data.first().copied().unwrap_or(0));
                let (far_x, far_z) = (base_x + rng.range(-32, 48), base_z + rng.range(-32, 48));
                let frozen_map = if rng.below(2) == 0 {
                    WorldgenHeightmap::WorldSurface
                } else {
                    WorldgenHeightmap::OceanFloor
                };
                let frozen = v.worldgen_height(frozen_map, far_x, far_z);
                let biome = v.biome(BlockPos::new(far_x, frozen, far_z));
                let choice =
                    u64::from(below.0) ^ (frozen as u64) ^ (u64::from(biome.0) << 3) ^ rng.next();
                let state = match kept {
                    Some(data) => wool(u64::from(data)),
                    None if below == blocks::OAK_LEAVES => blocks::GLASS,
                    None if choice.is_multiple_of(5) => blocks::CHEST,
                    None if choice % 5 == 1 => blocks::OAK_LEAVES,
                    None => wool(choice >> 8),
                };
                v.set_block(at, state);
                if state == blocks::CHEST {
                    let data = vec![(choice >> 16) as u8, frozen as u8, biome.0 as u8];
                    let entity = BlockEntity {
                        kind: chest_kind(),
                        data,
                    };
                    v.set_block_entity(at, Some(entity));
                }
                // Sometimes a second write, on top or in the same place again.
                match rng.below(4) {
                    0 => {
                        v.set_block(at.offset(0, 1, 0), wool(choice >> 12));
                    }
                    1 if state != blocks::CHEST => {
                        v.set_block(at, wool(choice >> 20));
                    }
                    _ => {}
                }
                let tick = ScheduledTick {
                    at: in_chunk(at),
                    of: Ticked::Block(state.0),
                    delay: rng.range(1, 40),
                    priority: rng.range(-3, 3) as i8,
                };
                v.schedule(at, tick);
                if rng.below(2) == 0 {
                    v.mark(at);
                }
            });
        }
    }

    /// The pieces of the starts that the centre's references name and that meet it:
    /// each writes what it reads of its state and sets it, some twice.
    fn pieces(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        let references = view.starts().to_vec();
        for reference in references {
            let starts = self.make_starts(reference.source);
            let start = &starts.starts[usize::from(reference.index)];
            for (index, piece) in start.pieces.iter().enumerate() {
                if !piece.bounds.meets(centre) {
                    continue;
                }
                let key = PieceKey {
                    start: reference,
                    piece: index as u16,
                };
                let salt = 1000 + u64::from(reference.index) * 16 + index as u64;
                let mut rng = Rng::new(mix(self.seed, centre.x, centre.z, salt));
                let rank = u16::from(rank(centre));
                view.attempt(Placing::Piece(key, start.structure), &mut |v| {
                    let state = v.piece(key);
                    let x = centre.x * 16 + rng.range(0, 16);
                    let z = centre.z * 16 + rng.range(0, 16);
                    let y = v.height(FinalHeightmap::WorldSurface, x, z);
                    let code = u64::from(state.flags)
                        ^ ((state.shift_y as u64) << 4)
                        ^ (u64::from(state.settled) << 2);
                    v.set_block(BlockPos::new(x, y, z), wool(code ^ (code >> 8)));
                    let next = PieceState {
                        shift_y: state.shift_y + 1,
                        settled: !state.settled,
                        flags: state.flags.wrapping_mul(31).wrapping_add(rank * 7 + 1),
                    };
                    v.set_piece(key, next);
                    if rng.below(2) == 0 {
                        v.set_piece(
                            key,
                            PieceState {
                                flags: next.flags ^ 0x5a5a,
                                ..next
                            },
                        );
                    }
                });
            }
        }
    }

    /// The three placements of test 6, in every run when the toy has a fault.
    fn fault_placements(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) -> bool {
        let at = |(x, z): (usize, usize), y: i32| {
            BlockPos::new(centre.x * 16 + x as i32, y, centre.z * 16 + z as i32)
        };
        view.attempt(Placing::Feature("toy:before"), &mut |v| {
            v.set_block(at(BEFORE_AT, FAULT_Y), blocks::SAND);
        });
        match self.fault_here(centre) {
            Some(Fault::CutShort) => {
                panic!("toy: the run is cut short between two placements")
            }
            Some(Fault::StopsEarly) => return false,
            Some(Fault::DoesNothing) => {}
            Some(Fault::Panics) => {
                let piece = self.faulty_piece().expect("the toy has a fault");
                view.attempt(Placing::Feature("toy:faulty"), &mut |v| {
                    let faulty = at(FAULTY_AT, FAULT_Y);
                    v.set_block(faulty, blocks::BEDROCK);
                    v.set_block(faulty.offset(16, 0, 0), blocks::BEDROCK);
                    let chest = at(FAULTY_CHEST_AT, FAULT_Y);
                    v.set_block(chest, blocks::CHEST);
                    let entity = BlockEntity {
                        kind: chest_kind(),
                        data: vec![1, 2, 3],
                    };
                    v.set_block_entity(chest, Some(entity));
                    let tick = ScheduledTick {
                        at: in_chunk(faulty),
                        of: Ticked::Block(blocks::BEDROCK.0),
                        delay: 5,
                        priority: 0,
                    };
                    v.schedule(faulty, tick);
                    v.mark(chest);
                    let state = PieceState {
                        shift_y: 40,
                        settled: true,
                        flags: 0xdead,
                    };
                    v.set_piece(piece, state);
                    panic!("toy: the faulty placement fails");
                });
            }
            None => {
                view.attempt(Placing::Feature("toy:faulty"), &mut |v| {
                    v.set_block(at(FAULTY_AT, FAULT_Y + 1), blocks::GLASS);
                });
            }
        }
        view.attempt(Placing::Feature("toy:after"), &mut |v| {
            v.set_block(at(AFTER_AT, FAULT_Y), blocks::GLASS_PANE);
        });
        true
    }

    fn reads_far(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        let mut rng = Rng::new(mix(self.seed, centre.x, centre.z, 7));
        // Every run paints the outermost column row of its chunk in +x where terrain
        // ends, so that what the run two chunks off in +x reads there live depends
        // on whether this run came first.
        view.attempt(Placing::Feature("toy:painter"), &mut |v| {
            let x = centre.x * 16 + 15;
            for z in centre.z * 16..centre.z * 16 + 16 {
                let y = v.worldgen_height(WorldgenHeightmap::WorldSurface, x, z);
                v.set_block(BlockPos::new(x, y, z), wool(4));
            }
        });
        view.attempt(Placing::Feature("toy:far_reader"), &mut |v| {
            let x = centre.x * 16 - 17;
            let z = centre.z * 16 + rng.range(0, 16);
            // Where terrain ends: air in terrain, and where the runs that may write
            // there put their first block.
            let y = v.worldgen_height(WorldgenHeightmap::WorldSurface, x, z);
            let seen = v.block(BlockPos::new(x, y, z));
            let (here_x, here_z) = (centre.x * 16 + 8, centre.z * 16 + 8);
            let here = v.height(FinalHeightmap::MotionBlocking, here_x, here_z);
            let state = if seen == blocks::AIR {
                blocks::GLASS
            } else {
                wool(u64::from(seen.0))
            };
            v.set_block(BlockPos::new(here_x, here, here_z), state);
        });
    }

    fn writes_far(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        view.attempt(Placing::Feature("toy:far_writer"), &mut |v| {
            let far = BlockPos::new(centre.x * 16 - 17, 150, centre.z * 16 + 4);
            let wrote = v.set_block(far, blocks::CHEST);
            let entity = BlockEntity {
                kind: chest_kind(),
                data: vec![9],
            };
            let gave = v.set_block_entity(far, Some(entity));
            let tick = ScheduledTick {
                at: in_chunk(far),
                of: Ticked::Block(blocks::CHEST.0),
                delay: 2,
                priority: 0,
            };
            v.schedule(far, tick);
            v.mark(far);
            let witness = BlockPos::new(centre.x * 16 + 9, 150, centre.z * 16 + 9);
            v.set_block(witness, wool(1 + u64::from(wrote) + 2 * u64::from(gave)));
        });
    }

    fn far_piece(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        let key = PieceKey {
            start: StartRef {
                source: FAR_SOURCE,
                index: 0,
            },
            piece: 0,
        };
        let starts = self.make_starts(FAR_SOURCE);
        let piece = &starts.starts[0].pieces[0];
        let reads = match self.mode {
            Mode::SetsFarPiece => piece.bounds.meets(centre),
            _ => true,
        };
        if !reads {
            return;
        }
        view.attempt(Placing::Piece(key, "toy:far"), &mut |v| {
            let state = v.piece(key);
            let (x, y, z) = PIECE_WITNESS;
            let witness = BlockPos::new(centre.x * 16 + x as i32, y, centre.z * 16 + z as i32);
            v.set_block(witness, wool(u64::from(state.flags)));
            if centre == FAR_SOURCE {
                let set = PieceState {
                    flags: FAR_SET,
                    ..state
                };
                v.set_piece(key, set);
            }
        });
    }
}

struct MapStarts<'a>(&'a BTreeMap<ChunkPos, Starts>);

impl StartsNear for MapStarts<'_> {
    fn starts(&self, source: ChunkPos) -> &Starts {
        self.0
            .get(&source)
            .expect("the model made the starts within 8 of its terrain")
    }
}

impl Stages for Toy {
    fn settings(&self) -> GeneratorSettings {
        GeneratorSettings {
            identity: format!(
                "toy;seed {};{:?};fault {:?};terrain panics {:?};starts panic {:?}",
                self.seed, self.mode, self.fault, self.terrain_panics_at, self.starts_panic_at
            ),
            version: TOY_VERSION,
        }
    }

    fn dimension(&self) -> &'static DimensionType {
        overworld()
    }

    fn starts(&self, source: ChunkPos) -> Starts {
        self.pass();
        if self.starts_panic_at == Some(source) {
            self.calls.at_panic.fetch_add(1, Ordering::SeqCst);
            panic!("toy: the starts of {source:?} fail");
        }
        let starts = self.make_starts(source);
        self.calls.starts.fetch_add(1, Ordering::SeqCst);
        starts
    }

    fn terrain(&self, position: ChunkPos, starts: &dyn StartsNear) -> Terrain {
        self.pass();
        if self.terrain_panics_at == Some(position) {
            self.calls.at_panic.fetch_add(1, Ordering::SeqCst);
            panic!("toy: the terrain of {position:?} fails");
        }
        let mut references = Vec::new();
        for source in within(position, 8) {
            for (index, start) in starts.starts(source).starts.iter().enumerate() {
                if start
                    .pieces
                    .iter()
                    .any(|piece| piece.bounds.meets(position))
                {
                    references.push(StartRef {
                        source,
                        index: index as u16,
                    });
                }
            }
        }
        // Columns filled to a height, which the starts nearby raise a little, in two
        // sections at the bottom of the world.
        let mut low = vec![blocks::AIR; 2 * BLOCKS_PER_SECTION];
        for z in 0..16 {
            for x in 0..16 {
                let (column_x, column_z) = (position.x * 16 + x as i32, position.z * 16 + z as i32);
                let height =
                    6 + mix(self.seed, column_x, column_z, 3) % 20 + references.len() as u64 % 3;
                for y in 0..height as usize {
                    let state = if y + 1 == height as usize {
                        blocks::DIRT
                    } else {
                        blocks::STONE
                    };
                    low[y << 8 | z << 4 | x] = state;
                }
            }
        }
        let biome = Biome((mix(self.seed, position.x, position.z, 4) % 4) as u16);
        let dimension = overworld();
        let count = dimension.height as usize / 16;
        let mut sections = Vec::with_capacity(count);
        for half in low.chunks(BLOCKS_PER_SECTION) {
            let states: Box<[BlockState; BLOCKS_PER_SECTION]> = half
                .to_vec()
                .into_boxed_slice()
                .try_into()
                .expect("a section's blocks");
            sections.push(Section::from_states(states, biome));
        }
        sections.resize(count, Section::filled(blocks::AIR, biome));
        let chunk = Chunk::from_sections(dimension.min_y, sections);
        let worldgen_heights = Box::new(WorldgenHeights {
            world_surface: chunk.heights().world_surface,
            ocean_floor: chunk.heights().ocean_floor,
        });
        self.calls.terrain.fetch_add(1, Ordering::SeqCst);
        Terrain {
            chunk,
            worldgen_heights,
            references,
        }
    }

    fn decorate(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        self.pass();
        match self.mode {
            Mode::SetsFarPiece | Mode::ReadsFarPiece => self.far_piece(view, centre),
            Mode::Main | Mode::ReadsFar | Mode::WritesFar => {
                let carry_on = self.fault.is_none() || self.fault_placements(view, centre);
                if carry_on {
                    self.pieces(view, centre);
                    self.scatter(view, centre);
                    match self.mode {
                        Mode::ReadsFar => self.reads_far(view, centre),
                        Mode::WritesFar => self.writes_far(view, centre),
                        _ => {}
                    }
                }
            }
        }
        self.calls.decorate.fetch_add(1, Ordering::SeqCst);
    }
}

// The plain model of section 2.

/// One change a run made, as the model keeps it for `Order::Alone`.
enum Change {
    Block(BlockPos, BlockState),
    Entity(BlockPos, Vec<u8>),
    Tick(BlockPos, ScheduledTick),
    Mark(BlockPos),
}

/// What the model's views counted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ModelCounts {
    pub runs: u64,
    pub far_reads: u64,
    /// Far reads where the shared world held something else than terrain: those that
    /// would have come out otherwise had they been answered live.
    pub far_reads_live_would_change: u64,
    pub refused_writes: u64,
    pub dropped: u64,
}

struct ModelView<'a> {
    centre: ChunkPos,
    /// The three by three as this run sees it live.
    live: &'a mut BTreeMap<ChunkPos, Chunk>,
    /// The shared world, for counting far reads that live blocks would change; `None`
    /// where `live` is the shared world itself.
    shared: Option<&'a BTreeMap<ChunkPos, Chunk>>,
    terrain: &'a BTreeMap<ChunkPos, Terrain>,
    pieces: &'a mut BTreeMap<PieceKey, PieceState>,
    made: &'a BTreeMap<ChunkPos, Starts>,
    log: Option<&'a mut Vec<Change>>,
    counts: &'a mut ModelCounts,
}

impl ModelView<'_> {
    fn near(&self, at: BlockPos) -> bool {
        distance(at.chunk(), self.centre) <= 1
    }

    fn in_height(at: BlockPos) -> bool {
        let dimension = overworld();
        (dimension.min_y..dimension.min_y + dimension.height as i32).contains(&at.y)
    }

    fn terrain(&self, chunk: ChunkPos) -> &Terrain {
        self.terrain
            .get(&chunk)
            .expect("the model made terrain within 2 of every run")
    }

    fn get(chunk: &Chunk, at: BlockPos) -> BlockState {
        let (x, z) = at.in_chunk();
        chunk.get(x, at.y, z).unwrap_or(blocks::AIR)
    }
}

impl Neighbourhood for ModelView<'_> {
    fn centre(&self) -> ChunkPos {
        self.centre
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        if self.near(at) {
            return Self::get(&self.live[&at.chunk()], at);
        }
        self.counts.far_reads += 1;
        let terrain = Self::get(&self.terrain(at.chunk()).chunk, at);
        let shared = self.shared.unwrap_or(&*self.live);
        if let Some(live) = shared.get(&at.chunk())
            && Self::get(live, at) != terrain
        {
            self.counts.far_reads_live_would_change += 1;
        }
        terrain
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        if !self.near(at) || !Self::in_height(at) {
            self.counts.refused_writes += u64::from(!self.near(at));
            return false;
        }
        let (x, z) = at.in_chunk();
        let chunk = self.live.get_mut(&at.chunk()).expect("the three by three");
        chunk.set(x, at.y, z, state);
        if let Some(log) = &mut self.log {
            log.push(Change::Block(at, state));
        }
        true
    }

    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        let chunk = if self.near(at) {
            &self.live[&at.chunk()]
        } else {
            self.counts.far_reads += 1;
            &self.terrain[&at.chunk()].chunk
        };
        chunk.block_entities().get(&in_chunk(at))
    }

    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        if !self.near(at) {
            self.counts.refused_writes += 1;
            return false;
        }
        let entity = entity.expect("the toys only give block entities data");
        let chunk = self.live.get_mut(&at.chunk()).expect("the three by three");
        chunk
            .set_block_entity_data(in_chunk(at), entity.data.clone())
            .expect("the toys give data only to a chest they just placed");
        if let Some(log) = &mut self.log {
            log.push(Change::Entity(at, entity.data));
        }
        true
    }

    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        let at = BlockPos::new(x, 0, z);
        let chunk = if self.near(at) {
            &self.live[&at.chunk()]
        } else {
            self.counts.far_reads += 1;
            &self.terrain(at.chunk()).chunk
        };
        let heights = chunk.heights();
        let maps = match map {
            FinalHeightmap::WorldSurface => &heights.world_surface,
            FinalHeightmap::OceanFloor => &heights.ocean_floor,
            FinalHeightmap::MotionBlocking => &heights.motion_blocking,
            FinalHeightmap::MotionBlockingNoLeaves => &heights.motion_blocking_no_leaves,
        };
        let (x, z) = at.in_chunk();
        chunk.min_y() + i32::from(maps[z << 4 | x])
    }

    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
        let at = BlockPos::new(x, 0, z);
        let terrain = self.terrain(at.chunk());
        let maps = match map {
            WorldgenHeightmap::WorldSurface => &terrain.worldgen_heights.world_surface,
            WorldgenHeightmap::OceanFloor => &terrain.worldgen_heights.ocean_floor,
        };
        let (x, z) = at.in_chunk();
        terrain.chunk.min_y() + i32::from(maps[z << 4 | x])
    }

    fn biome(&mut self, at: BlockPos) -> Biome {
        let chunk = &self.terrain(at.chunk()).chunk;
        let section = ((at.y - chunk.min_y()) >> 4) as usize;
        let (x, z) = at.in_chunk();
        chunk.sections()[section].biome_at(x, (at.y & 15) as usize, z)
    }

    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
        if !self.near(at) {
            self.counts.dropped += 1;
            return;
        }
        let chunk = self.live.get_mut(&at.chunk()).expect("the three by three");
        add_tick(chunk, tick);
        if let Some(log) = &mut self.log {
            log.push(Change::Tick(at, tick));
        }
    }

    fn mark(&mut self, at: BlockPos) {
        if !self.near(at) {
            self.counts.dropped += 1;
            return;
        }
        let chunk = self.live.get_mut(&at.chunk()).expect("the three by three");
        add_mark(chunk, at);
        if let Some(log) = &mut self.log {
            log.push(Change::Mark(at));
        }
    }

    fn starts(&self) -> &[StartRef] {
        &self.terrain(self.centre).references
    }

    fn piece(&mut self, piece: PieceKey) -> PieceState {
        if let Some(state) = self.pieces.get(&piece) {
            return *state;
        }
        let starts = &self.made[&piece.start.source];
        starts.starts[usize::from(piece.start.index)].pieces[usize::from(piece.piece)].state
    }

    fn set_piece(&mut self, piece: PieceKey, state: PieceState) {
        self.pieces.insert(piece, state);
    }

    fn attempt(&mut self, _what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        // The model's toys do not fail.
        place(self);
        true
    }
}

fn add_tick(chunk: &mut Chunk, tick: ScheduledTick) {
    let mut ticks = chunk.ticks().to_vec();
    ticks.push(tick);
    chunk.set_ticks(ticks);
}

fn add_mark(chunk: &mut Chunk, at: BlockPos) {
    let mut marks = chunk.marks().to_vec();
    marks.push(in_chunk(at));
    chunk.set_marks(marks);
}

/// The plain model over a rectangle `A`: terrain within 2 of it, starts within 8 of
/// that, then the runs of `A` rank by rank on one shared world.
pub struct Model {
    pub toy: Toy,
    pub area: BTreeSet<ChunkPos>,
    starts: BTreeMap<ChunkPos, Starts>,
    terrain: BTreeMap<ChunkPos, Terrain>,
}

impl Model {
    pub fn new(toy: Toy, min: ChunkPos, max: ChunkPos) -> Self {
        let area = rectangle(min, max);
        let terrain_area = dilate(&area, 2);
        let starts: BTreeMap<_, _> = dilate(&terrain_area, 8)
            .into_iter()
            .map(|source| (source, toy.make_starts(source)))
            .collect();
        let terrain = terrain_area
            .into_iter()
            .map(|position| (position, toy.terrain(position, &MapStarts(&starts))))
            .collect();
        Self {
            toy,
            area,
            starts,
            terrain,
        }
    }

    /// The chunks whose `R(e)` lies in the rectangle: those the model answers for.
    pub fn answered(&self) -> Vec<ChunkPos> {
        self.area
            .iter()
            .copied()
            .filter(|e| finished_needs(*e).runs.is_subset(&self.area))
            .collect()
    }

    /// Runs the model: for `Order::Nine` every run on the shared world, for
    /// `Order::Alone` every run on terrain alone with its changes then applied to the
    /// shared world. Within a rank the runs go in ascending order of position, or in
    /// descending order if `reverse`.
    pub fn run(&self, order: Order, reverse: bool) -> (BTreeMap<ChunkPos, Chunk>, ModelCounts) {
        let mut world: BTreeMap<ChunkPos, Chunk> = dilate(&self.area, 1)
            .into_iter()
            .map(|p| (p, self.terrain[&p].chunk.clone()))
            .collect();
        let mut pieces = BTreeMap::new();
        let mut counts = ModelCounts::default();
        for rank_now in 0..9 {
            let mut runs: Vec<ChunkPos> = self
                .area
                .iter()
                .copied()
                .filter(|c| rank(*c) == rank_now)
                .collect();
            if reverse {
                runs.reverse();
            }
            for centre in runs {
                counts.runs += 1;
                match order {
                    Order::Nine => {
                        let mut view = ModelView {
                            centre,
                            live: &mut world,
                            shared: None,
                            terrain: &self.terrain,
                            pieces: &mut pieces,
                            made: &self.starts,
                            log: None,
                            counts: &mut counts,
                        };
                        self.toy.decorate(&mut view, centre);
                    }
                    Order::Alone => {
                        let mut alone: BTreeMap<_, _> = within(centre, 1)
                            .map(|p| (p, self.terrain[&p].chunk.clone()))
                            .collect();
                        let mut own_pieces = BTreeMap::new();
                        let mut log = Vec::new();
                        let mut view = ModelView {
                            centre,
                            live: &mut alone,
                            shared: Some(&world),
                            terrain: &self.terrain,
                            pieces: &mut own_pieces,
                            made: &self.starts,
                            log: Some(&mut log),
                            counts: &mut counts,
                        };
                        self.toy.decorate(&mut view, centre);
                        apply(&mut world, log);
                    }
                    // Bare runs nothing: it is for stages that place nothing.
                    Order::Bare => {}
                }
            }
        }
        (world, counts)
    }
}

fn apply(world: &mut BTreeMap<ChunkPos, Chunk>, log: Vec<Change>) {
    for change in log {
        match change {
            Change::Block(at, state) => {
                let (x, z) = at.in_chunk();
                world
                    .get_mut(&at.chunk())
                    .expect("within 1 of the rectangle")
                    .set(x, at.y, z, state);
            }
            Change::Entity(at, data) => {
                world
                    .get_mut(&at.chunk())
                    .expect("within 1 of the rectangle")
                    .set_block_entity_data(in_chunk(at), data)
                    .expect("a chest placed by the same run");
            }
            Change::Tick(at, tick) => {
                add_tick(world.get_mut(&at.chunk()).expect("within 1"), tick);
            }
            Change::Mark(at) => add_mark(world.get_mut(&at.chunk()).expect("within 1"), at),
        }
    }
}

// Comparing chunks.

/// What of a finished chunk the record compares: blocks and biomes, block entities,
/// ticks and marks. `None` if equal, or the first difference.
pub fn finished_difference(got: &Chunk, expected: &Chunk) -> Option<String> {
    if got.min_y() != expected.min_y() {
        return Some(format!(
            "lowest y {} against {}",
            got.min_y(),
            expected.min_y()
        ));
    }
    for (index, (a, b)) in got.sections().iter().zip(expected.sections()).enumerate() {
        if a != b {
            let y = got.min_y() + 16 * index as i32;
            for dy in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let (a, b) = (got.get(x, y + dy, z), expected.get(x, y + dy, z));
                        if a != b {
                            return Some(format!(
                                "block at ({x}, {}, {z}): {a:?} against {b:?}",
                                y + dy
                            ));
                        }
                    }
                }
            }
            return Some(format!("biomes of section {index}"));
        }
    }
    if got.sections().len() != expected.sections().len() {
        return Some("the number of sections".into());
    }
    if got.block_entities() != expected.block_entities() {
        return Some(format!(
            "block entities {:?} against {:?}",
            got.block_entities(),
            expected.block_entities()
        ));
    }
    if got.ticks() != expected.ticks() {
        return Some(format!(
            "ticks {:?} against {:?}",
            got.ticks(),
            expected.ticks()
        ));
    }
    if got.marks() != expected.marks() {
        return Some(format!(
            "marks {:?} against {:?}",
            got.marks(),
            expected.marks()
        ));
    }
    None
}

pub fn assert_finished_equal(position: ChunkPos, got: &Chunk, expected: &Chunk, what: &str) {
    if let Some(difference) = finished_difference(got, expected) {
        panic!("{what}: the finished chunk at {position:?} differs: {difference}");
    }
}

// Asking a scheduler.

/// A scheduler's options with no bound on the cache and the oldest request first.
pub fn options(threads: usize, look_ahead: bool) -> Options {
    Options {
        threads,
        cache_bytes: None,
        order: Order::Nine,
        look_ahead,
        pick: Pick::Oldest,
    }
}

/// The finished chunks at `positions`, asked for from a few threads at once.
pub fn finished_all<S: Stages>(
    scheduler: &Scheduler<S>,
    positions: &[ChunkPos],
) -> BTreeMap<ChunkPos, Chunk> {
    let made = Mutex::new(BTreeMap::new());
    let share = positions.len().div_ceil(4).max(1);
    std::thread::scope(|scope| {
        for part in positions.chunks(share) {
            let made = &made;
            scope.spawn(move || {
                for position in part {
                    let chunk = scheduler
                        .finished(*position)
                        .unwrap_or_else(|_| panic!("the finished chunk at {position:?} failed"));
                    made.lock().unwrap().insert(*position, chunk);
                }
            });
        }
    });
    made.into_inner().unwrap()
}

/// The lit chunks at `positions`, all requested before any answer is waited for, and
/// every request held until all are answered.
pub fn lit_all<G: ChunkGenerator>(
    generator: &G,
    positions: impl IntoIterator<Item = ChunkPos>,
) -> BTreeMap<ChunkPos, Result<Chunk, Failed>> {
    let (send, answers) = mpsc::channel();
    let mut wanted = Vec::new();
    for position in positions {
        let send = send.clone();
        let done = Box::new(move |made| {
            let _ = send.send((position, made));
        });
        wanted.push(generator.request(position, done));
    }
    drop(send);
    let mut made = BTreeMap::new();
    for _ in 0..wanted.len() {
        let (position, chunk) = answers.recv().expect("every request is answered");
        assert!(
            made.insert(position, chunk).is_none(),
            "{position:?} was answered twice"
        );
    }
    drop(wanted);
    made
}
