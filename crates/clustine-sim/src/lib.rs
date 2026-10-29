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
pub mod generation;
mod items;
mod living;
pub(crate) mod menus;
mod mobs;
#[doc(hidden)]
pub mod path_harness;
mod physics;
pub mod placement;
mod region;
pub mod shown;
mod spawning;
mod state;
pub mod time;
mod trading;

pub use api::{
    CountsReport, PoiSeen, PublishedCounts, PublishedVillage, RaidSeen, SPAWN_CATEGORIES,
    SpawnCounts, VillageReport,
};
pub use api::{
    Durable, EdgeEvent, Entered, FaceChange, FaceSeq, Keys, LendAnswer, LendAsked, Misdirected,
    Place, PlayerChange, PlayerEvent, PlayerFace, PlayerJoin, PlayerTransfer, RemoteAction,
    RemoteStep, StayNote, TickInputs, TickOutput, Ticket, Unit, VehicleMove, WorldClock,
    WorldPlayer,
};
/// What the edge needs to show a cloud's radius and a potion's colour (part S12b).
pub use region::clouds::{arrow_colour, potion_colour, radius_at as cloud_radius_at};
pub use region::{
    Features, Holdings, KeptLight, Knowledge, LightChecks, LightSettings, NoSplit, Part,
    RANDOM_TICK_SPEED, Region, RegionConfig, Sides, Splitting,
};
pub use state::{
    Drag, DragKind, EdgeDelta, EdgeState, EnteringState, Menu, MenuOf, PlayerRiding, PlayerState,
    RegionState, StateDelta, VehicleCheck,
};
