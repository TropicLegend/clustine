//! Tests of the store's part in carrying effects between regions, written from sections
//! 2, 3, 5 and 8 of `docs/adr/0025-effects-between-regions.md` and its "Found while
//! building", by someone who did not write the code (step E6).
//!
//! The tests play every region through a handle of its own. What the store puts on a
//! region's stream comes on the same handle as its answers, so a [`Played`] region sorts
//! what it reads into the two. A test waits for a message, never for time to pass; where
//! it holds that something did not come, it first has the store finish everything asked
//! of it ([`Store::flush`]), behind which every answer the store owes has been sent.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_rpc::{CopyUse, Coupling, Uncoupling};
use clustine_world::{
    BlockPos, Chunk, ChunkPos, Due, Effect, EffectId, Effected, FiledEffect, SeamKind, Ticked,
};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{SURVIVALS, gap, give_back, store_on};
use crate::tests::{division, generator, hello};

/// The home chunk, which region 1 of [`division`] holds, as it holds every chunk with x
/// from 0 on; region 0 holds those west of it.
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
/// A chunk of region 0 of [`division`] and of [`gap`].
const WEST: ChunkPos = ChunkPos::new(-1, 0);
/// A chunk in the gap of [`gap`], which nobody holds until it is claimed.
const FREE: ChunkPos = ChunkPos::new(5, 5);
/// How often a wait looks again before it gives up: half a minute at a millisecond.
const LOOKS: usize = 30_000;

/// Whether a reply is one of those of the stream rather than an answer.
fn of_the_stream(reply: &StoreReply) -> bool {
    matches!(
        reply,
        StoreReply::Effects { .. }
            | StoreReply::CopyBase { .. }
            | StoreReply::CopyUpdate { .. }
            | StoreReply::CopyUsed { .. }
            | StoreReply::CoupleWith { .. }
            | StoreReply::Coupled { .. }
            | StoreReply::HandshakeOff { .. }
            | StoreReply::Shifted { .. }
            | StoreReply::Uncoupled { .. }
            | StoreReply::Progress { .. }
            | StoreReply::Mark { .. }
    )
}

/// A region a test plays: its handle, what came on its stream in order, and its answers.
struct Played {
    handle: StoreHandle,
    stream: Vec<StoreReply>,
    /// How much of the stream the waits have gone past.
    read: usize,
    answers: VecDeque<StoreReply>,
    committed: BTreeSet<u64>,
    /// The tick of the next commit the test makes for the region.
    next: u64,
}

impl Played {
    fn new(handle: StoreHandle) -> Self {
        Self {
            handle,
            stream: Vec::new(),
            read: 0,
            answers: VecDeque::new(),
            committed: BTreeSet::new(),
            next: 1,
        }
    }

    fn open(store: &Store, region: u32, epoch: u64) -> (Self, Restored) {
        let (handle, restored) = store
            .open_region(RegionHello {
                region: RegionId(region),
                epoch,
            })
            .unwrap_or_else(|error| panic!("region {region} with epoch {epoch}: {error}"));
        let mut played = Self::new(handle);
        played.next = restored.tick() + 1;
        (played, restored)
    }

    /// Reads whatever has arrived.
    fn pump(&mut self) {
        while let Some(reply) = self.handle.try_reply() {
            match reply {
                StoreReply::Committed { tick } => {
                    self.committed.insert(tick);
                }
                StoreReply::Generating { .. } | StoreReply::Dead { .. } => {}
                reply if of_the_stream(&reply) => self.stream.push(reply),
                reply => self.answers.push_back(reply),
            }
        }
    }

    /// The next answer that is not of the stream.
    fn answer(&mut self) -> StoreReply {
        for _ in 0..LOOKS {
            self.pump();
            if let Some(answer) = self.answers.pop_front() {
                return answer;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the store did not answer");
    }

    /// Waits for the next message of the stream past those waited for before that
    /// `wanted` holds of, and returns it.
    fn until(&mut self, what: &str, wanted: impl Fn(&StoreReply) -> bool) -> StoreReply {
        for _ in 0..LOOKS {
            self.pump();
            if let Some(found) = self.stream[self.read..].iter().position(&wanted) {
                self.read += found + 1;
                return self.stream[self.read - 1].clone();
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the stream never had {what}: {:?}", self.stream);
    }

    /// Waits for the effect `effect` on the stream, and returns it as it was filed.
    fn effect(&mut self, effect: Effect) -> FiledEffect {
        for _ in 0..LOOKS {
            self.pump();
            if let Some(filed) = self.filed().into_iter().find(|f| f.effect == effect) {
                return filed;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the stream never had {effect:?}: {:?}", self.stream);
    }

    /// Every effect that came on the stream, in the order it came.
    fn filed(&self) -> Vec<FiledEffect> {
        self.stream
            .iter()
            .flat_map(|reply| match reply {
                StoreReply::Effects { effects } => effects.clone(),
                _ => Vec::new(),
            })
            .collect()
    }

    /// Whether the stream ever had a message that `found` holds of.
    fn had(&mut self, found: impl Fn(&StoreReply) -> bool) -> bool {
        self.pump();
        self.stream.iter().any(found)
    }

    fn load(&mut self, position: ChunkPos) -> Chunk {
        self.handle.request(StoreRequest::Load { position });
        match self.answer() {
            StoreReply::Loaded {
                position: loaded,
                chunk,
            } if loaded == position => chunk,
            other => panic!("expected {position:?}, got {other:?}"),
        }
    }

    /// Commits the region's next tick and waits until it is durable.
    fn commit(
        &mut self,
        changes: &[(BlockPos, BlockState)],
        effects: &[Effect],
        taken: &[EffectId],
    ) -> u64 {
        let tick = self.send_commit(changes, effects, taken);
        self.wait_committed(tick);
        tick
    }

    /// Asks for a commit of the region's next tick, and returns the tick.
    fn send_commit(
        &mut self,
        changes: &[(BlockPos, BlockState)],
        effects: &[Effect],
        taken: &[EffectId],
    ) -> u64 {
        let tick = self.next;
        self.next += 1;
        self.handle
            .request(commit_of(tick, changes, effects, taken));
        tick
    }

    fn wait_committed(&mut self, tick: u64) {
        for _ in 0..LOOKS {
            self.pump();
            if self.committed.contains(&tick) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the commit of {tick} was not answered");
    }

    /// Claims `chunks` and returns those granted.
    fn claim(&mut self, chunks: &[ChunkPos]) -> Vec<ChunkPos> {
        self.ask(StoreRequest::Claim {
            chunks: chunks.to_vec(),
        });
        match self.answer() {
            StoreReply::Claimed { granted, .. } => granted,
            other => panic!("expected the answer to a claim, got {other:?}"),
        }
    }

    /// Has the region absorb `absorbed`, opened with `epoch`, at `tick`.
    fn absorb(&mut self, absorbed: u32, epoch: u64, tick: u64) -> StoreReply {
        self.ask(StoreRequest::AbsorbCommit {
            absorbed: RegionId(absorbed),
            absorbed_epoch: epoch,
            tick,
            state: b"merged".to_vec(),
        });
        self.next = tick + 1;
        self.answer()
    }

    /// Splits `chunks` off as the region `part`, to be opened with `as_epoch`.
    fn split(&mut self, tick: u64, chunks: &[ChunkPos], as_epoch: u64, part: u32) -> StoreReply {
        self.ask(StoreRequest::SplitCommit {
            tick,
            state: b"rest".to_vec(),
            part: clustine_rpc::SplitPart {
                chunks: chunks.to_vec(),
                state: b"part".to_vec(),
            },
            as_epoch,
            region: RegionId(part),
        });
        self.next = tick + 1;
        self.answer()
    }

    fn ask(&self, request: StoreRequest) {
        self.handle.request(request);
    }

    /// Waits until the store has done everything asked of it before, through this handle
    /// and any other, and reads what it sent.
    fn settled(&mut self, store: &Store) {
        store.flush().unwrap();
        self.pump();
    }
}

fn commit_of(
    tick: u64,
    changes: &[(BlockPos, BlockState)],
    effects: &[Effect],
    taken: &[EffectId],
) -> StoreRequest {
    StoreRequest::Commit {
        tick,
        changes: changes
            .iter()
            .copied()
            .map(clustine_world::Change::from)
            .collect(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: effects.to_vec(),
        taken: taken.to_vec(),
    }
}

/// A scheduled tick of a block at `at`, which a test tells apart from others by where
/// it is.
fn scheduled(at: BlockPos, delay: i32) -> Effect {
    Effect {
        at,
        what: Effected::Tick {
            of: Ticked::Block(1),
            delay,
            priority: 0,
        },
    }
}

fn update(at: BlockPos) -> Effect {
    Effect {
        at,
        what: Effected::Update {
            direction: 4,
            kind: SeamKind::Shape,
            source: blocks::STONE,
            flags: 3,
            depth: 511,
        },
    }
}

/// A block of the chunk at `chunk`, told apart by `n`.
fn block_in(chunk: ChunkPos, n: i32) -> BlockPos {
    BlockPos::new(chunk.x * 16 + n % 16, -50 + n / 16, chunk.z * 16 + 3)
}

/// The store's clock, which the test moves.
struct Hands(Arc<AtomicU64>);

impl Hands {
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    fn forward(&self, milliseconds: u64) {
        self.0.fetch_add(milliseconds, Ordering::SeqCst);
    }
}

/// A store on `disk` whose commit thread reads the clock the test moves.
fn clocked_on(disk: &Arc<MemoryDisk>, division: &Division) -> (Store, Hands) {
    let now = Arc::new(AtomicU64::new(1_000));
    let read = Arc::clone(&now);
    let clock: crate::lanes::Clock =
        Arc::new(move || Duration::from_millis(read.load(Ordering::SeqCst)));
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    let store = start_clocked(
        disk.clone(),
        root,
        Box::new(chunks),
        generator(),
        division,
        Some(clock),
    )
    .unwrap();
    (store, Hands(now))
}

// Section 5: use, and effects in.

/// Section 5, "Use begins in the group in which the commit thread passes a region's load
/// on", and "Filing": "one whose chunk nobody uses is dropped". An effect dropped so is
/// not kept for a region that loads the chunk later.
#[test]
fn an_effect_into_a_chunk_nobody_uses_is_dropped_and_not_kept_for_a_later_user() {
    let store = Store::memory_told(generator(), division()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    let early = scheduled(block_in(ORIGIN, 1), 3);
    west.commit(&[], &[early], &[]);
    east.load(ORIGIN);
    let late = scheduled(block_in(ORIGIN, 2), 3);
    west.commit(&[], &[late], &[]);
    let filed = east.effect(late);
    // A mark through the effect's sequence follows it (section 2).
    east.until(
        "a mark through the effect",
        |reply| matches!(reply, StoreReply::Mark { through } if *through >= filed.id.sequence),
    );
    east.settled(&store);
    let effects: Vec<Effect> = east.filed().iter().map(|filed| filed.effect).collect();
    assert_eq!(effects, [late]);
    // An effect of a region not coupled to the holder counts from its taking (section 8).
    assert_eq!(filed.due, Due::FromTaking);
    // The maker's own stream has nothing: it uses nothing, copies nothing, is coupled to
    // nobody.
    west.settled(&store);
    assert_eq!(west.stream, []);

    // Nor does a hello of the user bring the dropped one back.
    let (mut again, restored) = Played::open(&store, 1, 2);
    assert!(restored.in_use.contains(&ORIGIN), "{:?}", restored.in_use);
    again.load(ORIGIN);
    let sentinel = scheduled(block_in(ORIGIN, 3), 3);
    west.commit(&[], &[sentinel], &[]);
    again.effect(sentinel);
    let effects: BTreeSet<_> = again.filed().iter().map(|filed| filed.effect.at).collect();
    assert!(!effects.contains(&early.at), "{:?}", again.filed());
    assert!(effects.contains(&late.at), "the untaken effect is resent");
}

/// Section 2: "A region with no copies, no coupling and no effects gets no marks"; E3a:
/// "A group that has nothing for a region tells it nothing."
#[test]
fn a_region_with_no_copies_couplings_or_effects_gets_nothing_on_its_stream() {
    let store = Store::memory_told(generator(), division()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    west.load(WEST);
    for _ in 0..5 {
        west.commit(&[(block_in(WEST, 1), blocks::STONE)], &[], &[]);
        east.commit(&[(block_in(ORIGIN, 1), blocks::GLASS)], &[], &[]);
    }
    west.settled(&store);
    east.settled(&store);
    assert_eq!(west.stream, []);
    assert_eq!(east.stream, []);
}

/// Section 2: the stream is in sequence order and an effect's number is its record's
/// place and its index in it, "unique and increasing"; a mark says through which
/// sequence everything is before it. Effects of one commit come in the order the commit
/// had them.
#[test]
fn effects_come_in_the_order_of_their_numbers_each_behind_its_mark() {
    let store = Store::memory_told(generator(), division()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    let mut made = Vec::new();
    // Sent without waiting, so that several may go into one group.
    let mut last = 0;
    for tick in 0..6 {
        let effects: Vec<Effect> = (0..3)
            .map(|n| scheduled(block_in(ORIGIN, tick * 3 + n), n + 1))
            .collect();
        made.extend(effects.iter().copied());
        last = west.send_commit(&[], &effects, &[]);
    }
    west.wait_committed(last);
    let final_one = *made.last().unwrap();
    let filed = east.effect(final_one);
    east.until(
        "the mark behind the last effect",
        |reply| matches!(reply, StoreReply::Mark { through } if *through >= filed.id.sequence),
    );
    let filed = east.filed();
    let effects: Vec<Effect> = filed.iter().map(|filed| filed.effect).collect();
    assert_eq!(effects, made, "in the order they were made");
    let ids: Vec<EffectId> = filed.iter().map(|filed| filed.id).collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");
    // Every effect has a mark through its sequence after it, and marks do not go back.
    let mut marked = 0;
    let mut pending: Vec<u64> = Vec::new();
    for reply in &east.stream {
        match reply {
            StoreReply::Effects { effects } => {
                for effect in effects {
                    assert!(
                        effect.id.sequence > marked,
                        "an effect of sequence {} came after the mark through {marked}",
                        effect.id.sequence
                    );
                    pending.push(effect.id.sequence);
                }
            }
            StoreReply::Mark { through } => {
                assert!(
                    *through >= marked,
                    "a mark went back from {marked} to {through}"
                );
                marked = *through;
                pending.retain(|sequence| sequence > through);
            }
            other => panic!("nothing else belongs on this stream: {other:?}"),
        }
    }
    assert_eq!(
        pending,
        Vec::<u64>::new(),
        "effects without a mark behind them"
    );
}

/// Section 2: sequences increase "for the life of the world ... across segments and
/// restarts"; section 5, "Taken as a set": "The store drops what is taken. After a hello
/// ... it resends the untaken effects of chunks in use"; "Durable in the log": a taking
/// is a record. Across every way a crash can leave the disk.
#[test]
fn a_taken_effect_is_never_resent_and_numbers_go_on_increasing_after_a_restart() {
    let disk = Arc::new(MemoryDisk::default());
    let ids = {
        let store = store_on(&disk, &division()).unwrap();
        let (mut west, _) = Played::open(&store, 0, 1);
        let (mut east, _) = Played::open(&store, 1, 1);
        east.load(ORIGIN);
        let taken = scheduled(block_in(ORIGIN, 1), 2);
        let kept = update(block_in(ORIGIN, 2));
        west.commit(&[], &[taken, kept], &[]);
        let id = east.effect(taken).id;
        let kept_id = east.effect(kept).id;
        east.commit(&[], &[], &[id]);
        (id, kept_id)
    };
    for (epoch, survival) in (10..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (mut west, _) = Played::open(&store, 0, epoch);
        let (mut east, restored) = Played::open(&store, 1, epoch);
        assert!(
            restored.in_use.contains(&ORIGIN),
            "{survival:?}: {:?}",
            restored.in_use
        );
        east.load(ORIGIN);
        let sentinel = scheduled(block_in(ORIGIN, 9), 2);
        west.commit(&[], &[sentinel], &[]);
        let after = east.effect(sentinel);
        let ids_seen: Vec<EffectId> = east.filed().iter().map(|filed| filed.id).collect();
        assert!(
            !ids_seen.contains(&ids.0),
            "{survival:?}: the taken one came back"
        );
        assert!(
            ids_seen.contains(&ids.1),
            "{survival:?}: the untaken one was lost"
        );
        assert!(
            after.id > ids.1,
            "{survival:?}: {:?} after {:?}",
            after.id,
            ids.1
        );
    }
}

/// Section 5: use "ends ... at a new request `Dropped`", and "Effects still untaken when a
/// chunk's use ends are dropped the same way"; E2: such a dropping is a record of its own
/// (kind 11), so that a restart does not take them for kept.
#[test]
fn dropped_ends_the_use_and_drops_what_was_untaken_also_after_a_restart() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &division()).unwrap();
        let (mut west, _) = Played::open(&store, 0, 1);
        let (mut east, _) = Played::open(&store, 1, 1);
        east.load(ORIGIN);
        let untaken = scheduled(block_in(ORIGIN, 1), 2);
        west.commit(&[], &[untaken], &[]);
        east.effect(untaken);
        east.ask(StoreRequest::Dropped {
            chunks: vec![ORIGIN],
        });
        east.commit(&[], &[], &[]);
        let unused = scheduled(block_in(ORIGIN, 2), 2);
        west.commit(&[], &[unused], &[]);
        east.settled(&store);
        assert_eq!(east.filed().len(), 1, "{:?}", east.stream);
        // Nor does a hello bring either back: neither is in use any more.
        let (mut again, restored) = Played::open(&store, 1, 2);
        assert!(!restored.in_use.contains(&ORIGIN), "{:?}", restored.in_use);
        again.load(ORIGIN);
        let sentinel = scheduled(block_in(ORIGIN, 3), 2);
        west.commit(&[], &[sentinel], &[]);
        again.effect(sentinel);
        let effects: Vec<Effect> = again.filed().iter().map(|filed| filed.effect).collect();
        assert_eq!(effects, [sentinel]);
    }
    // After a restart, which puts every chunk of a region in use until its `Dropped`,
    // the dropped effects stay dropped, and only the one taken by nobody is resent.
    for (epoch, survival) in (10..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (mut west, _) = Played::open(&store, 0, epoch);
        let (mut east, _) = Played::open(&store, 1, epoch);
        east.load(ORIGIN);
        let sentinel = scheduled(block_in(ORIGIN, 4), 2);
        west.commit(&[], &[sentinel], &[]);
        east.effect(sentinel);
        let mut effects: Vec<BlockPos> = east.filed().iter().map(|f| f.effect.at).collect();
        effects.sort();
        let mut expected = vec![block_in(ORIGIN, 3), block_in(ORIGIN, 4)];
        expected.sort();
        assert_eq!(effects, expected, "{survival:?}");
    }
}

/// Section 5: use "ends at a return (from the request; a chunk being returned is not in
/// use)", and "A claim ... begins its use again".
#[test]
fn a_return_ends_the_use_and_a_later_claim_and_load_begin_it_again() {
    let store = Store::memory_told(generator(), gap()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut home, _) = Played::open(&store, 2, 1);
    assert_eq!(home.claim(&[FREE]), [FREE]);
    home.load(FREE);
    let first = scheduled(block_in(FREE, 1), 2);
    west.commit(&[], &[first], &[]);
    let first = home.effect(first);
    // The tick that took it says so; then the chunk goes back.
    home.commit(&[], &[], &[first.id]);
    give_back(&home.handle, &[FREE]);
    home.ask(StoreRequest::Flush);
    assert_eq!(home.answer(), StoreReply::Flushed);
    let while_free = scheduled(block_in(FREE, 2), 2);
    west.commit(&[], &[while_free], &[]);
    home.settled(&store);
    assert_eq!(home.filed().len(), 1, "{:?}", home.stream);

    assert_eq!(home.claim(&[FREE]), [FREE]);
    home.load(FREE);
    let again = scheduled(block_in(FREE, 3), 2);
    west.commit(&[], &[again], &[]);
    home.effect(again);
    home.settled(&store);
    let effects: Vec<Effect> = home.filed().iter().map(|filed| filed.effect).collect();
    assert_eq!(effects, [first.effect, again]);
}

/// Section 5: use "moves with the holding: the record of a merge makes the absorbed
/// region's chunks in use by the absorber, ... untaken effects kept with them. The
/// answer to the merge ... names the chunks whose use moved", and "After ... a merge ...
/// it resends the untaken effects of chunks in use on the stream".
#[test]
fn a_merge_moves_the_use_and_the_untaken_effects_to_the_survivor() {
    let store = Store::memory_told(generator(), gap()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 3);
    let (mut east, _) = Played::open(&store, 1, 1);
    let (mut home, _) = Played::open(&store, 2, 1);
    west.load(WEST);
    let before = update(block_in(WEST, 1));
    east.commit(&[], &[before], &[]);
    let before = west.effect(before);
    home.settled(&store);
    assert_eq!(home.filed(), []);

    match home.absorb(0, 3, 10) {
        StoreReply::Absorbed { in_use, .. } => assert!(in_use.contains(&WEST), "{in_use:?}"),
        other => panic!("expected the merge, got {other:?}"),
    }
    let resent = home.effect(before.effect);
    assert_eq!(resent.id, before.id, "an effect keeps its number");
    let after = update(block_in(WEST, 2));
    east.commit(&[], &[after], &[]);
    home.effect(after);
}

/// Section 5: use moves with the holding, "the record of a split makes the part's chunks
/// in use by the part"; `Restored` lists them; the part's untaken effects are resent to
/// it, and later effects into the part go to it and not to the old region.
#[test]
fn a_split_moves_the_use_of_the_parts_chunks_and_their_effects_to_the_part() {
    let store = Store::memory_told(generator(), gap()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    let parted = ChunkPos::new(-3, 0);
    west.load(WEST);
    west.load(parted);
    let before = scheduled(block_in(parted, 1), 4);
    let staying = scheduled(block_in(WEST, 1), 4);
    east.commit(&[], &[before, staying], &[]);
    west.effect(before);
    west.effect(staying);
    match west.split(5, &[parted], 7, 3) {
        StoreReply::Split { region } => assert_eq!(region, RegionId(3)),
        other => panic!("expected the split, got {other:?}"),
    }
    let (mut part, restored) = Played::open(&store, 3, 7);
    assert!(restored.in_use.contains(&parted), "{:?}", restored.in_use);
    assert!(!restored.in_use.contains(&WEST), "{:?}", restored.in_use);
    part.load(parted);
    part.effect(before);
    let after = scheduled(block_in(parted, 2), 4);
    let after_west = scheduled(block_in(WEST, 2), 4);
    east.commit(&[], &[after, after_west], &[]);
    part.effect(after);
    west.effect(after_west);
    west.settled(&store);
    part.settled(&store);
    assert!(!west.filed().iter().any(|filed| filed.effect == after));
    assert!(!part.filed().iter().any(|filed| filed.effect == after_west));
    assert!(!part.filed().iter().any(|filed| filed.effect == staying));
}

/// Section 5, "Durable in the log": "A group cut off takes its effects, takings and
/// droppings with it, undone in memory as stays are." A taking whose group failed is not
/// a taking: the effect is resent to the region opened again.
#[test]
fn a_taking_in_a_group_that_failed_is_undone_and_the_effect_resent() {
    let (store, disk) = crate::tests::switched_for(&division());
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    let effect = scheduled(block_in(ORIGIN, 1), 2);
    west.commit(&[], &[effect], &[]);
    let filed = east.effect(effect);
    east.handle.flush();
    disk.failing_syncs.store(true, Ordering::SeqCst);
    east.send_commit(&[], &[], &[filed.id]);
    east.handle.flush();
    assert!(east.handle.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (mut west, _) = (0..100)
        .find_map(|_| {
            thread::sleep(Duration::from_millis(1));
            store
                .open_region(hello(0, 2))
                .ok()
                .map(|(handle, restored)| {
                    let mut played = Played::new(handle);
                    played.next = restored.tick() + 1;
                    (played, restored)
                })
        })
        .expect("the store opens regions again once the failure is behind it");
    let (mut again, _) = Played::open(&store, 1, 2);
    again.load(ORIGIN);
    let sentinel = scheduled(block_in(ORIGIN, 2), 2);
    west.commit(&[], &[sentinel], &[]);
    again.effect(sentinel);
    let resent: Vec<EffectId> = again.filed().iter().map(|filed| filed.id).collect();
    assert!(resent.contains(&filed.id), "{:?}", again.stream);
}

// Section 3: border copies.

/// Section 3: "Its base is the chunk's committed form as of a sequence: the stored chunk
/// with its holder's commits since replayed into it"; "Copy updates are the block changes
/// of each later commit in the chunk", each naming "the sequence of that chunk's previous
/// change, so that a gap is visible" (section 2); E3b: the chain is anchored at the
/// base's `last_change`.
#[test]
fn a_copy_is_based_on_the_committed_form_and_kept_up_by_a_chain_of_updates() {
    let store = Store::memory_told(generator(), division()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    let mut expected = east.load(ORIGIN);
    let first = BlockPos::new(3, 100, 4);
    east.commit(&[(first, blocks::GLASS)], &[], &[]);
    expected.set(3, 100, 4, blocks::GLASS);

    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    let (sequence, last_change) =
        match west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. })) {
            StoreReply::CopyBase {
                position,
                sequence,
                last_change,
                chunk,
                used,
            } => {
                assert_eq!(position, ORIGIN);
                assert_eq!(chunk.as_ref(), Some(&expected), "the base has the commit");
                assert_eq!(used, CopyUse::Uncoupled(RegionId(1)));
                assert!(last_change <= sequence);
                (sequence, last_change)
            }
            _ => unreachable!(),
        };
    // A commit that changes nothing in the chunk is no update of it.
    east.commit(
        &[(block_in(ChunkPos::new(1, 0), 1), blocks::STONE)],
        &[],
        &[],
    );
    let mut previous = last_change;
    let mut last = sequence;
    for (n, state) in [blocks::STONE, blocks::DIRT, blocks::AIR]
        .into_iter()
        .enumerate()
    {
        let at = BlockPos::new(5 + n as i32, 90, 6);
        east.commit(&[(at, state)], &[], &[]);
        match west.until("an update", |r| matches!(r, StoreReply::CopyUpdate { .. })) {
            StoreReply::CopyUpdate {
                position,
                sequence,
                previous: before,
                changes,
            } => {
                assert_eq!(position, ORIGIN);
                assert_eq!(changes, [(at, state).into()]);
                assert_eq!(before, previous, "update {n} names the change before it");
                assert!(sequence > last, "update {n} is later than what it follows");
                previous = sequence;
                last = sequence;
            }
            _ => unreachable!(),
        }
        west.until(
            "a mark behind the update",
            |reply| matches!(reply, StoreReply::Mark { through } if *through >= last),
        );
    }
    west.settled(&store);
    let updates = west
        .stream
        .iter()
        .filter(|reply| matches!(reply, StoreReply::CopyUpdate { .. }))
        .count();
    assert_eq!(updates, 3, "{:?}", west.stream);
}

/// E3b: "A base's `last_change` is that anchor, or the chain's last update where the
/// chunk is copied by another region already", so that a second copier's chain joins the
/// first one's.
#[test]
fn a_second_copier_is_anchored_at_the_last_update_of_the_chain() {
    let store = Store::memory_told(generator(), gap()).unwrap();
    let held = ChunkPos::new(16, 0);
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    let (mut home, _) = Played::open(&store, 2, 1);
    east.load(held);
    west.ask(StoreRequest::Copies {
        wanted: vec![held],
        gone: Vec::new(),
    });
    west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
    east.commit(&[(BlockPos::new(260, 90, 3), blocks::STONE)], &[], &[]);
    let update = match west.until("an update", |r| matches!(r, StoreReply::CopyUpdate { .. })) {
        StoreReply::CopyUpdate { sequence, .. } => sequence,
        _ => unreachable!(),
    };
    home.ask(StoreRequest::Copies {
        wanted: vec![held],
        gone: Vec::new(),
    });
    match home.until("a base", |r| matches!(r, StoreReply::CopyBase { .. })) {
        StoreReply::CopyBase {
            last_change, chunk, ..
        } => {
            assert_eq!(last_change, update);
            let chunk = chunk.expect("the chunk is readable");
            assert_eq!(chunk.get(4, 90, 3), Some(blocks::STONE));
        }
        _ => unreachable!(),
    }
    east.commit(&[(BlockPos::new(261, 90, 3), blocks::GLASS)], &[], &[]);
    for copier in [&mut west, &mut home] {
        match copier.until("the next update", |r| {
            matches!(r, StoreReply::CopyUpdate { .. })
        }) {
            StoreReply::CopyUpdate { previous, .. } => assert_eq!(previous, update),
            _ => unreachable!(),
        }
    }
}

/// Section 3: the base is "the stored chunk alone where nobody holds it (land returned),
/// or the generator's chunk where none was ever made"; section 8: the base says the chunk
/// is used by nobody.
#[test]
fn a_copy_of_land_nobody_holds_is_the_generated_chunk_used_by_nobody() {
    let store = Store::memory_told(generator(), gap()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    west.ask(StoreRequest::Copies {
        wanted: vec![FREE],
        gone: Vec::new(),
    });
    match west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. })) {
        StoreReply::CopyBase {
            position,
            chunk,
            used,
            ..
        } => {
            assert_eq!(position, FREE);
            assert_eq!(chunk, Some(generator().generate(FREE)));
            assert_eq!(used, CopyUse::Nobody);
        }
        _ => unreachable!(),
    }
}

/// Section 8: "each later change of its chunk's use, says on the stream whether the chunk
/// is used, and by a region coupled to this one" (`CopyUsed`): a load begins the use,
/// `Dropped` ends it.
#[test]
fn a_copier_is_told_when_its_chunk_comes_to_be_used_and_when_it_is_used_no_more() {
    let store = Store::memory_told(generator(), division()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    match west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. })) {
        StoreReply::CopyBase { used, .. } => assert_eq!(used, CopyUse::Nobody),
        _ => unreachable!(),
    }
    east.load(ORIGIN);
    let used = west.until("word of the use", |r| {
        matches!(r, StoreReply::CopyUsed { .. })
    });
    assert_eq!(
        used,
        StoreReply::CopyUsed {
            position: ORIGIN,
            used: CopyUse::Uncoupled(RegionId(1))
        }
    );
    east.ask(StoreRequest::Dropped {
        chunks: vec![ORIGIN],
    });
    let unused = west.until("word of the end of the use", |r| {
        matches!(r, StoreReply::CopyUsed { .. })
    });
    assert_eq!(
        unused,
        StoreReply::CopyUsed {
            position: ORIGIN,
            used: CopyUse::Nobody
        }
    );
    // A copy that has gone hears nothing more of its chunk.
    west.ask(StoreRequest::Copies {
        wanted: Vec::new(),
        gone: vec![ORIGIN],
    });
    west.settled(&store);
    let heard = west.stream.len();
    east.load(ORIGIN);
    east.commit(&[(BlockPos::new(1, 90, 1), blocks::STONE)], &[], &[]);
    west.settled(&store);
    assert_eq!(west.stream.len(), heard, "{:?}", &west.stream[heard..]);
}

/// E3b: "A hello forgets the copies of its region. Whoever runs it asks for them again";
/// section 3: a copy asked for again is based anew.
#[test]
fn a_hello_forgets_the_regions_copies_until_it_asks_again() {
    let store = Store::memory_told(generator(), division()).unwrap();
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
    let (mut again, _) = Played::open(&store, 0, 2);
    east.commit(&[(BlockPos::new(1, 90, 1), blocks::STONE)], &[], &[]);
    again.settled(&store);
    assert_eq!(again.stream, []);
    again.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    match again.until("a new base", |r| matches!(r, StoreReply::CopyBase { .. })) {
        StoreReply::CopyBase { chunk, .. } => {
            assert_eq!(chunk.expect("readable").get(1, 90, 1), Some(blocks::STONE));
        }
        _ => unreachable!(),
    }
}

/// Section 3: "A stored chunk that cannot be read is answered `Unreadable`, and the copy
/// counts as absent": a base without a chunk.
#[test]
fn a_copy_of_a_stored_chunk_that_cannot_be_read_is_based_without_a_chunk() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::local_told(directory.path(), generator(), division()).unwrap();
    let position = ChunkPos::new(3, 4);
    let (mut west, _) = Played::open(&store, 0, 1);
    let (east, _) = Played::open(&store, 1, 1);
    east.handle.request(StoreRequest::Save {
        position,
        tick: 0,
        chunk: generator().generate(position),
    });
    east.handle.flush();
    let file = directory.path().join("chunks/overworld/0.0/3.4.chunk");
    std::fs::write(&file, b"not a chunk").unwrap();
    west.ask(StoreRequest::Copies {
        wanted: vec![position],
        gone: Vec::new(),
    });
    match west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. })) {
        StoreReply::CopyBase {
            position: based,
            chunk,
            ..
        } => {
            assert_eq!(based, position);
            assert_eq!(chunk, None);
        }
        _ => unreachable!(),
    }
}

// Section 8: lockstep.

/// Two regions of [`division`] coupled by the handshake: the western copies the home
/// chunk, which the eastern uses, beside a chunk that would tick. Each has committed some
/// ticks before; the western names its tick 4 and the eastern its tick 7.
struct Pair {
    store: Store,
    hands: Hands,
    west: Played,
    east: Played,
    /// The offsets each was told.
    west_offset: i64,
    east_offset: i64,
}

fn coupled_pair(disk: &Arc<MemoryDisk>) -> Pair {
    let (store, hands) = clocked_on(disk, &division());
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    west.load(WEST);
    for _ in 0..3 {
        west.commit(&[], &[], &[]);
    }
    for _ in 0..6 {
        east.commit(&[], &[], &[]);
    }
    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
    west.ask(StoreRequest::Beside {
        begins: vec![ORIGIN],
        ends: Vec::new(),
    });
    assert_eq!(
        west.until("couple with", |r| matches!(
            r,
            StoreReply::CoupleWith { .. }
        )),
        StoreReply::CoupleWith {
            partner: RegionId(1)
        }
    );
    assert_eq!(
        east.until("couple with", |r| matches!(
            r,
            StoreReply::CoupleWith { .. }
        )),
        StoreReply::CoupleWith {
            partner: RegionId(0)
        }
    );
    west.ask(StoreRequest::JoinsAt {
        partner: RegionId(1),
        tick: 4,
    });
    east.ask(StoreRequest::JoinsAt {
        partner: RegionId(0),
        tick: 7,
    });
    let west_offset = coupled_offset(&mut west, 1);
    let east_offset = coupled_offset(&mut east, 0);
    assert_eq!(
        4 + west_offset,
        7 + east_offset,
        "the named ticks share a border time"
    );
    Pair {
        store,
        hands,
        west,
        east,
        west_offset,
        east_offset,
    }
}

fn coupled_offset(played: &mut Played, partner: u32) -> i64 {
    match played.until("the coupling", |r| matches!(r, StoreReply::Coupled { .. })) {
        StoreReply::Coupled {
            partner: named,
            offset,
        } => {
            assert_eq!(named, RegionId(partner));
            offset
        }
        _ => unreachable!(),
    }
}

/// The border time of `tick` with `offset`.
fn border(tick: u64, offset: i64) -> u64 {
    u64::try_from(tick as i64 + offset).expect("border time is not below 0")
}

fn uncoupled(played: &mut Played) -> StoreReply {
    played.until("the end of a coupling", |r| {
        matches!(r, StoreReply::Uncoupled { .. })
    })
}

impl Pair {
    /// Both commit their next tick, and the clock moves on by a tick's length.
    fn tick_both(&mut self) {
        let west = self.west.send_commit(&[], &[], &[]);
        let east = self.east.send_commit(&[], &[], &[]);
        self.west.wait_committed(west);
        self.east.wait_committed(east);
        self.hands.forward(50);
    }
}

/// Section 8, the handshake, steps 1 to 3, and "The rule": progress is "given in border
/// time", and E3c: "A region joining a coupling counts as durable through the tick
/// before the one it named." A partner's commit is told as its progress.
#[test]
fn the_handshake_couples_at_the_named_ticks_and_progress_is_told_in_border_time() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    let joined = border(6, pair.east_offset);
    pair.west.until("the partner's progress through the tick before it joined", |r| {
        matches!(r, StoreReply::Progress { partner: RegionId(1), through } if *through == joined)
    });
    pair.east.until("the partner's progress through the tick before it joined", |r| {
        matches!(r, StoreReply::Progress { partner: RegionId(0), through } if *through == joined)
    });
    let east = pair.east.commit(&[], &[], &[]);
    assert_eq!(east, 7);
    let through = border(7, pair.east_offset);
    pair.west.until(
        "the partner's progress",
        |r| matches!(r, StoreReply::Progress { partner: RegionId(1), through: t } if *t == through),
    );
    // Progress never goes back, and is always the partner's.
    let mut last = 0;
    for reply in &pair.west.stream {
        if let StoreReply::Progress { partner, through } = reply {
            assert_eq!(*partner, RegionId(1));
            assert!(
                *through >= last,
                "progress went back: {:?}",
                pair.west.stream
            );
            last = *through;
        }
    }
}

/// Section 8, "Exact delays": a scheduled tick made at the maker's tick m with delay d is
/// due "at the holder's tick m + offset(maker) - offset(holder) + d"; E4: "every effect
/// is due, not only a scheduled tick: the store converts a coupled effect of any kind to
/// `Due::At` the holder's tick it was made in". An effect of a region not coupled to the
/// holder counts from its taking.
#[test]
fn a_coupled_effect_is_due_at_the_holders_tick_by_the_offsets() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    let tick = scheduled(block_in(ORIGIN, 1), 5);
    let shape = update(block_in(ORIGIN, 2));
    let made = pair.west.commit(&[], &[tick, shape], &[]);
    assert_eq!(made, 4);
    let at = |delay: i64| {
        u64::try_from(made as i64 + pair.west_offset - pair.east_offset + delay).unwrap()
    };
    assert_eq!(pair.east.effect(tick).due, Due::At { tick: at(5) });
    assert_eq!(pair.east.effect(shape).due, Due::At { tick: at(0) });
    // Of the two named ticks: the western one's tick 4 is the eastern one's tick 7.
    assert_eq!(at(0), 7);
}

/// E3c: "A change of coupling does not resend `CopyUsed`", and section 8: a base says
/// whether its chunk is used "by a region coupled to this one".
#[test]
fn a_base_made_while_coupled_says_the_chunk_is_used_by_a_coupled_region() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    let other = ChunkPos::new(0, 1);
    pair.east.load(other);
    pair.west.ask(StoreRequest::Copies {
        wanted: vec![other],
        gone: Vec::new(),
    });
    match pair
        .west
        .until("a base", |r| matches!(r, StoreReply::CopyBase { .. }))
    {
        StoreReply::CopyBase { position, used, .. } => {
            assert_eq!(position, other);
            assert_eq!(used, CopyUse::Coupled(RegionId(1)));
        }
        _ => unreachable!(),
    }
    pair.west.settled(&pair.store);
    let resent = pair
        .west
        .stream
        .iter()
        .filter(
            |reply| matches!(reply, StoreReply::CopyUsed { position, .. } if *position == ORIGIN),
        )
        .count();
    assert_eq!(resent, 0, "{:?}", pair.west.stream);
}

/// Section 8, "Ending": "The store ends a coupling, as a record on both streams, once
/// neither region has needed it for `COUPLING_GRACE` (a second ...). Effects filed after
/// that record convert as uncoupled."
#[test]
fn a_coupling_ends_a_second_after_neither_needs_it_and_later_effects_count_from_taking() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    pair.east.commit(&[], &[], &[]);
    pair.west.ask(StoreRequest::Beside {
        begins: Vec::new(),
        ends: vec![ORIGIN],
    });
    // Both go on ticking in lockstep through most of the second.
    let ended = pair.hands.now();
    while pair.hands.now() < ended + 900 {
        pair.tick_both();
    }
    pair.west.settled(&pair.store);
    pair.east.settled(&pair.store);
    assert!(
        !pair.west.had(|r| matches!(r, StoreReply::Uncoupled { .. })),
        "{:?}",
        pair.west.stream
    );
    while pair.hands.now() < ended + 1_100 {
        pair.tick_both();
    }
    let expected = |partner| StoreReply::Uncoupled {
        partner: RegionId(partner),
        why: Uncoupling::Unneeded,
    };
    assert_eq!(uncoupled(&mut pair.west), expected(1));
    assert_eq!(uncoupled(&mut pair.east), expected(0));
    let late = scheduled(block_in(ORIGIN, 3), 5);
    pair.west.commit(&[], &[late], &[]);
    assert_eq!(pair.east.effect(late).due, Due::FromTaking);
}

/// Section 8, "A handshake is not called off because the need ended: it completes".
#[test]
fn a_handshake_whose_need_ends_completes_all_the_same() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, _hands) = clocked_on(&disk, &division());
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
    west.ask(StoreRequest::Beside {
        begins: vec![ORIGIN],
        ends: Vec::new(),
    });
    west.until("couple with", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    east.until("couple with", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    west.ask(StoreRequest::Beside {
        begins: Vec::new(),
        ends: vec![ORIGIN],
    });
    west.ask(StoreRequest::JoinsAt {
        partner: RegionId(1),
        tick: 1,
    });
    east.ask(StoreRequest::JoinsAt {
        partner: RegionId(0),
        tick: 1,
    });
    coupled_offset(&mut west, 1);
    coupled_offset(&mut east, 0);
    assert!(!west.had(|r| matches!(r, StoreReply::HandshakeOff { .. })));
}

/// Section 8, the handshake, step 4: "when one side has not made its 'joins at' durable
/// within `COUPLING_PATIENCE` of the group that said 'couple with'; that side is then
/// given up, and must earn its coupling back".
#[test]
fn a_side_that_does_not_join_within_patience_is_given_up_and_the_handshake_called_off() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, hands) = clocked_on(&disk, &division());
    let (mut west, _) = Played::open(&store, 0, 1);
    let (mut east, _) = Played::open(&store, 1, 1);
    east.load(ORIGIN);
    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
    west.ask(StoreRequest::Beside {
        begins: vec![ORIGIN],
        ends: Vec::new(),
    });
    west.until("couple with", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    east.until("couple with", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    west.ask(StoreRequest::JoinsAt {
        partner: RegionId(1),
        tick: 1,
    });
    west.settled(&store);
    assert!(!west.had(|r| matches!(r, StoreReply::HandshakeOff { .. })));
    // The eastern one never answers.
    hands.forward(300);
    let off = west.until("the handshake called off", |r| {
        matches!(r, StoreReply::HandshakeOff { .. })
    });
    assert_eq!(
        off,
        StoreReply::HandshakeOff {
            partner: RegionId(1)
        }
    );
    east.until("the handshake called off", |r| {
        matches!(
            r,
            StoreReply::HandshakeOff {
                partner: RegionId(0)
            }
        )
    });
    // The western one, which joined, is not coupled for it.
    assert!(!west.had(|r| matches!(r, StoreReply::Coupled { .. })));
    // The side that did not join is given up, as its next owner is told.
    let restored = Played::open(&store, 1, 2).1;
    assert!(restored.given_up, "the side that did not join is given up");
}

/// Section 8, "Never stuck": "A region given up that still needs coupling commits every
/// tick, so that its pace can be seen, and is coupled again by the handshake once it has
/// committed at 15 ticks a second or more for five seconds"; E5: "`CoupleWith` tells a
/// runner given up that it has earned its coupling back".
#[test]
fn a_region_given_up_earns_its_coupling_back_by_keeping_pace_for_five_seconds() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    pair.tick_both();
    pair.west.commit(&[], &[], &[]);
    // The eastern one goes silent and is given up.
    pair.hands.forward(400);
    uncoupled(&mut pair.east);
    uncoupled(&mut pair.west);
    let heard = pair.east.stream.len();
    // It keeps pace from now on, committing every tick, and is handed the handshake
    // again, not before five seconds.
    let from = pair.hands.now();
    let mut earned = None;
    while pair.hands.now() < from + 7_000 {
        pair.tick_both();
        pair.east.pump();
        if pair.east.stream[heard..]
            .iter()
            .any(|r| matches!(r, StoreReply::CoupleWith { .. }))
        {
            earned = Some(pair.hands.now());
            break;
        }
    }
    let earned = earned.expect("the region given up never earned its coupling back");
    assert!(
        earned >= from + 5_000,
        "earned back after {} ms",
        earned - from
    );
    pair.west.until("couple with, again", |r| {
        matches!(
            r,
            StoreReply::CoupleWith {
                partner: RegionId(1)
            }
        )
    });
}

/// Section 8, "Never stuck": "A region is stalled when its progress has not moved for
/// `COUPLING_PATIENCE` ... counted from ... when it stopped waiting. ... The store gives
/// up a stalled region: it ends every coupling of that region, as a record ... the
/// regions that waited for it tick on." "A region that waits counts nothing."
#[test]
fn a_partner_that_stops_committing_is_given_up_after_patience_and_not_the_one_that_waits() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    // Each commits its named tick; the western one goes one further, as far as the
    // rule lets it, and waits from then on.
    pair.tick_both();
    pair.west.commit(&[], &[], &[]);
    pair.west.settled(&pair.store);
    pair.east.settled(&pair.store);
    // The eastern one goes silent. Within patience nothing happens.
    pair.hands.forward(150);
    pair.west.settled(&pair.store);
    assert!(!pair.west.had(|r| matches!(r, StoreReply::Uncoupled { .. })));
    pair.hands.forward(200);
    let expected = |partner| StoreReply::Uncoupled {
        partner: RegionId(partner),
        why: Uncoupling::GivenUp {
            region: RegionId(1),
        },
    };
    assert_eq!(uncoupled(&mut pair.west), expected(1));
    assert_eq!(uncoupled(&mut pair.east), expected(0));
}

/// Section 8, "Never stuck": a region "whose border time advances at under 15 ticks a
/// second for five seconds while a partner waits on it" is given up (the owner,
/// 2026-10-16); one that is merely slow is not stalled.
#[test]
fn a_partner_slower_than_fifteen_ticks_a_second_for_five_seconds_is_given_up() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    pair.tick_both();
    // The western one goes one ahead, as far as the rule lets it, and waits.
    pair.west.commit(&[], &[], &[]);
    let from = pair.hands.now();
    let mut given_up = None;
    // Ten ticks a second: the eastern one commits every 100 ms, and the western one
    // each time goes one ahead again and waits.
    while pair.hands.now() < from + 7_000 {
        pair.east.commit(&[], &[], &[]);
        pair.west.commit(&[], &[], &[]);
        pair.hands.forward(100);
        pair.west.pump();
        if pair.west.had(|r| matches!(r, StoreReply::Uncoupled { .. })) {
            given_up = Some(pair.hands.now());
            break;
        }
    }
    let given_up = given_up.unwrap_or_else(|| {
        pair.east.settled(&pair.store);
        panic!(
            "the slow partner was never given up: west {:?} east {:?}",
            &pair.west.stream[pair.west.stream.len().saturating_sub(6)..],
            &pair.east.stream[pair.east.stream.len().saturating_sub(6)..]
        )
    });
    assert!(
        given_up >= from + 5_000,
        "given up after {} ms",
        given_up - from
    );
    assert_eq!(
        uncoupled(&mut pair.west),
        StoreReply::Uncoupled {
            partner: RegionId(1),
            why: Uncoupling::GivenUp {
                region: RegionId(1)
            }
        }
    );
}

/// Section 8, "Reshapes, moves and restores": "A region entering `Preparing` ... asks the
/// store ... to end its couplings", and "The store also ends a region's couplings at once
/// when its handle is closed, when another owner's hello replaces it".
#[test]
fn couplings_end_at_a_reshape_a_closed_handle_and_a_replacing_hello() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    pair.west.ask(StoreRequest::EndCouplings);
    let reshaping = |partner| StoreReply::Uncoupled {
        partner: RegionId(partner),
        why: Uncoupling::Reshaping {
            region: RegionId(0),
        },
    };
    assert_eq!(uncoupled(&mut pair.west), reshaping(1));
    assert_eq!(uncoupled(&mut pair.east), reshaping(0));

    let mut pair = coupled_pair(&Arc::new(MemoryDisk::default()));
    drop(pair.west);
    assert_eq!(
        uncoupled(&mut pair.east),
        StoreReply::Uncoupled {
            partner: RegionId(0),
            why: Uncoupling::OwnerLost {
                region: RegionId(0)
            }
        }
    );

    let mut pair = coupled_pair(&Arc::new(MemoryDisk::default()));
    let (_replacing, _) = Played::open(&pair.store, 0, 2);
    assert_eq!(
        uncoupled(&mut pair.east),
        StoreReply::Uncoupled {
            partner: RegionId(0),
            why: Uncoupling::OwnerLost {
                region: RegionId(0)
            }
        }
    );
}

/// Section 8, the handshake, step 4: "The store calls a handshake off ... at a hello of
/// either region"; and step 1: "It sends none to a region ... without an owner", while
/// "a region restored after a move or a crash [is] coupled again by the handshake once
/// [it ticks]".
#[test]
fn a_handshake_is_called_off_at_a_hello_and_begun_again_once_the_region_ticks() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, _hands) = clocked_on(&disk, &division());
    let (mut west, _) = Played::open(&store, 0, 1);
    let (east, _) = Played::open(&store, 1, 1);
    let mut east = east;
    east.load(ORIGIN);
    west.ask(StoreRequest::Copies {
        wanted: vec![ORIGIN],
        gone: Vec::new(),
    });
    west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
    west.ask(StoreRequest::Beside {
        begins: vec![ORIGIN],
        ends: Vec::new(),
    });
    west.until("couple with", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    east.until("couple with", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    west.ask(StoreRequest::JoinsAt {
        partner: RegionId(1),
        tick: 1,
    });
    // The eastern one is restored after a crash, by a new owner.
    let (mut again, restored) = Played::open(&store, 1, 2);
    west.until("the handshake called off", |r| {
        matches!(
            r,
            StoreReply::HandshakeOff {
                partner: RegionId(1)
            }
        )
    });
    assert!(restored.in_use.contains(&ORIGIN));
    again.load(ORIGIN);
    again.commit(&[], &[], &[]);
    west.until("couple with, again", |r| {
        matches!(r, StoreReply::CoupleWith { .. })
    });
    again.until("couple with, again", |r| {
        matches!(
            r,
            StoreReply::CoupleWith {
                partner: RegionId(0)
            }
        )
    });
}

/// Section 8: "Couplings ... are kept in a file of their own ... (`regions/couplings`),
/// and `Restored` carries a region's couplings, offsets, partners' progress"; "A restart
/// of the store keeps couplings"; E3c: "After a restart of the store each region's
/// progress is its highest tick in the log".
#[test]
fn couplings_and_their_offsets_survive_a_restart_of_the_store() {
    let disk = Arc::new(MemoryDisk::default());
    let mut pair = coupled_pair(&disk);
    pair.tick_both();
    pair.tick_both();
    pair.west.settled(&pair.store);
    let (west_offset, east_offset) = (pair.west_offset, pair.east_offset);
    let east_last = pair.east.next - 1;
    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let (store, _hands) = clocked_on(&left, &division());
        // The handles are kept: one that is dropped loses its owner, which ends its
        // couplings at once.
        let (_west, west) = Played::open(&store, 0, 10);
        assert_eq!(west.offset, Some(west_offset), "{survival:?}");
        assert_eq!(
            west.couplings,
            [Coupling {
                partner: RegionId(1),
                through: Some(border(east_last, east_offset)),
            }],
            "{survival:?}"
        );
        assert!(!west.given_up);
        let (_east, east) = Played::open(&store, 1, 10);
        assert_eq!(east.offset, Some(east_offset), "{survival:?}");
        assert_eq!(east.couplings.len(), 1, "{survival:?}");
    }
}

/// Section 8: `Restored` carries "any handshake under way"; E3c: "the ticks named in
/// handshakes under way are in the file".
#[test]
fn a_handshake_under_way_survives_a_restart_of_the_store() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let (store, _hands) = clocked_on(&disk, &division());
        let (mut west, _) = Played::open(&store, 0, 1);
        let (mut east, _) = Played::open(&store, 1, 1);
        east.load(ORIGIN);
        west.commit(&[], &[], &[]);
        west.ask(StoreRequest::Copies {
            wanted: vec![ORIGIN],
            gone: Vec::new(),
        });
        west.until("a base", |r| matches!(r, StoreReply::CopyBase { .. }));
        west.ask(StoreRequest::Beside {
            begins: vec![ORIGIN],
            ends: Vec::new(),
        });
        west.until("couple with", |r| {
            matches!(r, StoreReply::CoupleWith { .. })
        });
        east.until("couple with", |r| {
            matches!(r, StoreReply::CoupleWith { .. })
        });
        west.ask(StoreRequest::JoinsAt {
            partner: RegionId(1),
            tick: 2,
        });
        west.commit(&[], &[], &[]);
        west.settled(&store);
    }
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let (store, _hands) = clocked_on(&left, &division());
    let (_, west) = Played::open(&store, 0, 10);
    assert_eq!(west.handshakes, [RegionId(1)]);
    assert_eq!(west.held_tick, Some(2));
}

/// Section 8, the handshake, step 3: "where both are in groups of coupled regions
/// already, the smaller group's offsets all shift by one constant, as a record in
/// sequence order"; E3c: "`Shifted` comes before the `Coupled` that caused it".
#[test]
fn joining_two_groups_shifts_the_offsets_of_one_of_them_before_the_coupling() {
    let disk = Arc::new(MemoryDisk::default());
    let stripes = Division::side_by_side(ORIGIN, &[0, 16, 32]).unwrap();
    let (store, _hands) = clocked_on(&disk, &stripes);
    let mut regions: Vec<Played> = (0..4)
        .map(|region| Played::open(&store, region, 1).0)
        .collect();
    // Region r commits 3 r + 2 ticks, so that every group has its own border time.
    for (n, played) in regions.iter_mut().enumerate() {
        for _ in 0..3 * n + 2 {
            played.commit(&[], &[], &[]);
        }
    }
    // Couples `a`, which copies `chunk`, to `b`, which uses it.
    let couple = |regions: &mut Vec<Played>, a: usize, b: usize, chunk: ChunkPos| {
        regions[b].load(chunk);
        regions[a].ask(StoreRequest::Copies {
            wanted: vec![chunk],
            gone: Vec::new(),
        });
        regions[a].until(
            "a base",
            |r| matches!(r, StoreReply::CopyBase { position, .. } if *position == chunk),
        );
        regions[a].ask(StoreRequest::Beside {
            begins: vec![chunk],
            ends: Vec::new(),
        });
        let named = [regions[a].next, regions[b].next];
        for (side, other) in [(a, b), (b, a)] {
            regions[side].until("couple with", |r| {
                matches!(r, StoreReply::CoupleWith { partner } if *partner == RegionId(other as u32))
            });
        }
        regions[a].ask(StoreRequest::JoinsAt {
            partner: RegionId(b as u32),
            tick: named[0],
        });
        regions[b].ask(StoreRequest::JoinsAt {
            partner: RegionId(a as u32),
            tick: named[1],
        });
        for (side, other) in [(a, b), (b, a)] {
            coupled_offset(&mut regions[side], other as u32);
        }
        named
    };
    couple(&mut regions, 0, 1, ChunkPos::new(0, 0));
    couple(&mut regions, 2, 3, ChunkPos::new(32, 0));
    let named = couple(&mut regions, 1, 2, ChunkPos::new(16, 0));
    for played in regions.iter_mut() {
        played.settled(&store);
    }
    // Each region's offset is the last it was told, by a coupling or a shift.
    let mut current = [0i64; 4];
    for (n, played) in regions.iter().enumerate() {
        let mut offset = None;
        for reply in &played.stream {
            match reply {
                StoreReply::Coupled { offset: told, .. } => {
                    if let Some(known) = offset {
                        // A region keeps its offset when it gains a partner, unless a shift
                        // said otherwise.
                        assert_eq!(*told, known, "region {n}: {:?}", played.stream);
                    }
                    offset = Some(*told);
                }
                StoreReply::Shifted { offset: told } => {
                    offset = Some(*told);
                }
                _ => {}
            }
        }
        current[n] = offset.expect("every region is coupled");
    }
    // The named ticks of the last handshake share a border time, and so do those of the
    // first two, by the offsets in force now.
    assert_eq!(
        named[0] as i64 + current[1],
        named[1] as i64 + current[2],
        "{current:?}"
    );
    // One group was shifted as a whole: both its regions moved by one constant.
    let first_group = current[0] - current[1];
    let second_group = current[2] - current[3];
    assert_eq!(first_group, offsets_difference(&regions[0], &regions[1]));
    assert_eq!(second_group, offsets_difference(&regions[2], &regions[3]));
    let shifted: Vec<usize> = (0..4)
        .filter(|n| {
            regions[*n]
                .stream
                .iter()
                .any(|r| matches!(r, StoreReply::Shifted { .. }))
        })
        .collect();
    assert!(
        shifted == [0, 1] || shifted == [2, 3],
        "shifted: {shifted:?}"
    );
    // On the shifted region of the last handshake, the shift came before its coupling.
    let joining = if shifted == [0, 1] { 1 } else { 2 };
    let stream = &regions[joining].stream;
    let shift = stream
        .iter()
        .position(|r| matches!(r, StoreReply::Shifted { .. }));
    let last_coupled = stream
        .iter()
        .rposition(|r| matches!(r, StoreReply::Coupled { .. }));
    assert!(shift < last_coupled, "{stream:?}");
}

/// The difference of the offsets the first coupling of two regions told them.
fn offsets_difference(a: &Played, b: &Played) -> i64 {
    let first = |played: &Played| {
        played
            .stream
            .iter()
            .find_map(|r| match r {
                StoreReply::Coupled { offset, .. } => Some(*offset),
                _ => None,
            })
            .expect("coupled")
    };
    first(a) - first(b)
}

// A crash at every point.

/// What [`effects_scenario`] did and was told.
#[derive(Debug, Default)]
struct Told {
    /// The western region's commits that were answered.
    made: BTreeSet<u64>,
    /// Whether the eastern region's commit of its taking was answered.
    taking_confirmed: bool,
    /// The effects the eastern region took.
    taken: Vec<Effect>,
    /// Every effect that reached the eastern region.
    delivered: Vec<Effect>,
}

/// The three effects of the scenario, one per commit of the western region.
fn made(tick: u64) -> Effect {
    scheduled(block_in(ORIGIN, tick as i32), 2)
}

/// Asks for a flush and collects answers until it is answered or the handle is lost.
fn settle(played: &mut Played) {
    played.handle.request(StoreRequest::Flush);
    while let Ok(reply) = played.handle.replies.recv() {
        match reply {
            StoreReply::Flushed => return,
            StoreReply::Committed { tick } => {
                played.committed.insert(tick);
            }
            reply if of_the_stream(&reply) => played.stream.push(reply),
            _ => {}
        }
    }
}

/// The eastern region uses the home chunk; the western one commits an effect into it,
/// then another; the eastern takes what has reached it; the western commits a third.
fn effects_scenario(store: &Store) -> Told {
    let mut told = Told::default();
    let Ok((east, _)) = store.open_region(hello(1, 1)) else {
        return told;
    };
    let Ok((west, _)) = store.open_region(hello(0, 1)) else {
        return told;
    };
    let mut east = Played::new(east);
    let mut west = Played::new(west);
    east.handle.request(StoreRequest::Load { position: ORIGIN });
    settle(&mut east);
    for tick in 1..=2 {
        west.handle
            .request(commit_of(tick, &[], &[made(tick)], &[]));
    }
    settle(&mut west);
    let _ = store.flush();
    settle(&mut east);
    let taken: Vec<FiledEffect> = east.filed();
    told.taken = taken.iter().map(|filed| filed.effect).collect();
    let ids: Vec<EffectId> = taken.iter().map(|filed| filed.id).collect();
    east.handle.request(commit_of(1, &[], &[], &ids));
    settle(&mut east);
    told.taking_confirmed = east.committed.contains(&1);
    west.handle.request(commit_of(3, &[], &[made(3)], &[]));
    settle(&mut west);
    told.made = west.committed.clone();
    let _ = store.flush();
    settle(&mut east);
    told.delivered = east.filed().iter().map(|filed| filed.effect).collect();
    told
}

/// Section 5, "Durable in the log", killed at every change and sync of a scenario: an
/// effect that is in the log and not taken by a durable taking is resent, and nothing
/// taken by a durable taking comes back; what was confirmed is in the log.
#[test]
fn effects_and_takings_are_as_durable_as_their_records_at_every_kill_point() {
    let operations = {
        let disk = Arc::new(MemoryDisk::default());
        effects_scenario(&store_on(&disk, &division()).unwrap());
        disk.operations()
    };
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n)] {
            let disk = Arc::new(MemoryDisk::default().with(fault));
            let told = match store_on(&disk, &division()) {
                Ok(store) => effects_scenario(&store),
                Err(_) => Told::default(),
            };
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store = store_on(&left, &division()).unwrap_or_else(|e| panic!("{case}: {e}"));
                let (mut west, restored_west) = Played::open(&store, 0, 100);
                let (mut east, restored_east) = Played::open(&store, 1, 100);
                let in_log: BTreeSet<u64> = restored_west
                    .deltas
                    .iter()
                    .map(|delta| delta.tick)
                    .collect();
                let taking_in_log = restored_east.deltas.iter().any(|delta| delta.tick == 1);
                assert!(told.made.is_subset(&in_log), "{case}: {told:?} {in_log:?}");
                assert!(!told.taking_confirmed || taking_in_log, "{case}");
                east.load(ORIGIN);
                let sentinel = scheduled(block_in(ORIGIN, 15), 2);
                west.commit(&[], &[sentinel], &[]);
                east.effect(sentinel);
                let resent: BTreeSet<BlockPos> =
                    east.filed().iter().map(|filed| filed.effect.at).collect();
                for tick in 1..=3 {
                    let effect = made(tick);
                    let taken = taking_in_log && told.taken.contains(&effect);
                    // An effect of a commit that is not in the log, or that a taking in
                    // the log took, does not come back. One whose chunk was not in use
                    // when it was made was never written; but one that reached its
                    // region was, and it is not lost.
                    if !in_log.contains(&tick) || taken {
                        assert!(
                            !resent.contains(&effect.at),
                            "{case}: the effect of tick {tick} came back (in the log: {in_log:?}, taken: {taken})"
                        );
                    } else if told.delivered.contains(&effect) {
                        assert!(
                            resent.contains(&effect.at),
                            "{case}: the effect of tick {tick} was lost (in the log: {in_log:?}): {told:?}"
                        );
                    }
                }
            }
        }
    }
}
