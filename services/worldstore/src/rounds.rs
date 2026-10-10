//! The rounds a checkpoint's chunks are written in: what
//! `docs/adr/0018-a-checkpoints-chunks-written-together.md` asks of the store, in the
//! order of the tests that record lists. They were written from the record and not
//! from the change, so that they judge it and not the other way round.
//!
//! Most of them start from a world of three regions in which chunks are stored
//! already, have two of the regions save those chunks anew, and look at what the files
//! have afterwards: after a crash at any point, with any part of a round synced, and
//! after a failure.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Instant;

use clustine_data::blocks;
use clustine_format::ChunkManifest;
use clustine_world::{Chunk, ChunkPos};

use super::*;
use crate::disk::{Fault, MemoryDisk, OsDisk, Survival};
use crate::regions::SURVIVALS;
use crate::tests::{committed, edited, generator, hello, load, log, open, save};

const ROOT: &str = "/world";

fn root() -> &'static Path {
    Path::new(ROOT)
}

/// The regions of these tests, pinned side by side: the first west of x = 0, the
/// second from there to x = 64.
const WEST: u32 = 0;
const EAST: u32 = 1;
/// The third, east of x = 64, which saves nothing while the other two do.
const ASIDE: u32 = 2;

fn three() -> Division {
    Division::side_by_side(ChunkPos::new(0, 0), &[0, 64]).unwrap()
}

/// The chunks that two regions save in one go, with the region of each. Two of their
/// manifests are in one directory and the others in directories of their own, so that
/// a round of directories has several and one that more than one chunk depends on.
const SAVED: [(ChunkPos, u32); 5] = [
    (ChunkPos::new(-1, 0), WEST),
    (ChunkPos::new(-40, 3), WEST),
    (ChunkPos::new(0, 0), EAST),
    (ChunkPos::new(1, 0), EAST),
    (ChunkPos::new(40, -2), EAST),
];

/// The one of them that was never stored before it is saved.
const NEW: usize = 4;

/// A chunk of the third region, which is stored and which nobody saves again.
const UNTOUCHED: ChunkPos = ChunkPos::new(70, 0);

/// The states of the checkpoints before and after the chunks are saved anew.
const BEFORE: &[u8] = b"before";
const AFTER: &[u8] = b"after";

/// The chunk `SAVED[index]` as it is stored before a test does anything. The section
/// at y = 100 is the same in all of them, and the one at y = 200 is each chunk's own.
fn before(index: usize) -> &'static Chunk {
    static CHUNKS: LazyLock<Vec<Chunk>> = LazyLock::new(|| {
        let chunk = |index: usize| {
            let mut chunk = generator().generate(SAVED[index].0);
            chunk.set(3, 100, 4, blocks::STONE);
            chunk.set(index, 200, 1, blocks::GLASS);
            chunk
        };
        (0..SAVED.len()).map(chunk).collect()
    });
    &CHUNKS[index]
}

/// The chunk as it is saved anew: again with a section that all of them share and one
/// of its own, neither of which is stored before. Like the others it is made once:
/// what is loaded is compared with it many thousand times.
fn after(index: usize) -> &'static Chunk {
    static CHUNKS: LazyLock<Vec<Chunk>> = LazyLock::new(|| {
        let chunk = |index: usize| {
            let mut chunk = generator().generate(SAVED[index].0);
            chunk.set(3, 100, 4, blocks::GLASS);
            chunk.set(index, 200, 2, blocks::STONE);
            chunk
        };
        (0..SAVED.len()).map(chunk).collect()
    });
    &CHUNKS[index]
}

fn untouched() -> &'static Chunk {
    static CHUNK: LazyLock<Chunk> = LazyLock::new(|| {
        let mut chunk = generator().generate(UNTOUCHED);
        chunk.set(8, 100, 8, blocks::GLASS);
        chunk
    });
    &CHUNK
}

/// Where the manifest of the chunk is kept.
fn manifest_path(position: ChunkPos) -> PathBuf {
    FileChunks::new(Arc::new(MemoryDisk::default()), root()).manifest_path(position)
}

/// Where the sections of the chunk are kept, by the layout `docs/world-format.md`
/// gives, which the record leaves as it is.
fn section_paths(position: ChunkPos, chunk: &Chunk) -> Vec<PathBuf> {
    let (_, sections) = ChunkManifest::describe(position, chunk, 0);
    let path = |(hash, _): &(Hash, Vec<u8>)| {
        let name = hash.to_string();
        root().join("blobs").join(&name[..2]).join(&name)
    };
    sections.iter().map(path).collect()
}

/// Whether the file is one that chunks are kept in.
fn of_chunks(path: &Path) -> bool {
    path.starts_with(root().join("blobs")) || path.starts_with(root().join("manifests"))
}

/// A round of syncs: of files, or of directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Round {
    Files,
    Directories,
}

/// What is done to a file or a directory that changes it or makes it durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Write,
    Sync,
    SyncDirectory,
    Rename,
    Remove,
}

/// What a [`Rigged`] disk refuses to do with the files of chunks for as long as it is
/// told to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    Writes,
    Syncs,
    Renames,
    /// Syncs of the directories that manifests are in, and nothing else.
    ManifestDirectories,
}

/// A disk in memory that goes wrong where a test says, in the files of chunks only:
/// the log and the files of regions are the commit thread's, and `kill.rs` holds that
/// thread to what it owes.
///
/// It counts every change and sync of a chunk file as a step, each path of a round by
/// itself and in the order of the paths, as the simulated disk does; and it sees the
/// rounds, which that disk cannot tell from syncs in turn. So it can stop for good in
/// the middle of a round with any part of the round synced, which is what a machine
/// that loses power while syncs wait together leaves.
struct Rigged {
    disk: MemoryDisk,
    rig: Mutex<Rig>,
}

#[derive(Default)]
struct Rig {
    /// How many steps there were.
    steps: u64,
    /// The step that fails, once, without doing anything.
    failing: Option<u64>,
    refused: Option<Refused>,
    /// The rounds so far, with their paths. A round of nothing is none.
    rounds: Vec<(Round, Vec<PathBuf>)>,
    /// The chunk files and directories that were synced by themselves.
    alone: Vec<PathBuf>,
    /// The round to stop in, counted from 1, and the seed that picks what of it is
    /// synced first.
    stopping: Option<(usize, u64)>,
    /// Set once the disk has stopped: nothing is changed or synced any more.
    stopped: bool,
}

/// Whether `seed` picks the path with this index of a round. The first six paths get
/// a bit of the seed each, and later ones those bits again: of a round of six paths
/// or fewer, the seeds below 64 pick every part there is.
fn picked(seed: u64, index: usize) -> bool {
    seed >> (index % 6) & 1 == 1
}

fn stopped() -> io::Error {
    io::Error::other("the machine has stopped")
}

impl Rigged {
    fn over(disk: MemoryDisk) -> Self {
        Self {
            disk,
            rig: Mutex::default(),
        }
    }

    /// The disk with this step failing, once.
    fn failing(self, step: u64) -> Self {
        self.rig().failing = Some(step);
        self
    }

    /// The disk that stops in this round, with what `seed` picks of it synced.
    fn stopping(self, round: usize, seed: u64) -> Self {
        self.rig().stopping = Some((round, seed));
        self
    }

    fn refuse(&self, refused: Option<Refused>) {
        self.rig().refused = refused;
    }

    fn rig(&self) -> MutexGuard<'_, Rig> {
        self.rig.lock().unwrap()
    }

    fn steps(&self) -> u64 {
        self.rig().steps
    }

    fn rounds(&self) -> Vec<(Round, Vec<PathBuf>)> {
        self.rig().rounds.clone()
    }

    fn alone(&self) -> Vec<PathBuf> {
        self.rig().alone.clone()
    }

    fn has_stopped(&self) -> bool {
        self.rig().stopped
    }

    /// Counts a step, if it is of a chunk file, and says whether it is to fail.
    fn step(&self, step: Step, path: &Path) -> io::Result<()> {
        let mut rig = self.rig();
        if rig.stopped {
            return Err(stopped());
        }
        if !of_chunks(path) {
            return Ok(());
        }
        rig.steps += 1;
        let refused = match rig.refused {
            Some(Refused::Writes) => step == Step::Write,
            Some(Refused::Syncs) => step == Step::Sync,
            Some(Refused::Renames) => step == Step::Rename,
            Some(Refused::ManifestDirectories) => {
                step == Step::SyncDirectory && path.starts_with(root().join("manifests"))
            }
            None => false,
        };
        if refused || rig.failing == Some(rig.steps) {
            return Err(io::Error::other("a fault was injected"));
        }
        Ok(())
    }

    /// A round: every path in turn, each a step, up to the first that fails; or, in
    /// the round the disk stops in, the paths the seed picks, and nothing after them.
    fn round(&self, round: Round, paths: &[PathBuf]) -> io::Result<()> {
        let stopping = {
            let mut rig = self.rig();
            if rig.stopped {
                return Err(stopped());
            }
            if paths.is_empty() {
                return Ok(());
            }
            rig.rounds.push((round, paths.to_vec()));
            rig.stopping
                .filter(|(stopping, _)| *stopping == rig.rounds.len())
                .map(|(_, seed)| seed)
        };
        let sync = |path: &Path| match round {
            Round::Files => self.disk.sync(path),
            Round::Directories => self.disk.sync_directory(path),
        };
        let step = match round {
            Round::Files => Step::Sync,
            Round::Directories => Step::SyncDirectory,
        };
        match stopping {
            Some(seed) => {
                for (index, path) in paths.iter().enumerate() {
                    if picked(seed, index) {
                        sync(path)?;
                    }
                }
                self.rig().stopped = true;
                Err(stopped())
            }
            None => paths.iter().try_for_each(|path| {
                self.step(step, path)?;
                sync(path)
            }),
        }
    }
}

impl Disk for Rigged {
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
        self.step(Step::Write, path)?;
        self.disk.write(path, contents)
    }
    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.step(Step::Write, path)?;
        self.disk.append(path, contents)
    }
    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        self.step(Step::Write, path)?;
        self.disk.truncate(path, length)
    }
    fn sync(&self, path: &Path) -> io::Result<()> {
        self.step(Step::Sync, path)?;
        if of_chunks(path) {
            self.rig().alone.push(path.to_owned());
        }
        self.disk.sync(path)
    }
    fn sync_directory(&self, directory: &Path) -> io::Result<()> {
        self.step(Step::SyncDirectory, directory)?;
        if of_chunks(directory) {
            self.rig().alone.push(directory.to_owned());
        }
        self.disk.sync_directory(directory)
    }
    fn sync_files(&self, files: &[PathBuf]) -> io::Result<()> {
        self.round(Round::Files, files)
    }
    fn sync_directories(&self, directories: &[PathBuf]) -> io::Result<()> {
        self.round(Round::Directories, directories)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step(Step::Rename, to)?;
        self.disk.rename(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.step(Step::Remove, path)?;
        self.disk.remove(path)
    }
}

/// The disk these tests go wrong with has to do what they take it to do, or they
/// would hold the store to nothing: no store reaches it before the change is built.
#[test]
fn the_disk_of_these_tests_syncs_what_its_seed_picks_of_a_round_and_then_stops() {
    let directory = root().join("blobs/00");
    let files = ["a", "b", "c"].map(|name| directory.join(name));
    let disk = Rigged::over(MemoryDisk::default()).stopping(2, 0b101);
    for file in &files {
        disk.write(file, b"written").unwrap();
    }
    // The first round goes through, and makes the names durable.
    disk.sync_directories(std::slice::from_ref(&directory))
        .unwrap();
    assert!(disk.sync_files(&files).is_err());
    assert!(disk.has_stopped());

    // The seed picks the first and the third.
    let left = disk.disk.crashed(Survival::Nothing);
    let read = |file: &PathBuf| left.read(file).unwrap().unwrap();
    assert_eq!(read(&files[0]), b"written");
    assert_eq!(read(&files[1]), b"");
    assert_eq!(read(&files[2]), b"written");
    // Nothing is changed or synced after it.
    assert!(disk.write(&files[1], b"later").is_err());
    assert!(disk.sync(&files[1]).is_err());
    assert!(disk.sync_files(&files).is_err());
    assert_eq!(disk.disk.read(&files[1]).unwrap().unwrap(), b"written");
    assert_eq!(disk.rounds().len(), 2);
    assert_eq!(disk.steps(), 4);
}

/// A store on `disk` for the three regions, with its chunks in files there as well.
fn store_over<D: Disk + 'static>(disk: &Arc<D>) -> Result<Store, StoreError> {
    let chunks = FileChunks::new(disk.clone(), root());
    start(
        disk.clone(),
        root(),
        Box::new(chunks),
        generator(),
        &three(),
    )
}

/// Opens the three regions, by their ids; `None` if the store cannot open one of them.
fn owners(store: &Store, epoch: u64) -> Option<[StoreHandle; 3]> {
    let open = |region| Some(store.open_region(hello(region, epoch)).ok()?.0);
    Some([open(WEST)?, open(EAST)?, open(ASIDE)?])
}

/// Has each of the two regions save its chunks anew: first the western region's, then
/// the eastern one's, and nothing after them that would make them durable.
fn save_anew(west: &StoreHandle, east: &StoreHandle) {
    for (index, (position, region)) in SAVED.iter().enumerate() {
        let owner = if *region == WEST { west } else { east };
        owner.request(StoreRequest::Save {
            position: *position,
            tick: 2,
            chunk: after(index).clone(),
        });
    }
}

/// Asks for a flush and says whether it was answered. It is not if the handle is
/// lost, and whoever was answered was told, also if the handle is lost right after.
fn flushed(handle: &StoreHandle) -> bool {
    handle.request(StoreRequest::Flush);
    while let Ok(reply) = handle.replies.recv() {
        if reply == StoreReply::Flushed {
            return true;
        }
    }
    false
}

/// Loads every chunk the two regions save, and says of each whether it is as it was
/// saved anew. Panics if one cannot be read, which a manifest that names a section
/// that is not there cannot, or is neither as it was before nor as it was saved.
fn as_before_or_as_saved(chunks: &mut dyn Chunks, case: &str) -> [bool; SAVED.len()] {
    let mut saved = [false; SAVED.len()];
    for (index, (position, _)) in SAVED.iter().enumerate() {
        let loaded = chunks
            .load(*position)
            .unwrap_or_else(|error| panic!("{case}: the chunk at {position:?} is lost: {error}"));
        let old = (index != NEW).then(|| before(index));
        saved[index] = loaded.as_ref() == Some(after(index));
        assert!(
            saved[index] || loaded.as_ref() == old,
            "{case}: the chunk at {position:?} is neither as it was before nor as it was saved"
        );
    }
    // A chunk nobody saved is none of a sync's business.
    let aside = chunks
        .load(UNTOUCHED)
        .unwrap_or_else(|error| panic!("{case}: the chunk nobody saved is lost: {error}"));
    assert!(
        aside.as_ref() == Some(untouched()),
        "{case}: the chunk nobody saved has changed"
    );
    saved
}

/// The chunks as the files of `disk` have them, read by a store that has nothing
/// pending.
fn in_the_files(disk: MemoryDisk, case: &str) -> [bool; SAVED.len()] {
    as_before_or_as_saved(&mut FileChunks::new(Arc::new(disk), root()), case)
}

/// A world in which every chunk of [`SAVED`] but the new one is stored as it is
/// [`before`], durably and with a checkpoint of each region: what a crash leaves of
/// it. Each test goes on from a copy.
fn world() -> MemoryDisk {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_over(&disk).unwrap();
    let owners = owners(&store, 1).unwrap();
    for (index, (position, region)) in SAVED.iter().enumerate() {
        if index != NEW {
            save(&owners[*region as usize], *position, before(index));
        }
    }
    save(&owners[ASIDE as usize], UNTOUCHED, untouched());
    for owner in &owners {
        owner.request(StoreRequest::Checkpoint {
            tick: 1,
            state: BEFORE.to_vec(),
        });
        assert!(flushed(owner));
    }
    store.flush().unwrap();
    let world = disk.crashed(Survival::Nothing);
    let saved = in_the_files(copy(&world), "the world before");
    assert_eq!(saved, [false; SAVED.len()]);
    world
}

/// A disk with what `world` has, all of which is durable.
fn copy(world: &MemoryDisk) -> MemoryDisk {
    world.crashed(Survival::Everything)
}

// 1. What is saved is pending until a sync.

/// A save only notes the chunk: it is what is loaded from then on, and nothing of it
/// is on the disk before a sync. That is the whole of the change: the files of all
/// the chunks of a checkpoint are written and synced together, which they cannot be
/// if each save writes its own.
#[test]
fn a_chunk_that_was_saved_and_not_synced_is_what_is_loaded_and_has_no_file() {
    let disk = Arc::new(MemoryDisk::default());
    let mut chunks = FileChunks::new(disk.clone(), root());
    let position = ChunkPos::new(3, -7);
    chunks.save(position, 5, &edited()).unwrap();
    assert_eq!(disk.operations(), 0, "the save changed or synced a file");
    assert!(!disk.exists(&chunks.manifest_path(position)).unwrap());
    assert!(chunks.load(position).unwrap() == Some(edited()));
    // It is nowhere but in the store that noted it.
    let mut other = FileChunks::new(disk.clone(), root());
    assert!(other.load(position).unwrap().is_none());

    chunks.sync().unwrap();
    assert!(chunks.load(position).unwrap() == Some(edited()));
    assert!(other.load(position).unwrap() == Some(edited()));
}

/// A later save of a chunk takes the place of an earlier one that was not synced yet,
/// for a load before the sync, for one after it and for whoever reads the files after
/// a crash: a region saves a chunk whenever its last ticket goes, and often again
/// before the next checkpoint.
#[test]
fn a_second_save_before_the_sync_is_what_counts_and_what_the_sync_makes_durable() {
    let disk = Arc::new(MemoryDisk::default());
    let mut chunks = FileChunks::new(disk.clone(), root());
    let position = ChunkPos::new(3, -7);
    let mut second = edited();
    second.set(1, 1, 1, blocks::GLASS);
    chunks.save(position, 5, &edited()).unwrap();
    chunks.save(position, 6, &second).unwrap();
    assert!(chunks.load(position).unwrap() == Some(second.clone()));

    chunks.sync().unwrap();
    assert!(chunks.load(position).unwrap() == Some(second.clone()));
    assert!(disk.exists(&chunks.manifest_path(position)).unwrap());
    for survival in SURVIVALS {
        let mut left = FileChunks::new(Arc::new(disk.crashed(survival)), root());
        assert!(
            left.load(position).unwrap() == Some(second.clone()),
            "{survival:?}"
        );
    }
}

/// Whether the manifest and every section of the chunk have a file.
fn files_of(disk: &MemoryDisk, position: ChunkPos, chunk: &Chunk) -> (bool, Vec<bool>) {
    let there = |path: &PathBuf| disk.exists(path).unwrap();
    let sections = section_paths(position, chunk);
    (
        there(&manifest_path(position)),
        sections.iter().map(there).collect(),
    )
}

/// Panics unless no file of the chunk is there.
fn has_no_file(disk: &MemoryDisk, position: ChunkPos, chunk: &Chunk) {
    let (manifest, sections) = files_of(disk, position, chunk);
    assert!(
        !manifest && !sections.contains(&true),
        "the chunk at {position:?} has files before anything wrote what is pending: \
         manifest {manifest}, sections {sections:?}"
    );
}

/// Panics unless every file of the chunk is there.
fn has_its_files(disk: &MemoryDisk, position: ChunkPos, chunk: &Chunk) {
    let (manifest, sections) = files_of(disk, position, chunk);
    assert!(
        manifest && !sections.contains(&false),
        "the chunk at {position:?} is not in the files: manifest {manifest}, sections {sections:?}"
    );
}

/// A flush means that every save before it is in the files, which is what whoever
/// flushes takes it to mean: a region that is released flushes and is gone, and the
/// tests of four crates flush before they look at files. Before the flush the save
/// is pending, also when the store is at rest, and is loaded all the same.
#[test]
fn a_save_is_in_the_files_after_a_flush_and_not_before() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_over(&disk).unwrap();
    let [_, east, _] = owners(&store, 1).unwrap();
    let position = ChunkPos::new(0, 0);
    save(&east, position, &edited());
    // The store has done with the save: being at rest writes nothing.
    store.flush().unwrap();
    has_no_file(&disk, position, &edited());
    assert!(load(&east, position) == edited());
    has_no_file(&disk, position, &edited());

    assert!(flushed(&east));
    has_its_files(&disk, position, &edited());
    assert!(load(&east, position) == edited());
    // Durably so: a flush is a sync, and nothing of a sync is left to a later one.
    let mut left = FileChunks::new(Arc::new(disk.crashed(Survival::Nothing)), root());
    assert!(left.load(position).unwrap() == Some(edited()));
}

/// An open that had commits to apply writes what is pending before it hands the
/// region over: what it applied is not held in memory, and neither is what another
/// region saved before it.
#[test]
fn a_save_is_in_the_files_after_an_open_that_applied_commits_and_not_before() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_over(&disk).unwrap();
    let [west, east, _] = owners(&store, 1).unwrap();
    let saved = ChunkPos::new(0, 0);
    save(&east, saved, &edited());
    // A change of the western region that is in its commit and in no saved chunk.
    let applied = ChunkPos::new(-1, 0);
    log(&west, 1, &[(-5, 150, 5, blocks::GLASS)]);
    committed(&west, 1);
    store.flush().unwrap();
    has_no_file(&disk, saved, &edited());
    assert!(!disk.exists(&manifest_path(applied)).unwrap());

    let (_west, restored) = store.open_region(hello(WEST, 2)).unwrap();
    assert_eq!(restored.deltas.len(), 1);
    let mut changed = generator().generate(applied);
    changed.set(11, 150, 5, blocks::GLASS);
    has_its_files(&disk, saved, &edited());
    has_its_files(&disk, applied, &changed);
    assert!(!east.is_lost());
    let mut left = FileChunks::new(Arc::new(disk.crashed(Survival::Nothing)), root());
    assert!(left.load(saved).unwrap() == Some(edited()));
    assert!(left.load(applied).unwrap() == Some(changed));
}

/// Nothing syncs between two checkpoints, which are five minutes apart, and a region
/// saves a chunk whenever its last ticket goes: so what is pending is written once
/// there are 128 chunks of it, whoever saved them, and a chunk saved twice is one.
#[test]
fn the_saves_are_in_the_files_once_128_chunks_are_pending_and_not_before() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_over(&disk).unwrap();
    let [west, east, _] = owners(&store, 1).unwrap();
    let chunk = edited();
    let mut again = edited();
    again.set(1, 1, 1, blocks::GLASS);
    let western: Vec<ChunkPos> = (1..=64).map(|x| ChunkPos::new(-x, 0)).collect();
    let eastern: Vec<ChunkPos> = (0..64).map(|x| ChunkPos::new(x, 0)).collect();
    let (last, rest) = eastern.split_last().unwrap();

    for position in &western {
        save(&west, *position, &chunk);
    }
    for position in rest {
        save(&east, *position, &chunk);
    }
    // The same chunk once more: there are still 127 of them.
    save(&east, eastern[0], &again);
    store.flush().unwrap();
    for position in western.iter().chain(rest) {
        assert!(
            !disk.exists(&manifest_path(*position)).unwrap(),
            "{position:?} is in the files with 127 chunks pending"
        );
    }

    save(&east, *last, &chunk);
    store.flush().unwrap();
    assert!(!west.is_lost() && !east.is_lost());
    let mut left = FileChunks::new(Arc::new(disk.crashed(Survival::Nothing)), root());
    for position in western.iter().chain(&eastern) {
        assert!(
            disk.exists(&manifest_path(*position)).unwrap(),
            "{position:?} is not in the files with 128 chunks pending"
        );
        let expected = if *position == eastern[0] {
            &again
        } else {
            &chunk
        };
        assert!(
            left.load(*position).unwrap().as_ref() == Some(expected),
            "{position:?} is not durable"
        );
    }

    // What is saved after that is pending anew.
    let later = ChunkPos::new(-100, 0);
    save(&west, later, &chunk);
    store.flush().unwrap();
    assert!(!disk.exists(&manifest_path(later)).unwrap());
    assert!(load(&west, later) == chunk);
}

// 2. Stopped at every change and sync.

/// What comes behind the saves of the two regions and makes them durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// A checkpoint of the eastern region.
    Checkpoint,
    /// A flush of the eastern region.
    Flush,
}

/// The scenario: the three regions are opened, the western and the eastern one save
/// their chunks anew, and the eastern one has all of it made durable by `trigger` and
/// flushes. Returns whether that flush was answered: if it was, the eastern region
/// was told that everything before it is done.
fn scenario(store: &Store, trigger: Trigger) -> bool {
    let Some(owners) = owners(store, 2) else {
        return false;
    };
    let [west, east, aside] = &owners;
    save_anew(west, east);
    if trigger == Trigger::Checkpoint {
        east.request(StoreRequest::Checkpoint {
            tick: 2,
            state: AFTER.to_vec(),
        });
    }
    let answered = flushed(east);
    flushed(west);
    flushed(aside);
    // At rest, so that what is looked at next does not change under the look. It
    // fails if the disk has stopped, and nothing is under way then either.
    let _ = store.flush();
    answered
}

/// Holds what a crash left to what the record promises at every moment: a chunk is
/// whole, as it was before or as it was saved, and a manifest names no section that
/// is not there; and nobody was told that something is durable that is not.
fn left_whole(left: MemoryDisk, trigger: Trigger, flushed: bool, case: &str) {
    let left = Arc::new(left);
    // First of all and from nothing but the files: before a store could save
    // anything again.
    let saved = as_before_or_as_saved(&mut FileChunks::new(left.clone(), root()), case);
    let store = store_over(&left).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (_, restored) = store
        .open_region(hello(EAST, 3))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let checkpointed = restored.state.is_some_and(|state| state.state == AFTER);
    if trigger == Trigger::Checkpoint {
        assert!(
            checkpointed || !flushed,
            "{case}: the flush behind the checkpoint was answered, and its state is lost"
        );
    }
    // A checkpoint cuts the commits it covers off the log, and a flush is taken to
    // mean that the saves are in the files: either says that the chunks are durable.
    if checkpointed || flushed {
        assert_eq!(
            saved,
            [true; SAVED.len()],
            "{case}: the chunks were said to be durable (checkpointed: {checkpointed}, \
             flushed: {flushed}) and these of them are as saved"
        );
    }
}

/// Runs the scenario on a copy of `world` with `fault`, and holds what a crash
/// leaves of it, in each way it can, to [`left_whole`]. Returns whether the flush of
/// the scenario was answered.
fn stopped_and_left_whole(world: &MemoryDisk, trigger: Trigger, fault: Fault) -> bool {
    let disk = Arc::new(copy(world).with(fault));
    let flushed = store_over(&disk).is_ok_and(|store| scenario(&store, trigger));
    for survival in SURVIVALS {
        let case = format!("{trigger:?}, {fault:?}, {survival:?}");
        left_whole(disk.crashed(survival), trigger, flushed, &case);
    }
    flushed
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some
/// to spare for the ways the store's two threads can interleave.
fn operations(world: &MemoryDisk, trigger: Trigger) -> u64 {
    let disk = Arc::new(copy(world));
    let store = store_over(&disk).unwrap();
    assert!(scenario(&store, trigger));
    disk.operations() + 10
}

fn stopped_at_every_point(trigger: Trigger) {
    let world = world();
    // Without a fault the flush is answered, and then `left_whole` sees to it that
    // everything is as saved.
    assert!(stopped_and_left_whole(
        &world,
        trigger,
        Fault::Stop(u64::MAX)
    ));
    for n in 1..=operations(&world, trigger) {
        stopped_and_left_whole(&world, trigger, Fault::Stop(n));
    }
}

/// A machine that stops at any change or sync of a checkpoint of several chunks, of
/// two regions, that share a section and have sections of their own leaves every
/// chunk whole: as it was before or as it was saved. The rounds write the files of
/// many chunks before any of them is durable, so this is where an order that is
/// wrong for one chunk among several would show; and a checkpoint whose state is
/// kept has cut the log, so its chunks have to be there.
#[test]
fn a_store_stopped_at_any_point_of_a_checkpoint_leaves_every_chunk_as_before_or_as_saved() {
    stopped_at_every_point(Trigger::Checkpoint);
}

/// The same for a flush, which writes what is pending as a checkpoint does: if it was
/// answered, the chunks saved before it are in the files whatever a crash keeps.
#[test]
fn a_store_stopped_at_any_point_of_a_flush_leaves_every_chunk_as_before_or_as_saved() {
    stopped_at_every_point(Trigger::Flush);
}

// 3. Stopped with any part of a round synced.

/// A sync is four rounds: the sections that are not stored yet and then their
/// directories, the manifests and then theirs. Nothing of a chunk is synced by
/// itself, which is what made a checkpoint take as long as it had chunks. The tests
/// that stop a round rest on this: they stop the rounds there are.
#[test]
fn a_sync_is_a_round_of_sections_and_one_of_manifests_each_with_its_directories() {
    let disk = Arc::new(Rigged::over(world()));
    let mut chunks = FileChunks::new(disk.clone(), root());
    let mut sections = BTreeSet::new();
    let mut manifests = BTreeSet::new();
    for (index, (position, _)) in SAVED.iter().enumerate() {
        for section in section_paths(*position, after(index)) {
            // A section that is stored is not written again.
            if !disk.exists(&section).unwrap() {
                sections.insert(section);
            }
        }
        manifests.insert(chunks.manifest_path(*position));
    }
    for (index, (position, _)) in SAVED.iter().enumerate() {
        chunks.save(*position, 2, after(index)).unwrap();
    }
    chunks.sync().unwrap();

    let directories = |files: &BTreeSet<PathBuf>| -> BTreeSet<PathBuf> {
        let directory = |file: &PathBuf| file.parent().unwrap().to_owned();
        files.iter().map(directory).collect()
    };
    let rounds = disk.rounds();
    let kinds: Vec<Round> = rounds.iter().map(|(round, _)| *round).collect();
    assert_eq!(
        kinds,
        [
            Round::Files,
            Round::Directories,
            Round::Files,
            Round::Directories
        ],
        "{rounds:?}"
    );
    let within = |round: usize, directory: &str| {
        let directory = root().join(directory);
        rounds[round]
            .1
            .iter()
            .all(|path| path.starts_with(&directory))
    };
    let set = |round: usize| -> BTreeSet<PathBuf> { rounds[round].1.iter().cloned().collect() };
    // The files are synced under the names they are written under, which the record
    // does not give: how many there are and where is what can be held to.
    assert_eq!(rounds[0].1.len(), sections.len(), "{rounds:?}");
    assert!(within(0, "blobs"), "{rounds:?}");
    assert_eq!(set(1), directories(&sections));
    assert_eq!(rounds[1].1.len(), set(1).len(), "{rounds:?}");
    assert_eq!(rounds[2].1.len(), manifests.len(), "{rounds:?}");
    assert!(within(2, "manifests"), "{rounds:?}");
    assert_eq!(set(3), directories(&manifests));
    assert_eq!(rounds[3].1.len(), set(3).len(), "{rounds:?}");
    assert_eq!(disk.alone(), Vec::<PathBuf>::new());
    for file in sections.iter().chain(&manifests) {
        assert!(disk.exists(file).unwrap(), "{file:?}");
    }
}

/// A machine that loses power while the syncs of a round wait together leaves any
/// part of the round durable, not only the files before a point, and the simulated
/// disk by itself shows only the latter. So the disk stops in each round with each
/// part of it synced, and what a crash leaves then is held to the same as in the
/// tests above. A directory of manifests that is durable while one of sections is
/// not would show here and nowhere else.
#[test]
fn a_store_stopped_with_any_part_of_a_round_synced_leaves_every_chunk_as_before_or_as_saved() {
    let world = world();
    for trigger in [Trigger::Checkpoint, Trigger::Flush] {
        let rounds = {
            let disk = Arc::new(Rigged::over(copy(&world)));
            assert!(scenario(&store_over(&disk).unwrap(), trigger));
            disk.rounds().len()
        };
        assert!(
            rounds >= 4,
            "{trigger:?}: the saves of five chunks were synced in {rounds} rounds"
        );
        for round in 1..=rounds {
            // No round of the scenario has more than six paths, so that the seeds
            // pick every part of each.
            for seed in 0..64 {
                let disk = Arc::new(Rigged::over(copy(&world)).stopping(round, seed));
                let flushed = store_over(&disk).is_ok_and(|store| scenario(&store, trigger));
                assert!(
                    disk.has_stopped(),
                    "{trigger:?}: round {round} of {rounds} never came"
                );
                for survival in SURVIVALS {
                    let case = format!("{trigger:?}, round {round}, seed {seed:#b}, {survival:?}");
                    left_whole(disk.disk.crashed(survival), trigger, flushed, &case);
                }
            }
        }
    }
}

// 4. Failing at every step.

/// A sync that fails, at any of its steps and with any number of the steps after it
/// failing too, returns the error and leaves every chunk whole, for a load right
/// after it and for whoever reads the files after a crash: a manifest in place
/// names sections that are there, also when the sync gave up half way. And the same
/// chunks saved again and synced by a sync that works are as saved and durably so.
/// With several steps failing in a row, what a failed sync could not clear away
/// stays behind: a section file that is in place and not durably so must not be
/// taken for stored by the sync that works, or a crash takes it from under a
/// manifest.
#[test]
fn a_sync_that_fails_at_any_step_leaves_every_chunk_whole_and_a_later_one_leaves_them_as_saved() {
    let world = world();
    let save_anew = |chunks: &mut FileChunks| -> Result<(), StoreError> {
        for (index, (position, _)) in SAVED.iter().enumerate() {
            chunks.save(*position, 2, after(index))?;
        }
        Ok(())
    };
    let steps = {
        let disk = Arc::new(copy(&world));
        let mut chunks = FileChunks::new(disk.clone(), root());
        save_anew(&mut chunks).unwrap();
        chunks.sync().unwrap();
        disk.operations()
    };
    assert!(steps > 0);
    // What a crash can make of the files of chunks: it keeps none of what was not
    // durable, or of a file that was written and not synced half. A crash that keeps
    // everything leaves these files as a load finds them without a crash.
    let crashes = [Survival::Nothing, Survival::Torn];
    let all = [true; SAVED.len()];
    for n in 1..=steps {
        // One step fails; or the first thing done about it fails too; or half of
        // what is done about it, or all of it: a sync has six section files to clear
        // away at most.
        for count in [1, 2, 4, 7] {
            let fault = Fault::Fails(n, count);
            let disk = Arc::new(copy(&world).with(fault));
            let mut chunks = FileChunks::new(disk.clone(), root());
            let mut failures = 0;
            // Each sync that fails has used up one of the faults at least.
            while let Err(error) = save_anew(&mut chunks).and_then(|()| chunks.sync()) {
                failures += 1;
                assert!(failures <= count, "{fault:?}: {error}");
                let case = format!("{fault:?}, after {failures} failed");
                as_before_or_as_saved(&mut chunks, &case);
                for survival in crashes {
                    in_the_files(disk.crashed(survival), &format!("{case}, {survival:?}"));
                }
            }
            assert!(failures > 0, "{fault:?}: the sync did not return the error");
            let case = format!("{fault:?}, after a sync that worked");
            assert_eq!(as_before_or_as_saved(&mut chunks, &case), all, "{case}");
            for survival in crashes {
                let case = format!("{case}, {survival:?}");
                assert_eq!(in_the_files(disk.crashed(survival), &case), all, "{case}");
            }
        }
    }
}

/// Loads every chunk the two regions save through the owner of its region, and says
/// of each whether it is as it was saved anew; it has to be that or as it was before.
fn loaded_by(west: &StoreHandle, east: &StoreHandle, case: &str) -> [bool; SAVED.len()] {
    let mut saved = [false; SAVED.len()];
    for (index, (position, region)) in SAVED.iter().enumerate() {
        let owner = if *region == WEST { west } else { east };
        let loaded = load(owner, *position);
        // A chunk that was never stored is generated.
        let old = if index == NEW {
            generator().generate(*position)
        } else {
            before(index).clone()
        };
        saved[index] = loaded == *after(index);
        assert!(
            saved[index] || loaded == old,
            "{case}: the chunk at {position:?} is neither as it was before nor as it was saved"
        );
    }
    saved
}

/// Opens the western and the eastern region again after a sync that failed lost them,
/// and holds the store to what it owes them then: every chunk is as it was before or
/// as it was saved, and the checkpoint that was not made is not what a region is
/// restored with. They save again and checkpoint, and from then on the chunks are as
/// saved, whatever a crash keeps.
fn saved_again_for_good(store: &Store, disk: &Rigged, case: &str) {
    let all = [true; SAVED.len()];
    let (west, _) = store.open_region(hello(WEST, 3)).unwrap();
    let (east, restored) = store.open_region(hello(EAST, 3)).unwrap();
    assert!(
        restored.state.is_some_and(|state| state.state == BEFORE),
        "{case}: the region is restored with a checkpoint whose chunks are not durable"
    );
    loaded_by(&west, &east, case);

    save_anew(&west, &east);
    east.request(StoreRequest::Checkpoint {
        tick: 3,
        state: AFTER.to_vec(),
    });
    assert!(flushed(&east) && flushed(&west), "{case}");
    assert_eq!(loaded_by(&west, &east, case), all, "{case}");
    store.flush().unwrap();
    for survival in SURVIVALS {
        let case = format!("{case}, {survival:?}");
        assert_eq!(
            in_the_files(disk.disk.crashed(survival), &case),
            all,
            "{case}"
        );
    }
}

/// A sync that fails at any of its steps loses the handle of every region that saved
/// since the last one, because the chunks of any of them may be what did not get to
/// the disk, and of no region that did not: that one is served on. The regions that
/// lost theirs are opened again and find what [`saved_again_for_good`] says.
#[test]
fn a_sync_that_fails_loses_the_handles_of_both_regions_that_saved_and_no_other() {
    let world = world();
    for trigger in [Trigger::Checkpoint, Trigger::Flush] {
        let steps = {
            let disk = Arc::new(Rigged::over(copy(&world)));
            assert!(scenario(&store_over(&disk).unwrap(), trigger));
            disk.steps()
        };
        assert!(steps > 0);
        for step in 1..=steps {
            let case = format!("{trigger:?}, step {step} of {steps}");
            let disk = Arc::new(Rigged::over(copy(&world)).failing(step));
            let store = store_over(&disk).unwrap();
            let [west, east, aside] = owners(&store, 2).unwrap();
            assert!(load(&aside, UNTOUCHED) == *untouched(), "{case}");
            save_anew(&west, &east);
            east.request(match trigger {
                Trigger::Checkpoint => StoreRequest::Checkpoint {
                    tick: 2,
                    state: AFTER.to_vec(),
                },
                Trigger::Flush => StoreRequest::Flush,
            });
            // At rest: whoever the failure loses is lost by now.
            store.flush().unwrap();
            assert!(
                west.is_lost() && east.is_lost(),
                "{case}: of the regions that saved, the western one is lost: {}, and the \
                 eastern one: {}",
                west.is_lost(),
                east.is_lost()
            );
            assert!(
                !aside.is_lost(),
                "{case}: a region that did not save is lost"
            );
            assert!(load(&aside, UNTOUCHED) == *untouched(), "{case}");
            assert!(flushed(&aside), "{case}");

            drop((west, east));
            saved_again_for_good(&store, &disk, &case);
        }
    }
}

/// An open syncs what is pending, whoever saved it, and a sync that fails drops what
/// is pending. So the regions that saved before an open that fails lose their handles
/// by it, as by any other sync that fails: they would take chunks for saved that are
/// nowhere, and cut their commits off the log with their next checkpoint.
#[test]
fn an_open_whose_sync_fails_loses_the_handles_of_the_regions_that_saved_before_it() {
    let disk = Arc::new(Rigged::over(world()));
    let store = store_over(&disk).unwrap();
    let [west, east, aside] = owners(&store, 2).unwrap();
    // A change of the third region that is in its commit and in no saved chunk.
    log(&aside, 2, &[(16 * UNTOUCHED.x + 1, 150, 1, blocks::GLASS)]);
    committed(&aside, 2);
    save_anew(&west, &east);
    // The saves are done with before the disk refuses anything.
    store.flush().unwrap();
    assert!(!west.is_lost() && !east.is_lost());

    disk.refuse(Some(Refused::Writes));
    assert!(store.open_region(hello(ASIDE, 3)).is_err());
    store.flush().unwrap();
    assert!(
        west.is_lost() && east.is_lost(),
        "of the regions that saved, the western one is lost: {}, and the eastern one: {}",
        west.is_lost(),
        east.is_lost()
    );

    disk.refuse(None);
    drop((west, east));
    saved_again_for_good(&store, &disk, "after an open that failed");
    let (aside, restored) = store.open_region(hello(ASIDE, 4)).unwrap();
    assert_eq!(restored.deltas.len(), 1);
    let mut changed = untouched().clone();
    changed.set(1, 150, 1, blocks::GLASS);
    assert!(load(&aside, UNTOUCHED) == changed);
}

/// So is the sync that the 128th pending chunk sets off: if it fails, every region
/// that saved since the last one loses its handle, and not only the one whose save
/// set it off; a region that saved nothing keeps its own.
#[test]
fn a_sync_set_off_by_the_128th_pending_chunk_that_fails_loses_the_handles_that_saved() {
    let disk = Arc::new(Rigged::over(MemoryDisk::default()));
    let store = store_over(&disk).unwrap();
    let [west, east, aside] = owners(&store, 1).unwrap();
    let chunk = edited();
    for x in 1..=64 {
        save(&west, ChunkPos::new(-x, 0), &chunk);
    }
    for x in 0..63 {
        save(&east, ChunkPos::new(x, 0), &chunk);
    }
    // The saves are done with before the disk refuses anything.
    store.flush().unwrap();
    assert!(!west.is_lost() && !east.is_lost());

    disk.refuse(Some(Refused::Writes));
    save(&east, ChunkPos::new(63, 0), &chunk);
    store.flush().unwrap();
    assert!(
        west.is_lost() && east.is_lost(),
        "of the regions that saved, the western one is lost: {}, and the eastern one: {}",
        west.is_lost(),
        east.is_lost()
    );
    assert!(!aside.is_lost());

    // What was dropped is not taken for saved by anyone: the region is opened again
    // and finds the chunk as it was before, which is as it is generated, or as saved.
    disk.refuse(None);
    let (west, _) = store.open_region(hello(WEST, 2)).unwrap();
    let position = ChunkPos::new(-1, 0);
    let loaded = load(&west, position);
    assert!(loaded == chunk || loaded == generator().generate(position));
}

// 5. Chunk files that cannot be written.

/// Commits a change of the eastern region that is in no saved chunk, has the disk
/// refuse `refused`, and holds the store to not opening the region for as long as it
/// does; and to opening it with the commit, and with the change in the chunk, once
/// the disk works again.
fn is_not_opened_while_the_disk_refuses(refused: Refused) {
    let disk = Arc::new(Rigged::over(world()));
    let store = store_over(&disk).unwrap();
    let position = SAVED[2].0;
    {
        let east = open(&store, hello(EAST, 2));
        log(&east, 2, &[(5, 150, 5, blocks::GLASS)]);
        committed(&east, 2);
    }

    disk.refuse(Some(refused));
    for attempt in 1..=2 {
        match store.open_region(hello(EAST, 3)) {
            Ok(_) => panic!("{refused:?}: the region was opened at attempt {attempt}"),
            Err(error) => assert!(matches!(error, StoreError::Io(_)), "{refused:?}: {error}"),
        }
    }

    disk.refuse(None);
    let (east, restored) = store.open_region(hello(EAST, 4)).unwrap();
    let ticks: Vec<u64> = restored.deltas.iter().map(|delta| delta.tick).collect();
    assert_eq!(ticks, [2], "{refused:?}");
    let mut changed = before(2).clone();
    changed.set(5, 150, 5, blocks::GLASS);
    assert!(load(&east, position) == changed, "{refused:?}");
}

/// A world whose chunk files cannot be written is not opened, instead of being opened
/// and lost at its first checkpoint: whoever opens a region whose commits have to be
/// applied is answered with the error, and the commits stay in the log for an open
/// that works.
#[test]
fn a_store_whose_chunk_files_cannot_be_written_does_not_open_a_region_whose_commits_have_to_be_applied()
 {
    for refused in [Refused::Writes, Refused::Syncs, Refused::Renames] {
        is_not_opened_while_the_disk_refuses(refused);
    }
}

/// Neither is it when the files can be written and their manifests cannot be made
/// durable: what an open applied is synced before the region is handed over, and not
/// left for a checkpoint to find out about.
#[test]
fn a_store_that_cannot_sync_the_directory_of_a_manifest_does_not_open_such_a_region_either() {
    is_not_opened_while_the_disk_refuses(Refused::ManifestDirectories);
}

// 6. The local file system.

/// The local file system syncs a round on threads, more paths than it has threads
/// for, and one path and none as well. A path that is not there is an error of the
/// round, wherever it is among the others, and the round returns all the same: the
/// thread for chunks must not be left waiting or panicked by one file, and what it
/// is told must not be that the round is durable.
#[test]
fn the_local_file_system_syncs_two_hundred_files_at_a_time_and_says_when_one_is_not_there() {
    let directory = tempfile::tempdir().unwrap();
    let disk = OsDisk::default();
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for n in 0..200 {
        let inside = directory.path().join(n.to_string());
        disk.create_dir_all(&inside).unwrap();
        let file = inside.join("file");
        disk.write(&file, n.to_string().as_bytes()).unwrap();
        files.push(file);
        directories.push(inside);
    }
    disk.sync_files(&files).unwrap();
    disk.sync_directories(&directories).unwrap();
    disk.sync_files(&files[..1]).unwrap();
    disk.sync_directories(&directories[..1]).unwrap();
    disk.sync_files(&[]).unwrap();
    disk.sync_directories(&[]).unwrap();

    let missing = directory.path().join("none");
    for at in [0, 100, 199] {
        let mut with = files.clone();
        with[at] = missing.join("file");
        assert!(disk.sync_files(&with).is_err(), "a file missing at {at}");
        let mut with = directories.clone();
        with[at] = missing.clone();
        assert!(
            disk.sync_directories(&with).is_err(),
            "a directory missing at {at}"
        );
    }
    assert!(disk.sync_files(std::slice::from_ref(&missing)).is_err());
    assert!(
        disk.sync_directories(std::slice::from_ref(&missing))
            .is_err()
    );
    // The others are none the worse for it.
    disk.sync_files(&files).unwrap();
    disk.sync_directories(&directories).unwrap();
    for (n, file) in files.iter().enumerate() {
        assert_eq!(disk.read(file).unwrap().unwrap(), n.to_string().as_bytes());
    }
}

// 7. How long it takes.

/// Prints how long two hundred chunks take to save and to sync on the real disk, each
/// with a section of its own as the chunks of a crowd have, for the roadmap. It is a
/// measurement and holds the store to nothing but that the chunks are there
/// afterwards. The files are made where temporary files go: `TMPDIR` says which disk
/// is measured, and the line that is printed says where it was. Run it by itself and
/// with `--ignored --nocapture`.
#[test]
#[ignore = "a measurement: prints how long two hundred chunks take on the real disk"]
fn two_hundred_chunks_are_saved_and_synced_on_the_real_disk() {
    let directory = tempfile::tempdir().unwrap();
    let saved: Vec<(ChunkPos, Chunk)> = (0..200)
        .map(|n: usize| {
            let position = ChunkPos::new(4 * (n % 20) as i32 - 40, (n / 20) as i32);
            let mut chunk = generator().generate(position);
            chunk.set(3, 100, 4, blocks::GLASS);
            chunk.set(n % 16, 200, n / 16, blocks::STONE);
            (position, chunk)
        })
        .collect();
    let mut chunks = FileChunks::new(Arc::new(OsDisk::default()), directory.path());

    let begun = Instant::now();
    for (position, chunk) in &saved {
        chunks.save(*position, 1, chunk).unwrap();
    }
    let noted = begun.elapsed();
    chunks.sync().unwrap();
    let whole = begun.elapsed();
    println!(
        "two hundred chunks in {}: {whole:?}, of which the saves took {noted:?} and the sync {:?}",
        directory.path().display(),
        whole - noted
    );

    let mut again = FileChunks::new(Arc::new(OsDisk::default()), directory.path());
    for (position, chunk) in &saved {
        assert!(
            again.load(*position).unwrap().as_ref() == Some(chunk),
            "{position:?}"
        );
    }
}
