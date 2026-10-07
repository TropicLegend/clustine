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

pub use messages::{
    Assignment, EdgeToWorker, FromCoordinator, RegionHello, RegionWelcome, StoreReply,
    StoreRequest, ToCoordinator, WorkerToEdge,
};
