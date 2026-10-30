//! What crosses from one dimension to another, or far within one, with a traveller
//! (`docs/groundwork/p-plan.md`, sections 5 and 15.2).

use serde::{Deserialize, Serialize};

use crate::entity::Double;
use crate::{BlockPos, ChunkPos, Dimension, Entity};

/// A chunk of one of the world's dimensions: what the store and the edge, which see
/// all three, key chunks by. A region holds chunks of one dimension, and names its own
/// by their `ChunkPos` alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LevelChunk {
    pub dimension: Dimension,
    pub position: ChunkPos,
}

impl LevelChunk {
    pub const fn new(dimension: Dimension, position: ChunkPos) -> Self {
        Self {
            dimension,
            position,
        }
    }
}

/// Where a traveller goes, committed in the tick its portal fired: the destination
/// places it by this, finding or making a portal there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Travel {
    pub to: Dimension,
    /// Where the game aims before it looks for a place: a Nether portal's scaled point
    /// inside the destination's border (`docs/groundwork/p-brief.md`, section 2.2, item
    /// 2), the End's spawn point, a gateway's exit.
    pub target: BlockPos,
    pub way: Way,
}

/// How a traveller goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Way {
    /// Through a Nether portal: its axis (none for a portal block without one, which
    /// the game takes as X) and the traveller's place in its rectangle, along, up and
    /// across (`PortalShape.getRelativePosition`; the brief, section 2.2, item 6).
    NetherPortal {
        axis: Option<PortalAxis>,
        relative: [Double; 3],
    },
    /// Through the End's portal: into the End, or out of it to the respawn.
    EndPortal,
    /// Through a gateway, within the End: the gateway gone through, and whether its exit
    /// is exact, which the game writes back to the gateway with the exit.
    Gateway { from: BlockPos, exact: bool },
    /// The respawn after the credits (the plan, section 5.3).
    Credits,
}

/// The horizontal axis a Nether portal lies along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortalAxis {
    X,
    Z,
}

/// An entity other than a player that a portal sent on, with its riders that are not
/// players, root first, filed for the chunk it is aimed at (the plan, sections 5.1 and
/// 5.2). The destination takes it once, in one commit with the portal it makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traveller {
    pub travel: Travel,
    pub chunk: LevelChunk,
    pub entities: Vec<Entity>,
}

/// Which kind of portal a portal clock counts for (`Portal` in the game).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortalKind {
    Nether,
    End,
    Gateway,
}

/// The game's `PortalProcessor`: the portal an entity stands in, where it entered,
/// how long it has stood there and whether it was in it this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalClock {
    pub kind: PortalKind,
    pub entry: BlockPos,
    /// `portalTime`.
    pub time: u32,
    /// `insidePortalThisTick`.
    pub inside: bool,
}

/// An entity's portal clock and its cooldown (`portalCooldown`), which the game keeps
/// for every entity (the brief, section 2.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalState {
    pub clock: Option<PortalClock>,
    pub cooldown: u32,
}

impl PortalState {
    /// Whether there is nothing to keep: no clock and no cooldown, as for every entity
    /// that never stood in a portal.
    pub fn is_idle(&self) -> bool {
        self.clock.is_none() && self.cooldown == 0
    }
}
