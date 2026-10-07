//! Coordinator service: region ownership leases, merge/split/migrate decisions, global world state.
//!
//! For now there is one coordinator, it keeps what it knows in memory, and the layout it
//! is started with stays as it is: all it decides is which worker runs which region.
//! Those decisions are made by [`Coordinator`], which does no I/O; the service that
//! speaks to workers and edges is built around it.

mod state;

pub use state::{Changes, Coordinator, CoordinatorConfig, Refusal};
