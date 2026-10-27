//! Inter-service API: control-plane messages and per-tick stream framing.
//!
//! The services talk to each other only through the messages defined here, over
//! [`link`]s. Within one process a link is a pair of channels; between processes the
//! same messages are serialised. Both kinds exist from the start so that nothing
//! unserialisable can creep into the interface.

pub mod link;
mod messages;
pub mod tcp;
pub mod wire;

// What the messages to the world store carry of a stay, for the store, which takes
// the messages from here and needs nothing else of the simulation.
pub use clustine_sim::api::{Experience, ItemStack, Pose, Respawn, Sleep, WorldClock};
pub use clustine_sim::{
    CountsReport, PoiSeen, PublishedCounts, PublishedVillage, SPAWN_CATEGORIES, SpawnCounts,
    VillageReport,
};
pub use clustine_sim::{FaceChange, FaceSeq, Place, PlayerFace, StayNote, Unit, WorldPlayer};
pub use messages::{
    Assignment, ChunkBox, CopyUse, Coupling, Crowds, Decline, EdgeMessage, EdgeToWorker,
    FromCoordinator, Lends, Off, Parts, PlayersOf, Presence, REDSTONE_REPORTED, RegionHello,
    RegionInfo, RegionList, RegionWelcome, Restored, RestoredItem, RestoredPart, RestoredPiece,
    SplitPart, StoreHello, StoreReply, StoreRequest, StoreWelcome, TickState, ToCoordinator,
    Uncoupling, Vouch, Welcome, WorkerToEdge, held_bytes, held_from_bytes, in_use_bytes,
    in_use_from_bytes,
};
