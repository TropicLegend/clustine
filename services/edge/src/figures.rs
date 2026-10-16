//! What the edge counts of its light and of the packets that carry it, for the
//! measurements of `docs/adr/0023-light-at-the-edge.md`, section 9 (step L7).
//!
//! The figures are the process's and not an edge's: a process runs one edge, and
//! the edge's tests, which run many in one process, look at none of them. Counting
//! is a relaxed atomic addition where it happens; nothing waits for it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::time::MissedTickBehavior;
use tracing::info;

/// How often the figures are logged while anything moves.
const EVERY: Duration = Duration::from_secs(2);

/// What has been counted since the process started, except where said.
#[derive(Debug)]
pub(crate) struct Figures {
    /// Jobs handed to the threads that light chunks.
    pub(crate) jobs: AtomicU64,
    /// Results taken for an entry whose job it still was.
    pub(crate) taken: AtomicU64,
    /// Jobs that failed: a panic, no thread left, or given up.
    pub(crate) failed: AtomicU64,
    /// Time the threads spent lighting, in nanoseconds.
    pub(crate) thread_nanos: AtomicU64,
    /// Chunk packets written to connections.
    pub(crate) chunks: AtomicU64,
    /// Light updates written to connections, and their bytes before compression.
    pub(crate) updates: AtomicU64,
    pub(crate) update_bytes: AtomicU64,
    /// Time spent compressing chunk packets and light updates, in nanoseconds.
    pub(crate) compress_chunk_nanos: AtomicU64,
    pub(crate) compress_update_nanos: AtomicU64,
    /// The fan-out task's turns: how many, and their time in nanoseconds.
    pub(crate) turns: AtomicU64,
    pub(crate) turn_nanos: AtomicU64,
    /// The longest turn since the figures were last logged, in nanoseconds.
    pub(crate) longest_turn_nanos: AtomicU64,
    /// Chunk packets the fan-out made, and the time that took, in nanoseconds.
    pub(crate) packets_made: AtomicU64,
    pub(crate) packet_nanos: AtomicU64,
    /// The longest time from a chunk's coming to its first sending since the
    /// figures were last logged, in nanoseconds: the hold, the job and the wait
    /// for a batch.
    pub(crate) longest_unsent_nanos: AtomicU64,
    /// Players dropped because their queue was full.
    pub(crate) dropped: AtomicU64,
    /// Chunks marked to be lit again because a block light sees changed in them or
    /// beside them: a chunk lit again for a reason.
    pub(crate) relit: AtomicU64,
}

/// The process's figures.
pub(crate) static FIGURES: Figures = Figures {
    jobs: AtomicU64::new(0),
    taken: AtomicU64::new(0),
    failed: AtomicU64::new(0),
    thread_nanos: AtomicU64::new(0),
    chunks: AtomicU64::new(0),
    updates: AtomicU64::new(0),
    update_bytes: AtomicU64::new(0),
    compress_chunk_nanos: AtomicU64::new(0),
    compress_update_nanos: AtomicU64::new(0),
    turns: AtomicU64::new(0),
    turn_nanos: AtomicU64::new(0),
    longest_turn_nanos: AtomicU64::new(0),
    packets_made: AtomicU64::new(0),
    packet_nanos: AtomicU64::new(0),
    longest_unsent_nanos: AtomicU64::new(0),
    dropped: AtomicU64::new(0),
    relit: AtomicU64::new(0),
};

/// Adds one to `counter`.
pub(crate) fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Adds the time since `since` to `counter`.
pub(crate) fn add_time(counter: &AtomicU64, since: Instant) {
    counter.fetch_add(nanos(since.elapsed()), Ordering::Relaxed);
}

/// Counts a turn of the fan-out task that began at `began`.
pub(crate) fn turn(began: Instant) {
    let took = nanos(began.elapsed());
    count(&FIGURES.turns);
    FIGURES.turn_nanos.fetch_add(took, Ordering::Relaxed);
    FIGURES
        .longest_turn_nanos
        .fetch_max(took, Ordering::Relaxed);
}

/// Counts a chunk packet whose making began at `began`.
pub(crate) fn made_packet(began: Instant) {
    count(&FIGURES.packets_made);
    add_time(&FIGURES.packet_nanos, began);
}

/// Counts the first sending of a chunk that came at `came`.
pub(crate) fn sent_after(came: Instant) {
    let waited = nanos(came.elapsed());
    FIGURES
        .longest_unsent_nanos
        .fetch_max(waited, Ordering::Relaxed);
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// The figures as one line says them, totals since the start but for the longest
/// turn, which is taken back to nothing.
fn line() -> String {
    let get = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let millis = |counter: &AtomicU64| get(counter) / 1_000_000;
    let figures = &FIGURES;
    format!(
        "jobs {} taken {} failed {} thread_ms {} chunks {} updates {} update_bytes {} \
         compress_chunks_ms {} compress_updates_ms {} turns {} turn_ms {} longest_turn_us {} \
         packets_made {} packet_ms {} longest_unsent_ms {} dropped {} relit {}",
        get(&figures.jobs),
        get(&figures.taken),
        get(&figures.failed),
        millis(&figures.thread_nanos),
        get(&figures.chunks),
        get(&figures.updates),
        get(&figures.update_bytes),
        millis(&figures.compress_chunk_nanos),
        millis(&figures.compress_update_nanos),
        get(&figures.turns),
        millis(&figures.turn_nanos),
        figures.longest_turn_nanos.swap(0, Ordering::Relaxed) / 1_000,
        get(&figures.packets_made),
        millis(&figures.packet_nanos),
        figures.longest_unsent_nanos.swap(0, Ordering::Relaxed) / 1_000_000,
        get(&figures.dropped),
        get(&figures.relit),
    )
}

/// Logs the figures every [`EVERY`] while the edge turns, for as long as it runs.
pub(crate) async fn log_them() {
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut turns = 0;
    loop {
        every.tick().await;
        let now = FIGURES.turns.load(Ordering::Relaxed);
        if now != turns {
            turns = now;
            info!(figures = %line(), "light figures");
        }
    }
}
