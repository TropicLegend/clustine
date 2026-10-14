//! The scheduler of `docs/adr/0021-generation-is-a-function.md`, section 5: it makes a
//! chunk from [`Stages`] on a pool of threads of its own, with a cache of what the
//! stages gave, and implements [`ChunkGenerator`] for the store.
//!
//! What is public here is fixed for W5a: the scheduler's tests are written against it
//! by someone who does not write the scheduler.

mod graph;
mod lattice;
mod output;
mod pool;
mod view;

#[cfg(test)]
mod tests;

pub use self::pool::Scheduler;

/// Which earlier runs a run sees (ADR-0021, section 2, "B as a switch").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// The runs of lower rank within 2: the order that is claimed to be exact.
    #[default]
    Nine,
    /// None: every run sees terrain alone (shape B, for measuring).
    Alone,
    /// No runs at all, and no starts: a finished chunk is its terrain. Only for stages
    /// that start and place nothing, such as the preview of bare land, which would
    /// otherwise make the terrain of 289 chunks to light one.
    Bare,
}

/// Which ready task a free thread takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pick {
    /// The ready task of the oldest request; what only the look-ahead wants after
    /// everything a request wants.
    #[default]
    Oldest,
    /// A ready task chosen by a random source seeded with this, for the test that
    /// the order of work never shows in a result.
    Shuffled(u64),
}

/// How a scheduler is made.
#[derive(Debug, Clone)]
pub struct Options {
    /// Threads of the pool, at least one.
    pub threads: usize,
    /// The cache's bound in bytes; `None` for no bound.
    pub cache_bytes: Option<usize>,
    pub order: Order,
    /// Whether the eight neighbours of every chunk asked for are lit ahead.
    pub look_ahead: bool,
    pub pick: Pick,
}

impl Options {
    /// What a server runs with: two thirds of `processors` (at least one thread),
    /// 512 MiB of cache, the exact order, look-ahead, the oldest request first.
    pub fn for_processors(processors: usize) -> Self {
        Self {
            threads: (processors * 2 / 3).max(1),
            cache_bytes: Some(512 * 1024 * 1024),
            order: Order::Nine,
            look_ahead: true,
            pick: Pick::Oldest,
        }
    }
}

/// What the recorder of a run counts (ADR-0021, section 3), added up over runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Recorded {
    /// Live reads in the ring around the three by three, answered from terrain.
    pub far_reads: u64,
    /// Reads beyond the five by five, answered from terrain fetched for them.
    pub beyond_margin: u64,
    /// Writes of blocks or block entities outside the three by three.
    pub refused_writes: u64,
    /// Ticks and marks outside the three by three.
    pub dropped: u64,
    /// Reads of the state of a piece whose box meets no chunk within 2 of the centre.
    pub far_piece_reads: u64,
    /// States set of a piece whose box spans chunks more than 2 apart.
    pub far_pieces: u64,
}

/// What a scheduler has done since it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Figures {
    /// Tasks run to their end, by kind. A task made again after its result was
    /// dropped from the cache counts again.
    pub starts: u64,
    pub terrain: u64,
    pub runs: u64,
    pub finished: u64,
    pub lit: u64,
    /// Runs made again after their result was dropped from the cache.
    pub runs_made_again: u64,
    /// Placements that failed and tasks that failed.
    pub failed_placements: u64,
    pub failed_tasks: u64,
    pub recorded: Recorded,
    /// Tasks that are wanted and have not started, and tasks running now.
    pub queued: u64,
    pub running: u64,
    /// What the cache holds now, in bytes as its entries reckon them.
    pub cache_bytes: u64,
}
