//! What the store waits for the disk for, operation by operation: every sync it makes,
//! in order, on which thread, and how many of them come before the answer that whoever
//! asked waits for.
//!
//! A sync is what an operation costs on a disk that takes its time for one: a round of
//! syncs that wait together costs about as much as a single one, and syncs in turn
//! each cost their own. So these tests count waits, where a round is one, and they
//! are the measure that `docs/groundwork/disk-syncs-measured.md` gives and that
//! `docs/groundwork/few-syncs-in-turn-draft.md` sets out to lower. A change that
//! makes the store wait less often changes what they expect, and says so there.
//!
//! The disk of these tests holds every wait back until the test lets it through, one
//! at a time, and the test looks for the answer each time the store has come to its
//! next wait: everything the store did before that wait is done then, also sending an
//! answer. So nothing here waits for time to pass.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use clustine_data::blocks;
use clustine_rpc::SplitPart;
use clustine_world::{Chunk, ChunkPos};

use super::*;
use crate::regions::{gap, hello_of, whole};
use crate::tests::{committed, generator, log};

const ROOT: &str = "/world";

/// A disk in memory that notes every wait for the disk and can hold each back until
/// it is let through.
#[derive(Default)]
struct Counting {
    disk: MemoryDisk,
    gate: Mutex<Gate>,
    moved: Condvar,
}

#[derive(Default)]
struct Gate {
    /// Whether waits are held back.
    held: bool,
    /// How many waits have come to the gate since it was last held.
    arrived: u64,
    /// How many of them were let through.
    let_through: u64,
    /// Whether the store is at rest with what it was asked while the gate was held.
    rested: bool,
    /// The waits that were let through, in that order.
    waits: Vec<String>,
}

impl Counting {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Notes a wait for the disk, and holds it back if the gate is held.
    fn wait(&self, what: &str) {
        let thread = thread::current();
        let by = match thread.name() {
            Some("worldstore") => "commit thread",
            Some("worldstore-chunks") => "chunk thread",
            _ => "another thread",
        };
        let mut gate = self.gate();
        if gate.held {
            gate.arrived += 1;
            let mine = gate.arrived;
            self.moved.notify_all();
            while gate.held && gate.let_through < mine {
                gate = self
                    .moved
                    .wait(gate)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        gate.waits.push(format!("{by}: {what}"));
    }
}

/// What a file that is synced by itself is.
fn file(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if path.starts_with(Path::new(ROOT).join("log")) {
        "the log's segment"
    } else if name.contains(".state.") {
        "a state file's temporary"
    } else if name.ends_with(".region.tmp") {
        "a region file's temporary"
    } else if name == "table.tmp" {
        "the table file's temporary"
    } else if name == "players.tmp" {
        "the players' file's temporary"
    } else {
        "another file"
    }
}

fn directory(path: &Path) -> &'static str {
    if path == Path::new(ROOT).join("log") {
        "log/"
    } else if path == Path::new(ROOT).join("regions") {
        "regions/"
    } else {
        "another directory"
    }
}

/// What a round of syncs that wait together is of.
fn round(paths: &[PathBuf], files: bool) -> String {
    let sections = paths
        .first()
        .is_some_and(|path| path.starts_with(Path::new(ROOT).join("blobs")));
    let of = match (sections, files) {
        (true, true) => "section files",
        (true, false) => "directories of sections",
        (false, true) => "manifests",
        (false, false) => "directories of manifests",
    };
    format!("{of} together ({})", paths.len())
}

impl Disk for Counting {
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        self.disk.read(path)
    }
    fn read_at(&self, path: &Path, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        self.disk.read_at(path, offset, length)
    }
    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.disk.exists(path)
    }
    fn list(&self, directory: &Path) -> io::Result<Vec<String>> {
        self.disk.list(directory)
    }
    fn create_dir_all(&self, directory: &Path) -> io::Result<()> {
        self.disk.create_dir_all(directory)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.disk.write(path, contents)
    }
    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.disk.append(path, contents)
    }
    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        self.disk.truncate(path, length)
    }
    fn sync(&self, path: &Path) -> io::Result<()> {
        self.wait(file(path));
        self.disk.sync(path)
    }
    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        self.wait(directory(path));
        self.disk.sync_directory(path)
    }
    fn sync_files(&self, files: &[PathBuf]) -> io::Result<()> {
        // A round of nothing waits for nothing, and a round is one wait: its syncs
        // wait at the same time on the local file system.
        if files.is_empty() {
            return Ok(());
        }
        self.wait(&round(files, true));
        self.disk.sync_files(files)
    }
    fn sync_directories(&self, directories: &[PathBuf]) -> io::Result<()> {
        if directories.is_empty() {
            return Ok(());
        }
        self.wait(&round(directories, false));
        self.disk.sync_directories(directories)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.disk.rename(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.disk.remove(path)
    }
}

/// The waits of one operation, and how many of them the store made before it sent the
/// answer; `None` if the operation has no answer.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    waits: Vec<String>,
    before_the_answer: Option<usize>,
}

fn seen(waits: &[&str], before_the_answer: Option<usize>) -> Seen {
    Seen {
        waits: waits.iter().map(|wait| (*wait).to_owned()).collect(),
        before_the_answer,
    }
}

/// A world with a gap (`regions::gap`): region 0 is pinned west of x = 0, region 1
/// from x = 16 on, and region 2 is home and holds the origin. Its chunks are in files
/// on the same disk.
struct World {
    disk: Arc<Counting>,
    store: Store,
}

impl World {
    fn new() -> Self {
        let disk = Arc::new(Counting::default());
        let chunks = FileChunks::new(disk.clone(), Path::new(ROOT));
        let store = start(
            disk.clone(),
            Path::new(ROOT),
            Box::new(chunks),
            generator(),
            &gap(),
        )
        .unwrap();
        Self { disk, store }
    }

    /// Does `ask`, lets the waits the store makes for it through one at a time until
    /// the store is at rest, and returns them with what `answer` found first and how
    /// many waits the store had made by then. `answer` must not block.
    fn watch<T>(
        &self,
        name: &str,
        ask: impl FnOnce(),
        mut answer: impl FnMut() -> Option<T>,
    ) -> (Seen, Option<T>) {
        // At rest before, so that every wait from here on is of what is asked now.
        self.store.flush().unwrap();
        {
            let mut gate = self.disk.gate();
            *gate = Gate {
                held: true,
                ..Gate::default()
            };
        }
        ask();
        let barrier = self.store.barrier();
        let disk = Arc::clone(&self.disk);
        let resting = thread::spawn(move || {
            let rested = Store::rested(&barrier);
            disk.gate().rested = true;
            disk.moved.notify_all();
            rested
        });
        let mut answered = None;
        let mut before_the_answer = None;
        loop {
            let (at_rest, count) = {
                let mut gate = self.disk.gate();
                while gate.arrived == gate.let_through && !gate.rested {
                    gate = self
                        .disk
                        .moved
                        .wait(gate)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                (gate.arrived == gate.let_through, gate.let_through as usize)
            };
            // The store has come to its next wait, or to rest: whatever it sends
            // before that it has sent.
            if answered.is_none() {
                answered = answer();
                if answered.is_some() {
                    before_the_answer = Some(count);
                }
            }
            if at_rest {
                break;
            }
            self.disk.gate().let_through += 1;
            self.disk.moved.notify_all();
        }
        resting.join().unwrap().unwrap();
        let waits = {
            let mut gate = self.disk.gate();
            gate.held = false;
            std::mem::take(&mut gate.waits)
        };
        println!("{name}:");
        for (index, wait) in waits.iter().enumerate() {
            let answered = match before_the_answer {
                Some(before) if index + 1 == before => "   <- then the answer",
                _ => "",
            };
            println!("  {}. {wait}{answered}", index + 1);
        }
        if before_the_answer == Some(0) {
            println!("  (answered before any of them)");
        }
        // How many files a round has is up to the test's chunks and their hashes.
        let plain = |wait: String| match wait.split_once(" (") {
            Some((plain, _)) => plain.to_owned(),
            None => wait,
        };
        let seen = Seen {
            waits: waits.into_iter().map(plain).collect(),
            before_the_answer,
        };
        (seen, answered)
    }

    /// As [`World::watch`], for what is asked through `handle` and answered with a
    /// reply that `is_answer` says is the one.
    fn watch_reply(
        &self,
        name: &str,
        handle: &StoreHandle,
        ask: impl FnOnce(),
        is_answer: impl Fn(&StoreReply) -> bool,
    ) -> Seen {
        let answer = || {
            while let Some(reply) = handle.try_reply() {
                if is_answer(&reply) {
                    return Some(());
                }
            }
            None
        };
        self.watch(name, ask, answer).0
    }

    /// Says hello and watches the store open the region.
    fn watch_open(&self, name: &str, hello: RegionHello) -> (Seen, StoreHandle, Restored) {
        let (answer, answered): (_, Receiver<Result<(Opened, Restored), StoreError>>) =
            mpsc::channel();
        let messages = self.store.messages.clone();
        let ask = || {
            let _ = messages.send(Message::Open {
                hello,
                reply_to: messages.clone(),
                answer,
            });
        };
        let (seen, opened) = self.watch(name, ask, || answered.try_recv().ok());
        let (opened, restored) = opened.expect("the hello is answered").unwrap();
        let handle = StoreHandle::local(opened, self.store.messages.clone());
        (seen, handle, restored)
    }

    fn open(&self, region: u32, epoch: u64) -> StoreHandle {
        let hello = hello_of(&gap(), region, epoch);
        self.store.open_region(hello).unwrap().0
    }

    /// A checkpoint as a runner asks for it: the changed chunks, the state, a flush.
    fn watch_checkpoint(
        &self,
        name: &str,
        handle: &StoreHandle,
        tick: u64,
        chunks: &[ChunkPos],
    ) -> Seen {
        let ask = || {
            for (index, position) in chunks.iter().enumerate() {
                handle.request(StoreRequest::Save {
                    position: *position,
                    tick,
                    chunk: changed(*position, tick, index),
                });
            }
            handle.request(StoreRequest::Checkpoint {
                tick,
                state: whole("state", tick),
            });
            handle.request(StoreRequest::Flush);
        };
        self.watch_reply(name, handle, ask, |reply| *reply == StoreReply::Flushed)
    }

    /// A commit of `tick` that changes nothing in a chunk.
    fn watch_commit(&self, name: &str, handle: &StoreHandle, tick: u64) -> Seen {
        let ask = || log(handle, tick, &[]);
        let committed = move |reply: &StoreReply| *reply == StoreReply::Committed { tick };
        self.watch_reply(name, handle, ask, committed)
    }
}

/// The chunk at `position` with a section that no other chunk and no earlier tick has.
fn changed(position: ChunkPos, tick: u64, index: usize) -> Chunk {
    let mut chunk = generator().generate(position);
    chunk.set(index % 16, 100, (tick % 16) as usize, blocks::GLASS);
    chunk.set(1, 200, 1, blocks::STONE);
    chunk.set(
        (position.x.rem_euclid(16)) as usize,
        200,
        (position.z.rem_euclid(16)) as usize,
        blocks::GLASS,
    );
    chunk
}

const COMMIT: &str = "commit thread: the log's segment";
const LOG: &str = "commit thread: log/";
const REGIONS: &str = "commit thread: regions/";
const STATE: &str = "chunk thread: a state file's temporary";
const REGION_FILE: &str = "commit thread: a region file's temporary";
const TABLE: &str = "commit thread: the table file's temporary";
const SECTIONS: &str = "chunk thread: section files together";
const SECTION_DIRECTORIES: &str = "chunk thread: directories of sections together";
const MANIFESTS: &str = "chunk thread: manifests together";
const MANIFEST_DIRECTORIES: &str = "chunk thread: directories of manifests together";
/// The four rounds in which saved chunks are written.
const ROUNDS: [&str; 4] = [
    SECTIONS,
    SECTION_DIRECTORIES,
    MANIFESTS,
    MANIFEST_DIRECTORIES,
];
/// A checkpoint of chunks that changed, up to the answer to the flush behind it.
const CHECKPOINT: [&str; 6] = [
    SECTIONS,
    SECTION_DIRECTORIES,
    MANIFESTS,
    MANIFEST_DIRECTORIES,
    STATE,
    REGIONS,
];

/// Chunks of region 0, in three directories of manifests.
const WEST: [ChunkPos; 3] = [
    ChunkPos::new(-1, 0),
    ChunkPos::new(-2, 0),
    ChunkPos::new(-40, 3),
];

/// A chunk of the gap, which nobody holds until it is claimed.
const FREE: ChunkPos = ChunkPos::new(5, 5);

#[test]
fn a_commit_waits_for_one_sync_and_the_first_after_a_checkpoint_for_two() {
    let world = World::new();
    let west = world.open(0, 1);
    let first = world.watch_commit("a commit", &west, 1);
    assert_eq!(first, seen(&[COMMIT], Some(1)));

    // A checkpoint closes the segment, so the next commit begins one, whose name is
    // made durable by a sync of the log's directory behind the sync of the segment.
    world.watch_checkpoint("(a checkpoint)", &west, 1, &[]);
    let next = world.watch_commit("the first commit after a checkpoint", &west, 2);
    assert_eq!(next, seen(&[COMMIT, LOG], Some(2)));
    let later = world.watch_commit("the commit after that", &west, 3);
    assert_eq!(later, seen(&[COMMIT], Some(1)));
}

#[test]
fn a_checkpoint_waits_for_two_syncs_in_turn_and_for_four_rounds_if_chunks_changed() {
    let world = World::new();
    let west = world.open(0, 1);
    log(&west, 1, &[]);
    committed(&west, 1);
    let state_only = world.watch_checkpoint("a checkpoint of no chunk", &west, 1, &[]);
    assert_eq!(state_only, seen(&[STATE, REGIONS], Some(2)));

    log(&west, 2, &[]);
    committed(&west, 2);
    let chunks = world.watch_checkpoint("a checkpoint of three chunks", &west, 2, &WEST);
    assert_eq!(chunks, seen(&CHECKPOINT, Some(6)));
}

/// In a world whose regions claim and return chunks, the first segment of the log is
/// as a rule kept for the table alone once a checkpoint has covered the commits in it,
/// and the checkpoint that finds it so writes the table file before it is answered.
#[test]
fn a_checkpoint_that_lets_a_segment_go_writes_the_table_file_first_with_two_more_syncs() {
    let world = World::new();
    let home = world.open(2, 1);
    let claimed = world.watch_reply(
        "a claim of a chunk nobody holds",
        &home,
        || {
            home.request(StoreRequest::Claim { chunks: vec![FREE] });
        },
        |reply| matches!(reply, StoreReply::Claimed { .. }),
    );
    assert_eq!(claimed, seen(&[COMMIT], Some(1)));
    log(&home, 1, &[]);
    committed(&home, 1);
    let checkpoint = world.watch_checkpoint("the checkpoint after a claim", &home, 1, &[]);
    assert_eq!(checkpoint, seen(&[STATE, REGIONS, TABLE, REGIONS], Some(4)));
}

#[test]
fn a_claim_is_answered_with_its_groups_sync_and_a_return_waits_for_one_with_no_answer() {
    let world = World::new();
    let home = world.open(2, 1);
    // With the commit of the same tick, as a runner sends them: one sync for both.
    let together = world.watch_reply(
        "a commit and a claim in one group",
        &home,
        || {
            log(&home, 1, &[]);
            home.request(StoreRequest::Claim { chunks: vec![FREE] });
        },
        |reply| matches!(reply, StoreReply::Claimed { .. }),
    );
    assert_eq!(together, seen(&[COMMIT], Some(1)));

    // A return of a chunk that was not saved since: the thread for chunks has nothing
    // to write, and the record that frees the chunk is synced with its group.
    let returned = world.watch(
        "a return of a chunk with nothing saved",
        || {
            home.request(StoreRequest::Return { chunks: vec![FREE] });
        },
        || None::<()>,
    );
    assert_eq!(returned.0, seen(&[COMMIT], None));

    // A return behind a save of the chunk: the save is written first, in its rounds.
    let again = world.watch_reply(
        "(the claim again)",
        &home,
        || {
            home.request(StoreRequest::Claim { chunks: vec![FREE] });
        },
        |reply| matches!(reply, StoreReply::Claimed { .. }),
    );
    assert_eq!(again, seen(&[COMMIT], Some(1)));
    let saved = world.watch(
        "a return of a chunk that was saved",
        || {
            home.request(StoreRequest::Save {
                position: FREE,
                tick: 1,
                chunk: changed(FREE, 1, 0),
            });
            home.request(StoreRequest::Return { chunks: vec![FREE] });
        },
        || None::<()>,
    );
    let mut expected = ROUNDS.to_vec();
    expected.push(COMMIT);
    assert_eq!(saved.0, seen(&expected, None));
}

/// A move: the owner checkpoints while it ticks, stops, checkpoints once more and lets
/// go; the next owner opens the region with a higher epoch.
#[test]
fn a_release_and_the_open_behind_it_wait_for_the_syncs_of_two_checkpoints_and_a_region_file() {
    let world = World::new();
    let west = world.open(0, 1);
    log(&west, 1, &[]);
    committed(&west, 1);
    let first = world.watch_checkpoint("a release: the first checkpoint", &west, 1, &WEST);
    assert_eq!(first, seen(&CHECKPOINT, Some(6)));
    // The tick that ran meanwhile: the region stands still from here on.
    let last = world.watch_commit("a release: the last tick's commit", &west, 2);
    assert_eq!(last, seen(&[COMMIT, LOG], Some(2)));
    let second = world.watch_checkpoint("a release: the second checkpoint", &west, 2, &[]);
    assert_eq!(second, seen(&[STATE, REGIONS], Some(2)));
    let closed = world.watch(
        "a release: the handle is dropped",
        || drop(west),
        || None::<()>,
    );
    assert_eq!(closed.0, seen(&[], None));

    let hello = hello_of(&gap(), 0, 2);
    let (opened, _next, restored) = world.watch_open("the open by the next owner", hello);
    assert!(restored.deltas.is_empty());
    // The epoch is on disk before the answer; the record that the region was opened
    // is made durable after it, in a segment of its own.
    assert_eq!(opened, seen(&[REGION_FILE, REGIONS, COMMIT, LOG], Some(2)));
}

/// A merge as ADR-0014 has it, section 6, from the store's side: the absorbed region
/// is released, the survivor's worker opens it with a new epoch, the survivor
/// checkpoints twice and hands the merge in.
#[test]
fn a_merge_waits_for_three_syncs_of_its_own_behind_those_of_its_checkpoints() {
    let world = World::new();
    let west = world.open(0, 1);
    let east = world.open(1, 1);
    for handle in [&west, &east] {
        log(handle, 1, &[]);
        committed(handle, 1);
    }
    // The absorbed region is released, as in the test of a release.
    world.watch_checkpoint("(the absorbed region's first checkpoint)", &east, 1, &[]);
    world.watch_commit("(the absorbed region's last tick)", &east, 2);
    world.watch_checkpoint("(the absorbed region's second checkpoint)", &east, 2, &[]);
    drop(east);
    let hello = hello_of(&gap(), 1, 2);
    let (opened, east, _) = world.watch_open(
        "a merge: the survivor's worker opens the absorbed region",
        hello,
    );
    assert_eq!(opened, seen(&[REGION_FILE, REGIONS, COMMIT, LOG], Some(2)));

    let first = world.watch_checkpoint("a merge: the survivor's first checkpoint", &west, 1, &WEST);
    assert_eq!(first, seen(&CHECKPOINT, Some(6)));
    let last = world.watch_commit("a merge: the survivor's last tick", &west, 2);
    assert_eq!(last, seen(&[COMMIT, LOG], Some(2)));
    let second = world.watch_checkpoint("a merge: the survivor's second checkpoint", &west, 2, &[]);
    assert_eq!(second, seen(&[STATE, REGIONS], Some(2)));

    let merged = world.watch_reply(
        "a merge: the record",
        &west,
        || {
            west.request(StoreRequest::AbsorbCommit {
                absorbed: RegionId(1),
                absorbed_epoch: 2,
                tick: 3,
                state: whole("merged", 3),
            });
        },
        |reply| matches!(reply, StoreReply::Absorbed { .. }),
    );
    // The record in a segment of its own, that segment's name, and the removal of the
    // absorbed region's state file.
    assert_eq!(merged, seen(&[COMMIT, LOG, REGIONS], Some(3)));
    drop(east);

    let next = world.watch_commit("after a merge: the survivor's next commit", &west, 4);
    assert_eq!(next, seen(&[COMMIT], Some(1)));
    // The record is the table's until the table file has it, which the next
    // checkpoint sees to.
    let checkpoint = world.watch_checkpoint(
        "after a merge: the survivor's next checkpoint",
        &west,
        4,
        &[],
    );
    assert_eq!(checkpoint, seen(&[STATE, REGIONS, TABLE, REGIONS], Some(4)));
}

#[test]
fn a_split_waits_for_four_syncs_of_its_own_behind_those_of_its_checkpoints() {
    let world = World::new();
    let west = world.open(0, 1);
    log(&west, 1, &[]);
    committed(&west, 1);
    let first = world.watch_checkpoint("a split: the first checkpoint", &west, 1, &WEST);
    assert_eq!(first, seen(&CHECKPOINT, Some(6)));
    let last = world.watch_commit("a split: the last tick", &west, 2);
    assert_eq!(last, seen(&[COMMIT, LOG], Some(2)));
    let second = world.watch_checkpoint("a split: the second checkpoint", &west, 2, &[]);
    assert_eq!(second, seen(&[STATE, REGIONS], Some(2)));

    let split = world.watch_reply(
        "a split: the record",
        &west,
        || {
            west.request(StoreRequest::SplitCommit {
                tick: 3,
                state: whole("rest", 3),
                part: SplitPart {
                    chunks: vec![WEST[2]],
                    state: whole("part", 3),
                },
                as_epoch: 7,
                region: RegionId(3),
            });
        },
        |reply| matches!(reply, StoreReply::Split { .. }),
    );
    // The record in a segment of its own, that segment's name, and the new region's
    // file with its name.
    assert_eq!(split, seen(&[COMMIT, LOG, REGION_FILE, REGIONS], Some(4)));

    // The worker's hello for the part finds its file as the hello would write it.
    let hello = hello_of(&gap(), 3, 7);
    let (opened, _part, _) = world.watch_open("a split: the hello for the part", hello);
    assert_eq!(opened, seen(&[COMMIT], Some(0)));
}

/// A worker dies with commits of its region that no checkpoint covers; the next owner
/// opens the region, and the store puts those commits into the chunks first.
#[test]
fn a_take_over_waits_for_a_region_file_and_for_the_rounds_of_the_chunks_it_replays() {
    let world = World::new();
    let west = world.open(0, 1);
    log(
        &west,
        1,
        &[(-5, 100, 5, blocks::GLASS), (-20, 100, 5, blocks::GLASS)],
    );
    committed(&west, 1);
    // Nothing tells the store that the worker has died but the next hello.
    let hello = hello_of(&gap(), 0, 2);
    let (opened, _next, restored) =
        world.watch_open("a take-over with two chunks to replay", hello);
    assert_eq!(restored.deltas.len(), 1);
    // The epoch first, on the commit thread, before anything else.
    assert_eq!(opened.waits[..2], [REGION_FILE, REGIONS]);
    // Then the thread for chunks writes the chunks in its four rounds and answers,
    // and the commit thread makes the record of the opening durable beside it. The
    // two do not wait for each other, so where the commit thread's sync falls among
    // the rounds is not said; the answer comes behind the last round.
    let rounds: Vec<&str> = opened.waits[2..]
        .iter()
        .map(String::as_str)
        .filter(|wait| *wait != COMMIT)
        .collect();
    assert_eq!(rounds, ROUNDS);
    assert_eq!(opened.waits.len(), 7);
    let last_round = opened.waits.iter().rposition(|wait| wait != COMMIT);
    let answered = opened.before_the_answer.expect("the hello is answered");
    assert!(last_round.is_some_and(|last| last < answered));
    drop(west);
}
