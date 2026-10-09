//! Coordinator service: region ownership leases, merge/split/migrate decisions, global world state.
//!
//! For now there is one coordinator and it keeps what it knows in memory. It decides
//! which worker runs which region, and has regions merged and split when somebody asks
//! for that; which regions there are it learns from the world store's list of them,
//! and it knows none until it has read that list.
//! Those decisions are made by [`Coordinator`], which does no I/O. [`serve`] is the
//! service around it, which workers reach with a [`WorkerClient`], edges with a
//! [`RoutingWatch`], whoever wants a region moved with a [`Mover`], and whoever wants
//! regions merged or one split with an [`Asker`]. [`serve_local`] is the same service
//! for clients in its own process, which reach it through a [`Reach`] without a
//! socket, around a coordinator that is alone with its workers
//! ([`Coordinator::alone`]).

mod client;
mod policy;
mod service;
mod state;

pub use client::{
    Asker, ClientError, HEARTBEAT_INTERVAL, LocalCoordinator, MoveAnswer, Mover, Orders, Reach,
    RoutingWatch, WorkerClient, WorkerEvent,
};
pub use policy::{Policy, Sighted, Wanted, Why, decide, named};
#[doc(hidden)]
pub use service::serve_with;
pub use service::{serve, serve_local};
pub use state::{
    Asked, Changes, Coordinator, CoordinatorConfig, MoveBegun, MoveOutcome, MoveRefusal, Order,
    Refusal, ReleaseOrder, ReshapeOrder, ReshapeRefusal, Reshaped, Undone,
};

/// Messages that may wait in each direction of a connection between the coordinator and
/// a client. What they say to each other is short and rare, so a side that is this far
/// behind is not listening at all.
const QUEUE: usize = 256;
