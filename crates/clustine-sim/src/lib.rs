//! Region simulation: the tick loop and game mechanics.
//!
//! A [`Region`] is a part of the world that is simulated as one unit. Its only entry
//! point is [`Region::tick`], which takes everything that happened since the last tick
//! as [`TickInputs`] and returns what resulted as a [`TickOutput`].
//!
//! Nothing in this crate performs I/O, reads a clock or uses unordered collections, so
//! that a tick is a function of the region's state and its inputs. `clippy.toml` in this
//! crate enforces part of that.

pub mod api;
mod region;

pub use api::{
    PlayerChange, PlayerEvent, PlayerJoin, PlayerTransfer, RemoteAction, RemoteOutcome, RemoteStep,
    TickInputs, TickOutput,
};
pub use region::{Region, RegionConfig};
