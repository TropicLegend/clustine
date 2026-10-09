//! Region simulation: the tick loop and game mechanics.
//!
//! A [`Region`] is a part of the world that is simulated as one unit. Its only entry
//! point is [`Region::tick`], which takes everything that happened since the last tick
//! as [`TickInputs`] and returns what resulted as a [`TickOutput`]. Apart from its
//! chunks, a region is its [`RegionState`], from which it can be rebuilt with
//! [`Region::restore`]; each tick says what changed of it as a [`StateDelta`].
//!
//! Which chunks a region simulates is not its own to say: the world store grants them.
//! A region claims what it wants through [`TickOutput::claims`], is answered through
//! [`TickInputs::granted`] and [`TickInputs::foreign`], and gives back what nothing uses
//! through [`TickOutput::returns`]; [`Region::knowledge`] tells what it knows of a
//! chunk. See `docs/adr/0012-the-tick-on-chunks.md`.
//!
//! Two things change a region besides its tick, each in the place of one: it takes in
//! another region ([`Region::absorb`], [`Region::take_absorbed`]), or a part of it
//! becomes a region of its own ([`Region::split`], [`Region::take_split`]). See
//! `docs/adr/0014-merging-and-splitting.md`, section 2.
//!
//! Nothing in this crate performs I/O, reads a clock or uses unordered collections, so
//! that a tick is a function of the region's state and its inputs. `clippy.toml` in this
//! crate enforces part of that.

pub mod api;
mod region;
mod state;

pub use api::{
    Durable, EdgeEvent, Misdirected, PlayerChange, PlayerEvent, PlayerJoin, PlayerTransfer,
    RemoteAction, RemoteStep, TickInputs, TickOutput, Ticket,
};
pub use region::{Holdings, Knowledge, NoSplit, Part, Region, RegionConfig, Sides, Splitting};
pub use state::{EdgeDelta, EdgeState, PlayerState, RegionState, StateDelta};
