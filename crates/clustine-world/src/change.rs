//! A change a tick made to a block or to what the block keeps, as commits, the log,
//! copies and the edge carry them (`docs/adr/0026-redstone.md`, section 6).

use clustine_data::BlockState;
use serde::{Deserialize, Serialize};

use crate::{BlockPos, Chunk, DataRefused};

/// One change, of a list in the order a tick made them. Applied in that order, as the
/// chunk's own [`Chunk::set`] does, a list ends in the same chunk however many of its
/// changes the chunk had already: each sets something to a definite state, and the
/// last one for a position wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// The block at the position is now this one; its block entity follows it as
    /// [`Chunk::set`] says.
    Block(BlockPos, BlockState),
    /// The block entity at the position, of the kind its block keeps, now has this
    /// data, the bytes of one NBT compound as [`crate::BlockEntity`] keeps them. Empty
    /// data where the game makes a fresh block entity for a block of the same kind.
    BlockEntity(BlockPos, Vec<u8>),
}

impl Change {
    /// Where the change is.
    pub fn position(&self) -> BlockPos {
        match self {
            Self::Block(position, _) | Self::BlockEntity(position, _) => *position,
        }
    }

    /// The block the change sets, if it sets one.
    pub fn block(&self) -> Option<(BlockPos, BlockState)> {
        match self {
            Self::Block(position, state) => Some((*position, *state)),
            Self::BlockEntity(..) => None,
        }
    }
}

impl From<(BlockPos, BlockState)> for Change {
    fn from((position, state): (BlockPos, BlockState)) -> Self {
        Self::Block(position, state)
    }
}

impl Chunk {
    /// Applies `change`, which lies in this chunk. A block outside the chunk's heights
    /// is passed over, as [`Chunk::set`] passes it; data the chunk refuses is refused
    /// to the caller, which keeps the block entity without it, as whoever brings data
    /// in does.
    pub fn apply(&mut self, change: &Change) -> Result<(), DataRefused> {
        let (x, z) = change.position().in_chunk();
        match change {
            Change::Block(position, state) => {
                self.set(x, position.y, z, *state);
                Ok(())
            }
            Change::BlockEntity(position, data) => {
                self.set_block_entity_data(crate::InChunk::new(x, position.y, z), data.clone())
            }
        }
    }
}
