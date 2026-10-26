//! Setting a block as the game's `Level.setBlock` does, with the updates it makes in
//! turn (`docs/adr/0024-block-behaviour-in-a-region.md`, section 1): the neighbours
//! told of the change, each neighbour's shape asked again, a neighbour that can no
//! longer stay destroyed, all run as the game's `CollectingNeighborUpdater` runs them,
//! depth first.
//!
//! At the edge of what the region has (`docs/adr/0025-effects-between-regions.md`,
//! sections 4 to 6):
//!
//! - a set or an update that reaches a block of a chunk that is another region's, or
//!   unknown and not wanted, is an effect for whoever uses the chunk;
//! - an update that reaches a block of land the region holds or has asked for, whose
//!   chunk or a neighbour's it can read neither loaded nor copied, is kept as a seam
//!   entry in the chunk of the block that changed, and made once it can read them;
//! - an update another region sent that cannot run yet is kept as a seam entry in its
//!   own chunk, and a seam entry whose block comes to be another region's is sent on.
//!
//! `neighborChanged` removes what can no longer stay (rails, repeaters and
//! comparators), has a liquid look again, and runs redstone's rules (wire, lamps,
//! doors, trapdoors and gates, in `redstone.rs`; torches and diodes, in `diodes.rs`);
//! it is told the block that changed and whether a piston moved it
//! (`docs/adr/0026-redstone.md`, section 2). A change of block tells its neighbours of
//! the block that was there, as the game's `Level.setBlock` does, and runs what the
//! old block does as it goes (`affectNeighborsAfterRemoval`) and the new one as it is
//! placed (`onPlace`).

use clustine_blocks::BlockView;
use clustine_blocks::data::{is_class, same_block};
use clustine_blocks::shape::update_shape;
use clustine_data::{BlockClass, BlockState, Face, blocks};
use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{
    BlockPos, ChunkPos, Due, Effect, Effected, FiledEffect, PlayerId, Seam, SeamKind, Ticked,
};

use super::Region;
use crate::api::{RegionEvent, TickOutput};

/// `Block.UPDATE_NEIGHBORS`: the neighbours are told (`neighborChanged`).
pub(crate) const UPDATE_NEIGHBORS: u32 = 1;
/// `Block.UPDATE_CLIENTS`: the clients are told. A change without it is sent only where
/// the block is to be sent for another (`RegionEvent::BlockChanged`).
pub(crate) const UPDATE_CLIENTS: u32 = 2;
/// `Block.UPDATE_KNOWN_SHAPE`: the neighbours' shapes are not asked again.
pub(crate) const UPDATE_KNOWN_SHAPE: u32 = 16;
/// `Block.UPDATE_SUPPRESS_DROPS`: a block destroyed by the change drops nothing.
pub(crate) const UPDATE_SUPPRESS_DROPS: u32 = 32;
/// `Block.UPDATE_MOVE_BY_PISTON`: the change is a piston's move. A kept update keeps it
/// among its flags, for the `movedByPiston` of the `neighborChanged` it makes.
pub(crate) const UPDATE_MOVE_BY_PISTON: u32 = 64;
/// `Block.UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`: a container replaced does not spill what
/// it holds (`BlockEntity.preRemoveSideEffects` is not run).
pub(crate) const UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS: u32 = 256;
/// `Block.UPDATE_ALL`: what a player's placing and breaking and `destroyBlock` use.
pub(crate) const UPDATE_ALL: u32 = UPDATE_NEIGHBORS | UPDATE_CLIENTS;

/// How deep a chain of shape updates goes (`Block.UPDATE_LIMIT`).
pub(crate) const UPDATE_LIMIT: i32 = 512;
/// How many updates one change may chain before the rest are dropped
/// (`max-chained-neighbor-updates`, 1,000,000 by default).
const MAX_CHAINED: usize = 1_000_000;

/// `Block.UPDATE_SHAPE_ORDER`.
pub(crate) const UPDATE_SHAPE_ORDER: [Face; 6] = [
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
    /// `MultiNeighborUpdate`: the six neighbours of `source` are told, one at a time,
    /// but the one towards `skip`; `next` is the next of [`UPDATE_ORDER`]. `block` is
    /// the block that changed as it was when the update was made, which each is told.
    Neighbours {
        source: BlockPos,
        block: BlockState,
        skip: Option<Face>,
        next: usize,
    },
    /// `SimpleNeighborUpdate` and `FullNeighborUpdate`: the block at `pos` is told that
    /// `block`, towards `from`, changed. `from` is a neighbour, or the block an update
    /// came through (`updateNeighbourForOutputSignal`), and is where the update is kept
    /// if it cannot be made.
    Single {
        pos: BlockPos,
        from: Face,
        block: BlockState,
        moved_by_piston: bool,
    },
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
    /// the block changed here.
    ///
    /// A block the region reads in a copy, of a chunk that is another region's or
    /// unknown and not wanted, is set by an effect, only if it is still what the rule
    /// read there, with the updates of a set (the record, section 4). What the region
    /// cannot read it does not set, as before.
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
        if !self.chunks.contains_key(&pos.chunk()) {
            if self.sends_to(pos.chunk()) {
                self.made.push(Effect {
                    at: pos,
                    what: Effected::SetIf {
                        expected: old,
                        state,
                        flags,
                    },
                });
            }
            return false;
        }
        // A moving piston replaced finishes its move first, unless the change says it
        // is not to (`BlockEntity.preRemoveSideEffects`); that may set the block itself,
        // which this change then replaces, as in the game.
        if is_class(old, BlockClass::MovingPistonBlock) && !same_block(old, state) {
            self.moving_piston_replaced(pos, flags, output);
        }
        // What a container that goes holds, read before its data goes with it.
        let spilled = self.container_to_spill(pos, old, state, flags);
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
            seen: flags & UPDATE_CLIENTS != 0,
        });
        // A circuit's part gone may leave a lent chunk unneeded, and a part that
        // changes is a circuit at work (`docs/adr/0027-lending-at-a-line.md`, sections
        // 4 and 7).
        let (was_part, is_part) = (
            clustine_blocks::signal::is_circuit_part(old),
            clustine_blocks::signal::is_circuit_part(state),
        );
        if was_part && !is_part {
            self.part_gone_near(pos);
        }
        if was_part || is_part {
            self.circuit_changed(pos.chunk());
        }
        // The typed block entity there goes with a block of another kind, and a new
        // one comes with a block that keeps one (`block_entities.rs`).
        self.block_entity_changed(pos, old, state);
        // `BlockEntity.preRemoveSideEffects`, after the chunk's set and before
        // `affectNeighborsAfterRemoval`: a container spills (`containers.rs`).
        if let Some(container) = spilled {
            self.spill_container(pos, container, output);
        }
        // `affectNeighborsAfterRemoval` of the block that went, inside the chunk's set
        // in the game, before the new block's `onPlace`: only where the block is
        // another and the neighbours are to be told, or a piston moved it.
        let moved_by_piston = flags & UPDATE_MOVE_BY_PISTON != 0;
        if !same_block(old, state) && (flags & UPDATE_NEIGHBORS != 0 || moved_by_piston) {
            self.redstone_after_removal(pos, old, moved_by_piston, output);
            self.piston_after_removal(pos, old, output);
            self.container_after_removal(pos, old, output);
            self.sensor_after_removal(pos, old, output);
        }
        // Wire placed works out its power and tells the blocks around it
        // (`RedStoneWireBlock.onPlace`).
        self.redstone_on_place(pos, state, old, output);
        // A piston placed checks whether to move (`PistonBaseBlock.onPlace`).
        self.piston_on_place(pos, state, old, output);
        // TNT placed beside power is primed (`TntBlock.onPlace`, `tnt.rs`).
        self.tnt_on_place(pos, state, old, output);
        // A hopper placed takes whether power locks it (`HopperBlock.onPlace`,
        // `hoppers.rs`).
        self.hopper_on_place(pos, state, old, output);
        // A sensor that comes with power and no tick loses it (`SculkSensorBlock.onPlace`,
        // `sculk.rs`).
        self.sensor_on_place(pos, state, old, output);
        // A sniffer egg, placed or cracked, waits for its next stage
        // (`SnifferEggBlock.onPlace`, `sniffer_eggs.rs`).
        self.sniffer_egg_on_place(pos, state, output);
        // `onPlace`, inside the chunk's set in the game: a block that falls looks two
        // ticks on whether it is held up, a dragon egg five (`FallingBlock.onPlace`,
        // `BrushableBlock`'s), and scaffolding one on how far it is held
        // (`ScaffoldingBlock.onPlace`).
        if is_class(state, BlockClass::FallingBlock)
            || is_class(state, BlockClass::BrushableBlock)
            || is_class(state, BlockClass::ScaffoldingBlock)
        {
            let block = clustine_blocks::data::tick_id(state);
            let delay = if is_class(state, BlockClass::ScaffoldingBlock) {
                1
            } else {
                super::falling::delay_after_place(state)
            };
            self.schedule_tick(pos, Ticked::Block(block), delay, 0);
        }
        // Fire's and coral's `onPlace` schedule their ticks, and fire that cannot stay
        // goes there and then.
        self.fire_on_place(pos, state, old, output);
        // `FrogspawnBlock.onPlace`: its hatching scheduled (`frogspawn.rs`).
        self.frogspawn_on_place(pos, state, old);
        // `BaseRailBlock.onPlace`: a rail joins the rails beside it and turns them
        // towards it (`rails.rs`).
        self.rail_on_place(pos, state, old, output);
        // `CarvedPumpkinBlock.onPlace`: a pumpkin that completes a golem's pattern
        // builds the golem (`golems.rs`).
        self.carved_pumpkin_on_place(pos, state, old, output);
        // `LiquidBlock.onPlace`: lava that meets water becomes stone of some kind there
        // and then; any other liquid looks again in its fluid's time.
        if is_class(state, BlockClass::LiquidBlock) {
            self.liquid_told(pos, state, output);
        }
        // `Level.setBlock` tells nobody of a block that `onPlace` changed again: the
        // change it made has told them.
        if self.block(pos) != Some(state) {
            return true;
        }
        // The neighbours are told of the block that was there, as the game's
        // `Level.setBlock` tells them, and a comparator of a block with an output.
        if flags & UPDATE_NEIGHBORS != 0 {
            self.update_neighbours_at(pos, old, output);
            if state.props().has_analog_output_signal() {
                self.update_neighbour_for_output_signal(pos, state, output);
            }
        }
        if flags & UPDATE_KNOWN_SHAPE == 0 && depth > 0 {
            let passed = flags & !(UPDATE_NEIGHBORS | UPDATE_SUPPRESS_DROPS);
            // `updateIndirectNeighbourShapes` of the block that was there, then the
            // neighbours' shapes, then the new block's: wire's diagonals.
            self.wire_indirect_shapes(pos, old, passed, depth - 1, output);
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
            self.wire_indirect_shapes(pos, state, passed, depth - 1, output);
        }
        true
    }

    /// Gives the block entity at `pos` in a chunk the region has loaded the data
    /// `data`, and says so in its place among the tick's changes, to be committed with
    /// them (`docs/adr/0026-redstone.md`, section 6). Returns whether it took it: there
    /// is no block entity there, or the chunk refuses that much data, which it keeps
    /// without, as the game keeps a block entity it could not save whole.
    pub(crate) fn set_block_entity_data(
        &mut self,
        pos: BlockPos,
        data: Vec<u8>,
        output: &mut TickOutput,
    ) -> bool {
        self.set_block_entity_data_as(pos, data, true, output)
    }

    /// [`Self::set_block_entity_data`], said to the edges only where `seen`: a
    /// container's data is committed and never sent (`containers.rs`).
    pub(crate) fn set_block_entity_data_as(
        &mut self,
        pos: BlockPos,
        data: Vec<u8>,
        seen: bool,
        output: &mut TickOutput,
    ) -> bool {
        let Some(chunk) = self.chunks.get_mut(&pos.chunk()) else {
            return false;
        };
        let (x, z) = pos.in_chunk();
        if chunk
            .set_block_entity_data(clustine_world::InChunk::new(x, pos.y, z), data.clone())
            .is_err()
        {
            return false;
        }
        output.events.push(RegionEvent::BlockEntityChanged {
            position: pos,
            data,
            seen,
        });
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
        self.head_broken_by_player(pos, state, output);
        if is_class(state, BlockClass::DoorBlock) || is_class(state, BlockClass::DoublePlantBlock) {
            self.prevent_drop_from_bottom_part(pos, state, player, output);
        }
        // `AbstractBedBlock.playerWillDestroy`: a bed broken at its foot in creative
        // loses its head first, without a drop, as others see.
        if is_class(state, BlockClass::AbstractBedBlock)
            && state.property("part") == Some("foot")
            && let Some(towards) = clustine_blocks::growth::bed_other_half(state)
        {
            let head_pos = relative(pos, towards);
            if let Some(head) = self.block(head_pos)
                && same_block(head, state)
                && head.property("part") == Some("head")
            {
                let flags = UPDATE_SUPPRESS_DROPS | UPDATE_CLIENTS | UPDATE_NEIGHBORS;
                // A head across the line goes by an effect, and is seen breaking where
                // it goes.
                if self.set_block_updated(head_pos, blocks::AIR, flags, UPDATE_LIMIT, output) {
                    output.events.push(RegionEvent::BlockDestroyed {
                        position: head_pos,
                        state: head,
                        breaker: player,
                    });
                }
            }
        }
        output.events.push(RegionEvent::BlockDestroyed {
            position: pos,
            state,
            breaker: player,
        });
        // `Block.playerWillDestroy`'s `block_destroy`, of the player and the block
        // (`vibrations.rs`).
        let source = player.and_then(|player| self.player_source(player));
        let context = super::vibrations::Context::of(source, Some(state));
        self.vibrate_at(super::vibrations::Vibration::BLOCK_DESTROY, pos, context);
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
        if self.set_block_updated(below, fluid_block(lower), flags, UPDATE_LIMIT, output) {
            output.events.push(RegionEvent::BlockDestroyed {
                position: below,
                state: lower,
                breaker: player,
            });
        }
    }

    /// `Level.updateNeighborsAt(pos, block)`: the six neighbours of `pos` are told that
    /// `block` there changed, in [`UPDATE_ORDER`].
    pub(crate) fn update_neighbours_at(
        &mut self,
        pos: BlockPos,
        block: BlockState,
        output: &mut TickOutput,
    ) {
        self.add_and_run(
            Pending::Neighbours {
                source: pos,
                block,
                skip: None,
                next: 0,
            },
            output,
        );
    }

    /// `Level.updateNeighborsAtExceptFromFacing`: as [`Self::update_neighbours_at`],
    /// but for the neighbour towards `skip`, which a diode or an observer has told
    /// already.
    pub(crate) fn update_neighbours_at_except(
        &mut self,
        pos: BlockPos,
        block: BlockState,
        skip: Face,
        output: &mut TickOutput,
    ) {
        self.add_and_run(
            Pending::Neighbours {
                source: pos,
                block,
                skip: Some(skip),
                next: 0,
            },
            output,
        );
    }

    /// `Level.neighborChanged(pos, block, orientation)`, and with `moved_by_piston`
    /// the game's `neighborChanged` of a state: the block at `pos` is told that
    /// `block`, towards `from`, changed.
    pub(crate) fn neighbour_changed_at(
        &mut self,
        pos: BlockPos,
        from: Face,
        block: BlockState,
        moved_by_piston: bool,
        output: &mut TickOutput,
    ) {
        self.add_and_run(
            Pending::Single {
                pos,
                from,
                block,
                moved_by_piston,
            },
            output,
        );
    }

    /// `Level.updateNeighbourForOutputSignal(pos, block)`: a comparator beside `pos`,
    /// or one beyond a conductor beside it, is told that `block` changed, for each
    /// side in the game's horizontal order (north, east, south, west).
    pub(crate) fn update_neighbour_for_output_signal(
        &mut self,
        pos: BlockPos,
        block: BlockState,
        output: &mut TickOutput,
    ) {
        for direction in [Face::North, Face::East, Face::South, Face::West] {
            let beside = relative(pos, direction);
            let Some(state) = self.block(beside) else {
                continue;
            };
            if is_class(state, BlockClass::ComparatorBlock) {
                self.neighbour_changed_at(beside, opposite(direction), block, false, output);
            } else if clustine_blocks::connect::is_conductor(state) {
                let beyond = relative(beside, direction);
                if self
                    .block(beyond)
                    .is_some_and(|state| is_class(state, BlockClass::ComparatorBlock))
                {
                    // Kept, if it cannot be made, towards the block it came through.
                    self.neighbour_changed_at(beyond, opposite(direction), block, false, output);
                }
            }
        }
    }

    /// `Level.neighborShapeChanged`: the block at `pos` takes its shape again, its
    /// neighbour towards `direction` being now `neighbour`, as the updater runs it.
    pub(super) fn shape_changed_at(
        &mut self,
        pos: BlockPos,
        direction: Face,
        neighbour: BlockState,
        flags: u32,
        depth: i32,
        output: &mut TickOutput,
    ) {
        self.add_and_run(
            Pending::Shape {
                pos,
                direction,
                neighbour,
                flags,
                depth,
            },
            output,
        );
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
            // A multiple update that skips a side passes over it: the step it takes is
            // the next it does not skip.
            if let Pending::Neighbours { skip, next, .. } = top
                && *skip == Some(UPDATE_ORDER[*next])
            {
                *next += 1;
            }
            let current = *top;
            let finished = match top {
                Pending::Shape { .. } | Pending::Single { .. } => true,
                Pending::Neighbours { skip, next, .. } => {
                    *next += 1;
                    if *skip == UPDATE_ORDER.get(*next).copied() {
                        *next += 1;
                    }
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
            Pending::Neighbours {
                source,
                block,
                next,
                ..
            } => {
                let towards = UPDATE_ORDER[next];
                self.tell(
                    relative(source, towards),
                    opposite(towards),
                    block,
                    false,
                    output,
                );
            }
            Pending::Single {
                pos,
                from,
                block,
                moved_by_piston,
            } => self.tell(pos, from, block, moved_by_piston, output),
        }
    }

    /// One block told that `block`, towards `from`, changed: here, or kept as a seam
    /// entry in the chunk of the block it came from, or sent on as an effect.
    fn tell(
        &mut self,
        target: BlockPos,
        from: Face,
        block: BlockState,
        moved_by_piston: bool,
        output: &mut TickOutput,
    ) {
        if self.sends_to(target.chunk()) || !self.may_run_at(target) {
            let mut flags = UPDATE_NEIGHBORS;
            if moved_by_piston {
                flags |= UPDATE_MOVE_BY_PISTON;
            }
            let seam = Seam {
                at: target,
                direction: from as u8,
                kind: SeamKind::Neighbour,
                source: block,
                flags,
                depth: 0,
            };
            self.send_or_keep(relative(target, from), seam);
            return;
        }
        self.neighbour_changed(target, block, moved_by_piston, output);
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
        if self.sends_to(pos.chunk()) || !self.readable_around(pos) {
            let seam = Seam {
                at: pos,
                direction: direction as u8,
                kind: SeamKind::Shape,
                source: neighbour,
                flags,
                depth,
            };
            self.send_or_keep(relative(pos, direction), seam);
            return;
        }
        let Some(old) = self.own_block(pos) else {
            return;
        };
        let new = update_shape(&mut RegionView(self), old, pos, direction, neighbour);
        // A chest does not join a chest across a line (`containers.rs`).
        if new == old || self.keeps_chest_single(pos, direction, old, new) {
            return;
        }
        if new.props().is_air() {
            self.destroy_block(pos, flags & UPDATE_SUPPRESS_DROPS == 0, depth, output);
        } else {
            self.set_block_updated(pos, new, flags & !UPDATE_SUPPRESS_DROPS, depth, output);
        }
    }

    /// `destroyBlock` of a block's own tick, with its drops (a cactus or sugar cane that
    /// can no longer stay).
    pub(super) fn destroy_block_by_tick(&mut self, pos: BlockPos, output: &mut TickOutput) {
        self.destroy_block(pos, true, UPDATE_LIMIT, output);
    }

    /// `Level.destroyBlock`: the block breaks where everyone sees it, drops what it
    /// drops where `drops` asks (`drops.rs`), and gives way to its fluid, with every
    /// update.
    pub(crate) fn destroy_block(
        &mut self,
        pos: BlockPos,
        drops: bool,
        depth: i32,
        output: &mut TickOutput,
    ) {
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
        if drops {
            self.drop_resources(state, pos, output);
        }
        if self.set_block_updated(pos, fluid_block(state), UPDATE_ALL, depth, output) {
            // `block_destroy` of no entity, once the block is gone (`vibrations.rs`).
            let context = super::vibrations::Context::of(None, Some(state));
            self.vibrate_at(super::vibrations::Vibration::BLOCK_DESTROY, pos, context);
        }
    }

    /// `neighborChanged` of the block at `pos`: a liquid looks again; rails, repeaters
    /// and comparators go when they can no longer stay (`removeBlock`, without the bits
    /// of a broken block); wire goes or works out its power again; lamps, doors,
    /// trapdoors and gates follow the power beside them (`redstone.rs`); torches and
    /// diodes schedule their ticks (`diodes.rs`).
    ///
    /// `block` is the block that changed and `moved_by_piston` whether a piston moved
    /// it, as the game's `neighborChanged` is given them (`docs/adr/0026-redstone.md`,
    /// section 2): a door is not moved by an update from its own kind.
    fn neighbour_changed(
        &mut self,
        pos: BlockPos,
        block: BlockState,
        moved_by_piston: bool,
        output: &mut TickOutput,
    ) {
        // Nothing of this step reads whether a piston moved the block (R5's pistons).
        let _ = moved_by_piston;
        if !self.may_run_at(pos) {
            return;
        }
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if is_class(state, BlockClass::LiquidBlock) {
            self.liquid_told(pos, state, output);
            return;
        }
        // A rail goes also where the block its slope rises onto went (`rails.rs`).
        if is_class(state, BlockClass::BaseRailBlock) {
            self.rail_neighbour_changed(pos, block, output);
            return;
        }
        // TNT is primed by power beside it (`TntBlock.neighborChanged`, `tnt.rs`).
        if is_class(state, BlockClass::TntBlock) {
            self.tnt_neighbour_changed(pos, output);
            return;
        }
        // A hopper is locked by power beside it (`HopperBlock.neighborChanged`).
        if self.hopper_neighbour_changed(pos, state, output) {
            return;
        }
        // Wire, lamps, doors, trapdoors, gates, torches and diodes (`redstone.rs`,
        // `diodes.rs`).
        self.redstone_neighbour_changed(pos, state, block, output);
    }

    /// What a liquid block does when it is placed or its neighbour changes
    /// (`LiquidBlock.onPlace`, `neighborChanged`): its fluid ticks in its own time,
    /// unless lava met water and became something else.
    fn liquid_told(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        if self.should_spread_liquid(pos, state, output) {
            let held = clustine_blocks::fluid::FluidState::of(state);
            if let Some(kind) = held.kind() {
                self.schedule_tick(pos, Ticked::Fluid(held.fluid), kind.tick_delay(), 0);
            }
        }
    }

    /// An update from the block at `source` that cannot be made here: an effect where
    /// its block is in a chunk that is another region's, or unknown and not wanted
    /// (the record, section 4); otherwise a seam entry in the chunk of `source`.
    fn send_or_keep(&mut self, source: BlockPos, seam: Seam) {
        if self.sends_to(seam.at.chunk()) {
            self.made.push(effect_of(seam));
        } else {
            self.keep_seam(source, seam);
        }
    }

    /// Keeps `seam`, an update that cannot be made, in the chunk of the block at
    /// `source` if the region has it loaded, unless it reaches outside the world, where
    /// the game makes nothing either. A copy is never written, so this needs none.
    fn keep_seam(&mut self, source: BlockPos, seam: Seam) {
        let chunk = source.chunk();
        let Some(loaded) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let height = i32::try_from(loaded.height()).unwrap_or(i32::MAX);
        if seam.at.y < loaded.min_y() || seam.at.y >= loaded.min_y() + height {
            return;
        }
        loaded.add_seam(seam);
        self.ticks_seams_changed(chunk);
    }

    /// Makes the seam entries and the effects other regions sent (`effects`, ready and
    /// in the order of their numbers), chunk by chunk in ascending position: in each,
    /// first its seam entries whose blocks the region can now read with their
    /// neighbours, in the order of their keys, and then the effects for its blocks in
    /// the order of their numbers (ADR-0024, section 6a; ADR-0025, section 5, "In the
    /// tick"). An entry that cannot be made yet stays; one whose block has come to be
    /// another region's is sent on as an effect (ADR-0025, section 6). Every effect
    /// handed in is taken, in [`TickOutput::taken`].
    pub(crate) fn run_seams(&mut self, effects: &[FiledEffect], output: &mut TickOutput) {
        let mut received: BTreeMap<ChunkPos, Vec<FiledEffect>> = BTreeMap::new();
        for filed in effects {
            received
                .entry(filed.effect.at.chunk())
                .or_default()
                .push(*filed);
            output.taken.push(filed.id);
        }
        output.taken.sort();
        let with_seams = self
            .chunks
            .iter()
            .filter(|(_, chunk)| !chunk.seams().is_empty())
            .map(|(position, _)| *position);
        let chunks: BTreeSet<ChunkPos> = with_seams.chain(received.keys().copied()).collect();
        for chunk in chunks {
            self.run_seams_of(chunk, output);
            for filed in received.remove(&chunk).unwrap_or_default() {
                self.run_effect(filed, output);
            }
        }
    }

    /// The seam entries of `chunk` that can be made or are to be sent on.
    fn run_seams_of(&mut self, chunk: ChunkPos, output: &mut TickOutput) {
        let Some(seams) = self
            .chunks
            .get(&chunk)
            .map(|loaded| loaded.seams().to_vec())
        else {
            return;
        };
        let mut waiting = Vec::new();
        let mut taken = Vec::new();
        let mut retried = Vec::new();
        for seam in seams {
            // A piston waiting for its land is tried again only when the land or the
            // chunks that tick change, so that one refused at a line does not rewrite
            // its chunk every tick (`docs/adr/0026-redstone.md`, section 10).
            if seam.flags & super::pistons::WAITING_PISTON != 0 {
                if self.pistons_may_retry {
                    retried.push(seam);
                } else {
                    waiting.push(seam);
                }
                continue;
            }
            // Land that is another's, or that was given back or never asked for and is
            // not wanted: there the entry would otherwise wait for good (ADR-0025,
            // "Found while building", E4).
            let sent = self.sends_to(seam.at.chunk());
            if sent || self.may_run(&seam) {
                taken.push((seam, sent));
            } else {
                waiting.push(seam);
            }
        }
        if taken.is_empty() && retried.is_empty() {
            return;
        }
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.set_seams(waiting);
        }
        self.ticks_seams_changed(chunk);
        for (seam, sent) in taken {
            if sent {
                self.made.push(effect_of(seam));
            } else {
                self.run_update(seam, output);
            }
        }
        self.retry_waiting_pistons(retried, output);
    }

    /// Makes the update a seam entry keeps, whose block and neighbours the region can
    /// read.
    fn run_update(&mut self, seam: Seam, output: &mut TickOutput) {
        match seam.kind {
            SeamKind::Shape => self.add_and_run(
                Pending::Shape {
                    pos: seam.at,
                    direction: face_of(seam.direction),
                    neighbour: seam.source,
                    flags: seam.flags,
                    depth: seam.depth,
                },
                output,
            ),
            SeamKind::Neighbour => self.neighbour_changed(
                seam.at,
                seam.source,
                seam.flags & UPDATE_MOVE_BY_PISTON != 0,
                output,
            ),
        }
    }

    /// Does what another region sent, by this region's rules (ADR-0025, sections 4 and
    /// 5): an update where its block's six neighbours can be read, or else kept as a
    /// seam entry in its block's chunk; a scheduled tick, due as the store converted
    /// it, or at this tick if that has passed; the target's side of a fluid's spread;
    /// a set, only where the block is still what the sender read. An effect for a chunk
    /// the region has not loaded, which the runner does not hand in, does nothing.
    fn run_effect(&mut self, filed: FiledEffect, output: &mut TickOutput) {
        let at = filed.effect.at;
        if !self.chunks.contains_key(&at.chunk()) {
            return;
        }
        match filed.effect.what {
            Effected::Update {
                direction,
                kind,
                source,
                flags,
                depth,
            } => {
                let seam = Seam {
                    at,
                    direction,
                    kind,
                    source,
                    flags,
                    depth,
                };
                if self.may_run(&seam) {
                    self.made_in = made_in(filed.due);
                    self.run_update(seam, output);
                    self.made_in = None;
                } else {
                    self.keep_seam(at, seam);
                }
            }
            Effected::Tick {
                of,
                delay,
                priority,
            } => {
                let now = self.tick as i64;
                let due = match filed.due {
                    Due::At { tick } => i64::try_from(tick).unwrap_or(i64::MAX).max(now),
                    Due::FromTaking => now + i64::from(delay),
                };
                self.schedule_due(at, of, due, priority);
            }
            Effected::Spread { direction, source } => {
                self.made_in = made_in(filed.due);
                self.spread_received(at, face_of(direction), source, output);
                self.made_in = None;
            }
            Effected::SetIf {
                expected,
                state,
                flags,
            } => {
                if self.own_block(at) == Some(expected) {
                    self.made_in = made_in(filed.due);
                    self.set_block_updated(at, state, flags, UPDATE_LIMIT, output);
                    self.made_in = None;
                }
            }
            Effected::OnEntity { key, by } => self.entity_used(key, by, output),
            Effected::Seated {
                player,
                vehicle,
                chunk,
            } => self.take_seated(player, vehicle, chunk, output),
            Effected::Exploded { expected, blast } => {
                self.take_exploded(at, expected, blast, output);
            }
            Effected::Burnt { expected } => self.take_burnt(at, expected, output),
            // A player of another region on a plate or a tripwire, counted for the tick
            // the effect is for (`pressed.rs`).
            Effected::PlayerInside { min, max } => {
                let due = match filed.due {
                    clustine_world::Due::At { tick } => tick,
                    clustine_world::Due::FromTaking => self.tick,
                };
                self.take_player_inside(at, due, min, max, output);
            }
            // What an animal across a line gave a player of the region (`given.rs`).
            Effected::Given {
                player,
                stack,
                no_duplicates,
            } => {
                if let Some(id) = self.player_with_entity(player) {
                    self.take_given(id, stack, no_duplicates, output);
                }
            }
        }
    }

    /// Whether the chunk of `pos` is loaded and those of its six neighbours are loaded
    /// or copied, so that an update there changes only what the region has and reads
    /// only what it can.
    fn readable_around(&self, pos: BlockPos) -> bool {
        self.chunks.contains_key(&pos.chunk())
            && UPDATE_SHAPE_ORDER
                .iter()
                .all(|face| self.readable(relative(pos, *face).chunk()))
    }

    /// Whether a kept update may run here: a shape asked again where its block's
    /// neighbours can be read, a neighbour told where [`Self::may_run_at`] says.
    fn may_run(&self, seam: &Seam) -> bool {
        match seam.kind {
            SeamKind::Shape => self.readable_around(seam.at),
            SeamKind::Neighbour => self.may_run_at(seam.at),
        }
    }

    /// Whether the block at `pos` may be told that its neighbour changed: its chunk
    /// loaded and its neighbours' read, and, for a block whose rules read a signal or
    /// what is two blocks away, its chunk one that ticks, where all of that is there
    /// (`docs/adr/0026-redstone.md`, section 1). Otherwise the update waits as a seam
    /// entry, so that nothing reads a block that is not there as air. A shape asked
    /// again reads only the neighbours, which [`Self::readable_around`] makes sure of.
    pub(crate) fn may_run_at(&self, pos: BlockPos) -> bool {
        if !self.readable_around(pos) {
            return false;
        }
        let reads = self
            .own_block(pos)
            .is_some_and(clustine_blocks::signal::reads_signal);
        !reads || self.ticks_chunk(pos.chunk())
    }
}

/// The region's tick an effect handed in was made in, which the ticks its doing
/// schedules count their delays from: for one from a coupled region, the tick the
/// store converted it to (ADR-0025, "Found while building", E4); none for one from a
/// region not coupled, whose ticks count from the taking.
fn made_in(due: Due) -> Option<i64> {
    match due {
        Due::At { tick } => Some(i64::try_from(tick).unwrap_or(i64::MAX)),
        Due::FromTaking => None,
    }
}

/// The effect that makes the update `seam` keeps.
fn effect_of(seam: Seam) -> Effect {
    Effect {
        at: seam.at,
        what: Effected::Update {
            direction: seam.direction,
            kind: seam.kind,
            source: seam.source,
            flags: seam.flags,
            depth: seam.depth,
        },
    }
}

/// The region's loaded and copied blocks as the shared rules read them, and where the
/// ticks they schedule go. A block of neither reads as air; the updates are made only
/// where the block is loaded and its six neighbours are loaded or copied.
pub(super) struct RegionView<'r>(pub(super) &'r mut Region);

impl BlockView for RegionView<'_> {
    /// Drawn from a stream of the world, the tick and the next draw's number in the
    /// tick, so that a tick stays a function of the region and its inputs.
    fn next_int(&mut self, bound: u32) -> u32 {
        self.0.next_draw().next_int(bound)
    }

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
        self.0.rules()
    }

    fn has_scheduled_block_tick(&mut self, at: BlockPos, block: u16) -> bool {
        self.0.has_scheduled_tick(at, Ticked::Block(block))
    }

    fn item_frame_signal(&mut self, at: BlockPos, facing: Face) -> Option<u8> {
        self.0.frame_signal(at, facing)
    }

    fn comparator_output(&mut self, at: BlockPos) -> u8 {
        self.0.comparator_output(at)
    }

    fn analog_output(&mut self, at: BlockPos, state: BlockState) -> u8 {
        self.0.analog_output_at(at, state)
    }

    fn trapped_chest_openers(&mut self, at: BlockPos) -> u8 {
        self.0.trapped_chest_openers(at)
    }
}

/// `FluidState.createLegacyBlock`: what a block leaves when it goes, water where it held
/// water and air otherwise.
pub(super) fn fluid_block(state: BlockState) -> BlockState {
    match state.props().fluid() {
        Some(fluid) if fluid.source && is_water(fluid.fluid) => blocks::WATER,
        _ => blocks::AIR,
    }
}

fn is_water(fluid: u8) -> bool {
    clustine_data::FLUIDS[usize::from(fluid)] == "minecraft:water"
}

/// The face numbered `number` (`Face as u8`).
fn face_of(number: u8) -> Face {
    match number {
        0 => Face::Down,
        1 => Face::Up,
        2 => Face::North,
        3 => Face::South,
        4 => Face::West,
        _ => Face::East,
    }
}

pub(crate) fn relative(pos: BlockPos, face: Face) -> BlockPos {
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
