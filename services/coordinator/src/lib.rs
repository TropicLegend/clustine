//! Coordinator service: region ownership leases, merge/split/migrate decisions, global world state.
//!
//! For now there is one coordinator, it keeps what it knows in memory, and the layout it
//! is started with stays as it is: all it decides is which worker runs which region.
//! Those decisions are made by [`Coordinator`], which does no I/O. [`serve`] is the
//! service around it, which workers reach with a [`WorkerClient`] and edges with a
//! [`RoutingWatch`].

mod client;
mod service;
mod state;

pub use client::{ClientError, HEARTBEAT_INTERVAL, Orders, RoutingWatch, WorkerClient};
pub use service::serve;
pub use state::{Changes, Coordinator, CoordinatorConfig, Refusal};

/// Messages that may wait in each direction of a connection between the coordinator and
/// a client. What they say to each other is short and rare, so a side that is this far
/// behind is not listening at all.
const QUEUE: usize = 256;
