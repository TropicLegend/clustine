//! The store's regions killed at every point: a scenario of claims, returns, a merge
//! and a split, run on a simulated disk that stops for good, or fails a few times in a
//! row, at the `n`th change or sync, for every `n`. What a crash leaves of that disk is
//! then opened by a new store, and what that store says of the regions, and has in
//! the chunks, is checked against what was answered. See section 4.3 of
//! `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! The scenario plays regions that keep their promises: each has the chunks it works
//! on as the store gave them to it, changes them as it commits, and saves them before
//! it makes a checkpoint or gives one away. What a region's state is, the store does
//! not look into; here it is the list of every commit the region stands on, so that
//! what a region is restored with says which block changes the world has to have.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks};
use clustine_rpc::{RegionList, SplitPart};
use clustine_world::{BlockPos, Chunk, ChunkPos};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{SURVIVALS, gap, hello_of, store_on};
use crate::tests::generator;

/// The chunks the scenario works on: one in each pinned area, a second one in the
/// western area, and two in the gap, which nobody holds until they are claimed.
const WEST: ChunkPos = ChunkPos::new(-2, 0);
const WEST_TOO: ChunkPos = ChunkPos::new(-3, 0);
const EAST: ChunkPos = ChunkPos::new(20, 0);
const FREE: ChunkPos = ChunkPos::new(5, 5);
const FREE_TOO: ChunkPos = ChunkPos::new(6, 5);
const CHUNKS: [ChunkPos; 5] = [WEST, WEST_TOO, EAST, FREE, FREE_TOO];

/// A commit: which region made it, in which tick, and in which of its lives, of which
/// a region begins a new one each time it is opened. `phase` says how far the scenario
/// was, and `chunk` which chunk the commit changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Commit {
    region: u32,
    tick: u64,
    life: u32,
    phase: u32,
    chunk: ChunkPos,
}

impl Commit {
    fn encode(&self) -> String {
        let Commit {
            region,
            tick,
            life,
            phase,
            chunk,
        } = self;
        format!("{region} {tick} {life} {phase} {} {}", chunk.x, chunk.z)
    }

    fn decode(text: &str) -> Commit {
        let numbers: Vec<i64> = text
            .split(' ')
            .map(|number| number.parse().expect("a commit is six numbers"))
            .collect();
        let [region, tick, life, phase, x, z] = numbers[..] else {
            panic!("not a commit: {text}");
        };
        Commit {
            region: region as u32,
            tick: tick as u64,
            life: life as u32,
            phase: phase as u32,
            chunk: ChunkPos::new(x as i32, z as i32),
        }
    }

    /// The block that this commit sets and no other: a change that got into a stored
    /// chunk without its commit being part of the world stays in sight.
    fn block(&self) -> BlockPos {
        BlockPos::new(
            self.chunk.x * 16 + (self.tick % 16) as i32,
            100 + self.region as i32 * 16 + self.life as i32 % 16,
            self.chunk.z * 16 + (self.tick / 16 % 15) as i32,
        )
    }

    /// The block of its chunk that every commit to the chunk sets, to a state that says
    /// which commit it was: the last one to the chunk has to win.
    fn flag(&self) -> (BlockPos, BlockState) {
        let position = BlockPos::new(self.chunk.x * 16 + 15, 90, self.chunk.z * 16 + 15);
        (
            position,
            BlockState(1 + self.phase as u16 * 64 + self.tick as u16 % 64),
        )
    }

    /// Makes the change of this commit in `chunk`.
    fn apply(&self, chunk: &mut Chunk) {
        let (x, z) = self.block().in_chunk();
        chunk.set(x, self.block().y, z, blocks::STONE);
        let (flag, state) = self.flag();
        let (x, z) = flag.in_chunk();
        chunk.set(x, flag.y, z, state);
    }
}

/// The whole state of a region: every commit it stands on.
fn encode(ledger: &[Commit]) -> Vec<u8> {
    let lines: Vec<String> = ledger.iter().map(Commit::encode).collect();
    lines.join("\n").into_bytes()
}

/// The commits a region that is restored with `restored` stands on. Panics if that is
/// not one history: a state the scenario made, and after it a commit of the region for
/// each tick.
fn ledger(region: u32, restored: &Restored) -> Vec<Commit> {
    let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).expect("states are text");
    let mut ledger: Vec<Commit> = match &restored.state {
        Some(state) => text(&state.state).lines().map(Commit::decode).collect(),
        None => Vec::new(),
    };
    let first = restored.state.as_ref().map_or(0, |state| state.tick) + 1;
    for (tick, delta) in (first..).zip(&restored.deltas) {
        let commit = Commit::decode(&text(&delta.state));
        assert_eq!(
            (commit.region, commit.tick, delta.tick),
            (region, tick, tick),
            "a tick is missing or twice, or of another region: {restored:?}"
        );
        ledger.push(commit);
    }
    let mut ticks = BTreeSet::new();
    for commit in ledger.iter().filter(|commit| commit.region == region) {
        assert!(ticks.insert(commit.tick), "a tick twice: {ledger:?}");
    }
    ledger
}

/// What the scenario asked for and was told.
#[derive(Debug, Default)]
struct Told {
    /// The commits that were answered: region, tick and life.
    confirmed: BTreeSet<(u32, u64, u32)>,
    /// The claims that were answered with the chunk granted.
    granted: BTreeSet<(u32, ChunkPos)>,
    /// The chunks a region asked to give back, and has not been granted again since.
    returned: BTreeSet<(u32, ChunkPos)>,
    /// The merge, if it was asked for and not declined.
    merge: Option<Merge>,
    /// The split, if it was asked for and not declined.
    split: Option<Split>,
}

#[derive(Debug)]
struct Merge {
    answered: bool,
    /// The state the survivor asked to be merged into.
    ledger: Vec<Commit>,
}

#[derive(Debug)]
struct Split {
    /// The new region, if the split was answered.
    answered: Option<u32>,
    chunks: Vec<ChunkPos>,
    /// The state of the part, and of what is left.
    ledger: Vec<Commit>,
}

/// A region as the scenario plays it.
struct Actor {
    region: u32,
    epoch: u64,
    life: u32,
    handle: StoreHandle,
    /// The last tick it has run.
    tick: u64,
    ledger: Vec<Commit>,
    /// The chunks it works on, as it has them.
    chunks: BTreeMap<ChunkPos, Chunk>,
}

impl Actor {
    /// Asks for a flush and takes the answers until it is answered or the handle is
    /// lost. Commits and claims that are answered count as such, also if the handle is
    /// lost right after. Returns the other answers.
    fn settle(&mut self, told: &mut Told) -> Vec<StoreReply> {
        self.handle.request(StoreRequest::Flush);
        let mut others = Vec::new();
        while let Ok(reply) = self.handle.replies.recv() {
            match reply {
                StoreReply::Committed { tick } => {
                    told.confirmed.insert((self.region, tick, self.life));
                }
                StoreReply::Claimed { granted, .. } => {
                    for chunk in granted {
                        // Granted, also after the region gave the chunk back before.
                        told.returned.remove(&(self.region, chunk));
                        told.granted.insert((self.region, chunk));
                    }
                }
                StoreReply::Flushed => break,
                other => others.push(other),
            }
        }
        others
    }

    /// Has the chunk as the store has it, if the region holds it.
    fn load(&mut self, told: &mut Told, position: ChunkPos) -> bool {
        if self.chunks.contains_key(&position) {
            return true;
        }
        self.handle.request(StoreRequest::Load { position });
        for reply in self.settle(told) {
            if let StoreReply::Loaded { position, chunk } = reply {
                self.chunks.insert(position, chunk);
            }
        }
        self.chunks.contains_key(&position)
    }

    /// Runs a tick that changes `chunk`, if the region holds the chunk.
    fn commit(&mut self, told: &mut Told, phase: u32, chunk: ChunkPos) {
        if !self.load(told, chunk) {
            return;
        }
        self.tick += 1;
        let commit = Commit {
            region: self.region,
            tick: self.tick,
            life: self.life,
            phase,
            chunk,
        };
        commit.apply(self.chunks.get_mut(&chunk).expect("loaded above"));
        self.handle.request(StoreRequest::Commit {
            tick: self.tick,
            changes: vec![(commit.block(), blocks::STONE), commit.flag()],
            state: commit.encode().into_bytes(),
        });
        self.ledger.push(commit);
    }

    fn save(&self, position: ChunkPos) {
        if let Some(chunk) = self.chunks.get(&position) {
            self.handle.request(StoreRequest::Save {
                position,
                tick: self.tick,
                chunk: chunk.clone(),
            });
        }
    }

    /// Saves what it has changed and hands the store its whole state.
    fn checkpoint(&mut self) {
        for position in self.chunks.keys() {
            self.save(*position);
        }
        self.handle.request(StoreRequest::Checkpoint {
            tick: self.tick,
            state: encode(&self.ledger),
        });
    }

    fn claim(&mut self, chunk: ChunkPos) {
        // What is there may be another region's work: it is loaded afresh.
        self.chunks.remove(&chunk);
        self.handle.request(StoreRequest::Claim {
            chunks: vec![chunk],
        });
    }

    /// Saves the chunk and gives it back, if the region has it.
    fn give_back(&mut self, told: &mut Told, chunk: ChunkPos) {
        if self.chunks.contains_key(&chunk) {
            self.save(chunk);
            self.handle.request(StoreRequest::Return {
                chunks: vec![chunk],
            });
            told.returned.insert((self.region, chunk));
            self.chunks.remove(&chunk);
        }
    }
}

/// Sees to it that `actor` plays `region` with a handle that is not lost, opening the
/// region again, with a higher epoch and in a new life, if it is. A fault may come in
/// the middle of that, so it is tried a few times. Returns whether there is a handle.
fn ensure(store: &Store, actor: &mut Option<Actor>, region: u32) -> bool {
    if actor.as_ref().is_some_and(|actor| !actor.handle.is_lost()) {
        return true;
    }
    let (epoch, life) = match actor.take() {
        Some(actor) => (actor.epoch + 1, actor.life + 1),
        None => (1, 0),
    };
    *actor = (epoch..epoch + 3).find_map(|epoch| {
        let (handle, restored) = store.open_region(hello_of(&gap(), region, epoch)).ok()?;
        Some(Actor {
            region,
            epoch,
            life,
            handle,
            tick: restored.tick(),
            ledger: ledger(region, &restored),
            chunks: BTreeMap::new(),
        })
    });
    actor.is_some()
}

/// The scenario: two pinned regions commit and claim chunks of the gap; one changes a
/// chunk, saves it and gives it back, the other claims it, builds over what the first
/// built and gives it back in turn, and the first claims it again, with its own change
/// still in its log; both make a checkpoint; the first absorbs the second, builds on
/// what came with it and makes a checkpoint; it splits a part off, both go on and make
/// a checkpoint each; and both are opened once more and commit on. After a fault that
/// is over, the regions are opened again and the scenario goes on from what they are
/// restored with.
///
/// `seen` is told how far the scenario is, each time it has got a step further and
/// everything it asked for is answered or lost.
fn scenario(store: &Store, seen: &mut dyn FnMut(&Told, u32)) -> Told {
    let mut told = Told::default();
    let (mut first, mut second, mut part) = (None, None, None);

    // A claim in a group with commits of two regions.
    if !ensure(store, &mut first, 0) || !ensure(store, &mut second, 1) {
        return told;
    }
    let (one, two) = (first.as_mut().unwrap(), second.as_mut().unwrap());
    one.load(&mut told, WEST);
    two.load(&mut told, EAST);
    one.commit(&mut told, 1, WEST);
    two.commit(&mut told, 1, EAST);
    one.claim(FREE);
    two.claim(FREE_TOO);
    one.settle(&mut told);
    two.settle(&mut told);
    seen(&told, 1);

    // A return behind a save, and a claim by another region of the chunk returned,
    // which builds over what the first built; and the same the other way round.
    if !ensure(store, &mut first, 0) || !ensure(store, &mut second, 1) {
        return told;
    }
    let (one, two) = (first.as_mut().unwrap(), second.as_mut().unwrap());
    one.commit(&mut told, 2, FREE);
    one.give_back(&mut told, FREE);
    one.settle(&mut told);
    two.claim(FREE);
    two.settle(&mut told);
    two.commit(&mut told, 3, FREE);
    two.commit(&mut told, 3, FREE_TOO);
    two.give_back(&mut told, FREE);
    two.settle(&mut told);
    one.claim(FREE);
    one.settle(&mut told);
    seen(&told, 2);
    two.checkpoint();
    one.checkpoint();
    one.settle(&mut told);
    two.settle(&mut told);
    seen(&told, 3);

    // A merge of two regions of which the absorbed one holds granted chunks, and a
    // checkpoint after it.
    if !ensure(store, &mut first, 0) {
        return told;
    }
    if ensure(store, &mut second, 1) {
        let (one, two) = (first.as_mut().unwrap(), second.as_ref().unwrap());
        let merged: Vec<Commit> = one.ledger.iter().chain(&two.ledger).copied().collect();
        let tick = one.tick + 1;
        one.handle.request(StoreRequest::AbsorbCommit {
            absorbed: RegionId(1),
            absorbed_epoch: two.epoch,
            tick,
            state: encode(&merged),
        });
        told.merge = Some(Merge {
            answered: false,
            ledger: merged.clone(),
        });
        for reply in one.settle(&mut told) {
            match reply {
                StoreReply::Absorbed { .. } => {
                    told.merge.as_mut().unwrap().answered = true;
                    one.tick = tick;
                    one.ledger = merged.clone();
                }
                StoreReply::Declined { .. } => told.merge = None,
                _ => {}
            }
        }
    }
    if !ensure(store, &mut first, 0) {
        return told;
    }
    let one = first.as_mut().unwrap();
    seen(&told, 4);
    one.commit(&mut told, 4, FREE_TOO);
    one.commit(&mut told, 4, WEST);
    one.checkpoint();
    one.settle(&mut told);
    seen(&told, 5);

    // A split, and a checkpoint of each of the two after it.
    if !ensure(store, &mut first, 0) {
        return told;
    }
    let one = first.as_mut().unwrap();
    let chunks: Vec<ChunkPos> = [WEST, FREE_TOO]
        .into_iter()
        .filter(|chunk| one.load(&mut told, *chunk))
        .collect();
    let tick = one.tick + 1;
    one.handle.request(StoreRequest::SplitCommit {
        tick,
        state: encode(&one.ledger),
        part: SplitPart {
            chunks: chunks.clone(),
            state: encode(&one.ledger),
        },
        as_epoch: 1,
        // The world began with three regions, and none was made since.
        region: RegionId(3),
    });
    told.split = Some(Split {
        answered: None,
        chunks: chunks.clone(),
        ledger: one.ledger.clone(),
    });
    for reply in one.settle(&mut told) {
        match reply {
            StoreReply::Split { region } => {
                told.split.as_mut().unwrap().answered = Some(region.0);
                one.tick = tick;
            }
            StoreReply::Declined { .. } => told.split = None,
            _ => {}
        }
    }
    // The chunks may be the part's now, answered or not; the region leaves them be.
    for chunk in &chunks {
        one.chunks.remove(chunk);
    }
    seen(&told, 6);
    // The worker that made the part says hello for it with the epoch it named. A split
    // that was not answered may have happened all the same; its part is the next id.
    let made = told.split.as_ref().map(|split| split.answered.unwrap_or(3));
    if !ensure(store, &mut first, 0) {
        return told;
    }
    if let Some(made) = made
        && ensure(store, &mut part, made)
    {
        let new = part.as_mut().unwrap();
        new.commit(&mut told, 5, FREE_TOO);
        new.checkpoint();
        new.settle(&mut told);
    }
    let one = first.as_mut().unwrap();
    one.commit(&mut told, 5, WEST_TOO);
    one.checkpoint();
    one.settle(&mut told);
    seen(&told, 7);

    // An opening of the survivor and of the part with a new epoch, after which they
    // commit on.
    if let Some(one) = &first {
        one.handle.lost.store(true, Ordering::SeqCst);
    }
    if ensure(store, &mut first, 0) {
        let one = first.as_mut().unwrap();
        one.commit(&mut told, 6, WEST_TOO);
        one.settle(&mut told);
    }
    if let Some(made) = made {
        if let Some(new) = &part {
            new.handle.lost.store(true, Ordering::SeqCst);
        }
        if ensure(store, &mut part, made) {
            let new = part.as_mut().unwrap();
            new.commit(&mut told, 6, WEST);
            new.settle(&mut told);
        }
    }
    told
}

/// What a store that starts on a disk says of the world on it: the list of regions,
/// and what each is restored with.
#[derive(Debug, PartialEq)]
struct World {
    list: RegionList,
    restored: Vec<(RegionId, Restored)>,
}

/// Starts a store on `disk`, reads the list and opens every living region with `epoch`.
/// Returns what it says, with the store and the handles.
fn world(disk: &Arc<MemoryDisk>, epoch: u64, case: &str) -> (World, Store, Vec<StoreHandle>) {
    let store = store_on(disk, &gap()).unwrap_or_else(|error| panic!("{case}: {error}"));
    let mut list = store
        .regions()
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let mut restored = Vec::new();
    let mut handles = Vec::new();
    for info in &mut list.regions {
        // The epochs are the scenario's and the checks' own, and no part of the world.
        info.epoch = 0;
        let (handle, region) = store
            .open_region(hello_of(&gap(), info.region.0, epoch))
            .unwrap_or_else(|error| panic!("{case}: region {}: {error}", info.region));
        // What it had replayed is in the stored chunks before the next one is opened.
        handle.flush();
        restored.push((info.region, region));
        handles.push(handle);
    }
    (World { list, restored }, store, handles)
}

/// Checks what a crash left on `disk` against what the scenario was told.
fn check(told: &Told, disk: &MemoryDisk, survival: Survival, case: &str) {
    let left = Arc::new(disk.crashed(survival));
    let (found, store, handles) = world(&left, 1000, case);
    let living: BTreeSet<u32> = found
        .list
        .regions
        .iter()
        .map(|info| info.region.0)
        .collect();
    let absorbed: BTreeMap<u32, u32> = found
        .list
        .absorbed
        .iter()
        .map(|(absorbed, into)| (absorbed.0, into.0))
        .collect();

    // No chunk is two regions', and no region that was absorbed can be opened.
    let mut holders: BTreeMap<ChunkPos, u32> = BTreeMap::new();
    for (region, restored) in &found.restored {
        for (chunk, _) in &restored.held {
            let other = holders.insert(*chunk, region.0);
            assert_eq!(other, None, "{case}: {chunk:?} is held twice");
        }
    }
    for (gone, into) in &absorbed {
        assert!(!living.contains(gone), "{case}");
        let opened = store.open_region(hello_of(&gap(), *gone, 2000));
        assert!(
            matches!(opened, Err(StoreError::Absorbed { into: named, .. }) if named.0 == *into),
            "{case}: {:?}",
            opened.err()
        );
    }

    // A merge that was answered has happened, and one that was declined or never asked
    // for has not; either way it is whole: the survivor stands on what it asked to be
    // merged into and the other is gone, or both are regions as before.
    let ledgers: BTreeMap<u32, Vec<Commit>> = found
        .restored
        .iter()
        .map(|(region, restored)| (region.0, ledger(region.0, restored)))
        .collect();
    let merged = absorbed.get(&1) == Some(&0);
    match &told.merge {
        Some(merge) => {
            assert!(
                merged || !merge.answered,
                "{case}: an answered merge is gone"
            );
            if merged {
                let survivor: BTreeSet<&Commit> = ledgers[&0].iter().collect();
                assert!(
                    merge.ledger.iter().all(|commit| survivor.contains(commit)),
                    "{case}: the survivor is not restored with the merged state"
                );
            }
        }
        None => assert!(!merged, "{case}: a merge nobody was to get"),
    }
    assert_eq!(merged, !living.contains(&1), "{case}");
    assert!(absorbed.keys().all(|gone| *gone == 1), "{case}");

    // The same for the split, with the part in the list or not.
    let parts: Vec<u32> = living
        .iter()
        .copied()
        .filter(|region| *region > 2)
        .collect();
    match &told.split {
        Some(split) => {
            if let Some(made) = split.answered {
                assert_eq!(parts, [made], "{case}: an answered split is gone");
            }
            if let [made] = parts[..] {
                let part: BTreeSet<&Commit> = ledgers[&made].iter().collect();
                assert!(
                    split.ledger.iter().all(|commit| part.contains(commit)),
                    "{case}: the part is not restored with its state"
                );
                let held: BTreeSet<ChunkPos> = holders
                    .iter()
                    .filter(|(_, holder)| **holder == made)
                    .map(|(chunk, _)| *chunk)
                    .collect();
                assert_eq!(held, split.chunks.iter().copied().collect(), "{case}");
            } else {
                assert_eq!(parts, [], "{case}");
            }
        }
        None => assert_eq!(parts, [], "{case}: a split nobody was to get"),
    }

    // Every claim that was answered holds, unless its region gave the chunk back or a
    // merge or a split that happened took it elsewhere.
    for (region, chunk) in &told.granted {
        if told.returned.contains(&(*region, *chunk)) {
            continue;
        }
        let holder = holders.get(chunk).copied();
        let moved = holder
            .is_some_and(|holder| absorbed.get(region) == Some(&holder) || parts.contains(&holder));
        assert!(
            holder == Some(*region) || moved,
            "{case}: {chunk:?} was granted to region {region} and is {holder:?}'s"
        );
    }

    // Every commit that was confirmed is part of the world: of what its region is
    // restored with, or of what the region that absorbed it is.
    let counted: BTreeSet<Commit> = ledgers.values().flatten().copied().collect();
    let known: BTreeSet<(u32, u64, u32)> = counted
        .iter()
        .map(|commit| (commit.region, commit.tick, commit.life))
        .collect();
    for confirmed in &told.confirmed {
        assert!(
            known.contains(confirmed),
            "{case}: commit {confirmed:?} was confirmed and is in nothing restored: {ledgers:?}"
        );
    }

    // A store that starts on what this one left says the same, also if it is killed at
    // any point of its start.
    for handle in &handles {
        handle.flush();
    }
    drop((handles, store));
    let changes = {
        let probe = Arc::new(disk.crashed(survival));
        drop(store_on(&probe, &gap()));
        probe.operations()
    };
    for n in 1..=changes {
        let stopped = Arc::new(disk.crashed(survival).with(Fault::Stop(n)));
        drop(store_on(&stopped, &gap()));
        for survival in SURVIVALS {
            let again = Arc::new(stopped.crashed(survival));
            let case = format!("{case}, started again after Stop({n}), {survival:?}");
            assert_eq!(world(&again, 1000, &case).0, found, "{case}");
        }
    }
    let again = Arc::new(left.crashed(Survival::Nothing));
    let case = format!("{case}, started again");
    let (same, _store, handles) = world(&again, 1001, &case);
    assert_eq!(same, found, "{case}");

    // The chunks are as the commits of the world left them, as whoever holds each
    // loads it: every block change of a commit that counts, none of one that does not,
    // and where several set the same block, what the last holder set.
    let home = handles
        .iter()
        .zip(&same.restored)
        .find(|(_, (region, _))| *region == same.list.home)
        .map(|(handle, _)| handle)
        .expect("the home region is always there");
    for position in CHUNKS {
        // A chunk that is nobody's is claimed, to be looked at.
        home.request(StoreRequest::Claim {
            chunks: vec![position],
        });
        let mut loaded = None;
        for handle in &handles {
            handle.request(StoreRequest::Load { position });
            handle.request(StoreRequest::Flush);
            while let Ok(reply) = handle.replies.recv() {
                match reply {
                    StoreReply::Loaded { chunk, .. } => {
                        assert!(loaded.is_none(), "{case}: two regions load {position:?}");
                        loaded = Some(chunk);
                    }
                    StoreReply::Flushed => break,
                    _ => {}
                }
            }
        }
        let mut expected = generator().generate(position);
        let mut commits: Vec<&Commit> = counted
            .iter()
            .filter(|commit| commit.chunk == position)
            .collect();
        // The flag is that of the commit made last, which is the one that is applied
        // last here.
        commits.sort_by_key(|commit| (commit.phase, commit.tick));
        for commit in commits {
            commit.apply(&mut expected);
        }
        let loaded = loaded.unwrap_or_else(|| panic!("{case}: nobody loads {position:?}"));
        assert!(
            loaded == expected,
            "{case}: {position:?} is not as the commits of the world left it: {:?}, with {ledgers:?}",
            differences(&loaded, &expected)
        );
    }
}

/// The blocks in which two chunks differ, each with what the first and the second has
/// there, for whoever reads why a check failed.
fn differences(one: &Chunk, other: &Chunk) -> Vec<String> {
    let mut differences = Vec::new();
    for y in one.min_y()..one.min_y() + one.height() as i32 {
        for z in 0..16 {
            for x in 0..16 {
                let (has, expected) = (one.get(x, y, z), other.get(x, y, z));
                if has != expected {
                    differences.push(format!("({x}, {y}, {z}): {has:?}, not {expected:?}"));
                }
            }
        }
    }
    differences
}

/// Runs the scenario on a disk with `fault`, and checks what every kind of crash
/// leaves of it at the end. A fault that ends lets the scenario go on, and what was cut
/// off the log for it is soon gone with its segment; so the disk is also looked at
/// once on the way, at the first step after the fault is over, with nothing kept and
/// with the truncations lost, which is where what was cut off would come back.
fn the_world_is_whole_after(fault: Fault) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    let over = match fault {
        Fault::Fails(n, count) => Some(n + count),
        Fault::Fail(_) | Fault::Stop(_) => None,
    };
    let mut looked = false;
    let mut seen = |told: &Told, step: u32| {
        if !looked && over.is_some_and(|over| disk.operations() >= over) {
            looked = true;
            for survival in [Survival::Nothing, Survival::Untruncated] {
                let case = format!("{fault:?}, after step {step}, {survival:?}");
                check(told, &disk, survival, &case);
            }
        }
    };
    let told = match store_on(&disk, &gap()) {
        Ok(store) => scenario(&store, &mut seen),
        Err(_) => Told::default(),
    };
    for survival in SURVIVALS {
        check(&told, &disk, survival, &format!("{fault:?}, {survival:?}"));
    }
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave. Without a fault the scenario gets
/// through all of it, or the faults would be tried on less than it is meant to cover.
fn operations() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let told = scenario(&store, &mut |_, _| {});
    assert!(told.merge.as_ref().is_some_and(|merge| merge.answered));
    let split = told.split.as_ref().expect("the split is asked for");
    assert_eq!(split.answered, Some(3), "{told:?}");
    assert_eq!(split.chunks, [WEST, FREE_TOO], "{told:?}");
    let granted = [(0, FREE), (1, FREE), (1, FREE_TOO)];
    assert_eq!(told.granted, BTreeSet::from(granted), "{told:?}");
    assert_eq!(told.returned, BTreeSet::from([(1, FREE)]), "{told:?}");
    assert_eq!(told.confirmed.len(), 11, "{told:?}");
    // The table file was written anew, and segments of the log went for it.
    let table = disk
        .read(Path::new("/world/regions/table"))
        .unwrap()
        .unwrap();
    let table = clustine_format::TableFile::decode(&table).unwrap();
    assert!(table.from > 1, "{table:?}");
    assert_eq!((table.next_region, table.absorbed), (4, vec![(1, 0)]));
    let segments = disk.list(Path::new("/world/log")).unwrap();
    assert!(
        segments
            .iter()
            .all(|name| *name >= format!("{:020}.wal", table.from)),
        "{segments:?}"
    );
    disk.operations() + 10
}

#[test]
fn nothing_goes_wrong_without_a_fault() {
    the_world_is_whole_after(Fault::Fail(u64::MAX));
}

#[test]
fn a_store_that_stops_at_any_point_leaves_its_regions_whole() {
    for n in 1..=operations() {
        the_world_is_whole_after(Fault::Stop(n));
    }
}

/// One fault, two and three in a row: the second and third are those that make cutting
/// the log back fail after the write before it has.
#[test]
fn a_store_that_fails_once_at_any_point_leaves_its_regions_whole() {
    for n in 1..=operations() {
        the_world_is_whole_after(Fault::Fails(n, 1));
    }
}

#[test]
fn a_store_that_fails_twice_at_any_point_leaves_its_regions_whole() {
    for n in 1..=operations() {
        the_world_is_whole_after(Fault::Fails(n, 2));
    }
}

#[test]
fn a_store_that_fails_three_times_at_any_point_leaves_its_regions_whole() {
    for n in 1..=operations() {
        the_world_is_whole_after(Fault::Fails(n, 3));
    }
}

/// What two faults in a row found in the chunk store: a section file whose directory
/// could not be synced, and which could then not be removed either, was taken for a
/// stored one by the next save of a chunk that has the section, whose manifest then
/// named a section that a crash takes away. Whatever fails while a chunk is saved, a
/// save that succeeds afterwards and is made durable is there after a crash.
#[test]
fn a_chunk_saved_after_faults_in_a_row_is_whole_after_a_crash() {
    let root = Path::new("/world");
    let mut chunk = generator().generate(WEST);
    chunk.set(1, 100, 1, blocks::GLASS);
    let changes = {
        let disk = Arc::new(MemoryDisk::default());
        let mut chunks = FileChunks::new(disk.clone(), root);
        chunks.save(WEST, 1, &chunk).unwrap();
        chunks.sync().unwrap();
        disk.operations()
    };
    for n in 1..=changes {
        for count in 1..=3 {
            let disk = Arc::new(MemoryDisk::failing(Fault::Fails(n, count)));
            let mut chunks = FileChunks::new(disk.clone(), root);
            // As a region does that is told its saves may not be durable: again.
            let saved =
                (0..5).any(|_| chunks.save(WEST, 1, &chunk).is_ok() && chunks.sync().is_ok());
            assert!(saved, "Fails({n}, {count})");
            for survival in SURVIVALS {
                let left = Arc::new(disk.crashed(survival));
                let loaded = FileChunks::new(left, root).load(WEST);
                assert!(
                    matches!(&loaded, Ok(Some(loaded)) if *loaded == chunk),
                    "Fails({n}, {count}), {survival:?}: {:?}",
                    loaded.err()
                );
            }
        }
    }
}
