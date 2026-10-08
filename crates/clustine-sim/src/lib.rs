//! Region simulation: the tick loop and game mechanics.
//!
//! A [`Region`] is a part of the world that is simulated as one unit. Its only entry
//! point is [`Region::tick`], which takes everything that happened since the last tick
//! as [`TickInputs`] and returns what resulted as a [`TickOutput`]. Apart from its
//! chunks, a region is its [`RegionState`], from which it can be rebuilt with
//! [`Region::restore`]; each tick says what changed of it as a [`StateDelta`].
//!
//! Nothing in this crate performs I/O, reads a clock or uses unordered collections, so
//! that a tick is a function of the region's state and its inputs. `clippy.toml` in this
//! crate enforces part of that.

pub mod api;
mod region;
mod state;

pub use api::{
    Durable, EdgeEvent, PlayerChange, PlayerEvent, PlayerJoin, PlayerTransfer, RemoteAction,
    RemoteStep, TickInputs, TickOutput,
};
pub use region::{Region, RegionConfig};
pub use state::{EdgeDelta, EdgeState, PlayerState, RegionState, StateDelta};
