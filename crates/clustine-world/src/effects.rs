//! What one region's tick does in land another region uses, carried to it through the
//! world store (`docs/adr/0025-effects-between-regions.md`): effects, their numbers and
//! when they are due, and the copies of the neighbours' border chunks a region reads.

use clustine_data::BlockState;
use serde::{Deserialize, Serialize};

use crate::{BlockPos, SeamKind, Ticked};

/// What a region's tick would have done to a block in a chunk it does not have loaded,
/// addressed to that block. The effect carries the call where the target's side decides
/// (the record, section 4), and is done by the rules of the region that uses the chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    /// The block the effect acts on.
    pub at: BlockPos,
    pub what: Effected,
}

/// What an [`Effect`] does to its block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effected {
    /// `neighborChanged` or `updateShape`, as a seam entry keeps it: the direction from
    /// the block to the one that changed, by `Face`'s numbers, the block that changed as
    /// it was then, the update's flags and how much deeper a chain of shape updates may
    /// go from it.
    Update {
        direction: u8,
        kind: SeamKind,
        source: BlockState,
        flags: u32,
        depth: i32,
    },
    /// `scheduleTick`. The delay counts from the tick the effect was made in; the store
    /// says when it is due in the holder's ticks ([`Due`]).
    Tick {
        of: Ticked,
        delay: i32,
        priority: i8,
    },
    /// `FlowingFluid.spreadTo` from the neighbour in `direction` (by `Face`'s numbers,
    /// from the block to the source), whose block, with its fluid, is `source`. The
    /// holder runs the target's side whole: `canSpreadTo` and `getNewLiquid`.
    Spread { direction: u8, source: BlockState },
    /// A block set by a rule (fire spreading, a rail turned towards a placed one, a
    /// plant grown into a block, a block removed): only if the block is still
    /// `expected`, and then with the set's `flags` (`Block.UPDATE_*`), which say which
    /// updates it makes: a vine grows with 2, which tells no neighbour.
    SetIf {
        expected: BlockState,
        state: BlockState,
        flags: u32,
    },
}

impl Effect {
    /// What tells two effects apart where a region keeps only one of each before it
    /// sends them (the record, section 4): an update per block, direction and kind, a
    /// scheduled tick per block and what it ticks, a spread per block and direction.
    /// A set has no key and is never deduplicated.
    pub fn key(&self) -> Option<EffectKey> {
        match self.what {
            Effected::Update {
                direction, kind, ..
            } => Some(EffectKey::Update(self.at, direction, kind)),
            Effected::Tick { of, .. } => Some(EffectKey::Tick(self.at, of)),
            Effected::Spread { direction, .. } => Some(EffectKey::Spread(self.at, direction)),
            Effected::SetIf { .. } => None,
        }
    }
}

/// See [`Effect::key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectKey {
    Update(BlockPos, u8, SeamKind),
    Tick(BlockPos, Ticked),
    Spread(BlockPos, u8),
}

/// An effect's number: the sequence of the group of commits that made it durable and
/// its place among that group's effects. Unique for the life of the world, and
/// increasing in the order the store files effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EffectId {
    pub sequence: u64,
    pub index: u32,
}

/// When an effect is due, in the ticks of the region that takes it (the record,
/// section 8): see [`FiledEffect::due`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Due {
    /// Its maker was coupled to the holder when it was made: due at the holder's
    /// `tick`, or at the tick that takes it if that is later.
    At { tick: u64 },
    /// Its maker was not coupled to the holder: the delay counts from the tick that
    /// takes it.
    FromTaking,
}

/// An effect as the store hands it to the region that uses its chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiledEffect {
    pub id: EffectId,
    pub effect: Effect,
    /// For a scheduled tick, when it is due. For every other effect, [`Due::At`] names
    /// the holder's tick that the effect was made in, which the ticks it schedules
    /// count their delays from (never earlier than the tick that takes it), so that
    /// water crosses a coupled line at its own pace (the record, section 8).
    pub due: Due,
}
