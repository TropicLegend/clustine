//! A world without pins killed at every point: its home region claims chunks, builds
//! in them, gives one back and makes checkpoints, a part is split off it, grows and
//! builds on, and the home region absorbs it again; all on a simulated disk that stops
//! for good at the `n`th change or sync, for every `n`. What each kind of crash leaves
//! of that disk is then opened by a new store, which has to have one home region,
//! region 0, and every claim and every commit that was answered.
//!
//! `kill_regions.rs` does this, and more, with regions that are pinned and a home
//! region that only looks on. Here the home region is the one that does everything,
//! as it is in a world whose regions follow their players
//! (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.1).

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::blocks;
use clustine_rpc::SplitPart;
use clustine_world::{BlockPos, Chunk, ChunkPos, EntityIds};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{SURVIVALS, hello_of, store_on};
use crate::unpinned::{HOME, NO_ENTITY_IDS, answer, open_world, opened};

/// The chunks the scenario works on besides the home chunk: one that stays the home
/// region's, two that go with the part, one that the home region gives back and the
/// part claims, and one that only the part claims.
const NEAR: ChunkPos = ChunkPos::new(3, -3);
const EAST: ChunkPos = ChunkPos::new(4, -3);
const EAST_TOO: ChunkPos = ChunkPos::new(5, -3);
const LEFT: ChunkPos = ChunkPos::new(-1, 0);
const GROWN: ChunkPos = ChunkPos::new(1, 7);

/// A block that a commit set to stone, and no other commit: its chunk, and where it
/// is in the chunk.
type Block = (ChunkPos, usize, i32, usize);

/// What the scenario asked for and was told.
#[derive(Debug, Default)]
struct Told {
    /// The blocks of the commits that were answered.
    confirmed: Vec<Block>,
    /// The chunks of the claims that were answered with the chunk granted, and that
    /// nobody has asked to give back since.
    granted: BTreeSet<ChunkPos>,
    /// Whether the split was asked for and not declined, and if so, whether it was
    /// answered.
    split: Option<bool>,
    /// The same of the merge.
    merge: Option<bool>,
}

/// A region as the scenario plays it, which keeps its promises: it has the chunks it
/// builds in as the store gave them to it, and saves them before it makes a
/// checkpoint or gives one back.
struct Actor {
    region: u32,
    epoch: u64,
    handle: StoreHandle,
    /// The last tick it has run.
    tick: u64,
    /// The chunks it builds in, as it has them.
    chunks: BTreeMap<ChunkPos, Chunk>,
    /// The blocks of its commits that are not answered yet, by their ticks.
    unanswered: BTreeMap<u64, Vec<Block>>,
}

impl Actor {
    /// Opens the region, if the store still does that.
    fn open(store: &Store, region: u32, epoch: u64) -> Option<Actor> {
        let hello = hello_of(&open_world(), region, epoch);
        let (handle, restored) = store.open_region(hello).ok()?;
        Some(Actor {
            region,
            epoch,
            handle,
            tick: restored.tick(),
            chunks: BTreeMap::new(),
            unanswered: BTreeMap::new(),
        })
    }

    /// Asks for a flush and takes the answers until it is answered: everything asked
    /// before is done then. Commits and claims that are answered count as such, also
    /// if the handle is lost right after. Returns the other answers, or `None` if the
    /// handle was lost first.
    fn settle(&mut self, told: &mut Told) -> Option<Vec<StoreReply>> {
        self.handle.request(StoreRequest::Flush);
        let mut others = Vec::new();
        while let Ok(reply) = self.handle.replies.recv() {
            match reply {
                StoreReply::Committed { tick } => {
                    let blocks = self.unanswered.remove(&tick);
                    told.confirmed.extend(blocks.unwrap_or_default());
                }
                StoreReply::Claimed { granted, .. } => told.granted.extend(granted),
                StoreReply::Flushed => return Some(others),
                other => others.push(other),
            }
        }
        None
    }

    fn claim(&mut self, told: &mut Told, chunks: &[ChunkPos]) -> Option<()> {
        self.handle.request(StoreRequest::Claim {
            chunks: chunks.to_vec(),
        });
        self.settle(told).map(|_| ())
    }

    /// Has the chunk as the store has it.
    fn load(&mut self, told: &mut Told, position: ChunkPos) -> Option<()> {
        if !self.chunks.contains_key(&position) {
            self.handle.request(StoreRequest::Load { position });
            for reply in self.settle(told)? {
                if let StoreReply::Loaded { position, chunk } = reply {
                    self.chunks.insert(position, chunk);
                }
            }
        }
        self.chunks.contains_key(&position).then_some(())
    }

    /// Runs a tick that sets a block in each of `chunks`, which it holds, and waits
    /// until the commit is answered.
    fn build(&mut self, told: &mut Told, chunks: &[ChunkPos]) -> Option<()> {
        for chunk in chunks {
            self.load(told, *chunk)?;
        }
        self.tick += 1;
        // A block of its own for every tick of every owner of every region.
        let (x, y, z) = (
            (self.tick % 16) as usize,
            100 + self.region as i32,
            (self.epoch % 16) as usize,
        );
        let mut changes = Vec::new();
        for chunk in chunks {
            let loaded = self.chunks.get_mut(chunk).expect("loaded above");
            loaded.set(x, y, z, blocks::STONE);
            let block = BlockPos::new(chunk.x * 16 + x as i32, y, chunk.z * 16 + z as i32);
            changes.push((block, blocks::STONE));
        }
        let blocks = chunks.iter().map(|chunk| (*chunk, x, y, z)).collect();
        self.unanswered.insert(self.tick, blocks);
        self.handle.request(StoreRequest::Commit {
            tick: self.tick,
            changes,
            state: self.state("delta"),
        });
        self.settle(told).map(|_| ())
    }

    /// A state of the region, which the store does not look into.
    fn state(&self, kind: &str) -> Vec<u8> {
        format!("{kind} of region {} at tick {}", self.region, self.tick).into_bytes()
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

    /// Saves the chunks it has and hands the store its whole state.
    fn checkpoint(&mut self, told: &mut Told) -> Option<()> {
        for position in self.chunks.keys() {
            self.save(*position);
        }
        self.handle.request(StoreRequest::Checkpoint {
            tick: self.tick,
            state: self.state("state"),
        });
        self.settle(told).map(|_| ())
    }

    /// Saves the chunk and gives it back.
    fn give_back(&mut self, told: &mut Told, chunk: ChunkPos) -> Option<()> {
        self.save(chunk);
        self.chunks.remove(&chunk);
        self.handle.request(StoreRequest::Return {
            chunks: vec![chunk],
        });
        told.granted.remove(&chunk);
        self.settle(told).map(|_| ())
    }

    /// Splits `chunks` off as the region `part`. `None` unless the split was answered.
    fn split(&mut self, told: &mut Told, chunks: &[ChunkPos], part: u32) -> Option<()> {
        let tick = self.tick + 1;
        self.handle.request(StoreRequest::SplitCommit {
            tick,
            state: self.state("rest"),
            part: SplitPart {
                chunks: chunks.to_vec(),
                state: self.state("part"),
            },
            as_epoch: 1,
            region: RegionId(part),
        });
        told.split = Some(false);
        // The chunks may be the part's now, answered or not; the region leaves them be.
        for chunk in chunks {
            self.chunks.remove(chunk);
        }
        for reply in self.settle(told)? {
            match reply {
                StoreReply::Split { .. } => {
                    told.split = Some(true);
                    self.tick = tick;
                    return Some(());
                }
                StoreReply::Declined { .. } => told.split = None,
                _ => {}
            }
        }
        None
    }

    /// Absorbs the region `absorbed`, which is open with `epoch`. `None` unless the
    /// merge was answered.
    fn absorb(&mut self, told: &mut Told, absorbed: u32, epoch: u64) -> Option<()> {
        let tick = self.tick + 1;
        self.handle.request(StoreRequest::AbsorbCommit {
            absorbed: RegionId(absorbed),
            absorbed_epoch: epoch,
            tick,
            state: self.state("merged"),
        });
        told.merge = Some(false);
        for reply in self.settle(told)? {
            match reply {
                StoreReply::Absorbed { .. } => {
                    told.merge = Some(true);
                    self.tick = tick;
                    return Some(());
                }
                StoreReply::Declined { .. } => told.merge = None,
                _ => {}
            }
        }
        None
    }
}

/// The scenario, which ends where the store first fails it: `None` then, and `Some`
/// if it got through.
fn scenario(store: &Store, told: &mut Told) -> Option<()> {
    let mut home = Actor::open(store, 0, 1)?;
    home.claim(told, &[NEAR, EAST, EAST_TOO, LEFT])?;
    home.build(told, &[HOME, NEAR, LEFT, EAST])?;
    home.give_back(told, LEFT)?;
    home.build(told, &[EAST_TOO])?;
    home.checkpoint(told)?;

    // The first region that is made in such a world is region 1.
    home.split(told, &[EAST, EAST_TOO], 1)?;
    let mut part = Actor::open(store, 1, 1)?;
    part.claim(told, &[GROWN, LEFT])?;
    part.build(told, &[EAST, GROWN, LEFT])?;
    home.build(told, &[NEAR, HOME])?;
    part.checkpoint(told)?;
    part.build(told, &[EAST_TOO])?;

    part.checkpoint(told)?;
    home.checkpoint(told)?;
    home.absorb(told, 1, part.epoch)?;
    home.build(told, &[GROWN, EAST_TOO, LEFT])?;

    // Another owner of the home region goes on from what the store has of it.
    let mut next = Actor::open(store, 0, 2)?;
    next.build(told, &[HOME, EAST])?;
    Some(())
}

/// Checks what a crash left on `disk` against what the scenario was told.
fn check(told: &Told, disk: &MemoryDisk, survival: Survival, case: &str) {
    let world = open_world();
    let left = Arc::new(disk.crashed(survival));
    let store = store_on(&left, &world).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (list, regions) = opened(&store, &world, 1000, case);

    // The home region is region 0 whatever happened, holds the home chunk from the
    // start, and is the only region that has entity ids; and nothing is pinned.
    let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
    assert_eq!((list.home, living[0]), (RegionId(0), 0), "{case}");
    assert!(regions[0].1.held.contains(&(HOME, 0)), "{case}");
    let first = EntityIds::block(0).unwrap();
    for (info, (_, restored)) in list.regions.iter().zip(&regions) {
        assert!(
            info.pinned.is_empty() && restored.pinned.is_empty(),
            "{case}"
        );
        let issued = if info.region == list.home {
            first
        } else {
            NO_ENTITY_IDS
        };
        assert_eq!(restored.entity_ids, issued, "{case}");
    }
    // No chunk is two regions'.
    let mut holders: BTreeMap<ChunkPos, u32> = BTreeMap::new();
    for (info, (_, restored)) in list.regions.iter().zip(&regions) {
        for (chunk, _) in &restored.held {
            let other = holders.insert(*chunk, info.region.0);
            assert_eq!(other, None, "{case}: {chunk:?} is held twice");
        }
    }

    // A split that was answered has happened, and one that was never asked for has
    // not; either way the part is a region, or was one and has been absorbed, or
    // never was, and its id is used or not accordingly. The same for the merge.
    let parted = living == [0, 1];
    let merged = list.absorbed == [(RegionId(1), RegionId(0))];
    assert!(parted || living == [0], "{case}: {living:?}");
    assert!(merged || list.absorbed.is_empty(), "{case}");
    assert!(!(parted && merged), "{case}");
    let next = if parted || merged { 2 } else { 1 };
    assert_eq!(list.next, RegionId(next), "{case}");
    match told.split {
        Some(true) => assert!(parted || merged, "{case}: an answered split is gone"),
        Some(false) => {}
        None => assert!(!parted && !merged, "{case}: a split nobody was to get"),
    }
    match told.merge {
        Some(true) => assert!(merged, "{case}: an answered merge is gone"),
        Some(false) => {}
        None => assert!(!merged, "{case}: a merge nobody was to get"),
    }
    if merged {
        let gone = store.open_region(hello_of(&world, 1, 2000));
        assert!(
            matches!(gone, Err(StoreError::Absorbed { .. })),
            "{case}: {:?}",
            gone.err()
        );
    }

    // Every claim that was answered holds: the chunk is its region's, or that of the
    // region a split or a merge took it to.
    for chunk in &told.granted {
        assert!(
            holders.contains_key(chunk),
            "{case}: {chunk:?} was granted and is nobody's"
        );
    }

    // A store that starts on what this one left says the same.
    for (handle, _) in &regions {
        handle.flush();
    }
    let found: Vec<Restored> = regions.into_iter().map(|(_, restored)| restored).collect();
    drop(store);
    let again = Arc::new(left.crashed(Survival::Nothing));
    let case = format!("{case}, started again");
    let store = store_on(&again, &world).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (same, regions) = opened(&store, &world, 1001, &case);
    assert_eq!(same, list, "{case}");
    let restored: Vec<&Restored> = regions.iter().map(|(_, restored)| restored).collect();
    assert_eq!(restored, found.iter().collect::<Vec<_>>(), "{case}");

    // Every block of a commit that was confirmed is in its chunk, as whoever holds the
    // chunk loads it. A chunk that is nobody's is claimed by the home region first.
    let chunks: BTreeSet<ChunkPos> = told.confirmed.iter().map(|block| block.0).collect();
    let home = &regions[0].0;
    home.request(StoreRequest::Claim {
        chunks: chunks.iter().copied().collect(),
    });
    answer(home, &case);
    for position in chunks {
        let mut loaded = Vec::new();
        for (handle, _) in &regions {
            handle.request(StoreRequest::Load { position });
            if let StoreReply::Loaded { chunk, .. } = answer(handle, &case) {
                loaded.push(chunk);
            }
        }
        assert_eq!(
            loaded.len(),
            1,
            "{case}: {position:?} is loaded by one region"
        );
        for (_, x, y, z) in told.confirmed.iter().filter(|block| block.0 == position) {
            assert_eq!(
                loaded[0].get(*x, *y, *z),
                Some(blocks::STONE),
                "{case}: the block at ({x}, {y}, {z}) of {position:?} was confirmed"
            );
        }
    }
}

/// Runs the scenario on a disk with `fault`, and checks what every kind of crash
/// leaves of it. Returns what the scenario was told, and how many changes and syncs
/// it made.
///
/// The fault is one that stops the disk for good, or none: a disk that has stopped
/// does not change any more, whatever the store's threads still try, and without a
/// fault the scenario ends with everything done. So what a crash leaves does not
/// depend on how far those threads have got.
fn the_world_is_whole_after(fault: Fault) -> (Told, u64) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    let mut told = Told::default();
    if let Ok(store) = store_on(&disk, &open_world()) {
        scenario(&store, &mut told);
    }
    let operations = disk.operations();
    for survival in SURVIVALS {
        check(&told, &disk, survival, &format!("{fault:?}, {survival:?}"));
    }
    (told, operations)
}

/// Without a fault the scenario gets through all of it, or the faults would be tried
/// on less than it is meant to cover. Returns how many changes and syncs it makes,
/// with some to spare for the ways the threads can interleave.
fn operations() -> u64 {
    let (told, operations) = the_world_is_whole_after(Fault::Fail(u64::MAX));
    assert_eq!(
        (told.split, told.merge),
        (Some(true), Some(true)),
        "{told:?}"
    );
    let granted = [NEAR, EAST, EAST_TOO, LEFT, GROWN];
    assert_eq!(told.granted, BTreeSet::from(granted), "{told:?}");
    assert_eq!(told.confirmed.len(), 16, "{told:?}");
    operations + 10
}

#[test]
fn a_store_that_stops_at_any_point_leaves_a_world_without_pins_whole() {
    for n in 1..=operations() {
        the_world_is_whole_after(Fault::Stop(n));
    }
}
