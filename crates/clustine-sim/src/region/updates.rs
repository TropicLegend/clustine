//! Setting a block as the game's `Level.setBlock` does, with the updates it makes in
//! turn (`docs/adr/0024-block-behaviour-in-a-region.md`, section 1): the neighbours
//! told of the change, each neighbour's shape asked again, a neighbour that can no
//! longer stay destroyed, all run as the game's `CollectingNeighborUpdater` runs them,
//! depth first.
//!
//! What phase B's later steps bring is left out here, each where it would be:
//!
//! - an update that would reach a block whose chunk, or a neighbour's, this region
//!   has not loaded is not made: B6 keeps it as a seam entry;
//! - the ticks an update schedules are not kept: B4 keeps them;
//! - `neighborChanged` only removes what can no longer stay (rails, repeaters and
//!   comparators, redstone wire); power waits for the phase of redstone.

use clustine_blocks::BlockView;
use clustine_blocks::data::{is_class, same_block};
use clustine_blocks::shape::update_shape;
use clustine_blocks::survive::can_survive;
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::{BlockPos, PlayerId, Ticked};

use super::Region;
use crate::api::{RegionEvent, TickOutput};

/// `Block.UPDATE_NEIGHBORS`: the neighbours are told (`neighborChanged`).
pub(crate) const UPDATE_NEIGHBORS: u32 = 1;
/// `Block.UPDATE_CLIENTS`: the clients are told. Clustine tells them of every change.
pub(crate) const UPDATE_CLIENTS: u32 = 2;
/// `Block.UPDATE_KNOWN_SHAPE`: the neighbours' shapes are not asked again.
pub(crate) const UPDATE_KNOWN_SHAPE: u32 = 16;
/// `Block.UPDATE_SUPPRESS_DROPS`: a block destroyed by the change drops nothing.
pub(crate) const UPDATE_SUPPRESS_DROPS: u32 = 32;
/// `Block.UPDATE_ALL`: what a player's placing and breaking and `destroyBlock` use.
pub(crate) const UPDATE_ALL: u32 = UPDATE_NEIGHBORS | UPDATE_CLIENTS;

/// How deep a chain of shape updates goes (`Block.UPDATE_LIMIT`).
pub(crate) const UPDATE_LIMIT: i32 = 512;
/// How many updates one change may chain before the rest are dropped
/// (`max-chained-neighbor-updates`, 1,000,000 by default).
const MAX_CHAINED: usize = 1_000_000;

/// `Block.UPDATE_SHAPE_ORDER`.
const UPDATE_SHAPE_ORDER: [Face; 6] = [
    Face::West,
    Face::East,
    Face::North,
    Face::South,
    Face::Down,
    Face::Up,
];

/// `NeighborUpdater.UPDATE_ORDER`, in which the neighbours are told.
const UPDATE_ORDER: [Face; 6] = [
    Face::West,
    Face::East,
    Face::Down,
    Face::Up,
    Face::North,
    Face::South,
];

/// One entry of the updater's stack.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pending {
    /// `ShapeUpdate`: the block at `pos` asks again for its shape, its neighbour
    /// towards `direction` being now `neighbour`.
    Shape {
        pos: BlockPos,
        direction: Face,
        neighbour: BlockState,
        flags: u32,
        depth: i32,
    },
    /// `MultiNeighborUpdate`: the six neighbours of `source` are told, one at a time;
    /// `next` is the next of [`UPDATE_ORDER`].
    Neighbours { source: BlockPos, next: usize },
}

/// The updater of one change: the game's `CollectingNeighborUpdater`, which runs
/// what a change makes in turn on a stack, so that what an update makes runs before
/// the rest of what made it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Updater {
    stack: Vec<Pending>,
    added: Vec<Pending>,
    count: usize,
}

impl Region {
    /// `Level.setBlock(pos, state, flags, depth)`: sets the block if its chunk is
    /// loaded and it changes, and makes the updates the flags ask for. Returns whether
    /// the block changed.
    pub(crate) fn set_block_updated(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        flags: u32,
        depth: i32,
        output: &mut TickOutput,
    ) -> bool {
        let Some(old) = self.block(pos) else {
            return false;
        };
        if old == state {
            return false;
        }
        let (x, z) = pos.in_chunk();
        let Some(chunk) = self.chunks.get_mut(&pos.chunk()) else {
            return false;
        };
        if chunk.set(x, pos.y, z, state).is_none() {
            return false;
        }
        output.events.push(RegionEvent::BlockChanged {
            position: pos,
            state,
        });
        // `onPlace`, inside the chunk's set in the game: a block that falls looks two
        // ticks on whether it is held up (`FallingBlock.onPlace`, `BrushableBlock`'s).
        if is_class(state, BlockClass::FallingBlock) || is_class(state, BlockClass::BrushableBlock)
        {
            let block = clustine_blocks::data::tick_id(state);
            self.schedule_tick(pos, Ticked::Block(block), 2, 0);
        }
        if flags & UPDATE_NEIGHBORS != 0 {
            self.add_and_run(
                Pending::Neighbours {
                    source: pos,
                    next: 0,
                },
                output,
            );
        }
        if flags & UPDATE_KNOWN_SHAPE == 0 && depth > 0 {
            let passed = flags & !(UPDATE_NEIGHBORS | UPDATE_SUPPRESS_DROPS);
            for direction in UPDATE_SHAPE_ORDER {
                let neighbour = relative(pos, direction);
                self.add_and_run(
                    Pending::Shape {
                        pos: neighbour,
                        direction: opposite(direction),
                        neighbour: state,
                        flags: passed,
                        depth: depth - 1,
                    },
                    output,
                );
            }
        }
        true
    }

    /// A player's or a region's own change of a block: `setBlock` with every update.
    pub(crate) fn set_block(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        self.set_block_updated(pos, state, UPDATE_ALL, UPDATE_LIMIT, output);
    }

    /// A player breaks the block at `pos` (`ServerPlayerGameMode.destroyBlock` in
    /// creative): the others see it break (`playerWillDestroy`), a door or a tall
    /// plant broken at its upper half loses its lower half without a drop, and the
    /// block gives way to its fluid (`removeBlock`).
    pub(crate) fn break_by_player(
        &mut self,
        pos: BlockPos,
        player: Option<PlayerId>,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.block(pos) else {
            return;
        };
        if state.props().is_air() {
            return;
        }
        if is_class(state, BlockClass::DoorBlock) || is_class(state, BlockClass::DoublePlantBlock) {
            self.prevent_drop_from_bottom_part(pos, state, player, output);
        }
        output.events.push(RegionEvent::BlockDestroyed {
            position: pos,
            state,
            breaker: player,
        });
        self.set_block(pos, fluid_block(state), output);
    }

    /// `DoublePlantBlock.preventDropFromBottomPart`: the lower half of what is broken at
    /// its upper half goes first, to water if it held water.
    fn prevent_drop_from_bottom_part(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player: Option<PlayerId>,
        output: &mut TickOutput,
    ) {
        if state.property("half") != Some("upper") {
            return;
        }
        let below = relative(pos, Face::Down);
        let Some(lower) = self.block(below) else {
            return;
        };
        if !same_block(lower, state) || lower.property("half") != Some("lower") {
            return;
        }
        let flags = UPDATE_SUPPRESS_DROPS | UPDATE_CLIENTS | UPDATE_NEIGHBORS;
        self.set_block_updated(below, fluid_block(lower), flags, UPDATE_LIMIT, output);
        output.events.push(RegionEvent::BlockDestroyed {
            position: below,
            state: lower,
            breaker: player,
        });
    }

    /// `CollectingNeighborUpdater.addAndRun`.
    fn add_and_run(&mut self, pending: Pending, output: &mut TickOutput) {
        let running = self.updater.count > 0;
        let limited = self.updater.count >= MAX_CHAINED;
        self.updater.count += 1;
        if !limited {
            if running {
                self.updater.added.push(pending);
            } else {
                self.updater.stack.push(pending);
            }
        }
        if !running {
            self.run_updates(output);
        }
    }

    /// `CollectingNeighborUpdater.runUpdates`: what the last step added goes on the
    /// stack, its first on top; then the entry on top takes its next step, and goes
    /// when that was its last.
    fn run_updates(&mut self, output: &mut TickOutput) {
        loop {
            while let Some(added) = self.updater.added.pop() {
                self.updater.stack.push(added);
            }
            let Some(top) = self.updater.stack.last_mut() else {
                break;
            };
            let current = *top;
            let finished = match top {
                Pending::Shape { .. } => true,
                Pending::Neighbours { next, .. } => {
                    *next += 1;
                    *next >= UPDATE_ORDER.len()
                }
            };
            if finished {
                self.updater.stack.pop();
            }
            self.run_one(current, output);
        }
        self.updater.count = 0;
    }

    /// Runs one step of an entry: a shape asked again, or one neighbour told.
    fn run_one(&mut self, pending: Pending, output: &mut TickOutput) {
        match pending {
            Pending::Shape {
                pos,
                direction,
                neighbour,
                flags,
                depth,
            } => self.shape_update(pos, direction, neighbour, flags, depth, output),
            Pending::Neighbours { source, next } => {
                let target = relative(source, UPDATE_ORDER[next]);
                self.neighbour_changed(target, output);
            }
        }
    }

    /// `executeShapeUpdate` and `Block.updateOrDestroy`.
    fn shape_update(
        &mut self,
        pos: BlockPos,
        direction: Face,
        neighbour: BlockState,
        flags: u32,
        depth: i32,
        output: &mut TickOutput,
    ) {
        if !self.loaded_around(pos) {
            return;
        }
        let Some(old) = self.block(pos) else {
            return;
        };
        let new = update_shape(&mut RegionView(self), old, pos, direction, neighbour);
        if new == old {
            return;
        }
        if new.props().is_air() {
            self.destroy_block(pos, flags & UPDATE_SUPPRESS_DROPS == 0, depth, output);
        } else {
            self.set_block_updated(pos, new, flags & !UPDATE_SUPPRESS_DROPS, depth, output);
        }
    }

    /// `destroyBlock` of a block's own tick (a cactus or sugar cane that can no longer
    /// stay).
    pub(super) fn destroy_block_by_tick(&mut self, pos: BlockPos, output: &mut TickOutput) {
        self.destroy_block(pos, true, UPDATE_LIMIT, output);
    }

    /// `Level.destroyBlock`: the block breaks where everyone sees it, and gives way to
    /// its fluid, with every update.
    fn destroy_block(&mut self, pos: BlockPos, _drops: bool, depth: i32, output: &mut TickOutput) {
        let Some(state) = self.block(pos) else {
            return;
        };
        if state.props().is_air() {
            return;
        }
        // Fire goes without the sound and the bits of a broken block.
        if !is_class(state, BlockClass::BaseFireBlock) {
            output.events.push(RegionEvent::BlockDestroyed {
                position: pos,
                state,
                breaker: None,
            });
        }
        self.set_block_updated(pos, fluid_block(state), UPDATE_ALL, depth, output);
    }

    /// `neighborChanged` of the block at `pos`, for what goes when it can no longer
    /// stay: rails, repeaters and comparators, redstone wire (`removeBlock`, without
    /// the bits of a broken block).
    fn neighbour_changed(&mut self, pos: BlockPos, output: &mut TickOutput) {
        if !self.loaded_around(pos) {
            return;
        }
        let Some(state) = self.block(pos) else {
            return;
        };
        let removable = is_class(state, BlockClass::BaseRailBlock)
            || is_class(state, BlockClass::DiodeBlock)
            || is_class(state, BlockClass::RedstoneWireBlock);
        if removable && !can_survive(state, pos, &mut RegionView(self)) {
            self.set_block(pos, fluid_block(state), output);
        }
    }

    /// Whether the chunks of `pos` and of its six neighbours are loaded, so that an
    /// update there reads only what the region has.
    fn loaded_around(&self, pos: BlockPos) -> bool {
        self.chunks.contains_key(&pos.chunk())
            && UPDATE_SHAPE_ORDER
                .iter()
                .all(|face| self.chunks.contains_key(&relative(pos, *face).chunk()))
    }
}

/// The region's loaded blocks as the shared rules read them, and where the ticks they
/// schedule go. A block outside what is loaded reads as air; the updates are made only
/// where the block and its six neighbours are loaded.
pub(super) struct RegionView<'r>(pub(super) &'r mut Region);

impl BlockView for RegionView<'_> {
    fn block(&mut self, at: BlockPos) -> BlockState {
        self.0.block(at).unwrap_or(blocks::AIR)
    }

    fn raw_brightness(&mut self, _at: BlockPos) -> Option<u8> {
        None
    }

    fn schedule_block_tick(&mut self, at: BlockPos, block: u16, delay: i32) {
        self.0.schedule_tick(at, Ticked::Block(block), delay, 0);
    }

    fn schedule_fluid_tick(&mut self, at: BlockPos, fluid: u8, delay: i32) {
        self.0.schedule_tick(at, Ticked::Fluid(fluid), delay, 0);
    }

    fn rules(&self) -> clustine_blocks::Rules {
        clustine_blocks::Rules::Placing
    }
}

/// `FluidState.createLegacyBlock`: what a block leaves when it goes, water where it held
/// water and air otherwise.
fn fluid_block(state: BlockState) -> BlockState {
    match state.props().fluid() {
        Some(fluid) if fluid.source && is_water(fluid.fluid) => blocks::WATER,
        _ => blocks::AIR,
    }
}

fn is_water(fluid: u8) -> bool {
    clustine_data::FLUIDS[usize::from(fluid)] == "minecraft:water"
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

fn opposite(face: Face) -> Face {
    match face {
        Face::Down => Face::Up,
        Face::Up => Face::Down,
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
    }
}
