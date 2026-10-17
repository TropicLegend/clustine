//! What a piston moves (`docs/adr/0026-redstone.md`, section 9): the game's
//! `PistonBaseBlock.isPushable` and `PistonStructureResolver`, and whether a piston is
//! powered, `PistonBaseBlock.getNeighborSignal`, which is quasi-connectivity. Pure
//! functions of the blocks a view reads; the region's piston rules move what these
//! find.

use clustine_data::{BlockClass, BlockState, Face, PushReaction, blocks};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{facing, flag, is_block, is_class, relative};
use crate::signal;

/// The most blocks a piston moves (`PistonStructureResolver.MAX_PUSH_DEPTH`).
pub const MAX_PUSH_DEPTH: usize = 12;

/// Where in height the world's blocks are: the lowest, and one above the highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heights {
    pub min_y: i32,
    pub max_y_exclusive: i32,
}

/// `PistonBaseBlock.getNeighborSignal`: a piston facing `facing` is powered by a signal
/// from any side but its front, from its own position downwards, or into the block
/// above it from any side but below, which is quasi-connectivity.
pub fn piston_powered<V: BlockView + ?Sized>(view: &mut V, pos: BlockPos, facing: Face) -> bool {
    for direction in Face::ALL {
        if direction != facing && signal::has_signal(view, relative(pos, direction), direction) {
            return true;
        }
    }
    if signal::has_signal(view, pos, Face::Down) {
        return true;
    }
    let above = relative(pos, Face::Up);
    for direction in Face::ALL {
        if direction != Face::Down
            && signal::has_signal(view, relative(above, direction), direction)
        {
            return true;
        }
    }
    false
}

/// Whether `state` is a piston's base, the game's `PistonBaseBlock`.
pub fn is_piston_base(state: BlockState) -> bool {
    is_class(state, BlockClass::PistonBaseBlock)
}

/// Whether a block of slime or honey sticks the blocks beside it to what it moves.
pub fn is_sticky(state: BlockState) -> bool {
    is_block(state, blocks::SLIME_BLOCK) || is_block(state, blocks::HONEY_BLOCK)
}

/// `PistonStructureResolver.canStickToEachOther`: slime and honey do not stick to each
/// other, and one of the two has to be sticky.
fn can_stick_to_each_other(first: BlockState, second: BlockState) -> bool {
    let honey_and_slime = (is_block(first, blocks::HONEY_BLOCK)
        && is_block(second, blocks::SLIME_BLOCK))
        || (is_block(first, blocks::SLIME_BLOCK) && is_block(second, blocks::HONEY_BLOCK));
    !honey_and_slime && (is_sticky(first) || is_sticky(second))
}

/// `PistonBaseBlock.isPushable`: whether `state` at `pos` may be moved `direction`,
/// as part of a line that runs `connection` (a block that is only pushed moves only
/// along the push), and with `allow_destroyable` whether a block a push breaks counts.
/// The world has no border; the build height is `heights`.
pub fn is_pushable(
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    allow_destroyable: bool,
    connection: Face,
    heights: Heights,
) -> bool {
    if pos.y < heights.min_y || pos.y >= heights.max_y_exclusive {
        return false;
    }
    if state.props().is_air() {
        return true;
    }
    if [
        blocks::OBSIDIAN,
        blocks::CRYING_OBSIDIAN,
        blocks::RESPAWN_ANCHOR,
        blocks::REINFORCED_DEEPSLATE,
    ]
    .into_iter()
    .any(|immovable| is_block(state, immovable))
    {
        return false;
    }
    if (direction == Face::Down && pos.y == heights.min_y)
        || (direction == Face::Up && pos.y == heights.max_y_exclusive - 1)
    {
        return false;
    }
    if is_piston_base(state) {
        if flag(state, "extended") {
            return false;
        }
    } else {
        if state.props().is_unbreakable() {
            return false;
        }
        match state.props().push_reaction() {
            PushReaction::Immovable => return false,
            PushReaction::Popped => return allow_destroyable,
            PushReaction::Push => return direction == connection,
            PushReaction::PushPull | PushReaction::IgnoreEntity => {}
        }
    }
    !state.props().has_block_entity()
}

/// What a piston at `piston` facing `facing` would move: the blocks it pushes or pulls,
/// in the game's order, and those it breaks (`PistonStructureResolver`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    /// The way the blocks move: the piston's facing when it extends, its opposite when
    /// it pulls.
    pub push_direction: Face,
    pub to_push: Vec<BlockPos>,
    pub to_destroy: Vec<BlockPos>,
}

/// `PistonStructureResolver.resolve`: the structure, or `None` where it cannot move.
pub fn resolve<V: BlockView + ?Sized>(
    view: &mut V,
    piston: BlockPos,
    facing: Face,
    extending: bool,
    heights: Heights,
) -> Option<Structure> {
    let (push_direction, start) = if extending {
        (facing, relative(piston, facing))
    } else {
        (
            facing.opposite(),
            relative(relative(piston, facing), facing),
        )
    };
    let mut resolver = Resolver {
        view,
        piston,
        push_direction,
        heights,
        to_push: Vec::new(),
        to_destroy: Vec::new(),
    };
    let first = resolver.view.block(start);
    if !is_pushable(first, start, push_direction, false, facing, heights) {
        if extending && first.props().push_reaction() == PushReaction::Popped {
            resolver.to_destroy.push(start);
            return Some(resolver.structure());
        }
        return None;
    }
    if !resolver.add_block_line(start, push_direction) {
        return None;
    }
    let mut index = 0;
    while index < resolver.to_push.len() {
        let pos = resolver.to_push[index];
        let state = resolver.view.block(pos);
        if is_sticky(state) && !resolver.add_branching_blocks(pos) {
            return None;
        }
        index += 1;
    }
    Some(resolver.structure())
}

struct Resolver<'a, V: BlockView + ?Sized> {
    view: &'a mut V,
    piston: BlockPos,
    push_direction: Face,
    heights: Heights,
    to_push: Vec<BlockPos>,
    to_destroy: Vec<BlockPos>,
}

impl<V: BlockView + ?Sized> Resolver<'_, V> {
    fn structure(self) -> Structure {
        Structure {
            push_direction: self.push_direction,
            to_push: self.to_push,
            to_destroy: self.to_destroy,
        }
    }

    fn pushable(&self, state: BlockState, pos: BlockPos, destroyable: bool, line: Face) -> bool {
        is_pushable(
            state,
            pos,
            self.push_direction,
            destroyable,
            line,
            self.heights,
        )
    }

    /// `addBlockLine`: the line through `start` along `direction`, the sticky blocks
    /// behind it and what it pushes ahead.
    fn add_block_line(&mut self, start: BlockPos, direction: Face) -> bool {
        let mut next = self.view.block(start);
        if next.props().is_air() {
            return true;
        }
        if !self.pushable(next, start, false, direction)
            || start == self.piston
            || self.to_push.contains(&start)
        {
            return true;
        }
        let mut count = 1;
        if count + self.to_push.len() > MAX_PUSH_DEPTH {
            return false;
        }
        let back = self.push_direction.opposite();
        while is_sticky(next) {
            let pos = offset(start, back, count);
            let previous = next;
            next = self.view.block(pos);
            if next.props().is_air()
                || !can_stick_to_each_other(previous, next)
                || !self.pushable(next, pos, false, back)
                || pos == self.piston
            {
                break;
            }
            count += 1;
            if count + self.to_push.len() > MAX_PUSH_DEPTH {
                return false;
            }
        }
        let mut added = 0;
        for behind in (0..count).rev() {
            self.to_push.push(offset(start, back, behind));
            added += 1;
        }
        let mut ahead = 1;
        loop {
            let pos = offset(start, self.push_direction, ahead);
            if let Some(collision) = self.to_push.iter().position(|pushed| *pushed == pos) {
                self.reorder_list_at_collision(added, collision);
                for index in 0..=collision + added {
                    let branch = self.to_push[index];
                    let state = self.view.block(branch);
                    if is_sticky(state) && !self.add_branching_blocks(branch) {
                        return false;
                    }
                }
                return true;
            }
            next = self.view.block(pos);
            if next.props().is_air() {
                return true;
            }
            if !self.pushable(next, pos, true, self.push_direction) || pos == self.piston {
                return false;
            }
            if next.props().push_reaction() == PushReaction::Popped {
                self.to_destroy.push(pos);
                return true;
            }
            if self.to_push.len() >= MAX_PUSH_DEPTH {
                return false;
            }
            self.to_push.push(pos);
            added += 1;
            ahead += 1;
        }
    }

    /// `reorderListAtCollision`: the line just added goes before the block it ran into.
    fn reorder_list_at_collision(&mut self, added: usize, collision: usize) {
        let last_line_start = self.to_push.len() - added;
        let mut reordered = Vec::with_capacity(self.to_push.len());
        reordered.extend_from_slice(&self.to_push[..collision]);
        reordered.extend_from_slice(&self.to_push[last_line_start..]);
        reordered.extend_from_slice(&self.to_push[collision..last_line_start]);
        self.to_push = reordered;
    }

    /// `addBranchingBlocks`: what sticks to a sticky block from the sides across the
    /// push, in the game's order of directions.
    fn add_branching_blocks(&mut self, from: BlockPos) -> bool {
        let from_state = self.view.block(from);
        for direction in Face::ALL {
            if axis(direction) == axis(self.push_direction) {
                continue;
            }
            let neighbour = relative(from, direction);
            let state = self.view.block(neighbour);
            if can_stick_to_each_other(state, from_state)
                && !self.add_block_line(neighbour, direction)
            {
                return false;
            }
        }
        true
    }
}

/// `pos` moved `count` blocks towards `face`.
fn offset(pos: BlockPos, face: Face, count: usize) -> BlockPos {
    (0..count).fold(pos, |at, _| relative(at, face))
}

/// The axis of a face, as a number of its own.
fn axis(face: Face) -> u8 {
    match face {
        Face::Down | Face::Up => 0,
        Face::North | Face::South => 1,
        Face::West | Face::East => 2,
    }
}

/// The game's `Direction.get3DDataValue`, which a piston's block events and its moving
/// block entity name a direction by.
pub fn legacy_id(face: Face) -> i32 {
    face as i32
}

/// `Direction.from3DDataValue`: the face of a legacy id, its absolute value modulo 6.
pub fn from_legacy_id(id: i32) -> Face {
    Face::ALL[(id % 6).unsigned_abs() as usize]
}

/// `PistonHeadBlock.isFittingBase`: `base` is the piston, of the head's kind, extended
/// and facing as the head faces.
pub fn is_fitting_base(head: BlockState, base: BlockState) -> bool {
    let sticky = head.property("type") == Some("sticky");
    let kind = if sticky {
        blocks::STICKY_PISTON
    } else {
        blocks::PISTON
    };
    is_block(base, kind) && flag(base, "extended") && facing(base) == facing(head)
}

/// `PistonHeadBlock.canSurvive`: a head stays in front of its base, or of a moving
/// piston facing its way.
pub fn head_can_survive<V: BlockView + ?Sized>(
    view: &mut V,
    head: BlockState,
    pos: BlockPos,
) -> bool {
    let Some(towards) = facing(head) else {
        return false;
    };
    let base = view.block(relative(pos, towards.opposite()));
    is_fitting_base(head, base)
        || (is_class(base, BlockClass::MovingPistonBlock) && facing(base) == Some(towards))
}
