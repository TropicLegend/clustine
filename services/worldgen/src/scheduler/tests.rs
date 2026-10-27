//! The scheduler's own tests, with toy stages. Section 9's tests 1 to 6 are written
//! apart, from the record, by someone who did not write the scheduler.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use clustine_data::{BlockState, DIMENSION_TYPES, DimensionType, blocks};
use clustine_world::generation::{
    BlockBox, FinalHeightmap, GeneratorSettings, MAX_GENERATED_OF_A_CLASS, Maker, Neighbourhood,
    PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{
    BLOCKS_PER_SECTION, Biome, BlockEntity, BlockPos, Chunk, ChunkGenerator, ChunkPos, Entity,
    EntityData, EntityId, Generated, InChunk, ItemStack, SECTION_SIZE, ScheduledTick, Section,
    Ticked,
};

use super::lattice::{Key, around, rank};
use super::{Options, Order, Pick, Recorded, Scheduler};
use crate::stages::{PieceMade, Stages, Start, Starts, StartsNear, Terrain, WorldgenHeights};

fn overworld() -> &'static DimensionType {
    DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .unwrap()
}

fn mix(a: u64, b: u64) -> u64 {
    let mut z = a.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ b.wrapping_add(0x632b_e59b_d9b4_e019);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn of_chunk(chunk: ChunkPos, salt: u64) -> u64 {
    mix(mix(chunk.x as u64, chunk.z as u64), salt)
}

/// Toy stages: columns of stone a few blocks high, a start with three pieces at every
/// fourth chunk each way, and features that stack blocks on what is there, so that
/// the order of runs shows in what they leave.
#[derive(Debug, Clone, Default)]
struct Toy {
    /// The run of this chunk has a placement that writes and then panics.
    doomed_at: Option<ChunkPos>,
    /// Leave that placement out altogether: the generator to compare with.
    without_doomed: bool,
    /// The run of this chunk panics between two placements.
    cut_short_at: Option<ChunkPos>,
    terrain_panics_at: Option<ChunkPos>,
    starts_panics_at: Option<ChunkPos>,
    /// Runs read, write and set pieces where they may not, and nothing else.
    probe: bool,
    /// The starts and terrain made, for counting.
    made: Option<Arc<Mutex<Vec<Key>>>>,
    /// Runs make the entities of [`toy_entities`], and the doomed placement makes two
    /// before it panics.
    entities: bool,
    /// The run of this chunk makes 65,537 entities in its centre.
    crowd_at: Option<ChunkPos>,
    /// The spawn makes this many animals in every chunk ([`toy_spawned`]).
    spawns: usize,
    spawn_panics_at: Option<ChunkPos>,
    /// The spawn also makes an animal in the chunk to the east of its own.
    spawn_strays: bool,
}

/// The toy's maker: it keys an entity, and draws its fall distance from the stream of
/// its key, as a real maker draws a UUID.
struct ToyMaker;

impl Maker for ToyMaker {
    fn record(&self, key: EntityId, mut entity: Entity) -> Entity {
        entity.key = key;
        entity.fall_distance = (mix(key.0, 5) % 1024) as f64;
        entity
    }
}

/// An entity of the toy at `at`, which says in its motion who made it: the run's or
/// the spawn's centre, and its place among what that made.
fn toy_entity(at: [f64; 3], made_by: ChunkPos, place: usize) -> Entity {
    Entity {
        key: EntityId(0),
        kind: 72,
        uuid: Default::default(),
        position: at,
        motion: [f64::from(made_by.x), f64::from(made_by.z), place as f64],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack: ItemStack::new(1, 1),
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

/// A position in the chunk `dx`, `dz` from `centre`, drawn for `place`.
fn toy_position(centre: ChunkPos, dx: i32, dz: i32, place: u64) -> [f64; 3] {
    let random = of_chunk(centre, 3000 + place);
    [
        f64::from((centre.x + dx) * 16) + (random % 160) as f64 / 10.0,
        70.0,
        f64::from((centre.z + dz) * 16) + ((random >> 16) % 160) as f64 / 10.0,
    ]
}

/// What the run of `centre` makes when the toy makes entities: two in the centre,
/// one in the chunk to its east and one to its north-west, in this order.
fn toy_entities(centre: ChunkPos) -> Vec<Entity> {
    [(0, 0), (1, 0), (-1, -1), (0, 0)]
        .into_iter()
        .enumerate()
        .map(|(place, (dx, dz))| {
            toy_entity(toy_position(centre, dx, dz, place as u64), centre, place)
        })
        .collect()
}

/// What the spawn of `position` makes, `count` of them, all in its chunk.
fn toy_spawned(position: ChunkPos, count: usize) -> Vec<Entity> {
    (0..count)
        .map(|place| {
            let at = toy_position(position, 0, 0, 500 + place as u64);
            toy_entity(at, position, 100 + place)
        })
        .collect()
}

const STRUCTURE: &str = "toy:hut";

fn is_source(chunk: ChunkPos) -> bool {
    chunk.x.rem_euclid(4) == 0 && chunk.z.rem_euclid(4) == 0
}

fn chunk_box(from: ChunkPos, to: ChunkPos, y: i32) -> BlockBox {
    BlockBox {
        min: BlockPos::new(from.x * 16, y, from.z * 16),
        max: BlockPos::new(to.x * 16 + 15, y + 4, to.z * 16 + 15),
    }
}

impl Toy {
    fn note(&self, key: Key) {
        if let Some(made) = &self.made {
            made.lock().unwrap().push(key);
        }
    }
}

impl Stages for Toy {
    fn settings(&self) -> GeneratorSettings {
        GeneratorSettings {
            identity: "toy;seed 7".to_owned(),
            version: 3,
        }
    }

    fn dimension(&self) -> &'static DimensionType {
        overworld()
    }

    fn starts(&self, source: ChunkPos) -> Starts {
        self.note(Key::Starts(source));
        assert_ne!(Some(source), self.starts_panics_at, "a toy start fails");
        if !is_source(source) {
            return Starts::default();
        }
        let y = -60;
        let east = |by: i32| ChunkPos::new(source.x + by, source.z);
        Starts {
            starts: vec![Start {
                structure: STRUCTURE,
                pieces: vec![
                    PieceMade {
                        bounds: chunk_box(source, source, y),
                        state: PieceState::default(),
                    },
                    PieceMade {
                        bounds: chunk_box(source, east(1), y),
                        state: PieceState {
                            flags: 5,
                            ..PieceState::default()
                        },
                    },
                    // Spans chunks 3 apart: only the probe touches it.
                    PieceMade {
                        bounds: chunk_box(source, east(3), y),
                        state: PieceState::default(),
                    },
                ],
            }],
        }
    }

    fn terrain(&self, position: ChunkPos, starts: &dyn StartsNear) -> Terrain {
        self.note(Key::Terrain(position));
        assert_ne!(
            Some(position),
            self.terrain_panics_at,
            "a toy terrain fails"
        );
        let dimension = overworld();
        let mut states = Box::new([blocks::AIR; BLOCKS_PER_SECTION]);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let column = of_chunk(position, (z * 16 + x) as u64);
                let high = 2 + (column % 6) as usize;
                for y in 0..high {
                    let state = if y + 1 == high && column.is_multiple_of(7) {
                        blocks::DIRT
                    } else {
                        blocks::STONE
                    };
                    states[y << 8 | z << 4 | x] = state;
                }
            }
        }
        let biome = Biome((of_chunk(position, 99) % 3) as u16);
        let mut sections = vec![Section::from_states(states, biome)];
        sections.resize(
            dimension.height as usize / SECTION_SIZE,
            Section::filled(blocks::AIR, biome),
        );
        let chunk = Chunk::from_sections(dimension.min_y, sections);
        let worldgen_heights = Box::new(WorldgenHeights {
            world_surface: chunk.heights().world_surface,
            ocean_floor: chunk.heights().ocean_floor,
        });
        let mut references = Vec::new();
        for source in around(position, 8) {
            for (index, start) in starts.starts(source).starts.iter().enumerate() {
                // The far piece is left out of references, so that only the probe
                // meets it.
                if start.pieces[..2]
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
        Terrain {
            chunk,
            worldgen_heights,
            references,
        }
    }

    fn decorate(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        if self.probe {
            probe(view, centre);
            return;
        }
        let (west, north) = (centre.x * 16, centre.z * 16);
        for i in 0..4u64 {
            if i == 2 && self.cut_short_at == Some(centre) {
                panic!("the run panics between two placements");
            }
            let random = of_chunk(centre, 1000 + i);
            // Live reads and writes up to 16 blocks beyond the centre chunk, frozen
            // reads up to 32.
            let x = west - 16 + (random % 48) as i32;
            let z = north - 16 + ((random >> 8) % 48) as i32;
            let far_x = west - 32 + ((random >> 16) % 80) as i32;
            let far_z = north - 32 + ((random >> 24) % 80) as i32;
            view.attempt(Placing::Feature("toy:stack"), &mut |view| {
                let y = view.height(FinalHeightmap::WorldSurface, x, z);
                let below = view.block(BlockPos::new(x, y - 1, z));
                let frozen = view.worldgen_height(WorldgenHeightmap::OceanFloor, far_x, far_z);
                let biome = view.biome(BlockPos::new(far_x, frozen, far_z));
                let state = if below == blocks::STONE {
                    blocks::GLASS
                } else if (frozen + i32::from(biome.0)) % 2 == 0 {
                    blocks::GOLD_BLOCK
                } else {
                    blocks::STONE
                };
                let at = BlockPos::new(x, y, z);
                assert!(view.set_block(at, state));
                view.schedule(
                    at,
                    ScheduledTick {
                        at: InChunk::new(0, 0, 0),
                        of: Ticked::Block(i as u16),
                        delay: rank(centre).into(),
                        priority: 0,
                    },
                );
                view.mark(at.offset(0, 1, 0));
            });
        }
        if self.doomed_at == Some(centre) && !self.without_doomed {
            let starts = view.starts().to_vec();
            let kept = view.attempt(Placing::Feature("toy:doomed"), &mut |view| {
                let at = BlockPos::new(west + 3, -50, north + 3);
                view.set_block(at, blocks::GOLD_BLOCK);
                view.set_block(at.offset(-16, 0, 0), blocks::CHEST);
                view.schedule(
                    at,
                    ScheduledTick {
                        at: InChunk::new(0, 0, 0),
                        of: Ticked::Fluid(1),
                        delay: 9,
                        priority: 1,
                    },
                );
                view.mark(at);
                for start in &starts {
                    let piece = PieceKey {
                        start: *start,
                        piece: 0,
                    };
                    view.set_piece(
                        piece,
                        PieceState {
                            shift_y: 99,
                            ..PieceState::default()
                        },
                    );
                }
                panic!("the doomed feature fails after it wrote");
            });
            assert!(!kept);
            if self.entities {
                let kept = view.attempt(Placing::Feature("toy:doomed_animals"), &mut |view| {
                    let first = toy_entity(toy_position(centre, 0, 0, 90), centre, 99);
                    let second = toy_entity(toy_position(centre, -1, 0, 91), centre, 99);
                    assert!(view.add_entity(first));
                    assert!(view.add_entity(second));
                    panic!("the doomed animals fail after they were made");
                });
                assert!(!kept);
            }
        }
        if self.entities {
            view.attempt(Placing::Feature("toy:animals"), &mut |view| {
                for entity in toy_entities(centre) {
                    assert!(view.add_entity(entity));
                }
            });
        }
        if self.crowd_at == Some(centre) {
            view.attempt(Placing::Feature("toy:crowd"), &mut |view| {
                let at = toy_position(centre, 0, 0, 0);
                for place in 0..=MAX_GENERATED_OF_A_CLASS {
                    assert!(view.add_entity(toy_entity(at, centre, place)));
                }
            });
        }
        // A chest with data, beside the centre's middle.
        let random = of_chunk(centre, 77);
        let x = west + (random % 16) as i32;
        let z = north + ((random >> 8) % 16) as i32;
        view.attempt(Placing::Feature("toy:chest"), &mut |view| {
            let y = view.height(FinalHeightmap::MotionBlocking, x, z);
            let at = BlockPos::new(x, y, z);
            view.set_block(at, blocks::CHEST);
            let kind = view.block_entity(at).expect("a chest keeps one").kind;
            let data = vec![rank(centre), (y & 0xff) as u8];
            assert!(view.set_block_entity(at, Some(BlockEntity { kind, data })));
        });
        // Pieces: each referenced start's two near pieces, the state set twice in one
        // run, and a block placed by what the state says.
        for start in view.starts().to_vec() {
            for index in 0..2u16 {
                let piece = PieceKey {
                    start,
                    piece: index,
                };
                view.attempt(Placing::Piece(piece, STRUCTURE), &mut |view| {
                    let mut state = view.piece(piece);
                    state.flags = state.flags.wrapping_add(1);
                    view.set_piece(piece, state);
                    state.shift_y += i32::from(rank(centre));
                    view.set_piece(piece, state);
                    let at = BlockPos::new(
                        west + i32::from(state.flags % 16),
                        -40 + state.shift_y % 50,
                        north + i32::from(index) * 3,
                    );
                    view.set_block(at, blocks::STONE_BRICKS);
                });
            }
        }
    }

    fn spawn(&self, position: ChunkPos, lit: &Chunk, nine: [&Chunk; 9]) -> Vec<Entity> {
        assert!(lit.light().is_some(), "the spawn is handed the lit chunk");
        assert!(
            lit.shows_the_same(nine[4]),
            "the centre is the fifth of the nine"
        );
        assert_ne!(
            Some(position),
            self.spawn_panics_at,
            "the toy's spawn fails"
        );
        let mut spawned = toy_spawned(position, self.spawns);
        if self.spawn_strays {
            let at = toy_position(position, 1, 0, 600);
            spawned.push(toy_entity(at, position, 200));
        }
        spawned
    }

    fn maker(&self) -> &dyn Maker {
        &ToyMaker
    }
}

/// What the probe does: one of each thing the recorder counts, each once per run.
fn probe(view: &mut dyn Neighbourhood, centre: ChunkPos) {
    let (east, north) = (centre.x * 16 + 15, centre.z * 16);
    // 17 blocks beyond the centre chunk: in the ring.
    let ring = BlockPos::new(east + 17, -60, north);
    assert_eq!(view.block(ring.offset(0, 200, 0)), blocks::AIR);
    assert!(!view.set_block(ring, blocks::GLASS));
    view.mark(ring);
    // Beyond the five by five, answered from terrain fetched for it.
    let beyond = BlockPos::new(east + 40, -64, north);
    assert_eq!(view.block(beyond), blocks::STONE);
    // Above the dimension: void air, nothing recorded.
    assert_eq!(
        view.block(BlockPos::new(east, 400, north)),
        blocks::VOID_AIR
    );
    // A piece of a start three chunks off, spanning chunks 3 apart.
    if let Some(start) = view.starts().first().copied() {
        let piece = PieceKey { start, piece: 2 };
        let state = view.piece(piece);
        view.set_piece(piece, state);
    }
    // A piece whose box meets no chunk within 2.
    let source = ChunkPos::new(
        (centre.x + 6).div_euclid(4) * 4 + 4,
        centre.z.div_euclid(4) * 4,
    );
    let piece = PieceKey {
        start: StartRef { source, index: 0 },
        piece: 0,
    };
    assert_eq!(view.piece(piece), PieceState::default());
    // An entity in the ring, refused.
    let far = [f64::from(east + 17) + 0.5, -60.0, f64::from(north) + 0.5];
    assert!(!view.add_entity(toy_entity(far, centre, 0)));
}

fn options(threads: usize) -> Options {
    Options {
        threads,
        cache_bytes: None,
        order: Order::Nine,
        look_ahead: false,
        pick: Pick::Oldest,
    }
}

/// The plain model of section 2: terrain around a rectangle, then the runs of each rank
/// in turn on one shared world.
struct Model {
    toy: Toy,
    terrain: BTreeMap<ChunkPos, Terrain>,
    starts: BTreeMap<ChunkPos, Starts>,
    world: BTreeMap<ChunkPos, Chunk>,
    pieces: BTreeMap<PieceKey, PieceState>,
    ticks: BTreeMap<ChunkPos, Vec<ScheduledTick>>,
    marks: BTreeMap<ChunkPos, BTreeSet<InChunk>>,
}

struct Near<'a>(&'a BTreeMap<ChunkPos, Starts>);

impl StartsNear for Near<'_> {
    fn starts(&self, source: ChunkPos) -> &Starts {
        &self.0[&source]
    }
}

impl Model {
    /// Runs every chunk of the square of `radius` around the origin, `reversed` within
    /// each rank or not.
    fn run(toy: Toy, radius: i32, reversed: bool) -> Self {
        let origin = ChunkPos::new(0, 0);
        let starts: BTreeMap<ChunkPos, Starts> = around(origin, radius + 10)
            .map(|source| (source, toy.starts(source)))
            .collect();
        let terrain: BTreeMap<ChunkPos, Terrain> = around(origin, radius + 2)
            .map(|at| (at, toy.terrain(at, &Near(&starts))))
            .collect();
        let world = terrain
            .iter()
            .map(|(at, terrain)| (*at, terrain.chunk.clone()))
            .collect();
        let mut model = Self {
            toy,
            terrain,
            starts,
            world,
            pieces: BTreeMap::new(),
            ticks: BTreeMap::new(),
            marks: BTreeMap::new(),
        };
        for wanted in 0..9 {
            let mut runs: Vec<ChunkPos> = around(origin, radius)
                .filter(|c| rank(*c) == wanted)
                .collect();
            if reversed {
                runs.reverse();
            }
            for centre in runs {
                let toy = model.toy.clone();
                let references = model.terrain[&centre].references.clone();
                let mut view = ModelView {
                    model: &mut model,
                    centre,
                    references,
                };
                toy.decorate(&mut view, centre);
            }
        }
        model
    }

    fn finished(&self, at: ChunkPos) -> Chunk {
        let mut chunk = self.world[&at].clone();
        chunk.set_ticks(self.ticks.get(&at).cloned().unwrap_or_default());
        chunk.set_marks(
            self.marks
                .get(&at)
                .map(|marks| marks.iter().copied().collect())
                .unwrap_or_default(),
        );
        chunk
    }
}

struct ModelView<'a> {
    model: &'a mut Model,
    centre: ChunkPos,
    references: Vec<StartRef>,
}

impl ModelView<'_> {
    fn near(&self, at: BlockPos) -> bool {
        let chunk = at.chunk();
        (chunk.x - self.centre.x).abs() <= 1 && (chunk.z - self.centre.z).abs() <= 1
    }

    fn chunk(&self, at: BlockPos) -> &Chunk {
        if self.near(at) {
            &self.model.world[&at.chunk()]
        } else {
            &self.model.terrain[&at.chunk()].chunk
        }
    }
}

impl Neighbourhood for ModelView<'_> {
    fn centre(&self) -> ChunkPos {
        self.centre
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        let (x, z) = at.in_chunk();
        self.chunk(at).get(x, at.y, z).unwrap_or(blocks::VOID_AIR)
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        if !self.near(at) {
            return false;
        }
        let (x, z) = at.in_chunk();
        let chunk = self.model.world.get_mut(&at.chunk()).unwrap();
        chunk.set(x, at.y, z, state).is_some()
    }

    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        let (x, z) = at.in_chunk();
        self.chunk(at)
            .block_entities()
            .get(&InChunk::new(x, at.y, z))
    }

    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        assert!(self.near(at));
        let (x, z) = at.in_chunk();
        let chunk = self.model.world.get_mut(&at.chunk()).unwrap();
        let data = entity.map(|entity| entity.data).unwrap_or_default();
        chunk
            .set_block_entity_data(InChunk::new(x, at.y, z), data)
            .is_ok()
    }

    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        let at = BlockPos::new(x, 0, z);
        let heights = self.chunk(at).heights();
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        let height = match map {
            FinalHeightmap::WorldSurface => heights.world_surface[column],
            FinalHeightmap::OceanFloor => heights.ocean_floor[column],
            FinalHeightmap::MotionBlocking => heights.motion_blocking[column],
            FinalHeightmap::MotionBlockingNoLeaves => heights.motion_blocking_no_leaves[column],
        };
        overworld().min_y + i32::from(height)
    }

    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
        let heights = &self.model.terrain[&BlockPos::new(x, 0, z).chunk()].worldgen_heights;
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        let height = match map {
            WorldgenHeightmap::WorldSurface => heights.world_surface[column],
            WorldgenHeightmap::OceanFloor => heights.ocean_floor[column],
        };
        overworld().min_y + i32::from(height)
    }

    fn biome(&mut self, at: BlockPos) -> Biome {
        let chunk = &self.model.terrain[&at.chunk()].chunk;
        let above = (at.y.clamp(-64, 319) + 64) as usize;
        let (x, z) = at.in_chunk();
        chunk.sections()[above / 16].biome_at(x, above % 16, z)
    }

    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
        if self.near(at) {
            let (x, z) = at.in_chunk();
            let tick = ScheduledTick {
                at: InChunk::new(x, at.y, z),
                ..tick
            };
            self.model.ticks.entry(at.chunk()).or_default().push(tick);
        }
    }

    fn mark(&mut self, at: BlockPos) {
        if self.near(at) {
            let (x, z) = at.in_chunk();
            let marks = self.model.marks.entry(at.chunk()).or_default();
            marks.insert(InChunk::new(x, at.y, z));
        }
    }

    fn starts(&self) -> &[StartRef] {
        &self.references
    }

    fn piece(&mut self, piece: PieceKey) -> PieceState {
        self.model.pieces.get(&piece).copied().unwrap_or_else(|| {
            let start = &self.model.starts[&piece.start.source].starts[piece.start.index as usize];
            start.pieces[piece.piece as usize].state
        })
    }

    fn set_piece(&mut self, piece: PieceKey, state: PieceState) {
        self.model.pieces.insert(piece, state);
    }

    fn attempt(&mut self, _: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        place(self);
        true
    }
}

#[test]
fn finished_chunks_equal_the_plain_model_in_either_order_within_a_rank() {
    let model = Model::run(Toy::default(), 9, false);
    let reversed = Model::run(Toy::default(), 9, true);
    let scheduler = Scheduler::new(Toy::default(), options(4));
    let mut changed = 0;
    for e in around(ChunkPos::new(0, 0), 1) {
        let finished = scheduler.finished(e).unwrap();
        assert_eq!(finished, model.finished(e), "{e:?}");
        assert_eq!(finished, reversed.finished(e), "{e:?}");
        if !finished.shows_the_same(&model.terrain[&e].chunk) {
            changed += 1;
        }
        assert!(!finished.ticks().is_empty());
        assert!(!finished.marks().is_empty());
        assert!(
            finished
                .block_entities()
                .values()
                .any(|e| !e.data.is_empty())
        );
    }
    assert_eq!(changed, 9);
    assert_eq!(scheduler.figures().recorded, Recorded::default());
}

#[test]
fn alone_differs_from_nine_and_each_is_the_same_on_any_number_of_threads() {
    let alone = |threads| {
        let options = Options {
            order: Order::Alone,
            ..options(threads)
        };
        let scheduler = Scheduler::new(Toy::default(), options);
        // Beside a start, whose pieces runs of two ranks set.
        let chunk = scheduler.finished(ChunkPos::new(1, 0)).unwrap();
        (chunk, scheduler.figures())
    };
    let (one, figures) = alone(1);
    let (three, _) = alone(3);
    assert_eq!(one, three);
    // A finished chunk needs its nine runs alone, and their terrain.
    assert_eq!(figures.runs, 9);
    assert_eq!(figures.terrain, 49);
    let nine = Scheduler::new(Toy::default(), options(3));
    assert_ne!(nine.finished(ChunkPos::new(1, 0)).unwrap(), one);
}

#[test]
fn the_order_of_work_and_the_cache_do_not_show() {
    let positions: Vec<ChunkPos> = around(ChunkPos::new(-4, 5), 1).collect();
    let lit = |options: Options| {
        let scheduler = Scheduler::new(Toy::default(), options);
        let (sender, receiver) = std::sync::mpsc::channel();
        let wanted: Vec<_> = positions
            .iter()
            .map(|at| {
                let sender = sender.clone();
                let at = *at;
                scheduler.request(at, Box::new(move |made| sender.send((at, made)).unwrap()))
            })
            .collect();
        let mut chunks: BTreeMap<ChunkPos, Chunk> = BTreeMap::new();
        for _ in &positions {
            let (at, made) = receiver.recv().unwrap();
            chunks.insert(at, made.unwrap());
        }
        drop(wanted);
        scheduler.wait_idle();
        let figures = scheduler.figures();
        assert_eq!((figures.queued, figures.running), (0, 0));
        assert_eq!(scheduler.graph().counted(), (0, 0));
        (chunks, figures)
    };
    let (one, _) = lit(options(1));
    let (eight, _) = lit(options(8));
    let (shuffled, _) = lit(Options {
        pick: Pick::Shuffled(0x5eed),
        ..options(3)
    });
    let (no_cache, figures) = lit(Options {
        cache_bytes: Some(0),
        look_ahead: true,
        ..options(2)
    });
    let (small_cache, _) = lit(Options {
        cache_bytes: Some(1 << 20),
        ..options(2)
    });
    assert_eq!(one, eight);
    assert_eq!(one, shuffled);
    assert_eq!(one, no_cache);
    assert_eq!(one, small_cache);
    // Nothing is held once nothing is under way, and with a bound of 0 nothing stays.
    assert_eq!(figures.cache_bytes, 0);
    for chunk in one.values() {
        assert_eq!(chunk.generated(), 3);
        assert!(!chunk.with_failure());
        let light = chunk.light().expect("a requested chunk is lit");
        assert_eq!(light.beside, 3);
    }
}

#[test]
fn a_lit_chunk_is_its_finished_chunk_lit_beside_its_neighbours() {
    let scheduler = Scheduler::new(Toy::default(), options(2));
    let at = ChunkPos::new(3, -3);
    let lit = scheduler.generate(at);
    let mut finished = scheduler.finished(at).unwrap();
    let neighbours: Vec<Chunk> = around(at, 1)
        .filter(|n| *n != at)
        .map(|n| scheduler.finished(n).unwrap())
        .collect();
    let mut light = clustine_light::light(&clustine_light::Around {
        centre: &finished,
        neighbours: std::array::from_fn(|index| Some(&neighbours[index])),
        sky: true,
    });
    light.beside = 3;
    finished.set_light(light);
    finished.mark_generated(3, false);
    assert_eq!(lit, finished);
    let settings = scheduler.settings();
    assert_eq!(
        (settings.identity.as_str(), settings.version),
        ("toy;seed 7", 3)
    );
}

/// Section 2's table: one finished chunk from nothing makes 94 runs, 289 terrain and
/// 1,089 starts, wherever it lies.
#[test]
fn one_finished_chunk_from_nothing_makes_the_sets_of_section_two() {
    for x in 0..3 {
        for z in 0..3 {
            let made = Arc::new(Mutex::new(Vec::new()));
            let toy = Toy {
                made: Some(Arc::clone(&made)),
                ..Toy::default()
            };
            let scheduler = Scheduler::new(toy, options(3));
            scheduler.finished(ChunkPos::new(x, z)).unwrap();
            scheduler.wait_idle();
            let figures = scheduler.figures();
            assert_eq!(
                (
                    figures.runs,
                    figures.terrain,
                    figures.starts,
                    figures.finished
                ),
                (94, 289, 1089, 1)
            );
            assert_eq!(figures.runs_made_again, 0);
            let made = made.lock().unwrap();
            let distinct: BTreeSet<Key> = made.iter().copied().collect();
            assert_eq!(distinct.len(), made.len(), "nothing is made twice");
        }
    }
}

#[test]
fn a_request_lights_its_eight_neighbours_ahead_and_they_are_answered_from_the_cache() {
    let scheduler = Scheduler::new(
        Toy::default(),
        Options {
            look_ahead: true,
            ..options(3)
        },
    );
    let at = ChunkPos::new(10, 10);
    let first = scheduler.generate(at);
    scheduler.wait_idle();
    let figures = scheduler.figures();
    assert_eq!(figures.lit, 9);
    let neighbour = ChunkPos::new(11, 9);
    let answered = scheduler.generate(neighbour);
    let after = scheduler.figures();
    // Answered without a task: what was made ahead, not made again. Its own
    // look-ahead is under way by now, but the chunk did not wait for it.
    assert_eq!(answered.generated(), 3);
    scheduler.wait_idle();
    // Five more chunks are lit ahead, around the neighbour; the first chunk was
    // handed out and is not lit again.
    assert_eq!(scheduler.figures().lit, 9 + 5);
    assert!(after.lit >= 9);
    assert_eq!(scheduler.generate(at), first);
}

#[test]
fn chunks_prepared_ahead_are_lit_and_answered_from_the_cache_without_being_made_again() {
    use clustine_world::ChunkGenerator;

    // Without the look-ahead of requests, so that only what was prepared is lit.
    let scheduler = Scheduler::new(
        Toy::default(),
        Options {
            look_ahead: false,
            ..options(3)
        },
    );
    let prepared = [
        ChunkPos::new(-4, 7),
        ChunkPos::new(-3, 7),
        ChunkPos::new(-4, 8),
    ];
    scheduler.prepare(&prepared);
    scheduler.wait_idle();
    assert_eq!(scheduler.figures().lit, 3);
    for position in prepared {
        assert_eq!(scheduler.generate(position).generated(), 3);
    }
    assert_eq!(scheduler.figures().lit, 3, "nothing was made again");
}

#[test]
fn a_dropped_request_leaves_no_task_that_has_not_started() {
    let scheduler = Scheduler::new(
        Toy::default(),
        Options {
            look_ahead: true,
            ..options(1)
        },
    );
    for x in 0..20 {
        let wanted = scheduler.request(ChunkPos::new(x * 40, 0), Box::new(|_| {}));
        drop(wanted);
    }
    scheduler.wait_idle();
    let figures = scheduler.figures();
    assert_eq!((figures.queued, figures.running), (0, 0));
    assert_eq!(scheduler.graph().counted(), (0, 0));
    assert!(figures.lit < 20, "{figures:?}");
    // And what was made is right: it is used by the next request.
    let made = scheduler.figures();
    let chunk = scheduler.finished(ChunkPos::new(0, 0)).unwrap();
    let fresh = Scheduler::new(Toy::default(), options(1));
    assert_eq!(chunk, fresh.finished(ChunkPos::new(0, 0)).unwrap());
    assert!(scheduler.figures().runs - made.runs <= 94);
}

#[test]
fn a_placement_that_panics_is_undone_and_marks_exactly_the_chunks_that_depend_on_it() {
    // A chunk of rank 8: (2, 0).
    let doomed = ChunkPos::new(2, 0);
    assert_eq!(rank(doomed), 8);
    let toy = Toy {
        doomed_at: Some(doomed),
        ..Toy::default()
    };
    let without = Toy {
        without_doomed: true,
        ..toy.clone()
    };
    let failing = Scheduler::new(toy, options(4));
    let reference = Scheduler::new(without, options(4));
    let mut marked = BTreeSet::new();
    for at in around(doomed, 3) {
        let made = failing.generate(at);
        let expected = reference.generate(at);
        assert!(made.shows_the_same(&expected), "{at:?}");
        assert_eq!(made.ticks(), expected.ticks());
        assert_eq!(made.marks(), expected.marks());
        assert_eq!(made.light(), expected.light());
        assert_eq!(made.generated(), 3);
        if made.with_failure() {
            marked.insert(at);
        }
    }
    let expected: BTreeSet<ChunkPos> = around(doomed, 2).collect();
    assert_eq!(marked, expected);
    assert_eq!(failing.figures().failed_placements, 1);
    assert_eq!(failing.figures().failed_tasks, 0);
}

#[test]
fn a_panic_between_placements_cuts_the_run_short_and_keeps_what_was_placed() {
    let at = ChunkPos::new(0, 0);
    let toy = Toy {
        cut_short_at: Some(at),
        ..Toy::default()
    };
    let short = Scheduler::new(toy, options(2));
    let whole = Scheduler::new(Toy::default(), options(2));
    // Each stack schedules one tick in the three by three, with the rank of its run
    // as its delay, and (0, 0) is the one chunk of rank 0 within 1 of any of them.
    let ticks_of_the_run = |scheduler: &Scheduler<Toy>| -> usize {
        around(at, 1)
            .map(|e| {
                let chunk = scheduler.finished(e).unwrap();
                chunk.ticks().iter().filter(|tick| tick.delay == 0).count()
            })
            .sum()
    };
    // Two stacks were placed before it was cut short; the chest and pieces were not.
    assert_eq!(ticks_of_the_run(&short), 2);
    assert_eq!(ticks_of_the_run(&whole), 4);
    assert_ne!(short.finished(at), whole.finished(at));
    assert_eq!(short.figures().failed_placements, 1);
    assert!(short.generate(at).with_failure());
    assert!(!whole.generate(at).with_failure());
}

#[test]
fn a_terrain_that_panics_fails_the_requests_that_need_it_for_the_scheduler_s_life() {
    let broken = ChunkPos::new(5, 5);
    let toy = Toy {
        terrain_panics_at: Some(broken),
        ..Toy::default()
    };
    let scheduler = Scheduler::new(toy.clone(), options(3));
    assert!(scheduler.finished(ChunkPos::new(7, 5)).is_err());
    scheduler.wait_idle();
    let figures = scheduler.figures();
    assert_eq!(figures.failed_tasks, 1);
    assert!(scheduler.graph().is_failed(Key::Terrain(broken)));
    // Asked again, nothing runs.
    assert!(scheduler.finished(ChunkPos::new(7, 5)).is_err());
    scheduler.wait_idle();
    assert_eq!(scheduler.figures(), figures);
    // Far enough away, chunks come.
    assert!(scheduler.finished(ChunkPos::new(30, 5)).is_ok());
    // A new scheduler tries again.
    let again = Scheduler::new(toy, options(1));
    assert!(again.finished(ChunkPos::new(7, 5)).is_err());
    assert_eq!(again.figures().failed_tasks, 1);
}

#[test]
fn a_start_that_panics_fails_the_lit_chunk_eight_away_but_not_nine() {
    let broken = ChunkPos::new(0, 0);
    let toy = Toy {
        starts_panics_at: Some(broken),
        ..Toy::default()
    };
    let scheduler = Scheduler::new(toy, options(4));
    // `Lit(p)` needs terrain within 2 of the runs within 1 of p's nine; those reach
    // 10 to 12 chunks in x, by rank, and 9 to 11 in z (section 2), plus 8 for starts.
    // Twenty-two off nothing needs it.
    let (sender, receiver) = std::sync::mpsc::channel();
    let _far = scheduler.request(
        ChunkPos::new(22, 0),
        Box::new(move |made| sender.send(made.is_ok()).unwrap()),
    );
    assert!(receiver.recv().unwrap());
    let (sender, receiver) = std::sync::mpsc::channel();
    let _near = scheduler.request(
        ChunkPos::new(3, 3),
        Box::new(move |made| sender.send(made.is_ok()).unwrap()),
    );
    assert!(!receiver.recv().unwrap());
}

#[test]
fn the_recorder_counts_each_read_and_write_beyond_what_a_run_may() {
    // Alone, so that a finished chunk needs its nine runs and no others.
    let scheduler = Scheduler::new(
        Toy {
            probe: true,
            ..Toy::default()
        },
        Options {
            order: Order::Alone,
            ..options(2)
        },
    );
    let at = ChunkPos::new(4, 4);
    scheduler.finished(at).unwrap();
    let figures = scheduler.figures();
    let runs = figures.runs;
    assert_eq!(runs, 9);
    let recorded = figures.recorded;
    assert_eq!(recorded.far_reads, runs);
    assert_eq!(recorded.beyond_margin, runs);
    assert_eq!(recorded.refused_writes, runs);
    assert_eq!(recorded.dropped, runs);
    assert_eq!(recorded.far_piece_reads, runs);
    // The far piece is set by the runs whose centre references a start.
    assert!(recorded.far_pieces > 0 && recorded.far_pieces <= runs);
    assert_eq!(recorded.refused_entities, runs);
    assert!(scheduler.finished(at).unwrap().entities().is_empty());
}

#[test]
fn a_cache_of_one_mebibyte_drops_and_makes_again_and_the_chunks_are_the_same() {
    let bounded = Scheduler::new(
        Toy::default(),
        Options {
            cache_bytes: Some(1 << 20),
            ..options(2)
        },
    );
    let unbounded = Scheduler::new(Toy::default(), options(2));
    for x in 0..3 {
        let at = ChunkPos::new(x * 5, 0);
        assert_eq!(bounded.finished(at), unbounded.finished(at));
    }
    bounded.wait_idle();
    assert!(bounded.figures().cache_bytes <= 1 << 20);
    assert!(unbounded.figures().cache_bytes > 1 << 20);
}

#[test]
fn a_dropped_scheduler_ends_its_threads_with_requests_under_way() {
    let scheduler = Scheduler::new(Toy::default(), options(3));
    let wanted: Vec<_> = (0..5)
        .map(|x| scheduler.request(ChunkPos::new(x * 30, 0), Box::new(|_| {})))
        .collect();
    drop(scheduler);
    drop(wanted);
}

/// What definition D says a finished chunk at `e` has of the runs' entities: for each
/// run within 1 of `e` in ascending rank, the entities it made whose position is in
/// `e`, in its order; each keyed by `e` as a structure's, by its place in that list
/// from 0, and its record made by the maker with that key.
fn definition_d_entities(e: ChunkPos) -> Vec<Entity> {
    let mut runs: Vec<ChunkPos> = around(e, 1).collect();
    runs.sort_by_key(|d| rank(*d));
    let landed: Vec<Entity> = runs
        .into_iter()
        .flat_map(toy_entities)
        .filter(|entity| entity.chunk() == e)
        .collect();
    keyed(e, true, landed)
}

/// `made` keyed in `chunk` in their order, of the class `structure`, their records made
/// by the toy's maker, ascending by key as a chunk keeps them.
fn keyed(chunk: ChunkPos, structure: bool, made: Vec<Entity>) -> Vec<Entity> {
    let mut records: Vec<Entity> = made
        .into_iter()
        .enumerate()
        .map(|(index, entity)| {
            let key = Generated {
                dimension: 0,
                chunk,
                structure,
                index: index as u16,
            }
            .key()
            .unwrap();
            ToyMaker.record(key, entity)
        })
        .collect();
    records.sort_by_key(|entity| entity.key);
    records
}

fn with_entities() -> Toy {
    Toy {
        entities: true,
        ..Toy::default()
    }
}

#[test]
fn a_run_s_entities_are_put_in_the_chunk_they_land_in_in_rank_order_and_keyed_there() {
    let scheduler = Scheduler::new(with_entities(), options(4));
    let mut from_several_runs = 0;
    for e in around(ChunkPos::new(0, 0), 2) {
        let finished = scheduler.finished(e).unwrap();
        assert_eq!(finished.entities(), definition_d_entities(e), "{e:?}");
        // Two of the run of `e` itself, and one from each of two neighbours' runs.
        assert_eq!(finished.entities().len(), 4, "{e:?}");
        let makers: BTreeSet<(i64, i64)> = finished
            .entities()
            .iter()
            .map(|entity| (entity.motion[0] as i64, entity.motion[1] as i64))
            .collect();
        if makers.len() > 1 {
            from_several_runs += 1;
        }
        for entity in finished.entities() {
            let named = Generated::of(entity.key).unwrap();
            assert_eq!((named.chunk, named.structure), (e, true));
            assert_eq!(entity.chunk(), e);
        }
    }
    assert_eq!(from_several_runs, 25);
    assert_eq!(scheduler.figures().recorded, Recorded::default());
    assert_eq!(scheduler.figures().failed_placements, 0);
}

#[test]
fn the_entities_of_a_chunk_made_twice_or_in_any_order_have_the_same_keys_and_records() {
    let positions: Vec<ChunkPos> = around(ChunkPos::new(-4, 5), 1).collect();
    let lit = |options: Options| -> BTreeMap<ChunkPos, Vec<Entity>> {
        let toy = Toy {
            spawns: 2,
            ..with_entities()
        };
        let scheduler = Scheduler::new(toy, options);
        positions
            .iter()
            .map(|at| (*at, scheduler.generate(*at).entities().to_vec()))
            .collect()
    };
    let one = lit(options(1));
    assert!(one.values().all(|entities| entities.len() == 4 + 2));
    assert_eq!(one, lit(options(8)));
    assert_eq!(
        one,
        lit(Options {
            pick: Pick::Shuffled(0x5eed),
            ..options(3)
        })
    );
    assert_eq!(
        one,
        lit(Options {
            pick: Pick::Shuffled(7),
            cache_bytes: Some(0),
            look_ahead: true,
            ..options(2)
        })
    );
    // Made twice by one scheduler that keeps nothing.
    let scheduler = Scheduler::new(
        with_entities(),
        Options {
            cache_bytes: Some(0),
            ..options(2)
        },
    );
    let at = ChunkPos::new(-4, 5);
    let first = scheduler.generate(at);
    scheduler.wait_idle();
    let finished = scheduler.figures().finished;
    let again = scheduler.generate(at);
    assert!(scheduler.figures().finished > finished, "made again");
    assert_eq!(first.entities(), again.entities());
    assert_eq!(first.entities(), definition_d_entities(at));
}

#[test]
fn an_entity_made_in_an_attempt_that_panics_is_undone_and_its_index_given_back() {
    let doomed = ChunkPos::new(2, 0);
    let toy = Toy {
        doomed_at: Some(doomed),
        ..with_entities()
    };
    let without = Toy {
        without_doomed: true,
        ..toy.clone()
    };
    let failing = Scheduler::new(toy, options(4));
    let reference = Scheduler::new(without, options(4));
    for e in around(doomed, 1) {
        let made = failing.finished(e).unwrap();
        // The animals of the doomed run's later placement take the places the undone
        // ones had: the same keys as if they had never been made.
        assert_eq!(made.entities(), reference.finished(e).unwrap().entities());
        assert_eq!(made.entities(), definition_d_entities(e), "{e:?}");
        assert!(
            made.entities()
                .iter()
                .all(|entity| entity.motion[2] != 99.0)
        );
    }
    // The block placement and the animals' placement.
    assert_eq!(failing.figures().failed_placements, 2);
}

#[test]
fn a_chunk_that_would_get_a_65_537th_entity_of_a_class_gets_no_more_and_is_marked() {
    let crowded = ChunkPos::new(1, 1);
    let toy = Toy {
        crowd_at: Some(crowded),
        ..Toy::default()
    };
    let scheduler = Scheduler::new(toy, options(2));
    let lit = scheduler.generate(crowded);
    assert_eq!(lit.entities().len(), MAX_GENERATED_OF_A_CLASS);
    assert!(lit.with_failure());
    // The first 65,536 in their order, each at its place: the last made is the one left
    // out.
    let indices: BTreeSet<u16> = lit
        .entities()
        .iter()
        .map(|entity| {
            let named = Generated::of(entity.key).unwrap();
            assert_eq!((named.chunk, named.structure), (crowded, true));
            assert_eq!(entity.motion[2], f64::from(named.index));
            named.index
        })
        .collect();
    assert_eq!(indices.len(), MAX_GENERATED_OF_A_CLASS);
    let figures = scheduler.figures();
    assert_eq!(figures.failed_tasks, 0);
    assert_eq!(figures.failed_placements, 1);
}

#[test]
fn the_spawn_s_animals_are_keyed_as_the_spawn_s_in_their_chunk_after_the_runs_entities() {
    let toy = Toy {
        spawns: 3,
        ..with_entities()
    };
    let scheduler = Scheduler::new(toy, options(3));
    for at in [ChunkPos::new(0, 0), ChunkPos::new(-7, 3)] {
        let lit = scheduler.generate(at);
        let (runs, spawned): (Vec<Entity>, Vec<Entity>) = lit
            .entities()
            .iter()
            .cloned()
            .partition(|entity| Generated::of(entity.key).unwrap().structure);
        assert_eq!(runs, scheduler.finished(at).unwrap().entities());
        assert_eq!(runs, definition_d_entities(at));
        // Indices from 0 in the order the stage made them, whatever the runs made.
        assert_eq!(spawned, keyed(at, false, toy_spawned(at, 3)));
        assert!(!lit.with_failure());
    }
    assert_eq!(scheduler.figures().failed_placements, 0);
}

#[test]
fn a_spawn_that_panics_fails_no_lit_task_and_leaves_its_chunk_marked_with_the_runs_entities() {
    let broken = ChunkPos::new(3, -2);
    let toy = Toy {
        spawns: 2,
        spawn_panics_at: Some(broken),
        ..with_entities()
    };
    let scheduler = Scheduler::new(toy, options(3));
    let lit = scheduler.generate(broken);
    assert!(lit.with_failure());
    assert_eq!(
        lit.entities(),
        scheduler.finished(broken).unwrap().entities()
    );
    assert_eq!(lit.entities(), definition_d_entities(broken));
    assert!(lit.light().is_some());
    // Its neighbour is lit from the same finished chunks and spawns as ever.
    let neighbour = scheduler.generate(ChunkPos::new(4, -2));
    assert!(!neighbour.with_failure());
    assert_eq!(neighbour.entities().len(), 4 + 2);
    let figures = scheduler.figures();
    assert_eq!(figures.failed_tasks, 0);
    assert_eq!(figures.failed_placements, 1);
}

#[test]
fn a_spawn_s_animal_outside_its_chunk_is_left_out_and_the_chunk_marked() {
    let toy = Toy {
        spawns: 1,
        spawn_strays: true,
        ..Toy::default()
    };
    let scheduler = Scheduler::new(toy, options(2));
    let at = ChunkPos::new(5, 5);
    let lit = scheduler.generate(at);
    assert_eq!(lit.entities(), keyed(at, false, toy_spawned(at, 1)));
    assert!(lit.with_failure());
    assert_eq!(scheduler.figures().failed_placements, 1);
}

/// A unit made in a run (a baby zombie villager on the chicken made for it,
/// `docs/groundwork/n7l-plan.md`, section 3.5) names its members by their makings' keys;
/// keyed in the chunk it lands in, each names the other by its own key there, and a
/// link to what the chunk did not get is dropped.
#[test]
fn a_unit_s_members_name_each_other_by_their_keys_in_the_chunk_they_land_in() {
    let centre = ChunkPos::new(3, 3);
    let making = |index| {
        Generated {
            dimension: 0,
            chunk: centre,
            structure: true,
            index,
        }
        .key()
        .unwrap()
    };
    let lands = ChunkPos::new(4, 3);
    let at = [4.0 * 16.0 + 2.5, 70.0, 3.0 * 16.0 + 2.5];
    let seat = |key| clustine_world::Seat {
        key,
        player: false,
        pending: None,
    };
    // A loner made first, then the chicken (made second, so its making is 2) carrying
    // the zombie (its making 1), then one riding something the chunk does not get.
    let mut loner = toy_entity(at, centre, 0);
    loner.key = making(0);
    let mut chicken = toy_entity(at, centre, 1);
    chicken.key = making(2);
    chicken.riding.passengers.push(seat(making(1)));
    let mut zombie = toy_entity(at, centre, 2);
    zombie.key = making(1);
    zombie.riding.vehicle = Some(making(2));
    let mut stray = toy_entity(at, centre, 3);
    stray.key = making(9);
    stray.riding.vehicle = Some(making(8));
    let mut chunk = Chunk::empty(overworld(), Biome(0));
    let context = super::view::Context {
        identity: "toy".into(),
        version: 1,
        dimension: "minecraft:overworld",
        dimension_number: 0,
    };
    let failures = super::output::add_keyed(
        &mut chunk,
        lands,
        true,
        vec![loner, chicken, zombie, stray],
        &ToyMaker,
        &context,
    );
    assert_eq!(failures, 0);
    let key = |index| {
        Generated {
            dimension: 0,
            chunk: lands,
            structure: true,
            index,
        }
        .key()
        .unwrap()
    };
    assert_eq!(chunk.entities().len(), 4);
    // The chunk keeps them by key.
    let of = |index| {
        chunk
            .entities()
            .iter()
            .find(|entity| entity.key == key(index))
            .unwrap()
    };
    assert!(of(0).riding.is_free());
    assert_eq!(of(1).riding.passengers, [seat(key(2))]);
    assert_eq!(of(2).riding.vehicle, Some(key(1)));
    assert!(of(3).riding.is_free(), "its vehicle is not here");
}
