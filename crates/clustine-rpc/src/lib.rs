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
pub use clustine_sim::api::{ItemStack, Pose};
pub use clustine_sim::{Place, StayNote};
pub use messages::{
    Assignment, ChunkBox, Crowds, Decline, EdgeMessage, EdgeToWorker, FromCoordinator, Off,
    PlayersOf, Presence, RegionHello, RegionInfo, RegionList, RegionWelcome, Restored,
    RestoredItem, RestoredPart, RestoredPiece, SplitPart, StoreHello, StoreReply, StoreRequest,
    StoreWelcome, TickState, ToCoordinator, Vouch, Welcome, WorkerToEdge, held_bytes,
    held_from_bytes,
};
