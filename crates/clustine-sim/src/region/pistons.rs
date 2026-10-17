//! Pistons (`docs/adr/0026-redstone.md`, section 9, step R5): the game's
//! `PistonBaseBlock`, `PistonHeadBlock`, `MovingPistonBlock` and
//! `PistonMovingBlockEntity`.
//!
//! A piston told of a change, placed or set in place checks whether it is to move
//! (`checkIfExtend`) and queues a block event: 0 to extend, 1 to retract, 2 to retract
//! at once, finishing the move of what it pushed. The event moves the blocks
//! (`moveBlocks`): each block of the structure becomes a moving piston one block on,
//! whose block entity carries the moved block over three ticks of the block entities'
//! phase, and the head goes out as one. The client re-runs each move from its block
//! event on its own world (`docs/adr/0026-redstone.md`, section 8).
//!
//! At a line, until lending is built (section 10): a piston moves only where every
//! block its move sets or moves is in a chunk that ticks and the region holds. One that
//! is refused is kept as a waiting piston, a seam entry tried again only when the
//! region's land changes or a chunk starts ticking.
//!
//! Players are not moved by the region: a client pushes itself by its own moving
//! piston, and its next move says where it is. Other entities wait for their phase.

use clustine_blocks::data::{flag, is_block, is_class, set_flag};
use clustine_blocks::piston::{self, Heights, Structure};
use clustine_blocks::signal;
use clustine_data::{BlockClass, BlockState, Face, blocks, sounds};
use clustine_world::{BlockPos, Seam, SeamKind};

use super::Region;
use super::block_entities::{DONE, Moving};
use super::redstone::play;
use super::updates::{
    RegionView, UPDATE_ALL, UPDATE_CLIENTS, UPDATE_KNOWN_SHAPE, UPDATE_LIMIT,
    UPDATE_MOVE_BY_PISTON, UPDATE_NEIGHBORS, relative,
};
use crate::api::TickOutput;

/// `Block.UPDATE_INVISIBLE`: nothing of it for the clients, which Clustine tells of
/// every change all the same.
const UPDATE_INVISIBLE: u32 = 4;
/// `Block.UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`: a moving piston replaced is not
/// finished by being replaced.
pub(crate) const UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS: u32 = 256;
/// The flags of a base set to a moving piston as it retracts.
const UPDATE_RETRACT_BASE: u32 =
    UPDATE_INVISIBLE | UPDATE_KNOWN_SHAPE | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS;
/// The flags of a block set to a moving piston where it moves to.
const UPDATE_MOVING_BLOCK: u32 =
    UPDATE_INVISIBLE | UPDATE_MOVE_BY_PISTON | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS;
/// The flags of a block a push breaks.
const UPDATE_DESTROYED_BLOCK: u32 = UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE;
/// The flags of a block cleared where it moved from.
const UPDATE_CLEARED_MOVED_BLOCK: u32 = UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE | UPDATE_MOVE_BY_PISTON;
/// A seam entry's flag that it is a waiting piston, not an update: a piston refused for
/// its land, tried again only when the land or the chunks that tick change.
pub(crate) const WAITING_PISTON: u32 = 1 << 30;

/// The block events of a piston (`PistonBaseBlock.triggerEvent`).
const EXTEND: i32 = 0;
const RETRACT: i32 = 1;
const RETRACT_AT_ONCE: i32 = 2;

impl Region {
    /// The world's heights at `pos`, as its chunk has them.
    fn heights_at(&self, pos: BlockPos) -> Heights {
        let min_y = self.config.lowest_y;
        let height = self
            .chunks
            .get(&pos.chunk())
            .map_or(384, |chunk| i32::try_from(chunk.height()).unwrap_or(384));
        Heights {
            min_y,
            max_y_exclusive: min_y + height,
        }
    }

    /// `PistonBaseBlock.checkIfExtend`: a piston powered and not extended extends if it
    /// can; one extended and not powered retracts, at once where the block it pushed is
    /// still on its way out and has not got half way, has ticked this tick, or the
    /// region handles its tick.
    pub(super) fn piston_check(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let _ = output;
        let Some(facing) = clustine_blocks::data::facing(state) else {
            return;
        };
        let powered = piston::piston_powered(&mut RegionView(self), pos, facing);
        let extended = flag(state, "extended");
        let heights = self.heights_at(pos);
        if powered && !extended {
            let Some(structure) =
                piston::resolve(&mut RegionView(self), pos, facing, true, heights)
            else {
                return;
            };
            if !self.holds_the_move(pos, facing, Some(&structure)) {
                self.keep_waiting_piston(pos, state);
                return;
            }
            self.block_event(pos, state, EXTEND, piston::legacy_id(facing));
            return;
        }
        if powered || !extended {
            return;
        }
        let pushed = relative(relative(pos, facing), facing);
        let at_once = self.own_block(pushed).is_some_and(|state| {
            is_class(state, BlockClass::MovingPistonBlock)
                && clustine_blocks::data::facing(state) == Some(facing)
        }) && self.moving_piston(pushed).is_some_and(|moving| {
            moving.extending
                && (moving.progress < 1 || moving.last_ticked == self.tick || self.handling_tick())
        });
        let event = if at_once { RETRACT_AT_ONCE } else { RETRACT };
        let pulled = if flag_sticky(state) {
            piston::resolve(&mut RegionView(self), pos, facing, false, heights)
        } else {
            None
        };
        if !self.holds_the_move(pos, facing, pulled.as_ref()) {
            self.keep_waiting_piston(pos, state);
            return;
        }
        self.block_event(pos, state, event, piston::legacy_id(facing));
    }

    /// Whether every block a move of the piston at `pos` facing `facing` would set or
    /// move is in a chunk that ticks and that the region holds: the piston, its head,
    /// the block two ahead whose move an instant retraction finishes, and each block of
    /// `structure` with where it goes (`docs/adr/0026-redstone.md`, section 10).
    fn holds_the_move(&self, pos: BlockPos, facing: Face, structure: Option<&Structure>) -> bool {
        let head = relative(pos, facing);
        let two = relative(head, facing);
        let mut touched = vec![pos, head];
        // A retraction finishes a move two ahead, and a sticky piston pulls from there.
        let moving_two = self
            .own_block(two)
            .is_some_and(|state| is_class(state, BlockClass::MovingPistonBlock));
        if moving_two || structure.is_some() {
            touched.push(two);
        }
        if let Some(structure) = structure {
            for moved in &structure.to_push {
                touched.push(*moved);
                touched.push(relative(*moved, structure.push_direction));
            }
            touched.extend(structure.to_destroy.iter().copied());
        }
        touched.iter().all(|at| {
            let chunk = at.chunk();
            self.chunks.contains_key(&chunk) && self.land.holds(chunk) && self.ticks_chunk(chunk)
        })
    }

    /// Keeps the piston at `pos` waiting for its land, as a seam entry in its chunk.
    fn keep_waiting_piston(&mut self, pos: BlockPos, state: BlockState) {
        let Some(loaded) = self.chunks.get_mut(&pos.chunk()) else {
            return;
        };
        let seam = Seam {
            at: pos,
            direction: Face::Down as u8,
            kind: SeamKind::Neighbour,
            source: state,
            flags: UPDATE_NEIGHBORS | WAITING_PISTON,
            depth: 0,
        };
        if loaded.seams().contains(&seam) {
            return;
        }
        loaded.add_seam(seam);
        self.ticks_seams_changed(pos.chunk());
    }

    /// `PistonBaseBlock.triggerEvent`: the piston's block event, which moves it if it
    /// still should. Returns whether it ran, and so is sent to the players near.
    pub(super) fn piston_event(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        kind: i32,
        data: i32,
        output: &mut TickOutput,
    ) -> Option<bool> {
        if !piston::is_piston_base(state) {
            return None;
        }
        let facing = clustine_blocks::data::facing(state)?;
        let extended = set_flag(state, "extended", true);
        let powered = piston::piston_powered(&mut RegionView(self), pos, facing);
        if powered && (kind == RETRACT || kind == RETRACT_AT_ONCE) {
            self.set_block_updated(pos, extended, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            return Some(false);
        }
        if !powered && kind == EXTEND {
            return Some(false);
        }
        if kind == EXTEND {
            if !self.move_blocks(pos, facing, true, output) {
                return Some(false);
            }
            self.set_block_updated(
                pos,
                extended,
                UPDATE_ALL | UPDATE_MOVE_BY_PISTON,
                UPDATE_LIMIT,
                output,
            );
            let pitch = self.next_draw().next_float() * 0.25 + 0.6;
            play(pos, sounds::BLOCK_PISTON_EXTEND, 0.5, pitch, None, output);
        } else if kind == RETRACT || kind == RETRACT_AT_ONCE {
            self.retract(pos, state, facing, kind, data, output);
        }
        Some(true)
    }

    /// The retraction of the piston at `pos` (`triggerEvent` for 1 and 2): the move of
    /// its head finished, the base itself a moving piston coming back, and a sticky
    /// piston's block pulled in, or its head dropped.
    fn retract(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        facing: Face,
        kind: i32,
        data: i32,
        output: &mut TickOutput,
    ) {
        let head = relative(pos, facing);
        self.finish_moving_piston(head, output);
        let sticky = flag_sticky(state);
        let piston_type = if sticky { "sticky" } else { "normal" };
        let moving_state = moving_piston_state(facing, piston_type);
        self.set_block_updated(pos, moving_state, UPDATE_RETRACT_BASE, UPDATE_LIMIT, output);
        let base = if sticky {
            blocks::STICKY_PISTON
        } else {
            blocks::PISTON
        };
        let moved = base
            .with_property("facing", face_name(piston::from_legacy_id(data & 7)))
            .expect("a piston faces");
        self.start_moving(pos, moved, facing, false, true, output);
        self.update_neighbours_at(pos, moving_state, output);
        self.update_neighbour_shapes(pos, moving_state, UPDATE_CLIENTS, output);
        if sticky {
            let two = relative(head, facing);
            let two_state = self.own_block(two).unwrap_or(blocks::AIR);
            let finished_a_piece = is_class(two_state, BlockClass::MovingPistonBlock)
                && self
                    .moving_piston(two)
                    .is_some_and(|moving| moving.direction == facing && moving.extending)
                && self.finish_moving_piston(two, output);
            if !finished_a_piece {
                let heights = self.heights_at(pos);
                let pullable = kind == RETRACT
                    && !two_state.props().is_air()
                    && piston::is_pushable(
                        two_state,
                        two,
                        facing.opposite(),
                        false,
                        facing,
                        heights,
                    )
                    && (two_state.props().push_reaction() == clustine_data::PushReaction::PushPull
                        || piston::is_piston_base(two_state));
                if pullable {
                    self.move_blocks(pos, facing, false, output);
                } else {
                    self.remove_piston_part(head, output);
                }
            }
        } else {
            self.remove_piston_part(head, output);
        }
        let pitch = self.next_draw().next_float() * 0.15 + 0.6;
        play(pos, sounds::BLOCK_PISTON_CONTRACT, 0.5, pitch, None, output);
    }

    /// `PistonBaseBlock.moveBlocks`: the structure moves one block on, each block of it
    /// a moving piston where it goes, what it breaks broken, the head going out where it
    /// extends; then what is left behind cleared, and every neighbour told, in the
    /// game's order. Returns whether it could move.
    fn move_blocks(
        &mut self,
        pos: BlockPos,
        facing: Face,
        extending: bool,
        output: &mut TickOutput,
    ) -> bool {
        let head = relative(pos, facing);
        if !extending
            && self
                .own_block(head)
                .is_some_and(|state| is_class(state, BlockClass::PistonHeadBlock))
        {
            self.set_block_updated(head, blocks::AIR, UPDATE_RETRACT_BASE, UPDATE_LIMIT, output);
        }
        let heights = self.heights_at(pos);
        let Some(structure) =
            piston::resolve(&mut RegionView(self), pos, facing, extending, heights)
        else {
            return false;
        };
        let push = structure.push_direction;
        let pushed_states: Vec<BlockState> = structure
            .to_push
            .iter()
            .map(|at| self.own_block(*at).unwrap_or(blocks::AIR))
            .collect();
        let mut delete_after_move: Vec<(BlockPos, BlockState)> = structure
            .to_push
            .iter()
            .copied()
            .zip(pushed_states.iter().copied())
            .collect();
        let mut to_update: Vec<BlockState> = Vec::new();
        for at in structure.to_destroy.iter().rev() {
            let state = self.own_block(*at).unwrap_or(blocks::AIR);
            // Nothing drops in creative, where every player is.
            self.set_block_updated(
                *at,
                blocks::AIR,
                UPDATE_DESTROYED_BLOCK,
                UPDATE_LIMIT,
                output,
            );
            to_update.push(state);
        }
        for (index, at) in structure.to_push.iter().enumerate().rev() {
            let state = self.own_block(*at).unwrap_or(blocks::AIR);
            let destination = relative(*at, push);
            delete_after_move.retain(|(deleted, _)| *deleted != destination);
            let moving_state = moving_piston_state(facing, "normal");
            self.set_block_updated(
                destination,
                moving_state,
                UPDATE_MOVING_BLOCK,
                UPDATE_LIMIT,
                output,
            );
            self.start_moving(
                destination,
                pushed_states[index],
                facing,
                extending,
                false,
                output,
            );
            to_update.push(state);
        }
        if extending {
            let piston_type = if flag_sticky(self.own_block(pos).unwrap_or(blocks::AIR)) {
                "sticky"
            } else {
                "normal"
            };
            let head_state = blocks::PISTON_HEAD
                .with_property("facing", face_name(facing))
                .and_then(|state| state.with_property("type", piston_type))
                .expect("a head has these");
            let moving_state = moving_piston_state(facing, piston_type);
            delete_after_move.retain(|(deleted, _)| *deleted != head);
            self.set_block_updated(
                head,
                moving_state,
                UPDATE_MOVING_BLOCK,
                UPDATE_LIMIT,
                output,
            );
            self.start_moving(head, head_state, facing, true, true, output);
        }
        // The game keeps them in a `HashMap` of at most twelve, iterated by bucket.
        delete_after_move.sort_by_key(|(at, _)| signal::java_bucket(*at));
        for (at, _) in &delete_after_move {
            self.set_block_updated(
                *at,
                blocks::AIR,
                UPDATE_CLEARED_MOVED_BLOCK,
                UPDATE_LIMIT,
                output,
            );
        }
        for (at, old) in &delete_after_move {
            self.wire_indirect_shapes(*at, *old, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            self.update_neighbour_shapes(*at, blocks::AIR, UPDATE_CLIENTS, output);
            self.wire_indirect_shapes(*at, blocks::AIR, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
        let mut updated = to_update.into_iter();
        for at in structure.to_destroy.iter().rev() {
            let state = updated.next().expect("one for each destroyed");
            self.redstone_after_removal(*at, state, false, output);
            self.wire_indirect_shapes(*at, state, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            self.update_neighbours_at(*at, state, output);
        }
        for at in structure.to_push.iter().rev() {
            let state = updated.next().expect("one for each pushed");
            self.update_neighbours_at(*at, state, output);
        }
        if extending {
            self.update_neighbours_at(head, blocks::PISTON_HEAD, output);
        }
        true
    }

    /// `Level.updateNeighbourShapes`: each neighbour of `pos` asks its shape again, the
    /// block at `pos` being now `state`.
    fn update_neighbour_shapes(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        flags: u32,
        output: &mut TickOutput,
    ) {
        for direction in super::updates::UPDATE_SHAPE_ORDER {
            let neighbour = relative(pos, direction);
            // `BlockState.updateNeighbourShapes` passes the limit as it is; only
            // `Level.setBlock` takes one off.
            self.shape_changed_at(
                neighbour,
                direction.opposite(),
                state,
                flags,
                UPDATE_LIMIT,
                output,
            );
        }
    }

    /// `Level.removeBlock` without drops: the block gives way to its fluid, with every
    /// update.
    fn remove_piston_part(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let left = super::updates::fluid_block(state);
        self.set_block_updated(pos, left, UPDATE_ALL, UPDATE_LIMIT, output);
    }

    /// The moving piston's block entity made at `pos`, which has just become a moving
    /// piston: `moved` on its way `direction`, out or in, the piston's own or not.
    fn start_moving(
        &mut self,
        pos: BlockPos,
        moved: BlockState,
        direction: Face,
        extending: bool,
        source: bool,
        output: &mut TickOutput,
    ) {
        let moving = Moving {
            moved,
            direction,
            extending,
            source,
            progress: 0,
            progress_before: 0,
            last_ticked: 0,
        };
        self.set_moving_piston(pos, moving, output);
    }

    /// `PistonMovingBlockEntity.tick`: half a block on, and the move done the tick
    /// after it has got all the way.
    pub(super) fn moving_piston_tick(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(mut moving) = self.moving_piston(pos) else {
            return;
        };
        moving.last_ticked = self.tick;
        moving.progress_before = moving.progress;
        if moving.progress_before >= DONE {
            self.remove_typed(pos);
            self.finish_tick(pos, moving, output);
            return;
        }
        moving.progress = (moving.progress + 1).min(DONE);
        self.set_moving_piston(pos, moving, output);
    }

    /// `PistonMovingBlockEntity.finishTick`, inside its tick: the moved block takes its
    /// place, its shape asked of its neighbours, and is told of itself; or breaks, where
    /// its neighbours leave it no shape.
    fn finish_tick(&mut self, pos: BlockPos, moving: Moving, output: &mut TickOutput) {
        if !self
            .own_block(pos)
            .is_some_and(|state| is_class(state, BlockClass::MovingPistonBlock))
        {
            return;
        }
        let mut placed = self.shaped_by_neighbours(moving.moved, pos);
        if placed.props().is_air() {
            let flags = UPDATE_INVISIBLE
                | UPDATE_KNOWN_SHAPE
                | UPDATE_MOVE_BY_PISTON
                | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS;
            self.set_block_updated(pos, moving.moved, flags, UPDATE_LIMIT, output);
            // `Block.updateOrDestroy` to air: the block breaks there.
            self.destroy_block(pos, true, UPDATE_LIMIT, output);
            return;
        }
        if placed.property("waterlogged") == Some("true") {
            placed = set_flag(placed, "waterlogged", false);
        }
        self.set_block_updated(
            pos,
            placed,
            UPDATE_ALL | UPDATE_MOVE_BY_PISTON,
            UPDATE_LIMIT,
            output,
        );
        self.neighbour_changed_at(pos, Face::Down, placed, false, output);
    }

    /// `PistonMovingBlockEntity.finalTick`: the move of the moving piston at `pos`
    /// finished at once, as an instant retraction or a moving piston replaced does.
    /// Returns whether there was a move to finish.
    pub(super) fn finish_moving_piston(&mut self, pos: BlockPos, output: &mut TickOutput) -> bool {
        let Some(moving) = self.moving_piston(pos) else {
            return false;
        };
        if moving.progress_before >= DONE {
            return false;
        }
        self.remove_typed(pos);
        if !self
            .own_block(pos)
            .is_some_and(|state| is_class(state, BlockClass::MovingPistonBlock))
        {
            return true;
        }
        let placed = if moving.source {
            blocks::AIR
        } else {
            self.shaped_by_neighbours(moving.moved, pos)
        };
        self.set_block_updated(pos, placed, UPDATE_ALL, UPDATE_LIMIT, output);
        self.neighbour_changed_at(pos, Face::Down, placed, false, output);
        true
    }

    /// `Block.updateFromNeighbourShapes`: `state` at `pos` as its neighbours shape it,
    /// asked in the game's order.
    fn shaped_by_neighbours(&mut self, state: BlockState, pos: BlockPos) -> BlockState {
        let mut shaped = state;
        for direction in super::updates::UPDATE_SHAPE_ORDER {
            let neighbour = self.block(relative(pos, direction)).unwrap_or(blocks::AIR);
            shaped = clustine_blocks::shape::update_shape(
                &mut RegionView(self),
                shaped,
                pos,
                direction,
                neighbour,
            );
        }
        shaped
    }

    /// `neighborChanged` of a piston's base and head, and the piston's `onPlace`, for a
    /// block at `pos` whose state is `state`. Returns whether it is one of them.
    pub(super) fn piston_neighbour_changed(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        block: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if piston::is_piston_base(state) {
            self.piston_check(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::PistonHeadBlock) {
            // `PistonHeadBlock.neighborChanged`: the base is told of what changed.
            if piston::head_can_survive(&mut RegionView(self), state, pos)
                && let Some(towards) = clustine_blocks::data::facing(state)
            {
                let base = relative(pos, towards.opposite());
                self.neighbour_changed_at(base, towards, block, false, output);
            }
            return true;
        }
        is_class(state, BlockClass::MovingPistonBlock)
    }

    /// `PistonBaseBlock.onPlace`: a piston that has just become a piston, with no block
    /// entity of a move there, checks whether to move.
    pub(super) fn piston_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if piston::is_piston_base(state)
            && !clustine_blocks::data::same_block(state, old)
            && self.moving_piston(pos).is_none()
        {
            self.piston_check(pos, state, output);
        }
    }

    /// What goes with a piston's part that goes at `pos` (`affectNeighborsAfterRemoval`
    /// of the head, `MovingPistonBlock`'s and the block entity's `preRemoveSideEffects`):
    /// a head's base goes with it, and a moving piston replaced finishes its move,
    /// unless the change says it is not to.
    pub(super) fn piston_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if is_class(old, BlockClass::PistonHeadBlock)
            && let Some(towards) = clustine_blocks::data::facing(old)
        {
            let base = relative(pos, towards.opposite());
            if self
                .own_block(base)
                .is_some_and(|state| piston::is_fitting_base(old, state))
            {
                self.destroy_block(base, true, UPDATE_LIMIT, output);
            }
        }
    }

    /// `PistonMovingBlockEntity.preRemoveSideEffects`, before a moving piston at `pos`
    /// is replaced by a change whose `flags` do not skip it: its move is finished.
    pub(super) fn moving_piston_replaced(
        &mut self,
        pos: BlockPos,
        flags: u32,
        output: &mut TickOutput,
    ) {
        if flags & UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS == 0 && self.moving_piston(pos).is_some() {
            self.finish_moving_piston(pos, output);
        }
    }

    /// `PistonHeadBlock.playerWillDestroy` in creative: a head broken takes its base
    /// with it, without drops.
    pub(super) fn head_broken_by_player(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_class(state, BlockClass::PistonHeadBlock) {
            return;
        }
        let Some(towards) = clustine_blocks::data::facing(state) else {
            return;
        };
        let base = relative(pos, towards.opposite());
        if self
            .own_block(base)
            .is_some_and(|base_state| piston::is_fitting_base(state, base_state))
        {
            self.destroy_block(base, false, UPDATE_LIMIT, output);
        }
    }

    /// Tries again every waiting piston of the chunk at `chunk` whose seam entries are
    /// `seams`, now that the land or the chunks that tick have changed: each checks
    /// again, and is kept again if it is still refused. Returns the entries that are
    /// not waiting pistons.
    pub(super) fn retry_waiting_pistons(
        &mut self,
        seams: Vec<Seam>,
        output: &mut TickOutput,
    ) -> Vec<Seam> {
        let (waiting, rest): (Vec<Seam>, Vec<Seam>) = seams
            .into_iter()
            .partition(|seam| seam.flags & WAITING_PISTON != 0);
        for seam in waiting {
            if let Some(state) = self.own_block(seam.at)
                && piston::is_piston_base(state)
            {
                self.piston_check(seam.at, state, output);
            }
        }
        rest
    }
}

/// A moving piston's block facing `facing`, of the piston kind `piston_type`.
fn moving_piston_state(facing: Face, piston_type: &str) -> BlockState {
    blocks::MOVING_PISTON
        .with_property("facing", face_name(facing))
        .and_then(|state| state.with_property("type", piston_type))
        .expect("a moving piston has these")
}

/// Whether `state` is a sticky piston's base.
fn flag_sticky(state: BlockState) -> bool {
    is_block(state, blocks::STICKY_PISTON)
}

/// The name a face has as a property's value.
fn face_name(face: Face) -> &'static str {
    match face {
        Face::Down => "down",
        Face::Up => "up",
        Face::North => "north",
        Face::South => "south",
        Face::West => "west",
        Face::East => "east",
    }
}
