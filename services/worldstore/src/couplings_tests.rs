//! Tests of lockstep in the store (step E3c of
//! `docs/adr/0025-effects-between-regions.md`, section 8): the handshake, offsets and
//! progress on the streams, effects converted, the end of couplings, patience, and
//! couplings that outlive a restart of the store. The clock is the test's own.
//!
//! The world is the one with a gap (`regions::gap`): region 0 west of x = 0, region 1
//! from chunk x = 16 on. Region 0 copies a chunk region 1 uses, beside a chunk that
//! would tick.

use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use clustine_data::blocks;
use clustine_rpc::{CopyUse, Coupling, Uncoupling};
use clustine_world::{BlockPos, ChunkPos, Due, Effect, Effected, Ticked};

use super::*;
use crate::chunks::FileChunks;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::gap;
use crate::tests::{generator, load};

const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);
/// A chunk of region 1 that it loads, which region 0 copies.
const USED: ChunkPos = ChunkPos::new(16, 0);

/// The test's clock, which it moves itself.
#[derive(Clone, Default)]
struct Hands(Arc<Mutex<Duration>>);

impl Hands {
    fn clock(&self) -> lanes::Clock {
        let hands = self.clone();
        Arc::new(move || *hands.0.lock().unwrap())
    }

    fn forward(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

fn store_on(disk: &Arc<MemoryDisk>, hands: &Hands) -> Store {
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    start_clocked(
        disk.clone(),
        root,
        Box::new(chunks),
        generator(),
        &gap(),
        Some(hands.clock()),
    )
    .unwrap()
}

fn hello(region: RegionId, epoch: u64) -> RegionHello {
    RegionHello { region, epoch }
}

/// Waits for an answer that `wanted` picks, passing over the others, and returns what
/// came before it with it last.
fn until(handle: &StoreHandle, wanted: impl Fn(&StoreReply) -> bool) -> Vec<StoreReply> {
    let mut told = Vec::new();
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(reply) => {
                let found = wanted(&reply);
                told.push(reply);
                if found {
                    return told;
                }
            }
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("it did not come: {told:?}");
}

/// What `handle` is told before the answer to a flush.
fn flushed(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut told = until(handle, |reply| *reply == StoreReply::Flushed);
    told.pop();
    told
}

fn commit(handle: &StoreHandle, tick: u64, effects: &[Effect]) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: Vec::new(),
        state: format!("tick {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: effects.to_vec(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: Vec::new(),
    });
}

fn couples(reply: &StoreReply) -> bool {
    matches!(reply, StoreReply::CoupleWith { .. })
}

/// The two regions, opened, with the eastern one using the chunk the western one
/// has beside a chunk that would tick: both are asked to couple.
fn needing(store: &Store) -> (StoreHandle, StoreHandle) {
    let (west, _) = store.open_region(hello(WEST, 1)).unwrap();
    let (east, _) = store.open_region(hello(EAST, 1)).unwrap();
    load(&east, USED);
    west.request(StoreRequest::Beside {
        begins: vec![USED],
        ends: Vec::new(),
    });
    for (handle, partner) in [(&west, EAST), (&east, WEST)] {
        let told = until(handle, couples);
        assert_eq!(told.last(), Some(&StoreReply::CoupleWith { partner }));
    }
    (west, east)
}

/// Both name their next tick: the west 100, the east 40.
fn coupled(west: &StoreHandle, east: &StoreHandle) {
    west.request(StoreRequest::JoinsAt {
        partner: EAST,
        tick: 100,
    });
    east.request(StoreRequest::JoinsAt {
        partner: WEST,
        tick: 40,
    });
    for (handle, partner) in [(west, EAST), (east, WEST)] {
        until(
            handle,
            |reply| matches!(reply, StoreReply::Progress { partner: p, .. } if *p == partner),
        );
    }
}

#[test]
fn regions_whose_border_ticks_are_coupled_by_a_handshake_and_told_each_others_progress() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &hands);
    let (west, east) = needing(&store);
    west.request(StoreRequest::JoinsAt {
        partner: EAST,
        tick: 100,
    });
    east.request(StoreRequest::JoinsAt {
        partner: WEST,
        tick: 40,
    });
    // Both named ticks have the same border time; each is durable through the one
    // before.
    let told = until(&west, |reply| matches!(reply, StoreReply::Progress { .. }));
    assert_eq!(
        told[told.len() - 2..],
        [
            StoreReply::Coupled {
                partner: EAST,
                offset: 0
            },
            StoreReply::Progress {
                partner: EAST,
                through: 99
            },
        ]
    );
    let told = until(&east, |reply| matches!(reply, StoreReply::Progress { .. }));
    assert_eq!(
        told[told.len() - 2..],
        [
            StoreReply::Coupled {
                partner: WEST,
                offset: 60
            },
            StoreReply::Progress {
                partner: WEST,
                through: 99
            },
        ]
    );
    // The west's sand at its tick 100 falls into the east's chunk; the east is told
    // the effect, due in its own ticks, and after it the west's progress.
    let sand = Effect {
        at: BlockPos::new(16 * 16, 70, 0),
        what: Effected::Tick {
            of: Ticked::Block(blocks::SAND.0),
            delay: 2,
            priority: 0,
        },
    };
    commit(&west, 100, &[sand]);
    let told = until(&east, |reply| matches!(reply, StoreReply::Progress { .. }));
    let effects: Vec<_> = told
        .iter()
        .filter_map(|reply| match reply {
            StoreReply::Effects { effects } => Some(effects.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].due, Due::At { tick: 42 });
    assert_eq!(
        told.last(),
        Some(&StoreReply::Progress {
            partner: WEST,
            through: 100
        })
    );
}

#[test]
fn copies_are_told_whether_who_uses_their_chunk_is_coupled_to_them() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &hands);
    let (west, east) = needing(&store);
    west.request(StoreRequest::Copies {
        wanted: vec![USED],
        gone: Vec::new(),
    });
    let told = until(&west, |reply| matches!(reply, StoreReply::CopyBase { .. }));
    assert!(matches!(
        told.last(),
        Some(StoreReply::CopyBase {
            used: CopyUse::Uncoupled(EAST),
            ..
        })
    ));
    coupled(&west, &east);
    west.request(StoreRequest::Copies {
        wanted: vec![USED],
        gone: Vec::new(),
    });
    let told = until(&west, |reply| matches!(reply, StoreReply::CopyBase { .. }));
    assert!(matches!(
        told.last(),
        Some(StoreReply::CopyBase {
            used: CopyUse::Coupled(EAST),
            ..
        })
    ));
}

#[test]
fn a_coupling_nobody_needs_ends_after_a_second() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &hands);
    let (west, east) = needing(&store);
    coupled(&west, &east);
    west.request(StoreRequest::Beside {
        begins: Vec::new(),
        ends: vec![USED],
    });
    // Handled before the clock moves: the grace counts from there.
    flushed(&west);
    // Both tick on meanwhile, so that neither is stalled.
    for step in 0..10 {
        hands.forward(Duration::from_millis(99));
        commit(&west, 100 + step, &[]);
        commit(&east, 40 + step, &[]);
        let told = flushed(&west);
        assert!(
            !told
                .iter()
                .any(|reply| matches!(reply, StoreReply::Uncoupled { .. })),
            "{told:?}"
        );
    }
    hands.forward(Duration::from_millis(20));
    let why = Uncoupling::Unneeded;
    for (handle, partner) in [(&west, EAST), (&east, WEST)] {
        let told = until(handle, |reply| {
            matches!(reply, StoreReply::Uncoupled { .. })
        });
        assert_eq!(told.last(), Some(&StoreReply::Uncoupled { partner, why }));
    }
}

#[test]
fn a_side_that_does_not_name_its_tick_within_patience_is_given_up() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &hands);
    let (west, east) = needing(&store);
    west.request(StoreRequest::JoinsAt {
        partner: EAST,
        tick: 100,
    });
    flushed(&west);
    hands.forward(Duration::from_millis(250));
    for (handle, partner) in [(&west, EAST), (&east, WEST)] {
        let told = until(handle, |reply| {
            matches!(reply, StoreReply::HandshakeOff { .. })
        });
        assert_eq!(told.last(), Some(&StoreReply::HandshakeOff { partner }));
    }
    drop(east);
    let (_east, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert!(restored.given_up);
}

#[test]
fn a_region_whose_handle_closes_is_uncoupled_at_once() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &hands);
    let (west, east) = needing(&store);
    coupled(&west, &east);
    drop(east);
    let told = until(&west, |reply| matches!(reply, StoreReply::Uncoupled { .. }));
    assert_eq!(
        told.last(),
        Some(&StoreReply::Uncoupled {
            partner: EAST,
            why: Uncoupling::OwnerLost { region: EAST }
        })
    );
}

#[test]
fn a_region_preparing_to_reshape_is_uncoupled_and_not_asked_to_couple() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &hands);
    let (west, east) = needing(&store);
    coupled(&west, &east);
    east.request(StoreRequest::EndCouplings);
    let why = Uncoupling::Reshaping { region: EAST };
    for (handle, partner) in [(&west, EAST), (&east, WEST)] {
        let told = until(handle, |reply| {
            matches!(reply, StoreReply::Uncoupled { .. })
        });
        assert_eq!(told.last(), Some(&StoreReply::Uncoupled { partner, why }));
    }
    // The need is there still, and nobody is asked to couple.
    hands.forward(Duration::from_millis(100));
    for handle in [&west, &east] {
        assert!(!flushed(handle).iter().any(couples));
    }
}

#[test]
fn couplings_outlive_a_restart_of_the_store() {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &hands);
        let (west, east) = needing(&store);
        coupled(&west, &east);
        commit(&west, 100, &[]);
        commit(&west, 101, &[]);
        commit(&east, 40, &[]);
        flushed(&west);
        flushed(&east);
    }
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &hands);
    let (_west, restored) = store.open_region(hello(WEST, 2)).unwrap();
    assert_eq!(restored.offset, Some(0));
    assert_eq!(
        restored.couplings,
        [Coupling {
            partner: EAST,
            through: Some(100)
        }]
    );
    let (_east, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(restored.offset, Some(60));
    assert_eq!(
        restored.couplings,
        [Coupling {
            partner: WEST,
            through: Some(101)
        }]
    );
    assert!(!restored.given_up);
}

/// What a restore after a crash costs a coupled region (ADR-0025, E7, "What it costs"):
/// coupled regions commit on every tick, so a region that crashes just before its next
/// checkpoint is restored with as many commits as there are ticks between two, about
/// 6,000. Two regions commit that many ticks each, a block changed and a state of a
/// few dozen bytes each tick, as the pair of `services/worker/tests/border_costs.rs`
/// did; the store then ends without a checkpoint, as at a crash after the last sync,
/// and a new store on the same files opens both. The files go under `TMPDIR`:
///
/// ```text
/// cargo test --release -p clustine-worldstore --lib -- --ignored --nocapture a_crash_beside
/// ```
#[test]
#[ignore = "a measurement, for a release build on the disk the server runs on"]
fn what_a_restore_costs_a_coupled_region_after_a_crash_beside_its_checkpoint() {
    use crate::disk::OsDisk;

    const TICKS: u64 = 6_000;
    let opened = |directory: &Path| {
        let disk = Arc::new(OsDisk::default());
        let chunks = FileChunks::new(disk.clone(), directory);
        start(disk, directory, Box::new(chunks), generator(), &gap()).unwrap()
    };
    for _ in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let committing = Instant::now();
        {
            let store = opened(directory.path());
            let west = store.open_region(hello(WEST, 1)).unwrap().0;
            let east = store.open_region(hello(EAST, 1)).unwrap().0;
            for tick in 1..=TICKS {
                for (handle, x) in [(&west, -40), (&east, 16 * 16 + 3)] {
                    let block = BlockPos::new(x, 100, (tick % 16) as i32);
                    let state = if tick % 2 == 0 {
                        blocks::GLASS
                    } else {
                        blocks::STONE
                    };
                    handle.request(StoreRequest::Commit {
                        tick,
                        changes: vec![(block, state).into()],
                        state: format!("the state of the region at tick {tick:>8}").into_bytes(),
                        stays: Vec::new(),
                        ticks: Vec::new(),
                        marks_taken: Vec::new(),
                        seams: Vec::new(),
                        effects: Vec::new(),
                        taken: Vec::new(),
                        events: Vec::new(),
                        entity_changes: Vec::new(),
                    });
                }
            }
            crate::tests::committed(&west, TICKS);
            crate::tests::committed(&east, TICKS);
        }
        let committed_in = committing.elapsed();
        let starting = Instant::now();
        let store = opened(directory.path());
        let started_in = starting.elapsed();
        let mut opens = Vec::new();
        for region in [WEST, EAST] {
            let opening = Instant::now();
            let (_, restored) = store.open_region(hello(region, 2)).unwrap();
            opens.push(format!(
                "{:.3} s with {} commits",
                opening.elapsed().as_secs_f64(),
                restored.deltas.len()
            ));
        }
        println!(
            "MEASURED: two regions committed {TICKS} ticks each in {:.1} s; the store \
             started again in {:.3} s and opened them in {}",
            committed_in.as_secs_f64(),
            started_in.as_secs_f64(),
            opens.join(" and ")
        );
    }
}
