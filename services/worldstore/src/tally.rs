//! A tally of what the store answers its regions, by the kind of answer: how many and
//! how many bytes they take on the wire, for the measurements of N7f
//! (`docs/groundwork/n7-plan.md`, section 9.2 item 5: the faces' share of the stream,
//! and the copies' updates). It is kept only where the process is started with
//! `CLUSTINE_STREAM_TALLY` set, as encoding every answer once more costs a store that
//! is not measured for nothing; it is then said in the log every ten seconds, as what
//! was answered since the last time.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use clustine_rpc::StoreReply;
use clustine_rpc::wire;
use tracing::info;

/// How often the tally is said and begun again.
const EVERY: Duration = Duration::from_secs(10);

/// The answers since the tally was last said, by kind: their count and bytes.
struct Tally {
    since: Instant,
    kinds: BTreeMap<&'static str, (u64, u64)>,
}

/// The tally, where it is kept.
fn tally() -> Option<&'static Mutex<Tally>> {
    static TALLY: OnceLock<Option<Mutex<Tally>>> = OnceLock::new();
    TALLY
        .get_or_init(|| {
            std::env::var_os("CLUSTINE_STREAM_TALLY").map(|_| {
                Mutex::new(Tally {
                    since: Instant::now(),
                    kinds: BTreeMap::new(),
                })
            })
        })
        .as_ref()
}

/// A writer that keeps nothing and counts what it is given.
struct Counted(u64);

impl Write for Counted {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Counts `reply`, which is being answered, where the tally is kept, and says the tally
/// when its ten seconds are over.
pub(crate) fn count(reply: &StoreReply) {
    let Some(tally) = tally() else {
        return;
    };
    let mut counted = Counted(0);
    let _ = wire::blocking::write(&mut counted, reply);
    let kind = kind_of(reply);
    let Ok(mut tally) = tally.lock() else {
        return;
    };
    let entry = tally.kinds.entry(kind).or_default();
    entry.0 += 1;
    entry.1 += counted.0;
    if tally.since.elapsed() >= EVERY {
        let seconds = tally.since.elapsed().as_secs_f64();
        let total: u64 = tally.kinds.values().map(|(_, bytes)| bytes).sum();
        let said: Vec<String> = tally
            .kinds
            .iter()
            .map(|(kind, (count, bytes))| format!("{kind} {count} in {bytes} B"))
            .collect();
        info!(
            seconds = format!("{seconds:.1}"),
            total,
            "the store answered: {}",
            said.join(", ")
        );
        tally.kinds.clear();
        tally.since = Instant::now();
    }
}

/// The name of `reply`'s kind, as its `Debug` begins.
fn kind_of(reply: &StoreReply) -> &'static str {
    let debug = format!("{reply:?}");
    let name = debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default();
    // The kinds are few: each name is kept once for the life of the process.
    static NAMES: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let names = NAMES.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut names) = names.lock() else {
        return "unknown";
    };
    if let Some(known) = names.iter().find(|known| **known == name) {
        return known;
    }
    let kept: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(kept);
    kept
}
