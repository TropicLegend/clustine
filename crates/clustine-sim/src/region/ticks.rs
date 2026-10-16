//! Scheduled ticks in a region (`docs/adr/0024-block-behaviour-in-a-region.md`,
//! sections 2, 3 and 6a): two queues, block ticks and fluid ticks, of the chunks the
//! region ticks; the ticks of a chunk that does not tick wait in the chunk itself, as
//! the delays left.
//!
//! A tick's due is the game's trigger time: the region's tick it was scheduled in and
//! its delay. Kept in a chunk, written at the end of tick `T`, a tick due in `D` is the
//! value `D - T - 1`; a chunk that starts ticking in `T'` makes it due in `T' + value`.
//! Within a queue the ticks run in (due, priority, class, order): a tick that came with
//! a chunk as it started ticking before one scheduled since, as the game's negative
//! orders for a loaded chunk's ticks do. An order is the region's tick and a count
//! within it, which grows as the game's sub-tick order grows and which a restored region
//! gives the same, so that the region's durable state needs no counter.
//!
//! What runs in this step: falling blocks fall, at once (the owner's answer to the plan's
//! question 2); leaves take their distance; a path or farmland under something solid
//! becomes dirt; a cactus or sugar cane that can no longer stay breaks. A block tick of
//! any other block runs as nothing. Fluid ticks wait in their queue for B5, which
//! gives the fluids their rules.

use std::collections::{BTreeMap, BTreeSet};

use clustine_blocks::data::is_class;
use clustine_blocks::survive::can_survive;
use clustine_data::{BLOCKS, BlockClass, BlockState, Face, blocks};
use clustine_world::{BlockPos, Chunk, ChunkPos, InChunk, ScheduledTick, Ticked};

use super::Region;
use super::updates::RegionView;
use crate::api::TickOutput;

/// How many ticks of one kind run in a tick at most (`LevelTicks`' limit).
const MAX_PER_TICK: usize = 65_536;

/// Where a tick stands in its queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    due: i64,
    priority: i8,
    /// 0 for a tick that came with its chunk, 1 for one scheduled since.
    class: u8,
    order: u64,
}

/// One queue of ticks, and which position and thing each is of, so that one is not
/// scheduled twice (`LevelChunkTicks.ticksPerPosition`).
#[derive(Debug, Clone, Default, PartialEq)]
struct Queue {
    entries: BTreeMap<Key, (BlockPos, Ticked)>,
    pending: BTreeSet<(BlockPos, Ticked)>,
}

impl Queue {
    fn insert(&mut self, key: Key, pos: BlockPos, of: Ticked) -> bool {
        if !self.pending.insert((pos, of)) {
            return false;
        }
        self.entries.insert(key, (pos, of));
        true
    }

    /// The ticks of `chunk`, in queue order, taken out.
    fn take_chunk(&mut self, chunk: ChunkPos) -> Vec<(Key, BlockPos, Ticked)> {
        let keys: Vec<Key> = self
            .entries
            .iter()
            .filter(|(_, (pos, _))| pos.chunk() == chunk)
            .map(|(key, _)| *key)
            .collect();
        keys.into_iter()
            .map(|key| {
                let (pos, of) = self.entries.remove(&key).expect("listed just now");
                self.pending.remove(&(pos, of));
                (key, pos, of)
            })
            .collect()
    }

    /// The ticks of `chunk`, in queue order.
    fn of_chunk(&self, chunk: ChunkPos) -> impl Iterator<Item = (&Key, &(BlockPos, Ticked))> {
        self.entries
            .iter()
            .filter(move |(_, (pos, _))| pos.chunk() == chunk)
    }

    /// The ticks due by `tick`, at most [`MAX_PER_TICK`], taken out
    /// (`LevelTicks.collectTicks`).
    fn take_due(&mut self, tick: i64) -> Vec<(BlockPos, Ticked)> {
        let mut due = Vec::new();
        while due.len() < MAX_PER_TICK {
            let Some((key, _)) = self.entries.first_key_value() else {
                break;
            };
            if key.due > tick {
                break;
            }
            let (_, (pos, of)) = self.entries.pop_first().expect("looked at just now");
            self.pending.remove(&(pos, of));
            due.push((pos, of));
        }
        due
    }
}

/// The scheduled ticks of a region: no part of its state, rebuilt from its chunks.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Ticks {
    blocks: Queue,
    fluids: Queue,
    /// The chunks the region ticks: loaded, with their eight neighbours.
    ticking: BTreeSet<ChunkPos>,
    /// The ticks given an order in this region's tick so far.
    counted: u64,
    /// The chunks whose ticks changed in this region's tick so far.
    changed: BTreeSet<ChunkPos>,
    /// The chunks that started ticking in this region's tick, ascending, which are
    /// post-processed after the tick's fluid ticks if they have marks.
    started: BTreeSet<ChunkPos>,
    /// The chunks whose seam entries changed in this region's tick so far.
    seams_changed: BTreeSet<ChunkPos>,
}

impl Region {
    /// Orders are counted afresh in each tick, and changes noted afresh.
    pub(crate) fn ticks_counted_from_zero(&mut self) {
        self.ticks.counted = 0;
        self.ticks.changed.clear();
        self.ticks.started.clear();
        self.ticks.seams_changed.clear();
    }

    /// The chunks whose ticks changed in this tick, ascending.
    pub(crate) fn ticks_changed(&self) -> Vec<ChunkPos> {
        self.ticks.changed.iter().copied().collect()
    }

    /// The chunks the region ticks, ascending.
    pub(crate) fn ticking_chunks(&self) -> Vec<ChunkPos> {
        self.ticks.ticking.iter().copied().collect()
    }

    /// Notes that the seam entries of `chunk` changed in this tick.
    pub(crate) fn ticks_seams_changed(&mut self, chunk: ChunkPos) {
        self.ticks.seams_changed.insert(chunk);
    }

    /// The chunks whose seam entries changed in this tick, ascending.
    pub(crate) fn seams_changed(&self) -> Vec<ChunkPos> {
        self.ticks.seams_changed.iter().copied().collect()
    }

    /// The chunks that tick with ticks pending, ascending. Only a chunk that ticks has
    /// its ticks in the queues.
    pub(crate) fn ticks_pending(&self) -> Vec<ChunkPos> {
        let queued = self.ticks.blocks.pending.iter();
        let chunks: BTreeSet<ChunkPos> = queued
            .chain(self.ticks.fluids.pending.iter())
            .map(|(pos, _)| pos.chunk())
            .collect();
        chunks.into_iter().collect()
    }

    /// The next order in this tick.
    fn next_order(&mut self) -> u64 {
        self.ticks.counted += 1;
        (self.tick << 20) | self.ticks.counted
    }

    /// `scheduleTick`: a tick of `of` at `pos` in `delay` ticks. A chunk the region
    /// ticks has it in its queue; a loaded chunk that does not keeps it as the delay
    /// left, as if written at the end of this tick; one not loaded does not get it.
    pub(crate) fn schedule_tick(&mut self, pos: BlockPos, of: Ticked, delay: i32, priority: i8) {
        let chunk = pos.chunk();
        if self.ticks.ticking.contains(&chunk) {
            let key = Key {
                due: self.tick as i64 + i64::from(delay),
                priority,
                class: 1,
                order: self.next_order(),
            };
            let queue = match of {
                Ticked::Block(_) => &mut self.ticks.blocks,
                Ticked::Fluid(_) => &mut self.ticks.fluids,
            };
            if queue.insert(key, pos, of) {
                self.ticks.changed.insert(chunk);
            }
            return;
        }
        let Some(loaded) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let at = in_chunk(pos);
        if loaded
            .ticks()
            .iter()
            .any(|tick| tick.at == at && tick.of == of)
        {
            return;
        }
        let mut ticks = loaded.ticks().to_vec();
        ticks.push(ScheduledTick {
            at,
            of,
            delay: delay - 1,
            priority,
        });
        loaded.set_ticks(ticks);
        self.ticks.changed.insert(chunk);
    }

    /// Starts and stops ticking chunks as their neighbours are loaded and let go, in
    /// ascending position: the ticks of a chunk that starts are taken from it, the ticks
    /// of one that stops are written into it.
    pub(crate) fn update_ticking(&mut self) {
        let should: BTreeSet<ChunkPos> = self
            .chunks
            .keys()
            .copied()
            .filter(|chunk| {
                (-1..=1).all(|dx| {
                    (-1..=1).all(|dz| {
                        self.chunks
                            .contains_key(&ChunkPos::new(chunk.x + dx, chunk.z + dz))
                    })
                })
            })
            .collect();
        let stopping: Vec<ChunkPos> = self.ticks.ticking.difference(&should).copied().collect();
        let starting: Vec<ChunkPos> = should.difference(&self.ticks.ticking).copied().collect();
        for chunk in stopping {
            self.stop_ticking(chunk);
        }
        for chunk in starting {
            self.start_ticking(chunk);
        }
    }

    /// Stops ticking every chunk, so that each holds its ticks.
    pub(crate) fn stop_ticking_all(&mut self) {
        let ticking: Vec<ChunkPos> = self.ticks.ticking.iter().copied().collect();
        for chunk in ticking {
            self.stop_ticking(chunk);
        }
    }

    fn start_ticking(&mut self, chunk: ChunkPos) {
        self.ticks.ticking.insert(chunk);
        self.ticks.started.insert(chunk);
        let Some(loaded) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let stored = loaded.ticks().to_vec();
        loaded.set_ticks(Vec::new());
        for tick in stored {
            let pos = world_pos(chunk, tick.at);
            let key = Key {
                due: self.tick as i64 + i64::from(tick.delay),
                priority: tick.priority,
                class: 0,
                order: self.next_order(),
            };
            let queue = match tick.of {
                Ticked::Block(_) => &mut self.ticks.blocks,
                Ticked::Fluid(_) => &mut self.ticks.fluids,
            };
            queue.insert(key, pos, tick.of);
        }
    }

    fn stop_ticking(&mut self, chunk: ChunkPos) {
        self.ticks.ticking.remove(&chunk);
        let mut taken = self.ticks.blocks.take_chunk(chunk);
        taken.extend(self.ticks.fluids.take_chunk(chunk));
        taken.sort_by_key(|(key, _, _)| *key);
        if !taken.is_empty() {
            self.ticks.changed.insert(chunk);
        }
        let tick = self.tick as i64;
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            let mut ticks = loaded.ticks().to_vec();
            ticks.extend(taken.into_iter().map(|(key, pos, of)| ScheduledTick {
                at: in_chunk(pos),
                of,
                delay: i32::try_from(key.due - tick - 1).unwrap_or(i32::MAX),
                priority: key.priority,
            }));
            loaded.set_ticks(ticks);
        }
    }

    /// The ticks of `chunk` as it is to be saved: those it keeps, and those of the
    /// queues as the delays left at the end of this tick, in queue order.
    pub(crate) fn ticks_to_save(&self, chunk: ChunkPos) -> Vec<ScheduledTick> {
        let Some(loaded) = self.chunks.get(&chunk) else {
            return Vec::new();
        };
        let tick = self.tick as i64;
        let mut queued: Vec<(&Key, &(BlockPos, Ticked))> = self
            .ticks
            .blocks
            .of_chunk(chunk)
            .chain(self.ticks.fluids.of_chunk(chunk))
            .collect();
        queued.sort_by_key(|(key, _)| **key);
        let mut ticks = loaded.ticks().to_vec();
        ticks.extend(queued.into_iter().map(|(key, (pos, of))| ScheduledTick {
            at: in_chunk(*pos),
            of: *of,
            delay: i32::try_from(key.due - tick - 1).unwrap_or(i32::MAX),
            priority: key.priority,
        }));
        ticks
    }

    /// The due block ticks of this tick, collected before any runs, each run where its
    /// block is still there (`LevelTicks.tick`).
    pub(crate) fn run_block_ticks(&mut self, output: &mut TickOutput) {
        let due = self.ticks.blocks.take_due(self.tick as i64);
        self.ticks
            .changed
            .extend(due.iter().map(|(pos, _)| pos.chunk()));
        for (pos, of) in due {
            let Ticked::Block(block) = of else {
                continue;
            };
            let Some(state) = self.block(pos) else {
                continue;
            };
            if state.block().map(|b| b.first_state) != Some(BLOCKS[usize::from(block)].first_state)
            {
                continue;
            }
            self.block_tick(pos, state, output);
        }
    }

    /// The due fluid ticks of this tick, collected before any runs, each run where its
    /// fluid is still there; after the block ticks, as the game runs them.
    pub(crate) fn run_fluid_ticks(&mut self, output: &mut TickOutput) {
        let due = self.ticks.fluids.take_due(self.tick as i64);
        self.ticks
            .changed
            .extend(due.iter().map(|(pos, _)| pos.chunk()));
        for (pos, of) in due {
            if let Ticked::Fluid(fluid) = of {
                self.fluid_tick(pos, fluid, output);
            }
        }
    }

    /// Post-processes the chunks that started ticking in this tick and have marks, in
    /// ascending position (`docs/adr/0024-block-behaviour-in-a-region.md`, sections 6
    /// and 6a), as the game's `LevelChunk.postProcessGeneration` does as a chunk becomes
    /// ticking: at each mark the fluid there ticks, and a block that is not a liquid
    /// takes its shape from its neighbours, set without telling them. The marks are
    /// taken then, which the tick's output names.
    ///
    /// The game goes through a chunk's marks section by section in the order they were
    /// made; Clustine keeps them in order of position, which it goes through.
    pub(crate) fn post_process(&mut self, output: &mut TickOutput) {
        let started: Vec<ChunkPos> = self.ticks.started.iter().copied().collect();
        for chunk in started {
            let Some(marks) = self
                .chunks
                .get(&chunk)
                .map(|loaded| loaded.marks().to_vec())
            else {
                continue;
            };
            if marks.is_empty() {
                continue;
            }
            for mark in marks {
                self.post_process_at(world_pos(chunk, mark), output);
            }
            if let Some(loaded) = self.chunks.get_mut(&chunk) {
                loaded.set_marks(Vec::new());
            }
            output.marks_taken.push(chunk);
        }
    }

    /// What post-processing does at one mark.
    fn post_process_at(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.block(pos) else {
            return;
        };
        let fluid = clustine_blocks::fluid::FluidState::of(state);
        if !fluid.is_empty() {
            self.fluid_tick(pos, fluid.fluid, output);
        }
        // A liquid block's own tick makes a bubble column, which is not ported.
        if is_class(state, BlockClass::LiquidBlock) {
            return;
        }
        // `Block.updateFromNeighbourShapes`, of the state as it was before the fluid's
        // tick, and set with the game's flags 276: no neighbour is told.
        let mut updated = state;
        for face in [
            Face::West,
            Face::East,
            Face::North,
            Face::South,
            Face::Down,
            Face::Up,
        ] {
            let neighbour = self.block(relative(pos, face)).unwrap_or(blocks::AIR);
            updated =
                clustine_blocks::update_shape(&mut RegionView(self), updated, pos, face, neighbour);
        }
        if updated != state {
            let flags = super::updates::UPDATE_KNOWN_SHAPE | super::updates::UPDATE_CLIENTS;
            self.set_block_updated(pos, updated, flags, super::updates::UPDATE_LIMIT, output);
        }
    }

    /// One block's `tick`.
    fn block_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        if self.fire_block_tick(pos, state, output) {
            return;
        }
        // Kelp, a vine that grows or a speleothem that its support left goes
        // (`GrowingPlantBlock.tick`; a stalactite falls in the game, and is destroyed
        // here, as falling blocks land at once and dripstone's fall is not ported).
        if let Some(stays) =
            clustine_blocks::growth::grows_and_can_stay(&mut RegionView(self), state, pos)
        {
            if !stays {
                self.destroy_block_by_tick(pos, output);
            }
            return;
        }
        if is_class(state, BlockClass::FallingBlock) {
            self.fall(pos, state, output);
        } else if is_class(state, BlockClass::LeavesBlock) {
            let distance = self.leaf_distance(pos);
            let updated = state
                .with_property("distance", &distance.to_string())
                .unwrap_or(state);
            self.set_block(pos, updated, output);
        } else if is_class(state, BlockClass::PathBlock) {
            self.set_block(pos, blocks::DIRT, output);
        } else if is_class(state, BlockClass::FarmlandBlock) {
            if !can_survive(state, pos, &mut RegionView(self)) {
                self.set_block(pos, blocks::DIRT, output);
            }
        } else if (is_class(state, BlockClass::CactusBlock)
            || is_class(state, BlockClass::SugarCaneBlock))
            && !can_survive(state, pos, &mut RegionView(self))
        {
            self.destroy_block_by_tick(pos, output);
        }
    }

    /// `LeavesBlock.updateDistance`: one more than the nearest log or leaves beside it,
    /// at most seven.
    fn leaf_distance(&self, pos: BlockPos) -> u8 {
        let mut distance = 7;
        for face in [
            Face::Down,
            Face::Up,
            Face::North,
            Face::South,
            Face::West,
            Face::East,
        ] {
            let neighbour = self.block(relative(pos, face)).unwrap_or(blocks::AIR);
            let of = clustine_blocks::shape::leaf_distance(neighbour).unwrap_or(7);
            distance = distance.min(of + 1);
            if distance == 1 {
                break;
            }
        }
        u8::try_from(distance).unwrap_or(7)
    }

    /// `FallingBlock.tick` and the fall of its entity, at once: where the block below
    /// is free (air, fire, a liquid, what can be replaced), the block leaves its place
    /// and comes to rest where the game's falling block would, on the first block below
    /// with something to stand on; inside a block whose top is lower than a whole block,
    /// on top of one that fills its height. It is placed there where the block there can
    /// be replaced; otherwise it is lost, as the game's entity drops as an item, which
    /// Clustine has none of.
    fn fall(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let Some(min_y) = self.chunks.get(&pos.chunk()).map(Chunk::min_y) else {
            return;
        };
        let below = relative(pos, Face::Down);
        let Some(under) = self.block(below) else {
            return;
        };
        if !is_free(under) || pos.y < min_y {
            return;
        }
        self.set_block(pos, fluid_block(state), output);
        let mut y = below.y;
        // Concrete powder stops in the first water it falls into, where it hardens
        // (`FallingBlockEntity.tick`, `ConcretePowderBlock.onLand`).
        if let Some(concrete) = clustine_blocks::fluid::concrete_of(state) {
            loop {
                let at = BlockPos::new(pos.x, y, pos.z);
                let Some(here) = self.block(at) else {
                    return;
                };
                if clustine_blocks::data::has_water(here) {
                    if here.props().can_be_replaced()
                        && can_survive(state, at, &mut RegionView(self))
                    {
                        self.set_block(at, state, output);
                        self.set_block(at, concrete, output);
                    }
                    return;
                }
                let next = BlockPos::new(pos.x, y - 1, pos.z);
                if y <= min_y
                    || !self
                        .block(next)
                        .is_some_and(|b| b.props().collision_is_empty())
                {
                    break;
                }
                y -= 1;
            }
            y = below.y;
        }
        // Down through what has nothing to stand on.
        while y > min_y {
            let at = BlockPos::new(pos.x, y - 1, pos.z);
            let Some(here) = self.block(at) else {
                return;
            };
            if !here.props().collision_is_empty() {
                break;
            }
            y -= 1;
        }
        let support = BlockPos::new(pos.x, y - 1, pos.z);
        let rests_inside = self
            .block(support)
            .is_some_and(|block| !block.props().collision_is_empty() && block_top(block) < 1.0);
        let landing = if rests_inside {
            support
        } else {
            BlockPos::new(pos.x, y, pos.z)
        };
        let Some(there) = self.block(landing) else {
            return;
        };
        let beneath_free = self
            .block(relative(landing, Face::Down))
            .is_some_and(is_free);
        if there.props().can_be_replaced()
            && can_survive(state, landing, &mut RegionView(self))
            && !beneath_free
        {
            self.set_block(landing, state, output);
            if let Some(concrete) = clustine_blocks::fluid::concrete_of(state)
                && clustine_blocks::fluid::powder_hardens(&mut RegionView(self), landing, there)
            {
                self.set_block(landing, concrete, output);
            }
        }
    }
}

/// `FallingBlock.isFree`.
fn is_free(state: BlockState) -> bool {
    let props = state.props();
    props.is_air()
        || clustine_blocks::data::in_tag(state, "minecraft:fire")
        || props.is_liquid()
        || props.can_be_replaced()
}

/// The top of a block's collision, as a part of a whole block.
fn block_top(state: BlockState) -> f64 {
    if state.props().collision_is_full_block() {
        return 1.0;
    }
    state
        .props()
        .collision_shape()
        .boxes()
        .map(|shape_box| shape_box[4])
        .fold(0.0, f64::max)
}

/// What a block leaves when it goes: water where it held a water source, air otherwise.
fn fluid_block(state: BlockState) -> BlockState {
    match state.props().fluid() {
        Some(fluid)
            if fluid.source
                && clustine_data::FLUIDS[usize::from(fluid.fluid)] == "minecraft:water" =>
        {
            blocks::WATER
        }
        _ => blocks::AIR,
    }
}

fn in_chunk(pos: BlockPos) -> InChunk {
    let (x, z) = pos.in_chunk();
    InChunk::new(x, pos.y, z)
}

fn world_pos(chunk: ChunkPos, at: InChunk) -> BlockPos {
    BlockPos::new(
        chunk.x * 16 + i32::from(at.x),
        i32::from(at.y),
        chunk.z * 16 + i32::from(at.z),
    )
}

fn relative(pos: BlockPos, face: Face) -> BlockPos {
    let (dx, dy, dz) = match face {
        Face::Down => (0, -1, 0),
        Face::Up => (0, 1, 0),
        Face::North => (0, 0, -1),
        Face::South => (0, 0, 1),
        Face::West => (-1, 0, 0),
        Face::East => (1, 0, 0),
    };
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}
