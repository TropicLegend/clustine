//! Tests of arrivals in the store, written from `docs/adr/0028-entities.md` (sections 5
//! to 7 and its review) and `docs/groundwork/n3-plan.md` (all of it, sections 5 and 6
//! over the earlier ones) by someone who did not write the code. An entity sent across a
//! line is in exactly one place in every durable state: in the sender's chunk, filed as
//! an arrival for the holder of the chunk it went into, or taken there, whatever the
//! store, the sender or the holder lose meanwhile. Arrivals are filed by the chunk's
//! holder and not by its use, taken once by number although they are sent again at
//! every change that could have lost them, and never dropped untaken.
//!
//! Two worlds: the pinned one of [`division`] (region 0 west of x = 0, region 1 east of
//! it, with the home chunk at the origin), where every chunk has a holder, and the one
//! with a gap of [`gap`] (region 0 west of x = 0, region 1 from chunk x = 16 on, the
//! home region 2 holding the origin alone), where land between them is nobody's until
//! it is granted.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Barrier, Mutex};
use std::time::Duration;

use clustine_data::blocks;
use clustine_format::{LogRecord, read_log};
use clustine_rpc::{Parts, Uncoupling};
use clustine_world::{
    Arrival, BlockPos, Chunk, ChunkPos, Due, Effect, EffectId, Effected, Entity, EntityChange,
    EntityData, EntityId, ItemStack, Ticked,
};

use super::*;
use crate::chunks::{Chunks, FileChunks};
use crate::disk::{Fault, MemoryDisk, Survival, replace};
use crate::regions::{SURVIVALS, absorb, claim, gap, split, store_on};
use crate::tests::{division, generator, hello};

const WEST: u32 = 0;
const EAST: u32 = 1;
/// In the world with a gap: the home region, which holds the origin and nothing else.
const HOME: u32 = 2;

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
/// The chunk west of the origin, the western region's in the pinned world.
const W: ChunkPos = ChunkPos::new(-1, 0);
/// In the world with a gap, a chunk nobody holds until it is granted.
const FREE: ChunkPos = ChunkPos::new(1, 0);
/// In the world with a gap, a chunk of region 1's pinned land.
const FAR_EAST: ChunkPos = ChunkPos::new(16, 0);

/// An item lying in the middle of `chunk`, with `hops`.
fn item(key: u64, chunk: ChunkPos, hops: u32) -> Entity {
    Entity {
        key: EntityId(key),
        kind: 71,
        uuid: uuid::Uuid::from_u128(u128::from(key) << 8 | 0x3),
        position: [
            f64::from(chunk.x * 16) + 8.5,
            -60.0,
            f64::from(chunk.z * 16) + 8.5,
        ],
        motion: [0.2, 0.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops,
        lived: 30,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Default::default(),
        data: EntityData::Item {
            stack: ItemStack::new(1, 3),
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

/// The key of the item these tests send across lines.
const KEY: u64 = (1 << 32) | 17;
const OTHER_KEY: u64 = (1 << 32) | 18;

/// A commit of `tick` with these entity changes, arrivals sent and arrivals taken, and
/// nothing else but the region's state.
fn commit(
    handle: &StoreHandle,
    tick: u64,
    entity_changes: Vec<(ChunkPos, EntityChange)>,
    arrivals: Vec<(ChunkPos, Entity)>,
    arrivals_taken: Vec<EffectId>,
) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes,
        arrivals,
        arrivals_taken,
        clock: Default::default(),
        claims: Vec::new(),
        raids: Vec::new(),
    });
}

/// The entity comes to be in its chunk.
fn put(handle: &StoreHandle, tick: u64, entity: &Entity) {
    commit(
        handle,
        tick,
        vec![(entity.chunk(), EntityChange::Whole(entity.clone()))],
        Vec::new(),
        Vec::new(),
    );
}

/// The entity, in its chunk, goes into `to` as an arrival: `Gone` where it was and
/// whole, with the `hops` it had, in the arrival, in one commit (section 5).
fn send(handle: &StoreHandle, tick: u64, entity: &Entity, to: ChunkPos) {
    let mut moved = item(entity.key.0, to, entity.hops);
    moved.uuid = entity.uuid;
    commit(
        handle,
        tick,
        vec![(entity.chunk(), EntityChange::Gone(entity.key))],
        vec![(to, moved)],
        Vec::new(),
    );
}

/// The arrivals are taken: each `Whole` in its chunk with its `hops` one higher, and
/// named by number (section 5, "Taken once").
fn take(handle: &StoreHandle, tick: u64, arrivals: &[Arrival]) {
    let mut taken: Vec<EffectId> = arrivals.iter().map(|arrival| arrival.id).collect();
    taken.sort_unstable();
    commit(
        handle,
        tick,
        arrivals
            .iter()
            .map(|arrival| {
                (
                    arrival.chunk,
                    EntityChange::Whole(taken_in(&arrival.entity)),
                )
            })
            .collect(),
        Vec::new(),
        taken,
    );
}

fn taken_in(entity: &Entity) -> Entity {
    let mut entity = entity.clone();
    entity.hops += 1;
    entity
}

/// Everything `handle` is told until a flush asked for now is answered, or the handle
/// is lost, but for the answers to commits and that chunks are being made.
fn flushed(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut told = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Flushed => break,
            StoreReply::Committed { .. } | StoreReply::Generating { .. } => {}
            other => told.push(other),
        }
    }
    told
}

/// As [`flushed`], with the commits confirmed apart.
fn settle(handle: &StoreHandle) -> (Vec<u64>, Vec<StoreReply>) {
    handle.request(StoreRequest::Flush);
    let mut committed = Vec::new();
    let mut told = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Flushed => break,
            StoreReply::Committed { tick } => committed.push(tick),
            StoreReply::Generating { .. } => {}
            other => told.push(other),
        }
    }
    (committed, told)
}

/// The arrivals among what a region was told, in order.
fn arrivals_in(told: &[StoreReply]) -> Vec<Arrival> {
    told.iter()
        .filter_map(|reply| match reply {
            StoreReply::Arrivals { arrivals } => Some(arrivals.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn keys(arrivals: &[Arrival]) -> Vec<u64> {
    arrivals
        .iter()
        .map(|arrival| arrival.entity.key.0)
        .collect()
}

/// Where in what a region was told the first reply `wanted` picks is.
fn position(told: &[StoreReply], wanted: impl Fn(&StoreReply) -> bool) -> Option<usize> {
    told.iter().position(wanted)
}

fn is_arrivals(reply: &StoreReply) -> bool {
    matches!(reply, StoreReply::Arrivals { .. })
}

/// Loads the chunk, passing over whatever else comes first; `None` if the region does
/// not hold it or it cannot be read.
fn loaded(handle: &StoreHandle, position: ChunkPos) -> Option<Chunk> {
    handle.request(StoreRequest::Load { position });
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Loaded {
                position: at,
                chunk,
            } if at == position => return Some(chunk),
            StoreReply::NotHeld { position: at, .. } | StoreReply::Unreadable { position: at }
                if at == position =>
            {
                return None;
            }
            _ => {}
        }
    }
    None
}

/// How many times the entity of `key` is in the chunk.
fn count_in(chunk: Option<&Chunk>, key: u64) -> usize {
    chunk.map_or(0, |chunk| {
        chunk
            .entities()
            .iter()
            .filter(|entity| entity.key.0 == key)
            .count()
    })
}

/// The chunk at `position` as generated, with `entities` and nothing marked as just
/// made, as a region saves it.
fn chunk_with(position: ChunkPos, entities: Vec<Entity>) -> Chunk {
    let mut chunk = generator().generate(position);
    chunk.set_entities(entities);
    chunk.clear_generated();
    chunk
}

fn save_chunk(handle: &StoreHandle, position: ChunkPos, tick: u64, chunk: Chunk) {
    handle.request(StoreRequest::Save {
        position,
        tick,
        chunk,
        parts: Parts::ALL,
    });
}

/// The records of the log on `disk` as it is now, segment by segment.
fn logged(disk: &MemoryDisk) -> Vec<LogRecord> {
    disk.paths()
        .iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "wal"))
        .flat_map(|path| read_log(&disk.read(path).unwrap().unwrap()).unwrap().0)
        .collect()
}

fn opened(store: &Store, region: u32, epoch: u64) -> (StoreHandle, Restored) {
    store.open_region(hello(region, epoch)).unwrap()
}

fn last_tick(restored: &Restored) -> u64 {
    restored
        .deltas
        .last()
        .map(|delta| delta.tick)
        .or(restored.state.as_ref().map(|state| state.tick))
        .unwrap_or(0)
}

// What is filed, for whom, and sent when.

/// Section 5: the arrival is filed for the chunk's holder and goes on its stream, ahead
/// of a mark through the group that filed it; the sender is sent nothing of it.
#[test]
fn an_arrival_is_filed_for_its_chunks_holder_and_streamed_to_it_ahead_of_a_mark() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (west, _) = opened(&store, WEST, 1);
    let (east, _) = opened(&store, EAST, 1);
    flushed(&west);
    let entity = item(KEY, ORIGIN, 4);
    put(&east, 1, &entity);
    send(&east, 2, &entity, W);
    let told = flushed(&east);
    assert_eq!(arrivals_in(&told), [], "the sender is sent nothing");

    let told = flushed(&west);
    let arrivals = arrivals_in(&told);
    assert_eq!(arrivals.len(), 1, "{told:?}");
    let arrival = &arrivals[0];
    assert_eq!(arrival.chunk, W);
    assert_eq!(arrival.entity.key, EntityId(KEY));
    // With the hops it had: the taking raises them (section 6).
    assert_eq!(arrival.entity.hops, 4);
    let at = position(&told, is_arrivals).unwrap();
    assert!(
        told[at + 1..].iter().any(|reply| matches!(
            reply,
            StoreReply::Mark { through } if *through >= arrival.id.sequence
        )),
        "a mark through the arrival's group comes behind it: {told:?}"
    );
}

/// Section 5 and the plan, section 6 (1): an entity a region sends into a chunk of its
/// own that it has not loaded is filed for itself, on its own stream, due at once.
#[test]
fn an_arrival_into_the_senders_own_chunk_not_loaded_is_its_own_and_due_at_once() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (east, _) = opened(&store, EAST, 1);
    let entity = item(KEY, ORIGIN, 0);
    put(&east, 1, &entity);
    send(&east, 2, &entity, ChunkPos::new(5, 0));
    let arrivals = arrivals_in(&flushed(&east));
    assert_eq!(keys(&arrivals), [KEY]);
    assert_eq!(arrivals[0].chunk, ChunkPos::new(5, 0));
    assert_eq!(arrivals[0].due, Due::FromTaking);
}

/// Section 5, "Held and not loaded", and the plan's N3b: an arrival into the holder's
/// pinned land that it neither loads nor uses still reaches it, and comes again at each
/// hello until it is taken; taken, it never comes again.
#[test]
fn an_arrival_into_pinned_land_the_holder_does_not_use_comes_at_each_hello_until_taken() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (east, _) = opened(&store, EAST, 1);
    let far_west = ChunkPos::new(-30, 12);
    let first = item(KEY, ORIGIN, 0);
    let second = item(OTHER_KEY, ORIGIN, 0);
    put(&east, 1, &first);
    put(&east, 2, &second);
    send(&east, 3, &first, far_west);
    send(&east, 4, &second, W);
    flushed(&east);

    // Nobody had the western region open: its hello brings both, by holder.
    let (west, restored) = opened(&store, WEST, 1);
    assert_eq!(restored.in_use, []);
    let told = flushed(&west);
    let arrivals = arrivals_in(&told);
    assert_eq!(keys(&arrivals), [KEY, OTHER_KEY], "{told:?}");
    assert!(
        matches!(told.last(), Some(StoreReply::Mark { .. })),
        "a mark behind what the hello sends again: {told:?}"
    );
    // Restarted before taking them: both again.
    drop(west);
    let (west, _) = opened(&store, WEST, 2);
    assert_eq!(keys(&arrivals_in(&flushed(&west))), [KEY, OTHER_KEY]);
    // One taken; then only the other.
    take(&west, 1, &arrivals[..1]);
    flushed(&west);
    drop(west);
    let (west, _) = opened(&store, WEST, 3);
    assert_eq!(keys(&arrivals_in(&flushed(&west))), [OTHER_KEY]);
    // And so after the store's own restart, whatever a crash keeps.
    drop((west, east, store));
    for (epoch, survival) in (4..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (west, _) = opened(&store, WEST, epoch);
        assert_eq!(
            keys(&arrivals_in(&flushed(&west))),
            [OTHER_KEY],
            "{survival:?}"
        );
        let far = loaded(&west, far_west);
        assert_eq!(count_in(far.as_ref(), KEY), 1, "{survival:?}");
        assert_eq!(far.unwrap().entities()[0].hops, 1, "{survival:?}");
    }
}

/// Section 5, "Takings are checked by holder", and the plan's sections 5 (D1) and 6
/// (8): a commit that names an arrival not filed, one already taken, or one filed for a
/// chunk another region holds, is refused before anything is appended, and the region
/// is lost; the arrival stays as it was.
#[test]
fn a_commit_taking_an_arrival_not_filed_for_a_chunk_the_region_holds_is_refused_and_appends_nothing()
 {
    for case in ["not filed", "taken already", "held by another"] {
        let disk = Arc::new(MemoryDisk::default());
        let store = store_on(&disk, &division()).unwrap();
        let (west, _) = opened(&store, WEST, 1);
        let (east, _) = opened(&store, EAST, 1);
        let entity = item(KEY, ORIGIN, 0);
        put(&east, 1, &entity);
        send(&east, 2, &entity, W);
        flushed(&east);
        let arrival = arrivals_in(&flushed(&west)).remove(0);
        let (refused_by, tick, ids) = match case {
            "not filed" => (
                &west,
                1,
                vec![EffectId {
                    sequence: arrival.id.sequence,
                    index: 9,
                }],
            ),
            "taken already" => {
                take(&west, 1, std::slice::from_ref(&arrival));
                flushed(&west);
                (&west, 2, vec![arrival.id])
            }
            _ => (&east, 3, vec![arrival.id]),
        };
        let before = logged(&disk);
        commit(
            refused_by,
            tick,
            vec![(W, EntityChange::Whole(taken_in(&arrival.entity)))],
            Vec::new(),
            ids,
        );
        let (committed, _) = settle(refused_by);
        assert_eq!(committed, [], "{case}");
        assert!(refused_by.is_lost(), "{case}: the region is lost");
        assert_eq!(logged(&disk), before, "{case}: nothing is appended");
        // The arrival is as it was: still filed unless it was taken before.
        let (again, _) = opened(&store, WEST, 5);
        let resent = arrivals_in(&flushed(&again));
        if case == "taken already" {
            assert_eq!(resent, [], "{case}");
        } else {
            assert_eq!(resent, std::slice::from_ref(&arrival), "{case}");
        }
    }
}

/// The plan, section 6 (8): a refused commit loses the region with its owner, so that
/// its couplings end: its partner is told.
#[test]
fn a_refused_taking_ends_the_regions_couplings() {
    let (store, west, east) = coupled_pair();
    commit(
        &west,
        100,
        Vec::new(),
        Vec::new(),
        vec![EffectId {
            sequence: 1,
            index: 0,
        }],
    );
    let told = until(&east, |reply| matches!(reply, StoreReply::Uncoupled { .. }));
    assert_eq!(
        told.last(),
        Some(&StoreReply::Uncoupled {
            partner: RegionId(WEST),
            why: Uncoupling::OwnerLost {
                region: RegionId(WEST)
            },
        })
    );
    assert!(west.is_lost());
    drop(store);
}

/// The plan, section 6 (1): an arrival's `Due` is made at filing, from the couplings,
/// as an effect with no delay is: for a coupled holder, the holder's tick that the
/// sender's tick was, so that the entity is ticked once in the tick it crossed in.
#[test]
fn a_coupled_holder_is_sent_an_arrival_due_in_the_tick_the_senders_was() {
    let (_store, west, east) = coupled_pair();
    let entity = item(KEY, ChunkPos::new(-1, 0), 0);
    put(&west, 99, &entity);
    send(&west, 100, &entity, FAR_EAST);
    let told = until(&east, |reply| {
        matches!(reply, StoreReply::Progress { through: 100, .. })
    });
    let arrivals = arrivals_in(&told);
    assert_eq!(arrivals.len(), 1, "{told:?}");
    // The west's tick 100 is the east's 40 (offset 60, `coupled_pair`).
    assert_eq!(arrivals[0].due, Due::At { tick: 40 });
    // Ahead of the progress that says the sender is durable through it.
    let at = position(&told, is_arrivals).unwrap();
    assert!(
        matches!(
            told[at..].last(),
            Some(StoreReply::Progress { through: 100, .. })
        ),
        "{told:?}"
    );
}

/// The store's clock, which the coupled tests move themselves.
#[derive(Clone, Default)]
struct Hands(Arc<Mutex<Duration>>);

impl Hands {
    fn clock(&self) -> lanes::Clock {
        let hands = self.clone();
        Arc::new(move || *hands.0.lock().unwrap())
    }
}

/// Waits for an answer that `wanted` picks and returns what came up to it. A store that
/// never sends it fails the test, after half a minute, rather than hanging it.
fn until(handle: &StoreHandle, wanted: impl Fn(&StoreReply) -> bool) -> Vec<StoreReply> {
    let mut told = Vec::new();
    loop {
        match handle.replies.recv_timeout(Duration::from_secs(30)) {
            Ok(reply) => {
                let found = wanted(&reply);
                told.push(reply);
                if found {
                    return told;
                }
            }
            Err(_) => panic!("it did not come: {told:?}"),
        }
    }
}

/// Regions 0 and 1 of the world with a gap, coupled with the west at its tick 100 and
/// the east at its 40, as `couplings_tests` couples them.
fn coupled_pair() -> (Store, StoreHandle, StoreHandle) {
    let hands = Hands::default();
    let disk = Arc::new(MemoryDisk::default());
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    let store = start_clocked(
        disk.clone(),
        root,
        Box::new(chunks),
        generator(),
        &gap(),
        Some(hands.clock()),
    )
    .unwrap();
    let (west, _) = opened(&store, WEST, 1);
    let (east, _) = opened(&store, EAST, 1);
    assert!(loaded(&east, FAR_EAST).is_some());
    west.request(StoreRequest::Beside {
        begins: vec![FAR_EAST],
        ends: Vec::new(),
    });
    for handle in [&west, &east] {
        until(handle, |reply| {
            matches!(reply, StoreReply::CoupleWith { .. })
        });
    }
    west.request(StoreRequest::JoinsAt {
        partner: RegionId(EAST),
        tick: 100,
    });
    east.request(StoreRequest::JoinsAt {
        partner: RegionId(WEST),
        tick: 40,
    });
    for handle in [&west, &east] {
        until(handle, |reply| matches!(reply, StoreReply::Progress { .. }));
    }
    (store, west, east)
}

// Land nobody holds.

/// Section 5, "into land held by no region": the chunk is granted to the sender in the
/// arrival's group, its `Granted` record before the commit's, once for two arrivals
/// into it (the plan, section 6, 8); the sender is told `Claimed` ahead of the arrivals,
/// which are its own and due at once; and the grant outlives the store.
#[test]
fn an_entity_sent_into_land_nobody_holds_grants_it_to_the_sender_ahead_of_its_arrival() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (home, _) = opened(&store, HOME, 1);
    let (west, _) = opened(&store, WEST, 1);
    let first = item(KEY, ORIGIN, 0);
    let second = item(OTHER_KEY, ORIGIN, 0);
    commit(
        &home,
        1,
        vec![
            (ORIGIN, EntityChange::Whole(first.clone())),
            (ORIGIN, EntityChange::Whole(second.clone())),
        ],
        Vec::new(),
        Vec::new(),
    );
    flushed(&home);
    let before = logged(&disk).len();
    commit(
        &home,
        2,
        vec![
            (ORIGIN, EntityChange::Gone(first.key)),
            (ORIGIN, EntityChange::Gone(second.key)),
        ],
        vec![(FREE, item(KEY, FREE, 0)), (FREE, item(OTHER_KEY, FREE, 0))],
        Vec::new(),
    );
    let told = flushed(&home);
    let claimed = position(&told, |reply| {
        *reply
            == StoreReply::Claimed {
                granted: vec![FREE],
                foreign: Vec::new(),
            }
    })
    .unwrap_or_else(|| panic!("the sender is told of the grant: {told:?}"));
    let arrived = position(&told, is_arrivals).expect("the arrivals are the sender's");
    assert!(claimed < arrived, "{told:?}");
    let arrivals = arrivals_in(&told);
    assert_eq!(keys(&arrivals), [KEY, OTHER_KEY]);
    assert!(
        arrivals
            .iter()
            .all(|arrival| arrival.due == Due::FromTaking)
    );

    let records = logged(&disk);
    let new = &records[before..];
    let grants: Vec<LogRecord> = new
        .iter()
        .filter(|record| matches!(record, LogRecord::Granted { .. }))
        .cloned()
        .collect();
    assert_eq!(
        grants,
        [LogRecord::Granted {
            region: HOME,
            tick: 1,
            chunks: vec![FREE],
        }],
        "one grant, of the chunk once: {new:?}"
    );
    let granted_at = new
        .iter()
        .position(|record| matches!(record, LogRecord::Granted { .. }))
        .unwrap();
    let committed_at = new
        .iter()
        .position(|record| matches!(record, LogRecord::Commit { tick: 2, .. }))
        .unwrap();
    assert!(granted_at < committed_at, "{new:?}");

    // Nobody else is granted it now.
    assert_eq!(
        claim(&west, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(HOME))])
    );
    drop((home, west, store));
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (home, restored) = opened(&store, HOME, epoch);
        assert!(
            restored.held.iter().any(|(chunk, _)| *chunk == FREE),
            "{survival:?}: {:?}",
            restored.held
        );
        assert_eq!(
            keys(&arrivals_in(&flushed(&home))),
            [KEY, OTHER_KEY],
            "{survival:?}"
        );
    }
}

/// Section 5: "the commit is checked before its `Granted` record is appended, so that a
/// refused commit leaves no grant behind".
#[test]
fn a_refused_commit_with_an_arrival_into_land_nobody_holds_grants_nothing() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (home, _) = opened(&store, HOME, 1);
    let (west, _) = opened(&store, WEST, 1);
    let entity = item(KEY, ORIGIN, 0);
    put(&home, 1, &entity);
    flushed(&home);
    let before = logged(&disk);
    home.request(StoreRequest::Commit {
        tick: 2,
        changes: Vec::new(),
        state: b"delta 2".to_vec(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: vec![(ORIGIN, EntityChange::Gone(entity.key))],
        arrivals: vec![(FREE, item(KEY, FREE, 0))],
        arrivals_taken: vec![EffectId {
            sequence: 77,
            index: 0,
        }],
        clock: Default::default(),
        claims: Vec::new(),
        raids: Vec::new(),
    });
    let (committed, told) = settle(&home);
    assert_eq!(committed, []);
    assert!(home.is_lost());
    assert!(
        !told
            .iter()
            .any(|reply| matches!(reply, StoreReply::Claimed { .. })),
        "{told:?}"
    );
    assert_eq!(logged(&disk), before);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], Vec::new()));
}

// Returns.

/// The world with a gap, its store's chunk thread kept busy by a save of [`BUSY`] by
/// region 1 until the test lets it go on, so that a return waits behind it.
struct Gated<C> {
    chunks: C,
    gated: Vec<ChunkPos>,
    barrier: Arc<Barrier>,
}

impl<C: Chunks> Chunks for Gated<C> {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        self.chunks.load(position)
    }

    fn save_parts(
        &mut self,
        position: ChunkPos,
        tick: u64,
        epoch: u64,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError> {
        if self.gated.contains(&position) {
            // Once to say that the save has started, once to be let go on.
            self.barrier.wait();
            self.barrier.wait();
        }
        self.chunks.save_parts(position, tick, epoch, chunk, parts)
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        self.chunks.sync()
    }

    fn note(
        &mut self,
        position: ChunkPos,
        chunk: &Chunk,
        parts: clustine_rpc::Parts,
    ) -> Result<(), StoreError> {
        self.chunks.note(position, chunk, parts)
    }

    fn noted(&self) -> usize {
        self.chunks.noted()
    }
}

const BUSY: ChunkPos = ChunkPos::new(1000, 1000);
/// The same far in the west, which the western region holds.
const BUSY_WEST: ChunkPos = ChunkPos::new(-1000, 1000);

fn gated(disk: &Arc<MemoryDisk>, division: &Division) -> (Store, Arc<Barrier>) {
    let barrier = Arc::new(Barrier::new(2));
    let root = Path::new("/world");
    let chunks = Gated {
        chunks: FileChunks::new(disk.clone(), root),
        gated: vec![BUSY, BUSY_WEST],
        barrier: Arc::clone(&barrier),
    };
    let store = start(disk.clone(), root, Box::new(chunks), generator(), division).unwrap();
    (store, barrier)
}

/// An effect of region 1 into `chunk`, by which a test sees whether the chunk is in
/// use: an effect is filed and sent only for a chunk in use (ADR-0025, section 5).
fn effect_into(chunk: ChunkPos) -> Effect {
    Effect {
        at: BlockPos::new(chunk.x * 16 + 3, -61, chunk.z * 16 + 3),
        what: Effected::Tick {
            of: Ticked::Block(blocks::SAND.0),
            delay: 2,
            priority: 0,
        },
    }
}

fn effects_in(told: &[StoreReply]) -> usize {
    told.iter()
        .map(|reply| match reply {
            StoreReply::Effects { effects } => effects.len(),
            _ => 0,
        })
        .sum()
}

fn commit_effect(handle: &StoreHandle, tick: u64, effect: Effect) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: vec![effect],
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: Default::default(),
        claims: Vec::new(),
        raids: Vec::new(),
    });
}

/// Section 5, "a `Return`": a chunk with an arrival filed, which the release would leave
/// to nobody, is not given back: the region is answered `Claimed` for it and sent the
/// arrival again, the chunk stays its own and in its use; once the arrival is taken the
/// chunk goes back.
#[test]
fn a_return_of_a_chunk_with_an_arrival_filed_is_kept_back_answered_claimed_and_sent_again() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (home, _) = opened(&store, HOME, 1);
    let (west, _) = opened(&store, WEST, 1);
    let (east, _) = opened(&store, EAST, 1);
    assert_eq!(claim(&home, &[FREE]).0, [FREE]);
    assert!(loaded(&home, FREE).is_some());
    let entity = item(KEY, FAR_EAST, 0);
    put(&east, 1, &entity);
    send(&east, 2, &entity, FREE);
    flushed(&east);
    let arrival = arrivals_in(&flushed(&home)).remove(0);

    // The worker forgets the chunk as it sends the return (section 5).
    home.request(StoreRequest::Return {
        chunks: vec![FREE],
        unreadable: Vec::new(),
    });
    let told = flushed(&home);
    let claimed = position(&told, |reply| {
        *reply
            == StoreReply::Claimed {
                granted: vec![FREE],
                foreign: Vec::new(),
            }
    })
    .unwrap_or_else(|| panic!("the return is answered Claimed: {told:?}"));
    let arrived = position(&told, is_arrivals).expect("the arrival is sent again");
    assert!(claimed < arrived, "{told:?}");
    assert_eq!(arrivals_in(&told), std::slice::from_ref(&arrival));
    assert_eq!(
        claim(&west, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(HOME))])
    );
    // Still in its use.
    commit_effect(&east, 3, effect_into(FREE));
    flushed(&east);
    assert_eq!(effects_in(&flushed(&home)), 1, "the chunk is still in use");

    // Taken, the chunk can go.
    take(&home, 1, std::slice::from_ref(&arrival));
    home.request(StoreRequest::Return {
        chunks: vec![FREE],
        unreadable: Vec::new(),
    });
    let told = flushed(&home);
    assert!(
        !told
            .iter()
            .any(|reply| matches!(reply, StoreReply::Claimed { .. })),
        "{told:?}"
    );
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], Vec::new()));
}

/// Section 5 and the plan, sections 5 (G3) and 6 (4): an arrival filed for a chunk its
/// holder is giving back calls the return off: the holder is answered `Claimed` for it
/// in the group and sent the arrival, and the chunk's use begins again. Both for an
/// arrival another region sends and for one the holder sends itself.
#[test]
fn an_arrival_for_a_chunk_being_returned_calls_the_return_off_and_its_use_begins_again() {
    for own in [false, true] {
        let case = if own { "its own" } else { "another's" };
        let disk = Arc::new(MemoryDisk::default());
        let (store, barrier) = gated(&disk, &gap());
        let (home, _) = opened(&store, HOME, 1);
        let (west, _) = opened(&store, WEST, 1);
        let (east, _) = opened(&store, EAST, 1);
        assert_eq!(claim(&home, &[FREE]).0, [FREE]);
        assert!(loaded(&home, FREE).is_some());
        let theirs = item(KEY, FAR_EAST, 0);
        let ours = item(KEY, ORIGIN, 0);
        if own {
            put(&home, 1, &ours);
        } else {
            put(&east, 1, &theirs);
        }
        flushed(&home);
        flushed(&east);

        save_chunk(&east, BUSY, 1, chunk_with(BUSY, Vec::new()));
        barrier.wait();
        // The return waits behind the save; the arrival comes meanwhile.
        home.request(StoreRequest::Return {
            chunks: vec![FREE],
            unreadable: Vec::new(),
        });
        if own {
            send(&home, 2, &ours, FREE);
        } else {
            send(&east, 2, &theirs, FREE);
        }
        home.request(StoreRequest::Flush);
        let told = until(&home, is_arrivals);
        assert!(
            told.contains(&StoreReply::Claimed {
                granted: vec![FREE],
                foreign: Vec::new(),
            }),
            "{case}: {told:?}"
        );
        assert_eq!(keys(&arrivals_in(&told)), [KEY], "{case}");
        barrier.wait();
        until(&home, |reply| *reply == StoreReply::Flushed);
        assert_eq!(
            claim(&west, &[FREE]),
            (Vec::new(), vec![(FREE, RegionId(HOME))]),
            "{case}: the return was called off"
        );
        commit_effect(&east, 3, effect_into(FREE));
        flushed(&east);
        assert_eq!(effects_in(&flushed(&home)), 1, "{case}: in use again");
    }
}

/// Section 5, "Held and not loaded", the plan, section 6 (3), and "any grant of a chunk
/// with arrivals filed": a return of a chunk the region found unreadable goes on, the
/// arrival stays filed for nobody, and whoever is granted the chunk next is sent it,
/// due at once.
#[test]
fn an_unreadable_chunk_goes_back_with_its_arrival_filed_which_goes_to_whoever_is_granted_it() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (home, _) = opened(&store, HOME, 1);
    let (west, _) = opened(&store, WEST, 1);
    let (east, _) = opened(&store, EAST, 1);
    assert_eq!(claim(&home, &[FREE]).0, [FREE]);
    let entity = item(KEY, FAR_EAST, 0);
    put(&east, 1, &entity);
    send(&east, 2, &entity, FREE);
    flushed(&east);
    assert_eq!(keys(&arrivals_in(&flushed(&home))), [KEY]);

    home.request(StoreRequest::Return {
        chunks: vec![FREE],
        unreadable: vec![FREE],
    });
    let told = flushed(&home);
    assert!(
        !told
            .iter()
            .any(|reply| matches!(reply, StoreReply::Claimed { .. })),
        "{told:?}"
    );
    // Nor does an arrival that comes for it now call the return off: it is nobody's.
    let (granted, _) = claim(&west, &[FREE]);
    assert_eq!(granted, [FREE]);
    let told = flushed(&west);
    let arrivals = arrivals_in(&told);
    assert_eq!(keys(&arrivals), [KEY], "{told:?}");
    assert_eq!(arrivals[0].due, Due::FromTaking);
}

// Lends.

const LENDER: u32 = 0;
const BORROWER: u32 = 1;
const AT_LINE: ChunkPos = ChunkPos::new(-1, 3);

fn lend_asked(lender: &StoreHandle) -> u64 {
    let told = until(lender, |reply| {
        matches!(reply, StoreReply::LendAsked { .. })
    });
    match told.last() {
        Some(StoreReply::LendAsked { lend, .. }) => *lend,
        _ => unreachable!(),
    }
}

/// Lends `AT_LINE`; returns what the borrower and the lender were told up to the grant.
fn lend(lender: &StoreHandle, borrower: &StoreHandle) -> (Vec<StoreReply>, Vec<StoreReply>) {
    borrower.request(StoreRequest::Lend {
        chunks: vec![AT_LINE],
    });
    let lend = lend_asked(lender);
    save_chunk(lender, AT_LINE, 1, chunk_with(AT_LINE, Vec::new()));
    lender.request(StoreRequest::Lending {
        lend,
        chunks: vec![AT_LINE],
    });
    let to_lender = until(lender, |reply| matches!(reply, StoreReply::Lent { .. }));
    let to_borrower = flushed(borrower);
    (to_borrower, to_lender)
}

/// Section 5, "a lent chunk": the arrivals filed for a chunk the lender lends go to the
/// borrower with the grant, behind its `Claimed`, due at once (the plan, section 6, 1);
/// the lender is not sent them again.
#[test]
fn an_arrival_for_a_lent_chunk_goes_to_the_borrower_with_the_grant_and_no_more_to_the_lender() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    let entity = item(KEY, ORIGIN, 0);
    put(&borrower, 1, &entity);
    send(&borrower, 2, &entity, AT_LINE);
    flushed(&borrower);
    assert_eq!(keys(&arrivals_in(&flushed(&lender))), [KEY]);

    let (to_borrower, _) = lend(&lender, &borrower);
    let claimed = position(&to_borrower, |reply| {
        *reply
            == StoreReply::Claimed {
                granted: vec![AT_LINE],
                foreign: Vec::new(),
            }
    })
    .unwrap_or_else(|| panic!("{to_borrower:?}"));
    let arrived = position(&to_borrower, is_arrivals)
        .unwrap_or_else(|| panic!("the borrower is sent the arrival: {to_borrower:?}"));
    assert!(claimed < arrived, "{to_borrower:?}");
    let arrivals = arrivals_in(&to_borrower);
    assert_eq!(keys(&arrivals), [KEY]);
    assert_eq!(arrivals[0].due, Due::FromTaking);
    drop(lender);
    let (lender, _) = opened(&store, LENDER, 2);
    assert_eq!(arrivals_in(&flushed(&lender)), []);
    drop(borrower);
    let (borrower, _) = opened(&store, BORROWER, 2);
    // A hello calls the borrower's lends off, but a grant made is a grant.
    assert_eq!(keys(&arrivals_in(&flushed(&borrower))), [KEY]);
}

/// Section 5, "a lent chunk given back": the borrower's return goes on whatever is
/// filed (a pinned holder is left), and the lender is sent the arrivals behind
/// `PinnedBack`, due at once; the borrower no more.
#[test]
fn an_arrival_for_a_lent_chunk_given_back_goes_to_the_lender_behind_pinned_back() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    lend(&lender, &borrower);
    let entity = item(KEY, W, 0);
    put(&lender, 1, &entity);
    send(&lender, 2, &entity, AT_LINE);
    flushed(&lender);
    assert_eq!(keys(&arrivals_in(&flushed(&borrower))), [KEY]);

    borrower.request(StoreRequest::Return {
        chunks: vec![AT_LINE],
        unreadable: Vec::new(),
    });
    let told = flushed(&borrower);
    assert!(
        !told
            .iter()
            .any(|reply| matches!(reply, StoreReply::Claimed { .. })),
        "a borrower's return is not called off: {told:?}"
    );
    let told = until(&lender, is_arrivals);
    let back = position(&told, |reply| {
        *reply
            == StoreReply::PinnedBack {
                chunks: vec![AT_LINE],
            }
    })
    .unwrap_or_else(|| panic!("{told:?}"));
    assert!(back < told.len() - 1, "{told:?}");
    let arrivals = arrivals_in(&told);
    assert_eq!(keys(&arrivals), [KEY]);
    assert_eq!(arrivals[0].due, Due::FromTaking);
    drop(borrower);
    let (borrower, _) = opened(&store, BORROWER, 2);
    assert_eq!(arrivals_in(&flushed(&borrower)), []);
}

/// Section 5, "a return or a lend called off": the lender, which dropped what it kept
/// for a chunk as it said `Lending`, is sent the arrivals again when the lend is called
/// off (here by the borrower's hello while the lender's saves wait).
#[test]
fn a_lend_called_off_after_the_lender_let_go_sends_the_arrivals_again_to_the_lender() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &division());
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    let entity = item(KEY, ORIGIN, 0);
    put(&borrower, 1, &entity);
    send(&borrower, 2, &entity, AT_LINE);
    // Checkpointed, so that its hello below has nothing for the thread for chunks to
    // replay, which the held save keeps busy.
    save_chunk(&borrower, ORIGIN, 2, chunk_with(ORIGIN, Vec::new()));
    borrower.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"borrower 2".to_vec(),
    });
    flushed(&borrower);
    assert_eq!(keys(&arrivals_in(&flushed(&lender))), [KEY]);

    borrower.request(StoreRequest::Lend {
        chunks: vec![AT_LINE],
    });
    let number = lend_asked(&lender);
    // The lender's save is held, so that the lend waits for it.
    save_chunk(&lender, BUSY_WEST, 3, chunk_with(BUSY_WEST, Vec::new()));
    barrier.wait();
    lender.request(StoreRequest::Lending {
        lend: number,
        chunks: vec![AT_LINE],
    });
    // The borrower restarts: its lends are called off.
    drop(borrower);
    let (borrower, _) = opened(&store, BORROWER, 2);
    let told = until(&lender, is_arrivals);
    assert!(
        told.iter()
            .any(|reply| matches!(reply, StoreReply::LendCalledOff { .. })),
        "{told:?}"
    );
    assert_eq!(keys(&arrivals_in(&told)), [KEY]);
    barrier.wait();
    flushed(&lender);
    // And the chunk is still the lender's.
    assert_eq!(
        claim(&borrower, &[AT_LINE]),
        (Vec::new(), vec![(AT_LINE, RegionId(LENDER))])
    );
}

// Merges and splits.

/// Section 5, "A holder that cannot take it yet": at a merge every arrival whose holder
/// the merge made the survivor goes on the survivor's stream, behind `Absorbed`, with a
/// mark, due at once; one the absorbed region took does not.
#[test]
fn a_merge_sends_the_absorbed_regions_untaken_arrivals_to_the_survivor_behind_absorbed() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (survivor, _) = opened(&store, WEST, 1);
    let (absorbed, _) = opened(&store, EAST, 1);
    let (home, _) = opened(&store, HOME, 1);
    let first = item(KEY, ORIGIN, 0);
    let second = item(OTHER_KEY, ORIGIN, 0);
    put(&home, 1, &first);
    put(&home, 2, &second);
    send(&home, 3, &first, FAR_EAST);
    send(&home, 4, &second, FAR_EAST);
    flushed(&home);
    let arrivals = arrivals_in(&flushed(&absorbed));
    assert_eq!(keys(&arrivals), [KEY, OTHER_KEY]);
    take(&absorbed, 1, &arrivals[..1]);
    save_chunk(
        &absorbed,
        FAR_EAST,
        1,
        chunk_with(FAR_EAST, vec![taken_in(&arrivals[0].entity)]),
    );
    crate::regions::checkpoint(&absorbed, 2);
    crate::regions::checkpoint(&survivor, 2);

    assert!(matches!(
        absorb(&survivor, EAST, 1, 10),
        StoreReply::Absorbed { .. }
    ));
    let told = flushed(&survivor);
    let sent = arrivals_in(&told);
    assert_eq!(keys(&sent), [OTHER_KEY], "{told:?}");
    assert_eq!(sent[0].due, Due::FromTaking);
    let at = position(&told, is_arrivals).unwrap();
    assert!(
        told[at..]
            .iter()
            .any(|reply| matches!(reply, StoreReply::Mark { .. })),
        "{told:?}"
    );
    drop(survivor);
    let (survivor, _) = opened(&store, WEST, 2);
    assert_eq!(keys(&arrivals_in(&flushed(&survivor))), [OTHER_KEY]);
    assert_eq!(count_in(loaded(&survivor, FAR_EAST).as_ref(), KEY), 1);
}

/// The same for a split: the part's hello brings the arrivals of its chunks, and the
/// region it was split from is sent them no more.
#[test]
fn a_splits_part_is_sent_the_arrivals_of_its_chunks_and_the_rest_of_the_region_none() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (old, _) = opened(&store, WEST, 1);
    let (east, _) = opened(&store, EAST, 1);
    assert_eq!(claim(&old, &[FREE]).0, [FREE]);
    let entity = item(KEY, FAR_EAST, 0);
    put(&east, 1, &entity);
    send(&east, 2, &entity, FREE);
    flushed(&east);
    assert_eq!(keys(&arrivals_in(&flushed(&old))), [KEY]);
    crate::regions::checkpoint(&old, 2);
    assert_eq!(
        split(&old, 7, &[FREE], 4, 3),
        StoreReply::Split {
            region: RegionId(3)
        }
    );
    flushed(&old);
    let (part, _) = opened(&store, 3, 4);
    let sent = arrivals_in(&flushed(&part));
    assert_eq!(keys(&sent), [KEY]);
    assert_eq!(sent[0].due, Due::FromTaking);
    drop(old);
    let (old, _) = opened(&store, WEST, 2);
    assert_eq!(arrivals_in(&flushed(&old)), []);
}

// A group cut off, and the log read at a start.

/// Section 5: the filing goes into the group's undo and the grant into its changes, so
/// that a group that cannot be made durable takes both back.
#[test]
fn a_group_cut_off_takes_back_its_arrivals_its_takings_and_its_grant() {
    let (store, disk) = crate::tests::switched_for(&gap());
    let (home, _) = opened(&store, HOME, 1);
    let (west, _) = opened(&store, WEST, 1);
    let (east, _) = opened(&store, EAST, 1);
    let theirs = item(KEY, FAR_EAST, 0);
    put(&east, 1, &theirs);
    send(&east, 2, &theirs, ORIGIN);
    flushed(&east);
    let arrival = arrivals_in(&flushed(&home)).remove(0);
    let ours = item(OTHER_KEY, ORIGIN, 0);
    put(&home, 1, &ours);
    flushed(&home);

    disk.failing_syncs
        .store(true, std::sync::atomic::Ordering::SeqCst);
    // One group: the home region takes the arrival, and sends its own item into land
    // nobody holds.
    take(&home, 2, std::slice::from_ref(&arrival));
    send(&home, 3, &ours, FREE);
    home.request(StoreRequest::Flush);
    while home.replies.recv().is_ok() {}
    assert!(home.is_lost());
    disk.failing_syncs
        .store(false, std::sync::atomic::Ordering::SeqCst);

    let (home, restored) = opened(&store, HOME, 2);
    assert_eq!(last_tick(&restored), 1);
    assert!(
        !restored.held.iter().any(|(chunk, _)| *chunk == FREE),
        "{:?}",
        restored.held
    );
    assert_eq!(arrivals_in(&flushed(&home)), [arrival]);
    drop((west, east));
    let (west, _) = opened(&store, WEST, 2);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], Vec::new()));
}

/// The plan, section 5 (D2) and section 6 (6): a commit that a later `Opened` of its
/// region leaves out of the region's history (a write never answered that reached the
/// disk after all) takes back, at the store's start, what it did to arrivals: the
/// taking is undone, so that the entity is filed again and not in the chunk, and is in
/// one place.
#[test]
fn a_taking_that_an_opened_record_leaves_out_is_taken_back_at_the_stores_start() {
    let disk = Arc::new(MemoryDisk::default());
    let arrival;
    {
        let store = store_on(&disk, &division()).unwrap();
        let (west, _) = opened(&store, WEST, 1);
        let (east, _) = opened(&store, EAST, 1);
        let entity = item(KEY, ORIGIN, 0);
        put(&east, 1, &entity);
        send(&east, 2, &entity, W);
        flushed(&east);
        arrival = arrivals_in(&flushed(&west)).remove(0);
        take(&west, 1, std::slice::from_ref(&arrival));
        flushed(&west);
    }
    // The western region opened again, restored to before its taking, as a store writes
    // it for a region whose commit it never answered.
    let last = disk
        .paths()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "wal"))
        .max()
        .unwrap();
    let mut bytes = disk.read(&last).unwrap().unwrap();
    bytes.extend(
        LogRecord::Opened {
            region: WEST,
            epoch: 1,
            restored: 0,
        }
        .encode(),
    );
    crate::regions::put(&disk, last.to_str().unwrap(), &bytes);

    for start in 0..2 {
        let store = store_on(&disk, &division()).unwrap();
        let (west, restored) = opened(&store, WEST, 2 + start);
        assert_eq!(last_tick(&restored), 0, "start {start}");
        assert_eq!(
            arrivals_in(&flushed(&west)),
            std::slice::from_ref(&arrival),
            "start {start}: the taking left out is taken back"
        );
        assert_eq!(count_in(loaded(&west, W).as_ref(), KEY), 0, "start {start}");
        let (east, _) = opened(&store, EAST, 2 + start);
        assert_eq!(
            count_in(loaded(&east, ORIGIN).as_ref(), KEY),
            0,
            "start {start}"
        );
    }
}

/// The same for the sending: a sender's commit that an `Opened` leaves out takes its
/// arrival back, and the entity is where the sender's history has it, in its chunk.
#[test]
fn a_sending_that_an_opened_record_leaves_out_is_taken_back_at_the_stores_start() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &division()).unwrap();
        let (east, _) = opened(&store, EAST, 1);
        let entity = item(KEY, ORIGIN, 0);
        put(&east, 1, &entity);
        send(&east, 2, &entity, W);
        flushed(&east);
    }
    let last = disk
        .paths()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "wal"))
        .max()
        .unwrap();
    let mut bytes = disk.read(&last).unwrap().unwrap();
    bytes.extend(
        LogRecord::Opened {
            region: EAST,
            epoch: 1,
            restored: 1,
        }
        .encode(),
    );
    crate::regions::put(&disk, last.to_str().unwrap(), &bytes);
    let store = store_on(&disk, &division()).unwrap();
    let (west, _) = opened(&store, WEST, 2);
    assert_eq!(arrivals_in(&flushed(&west)), []);
    assert_eq!(count_in(loaded(&west, W).as_ref(), KEY), 0);
    let (east, restored) = opened(&store, EAST, 2);
    assert_eq!(last_tick(&restored), 1);
    assert_eq!(count_in(loaded(&east, ORIGIN).as_ref(), KEY), 1);
}

/// Section 5, "Large", and the plan's N3b: an arrival in a segment that the log would let
/// go is written forward first, with its number, and a store that starts reads it from
/// there; taken by that number, it is gone.
#[test]
fn an_arrival_in_a_segment_let_go_is_written_forward_and_read_at_the_start() {
    let disk = Arc::new(MemoryDisk::default());
    let arrival;
    {
        let store = store_on(&disk, &division()).unwrap();
        let (west, _) = opened(&store, WEST, 1);
        let (east, _) = opened(&store, EAST, 1);
        let entity = item(KEY, ORIGIN, 0);
        put(&east, 1, &entity);
        send(&east, 2, &entity, W);
        flushed(&east);
        arrival = arrivals_in(&flushed(&west)).remove(0);
        save_chunk(&east, ORIGIN, 2, chunk_with(ORIGIN, Vec::new()));
        east.request(StoreRequest::Checkpoint {
            tick: 2,
            state: b"east 2".to_vec(),
        });
        flushed(&east);
        let records = logged(&disk);
        assert!(
            !records
                .iter()
                .any(|record| matches!(record, LogRecord::Commit { .. })),
            "the commits' segment went: {records:?}"
        );
        let forward: Vec<Vec<Arrival>> = records
            .into_iter()
            .filter_map(|record| match record {
                LogRecord::ArrivalsForward { arrivals } => Some(arrivals),
                _ => None,
            })
            .collect();
        assert_eq!(forward, [vec![arrival.clone()]]);
    }
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (west, _) = opened(&store, WEST, epoch);
        assert_eq!(
            arrivals_in(&flushed(&west)),
            std::slice::from_ref(&arrival),
            "{survival:?}"
        );
        take(&west, 1, std::slice::from_ref(&arrival));
        flushed(&west);
        drop((west, store));
        let store = store_on(&left, &division()).unwrap();
        let (west, _) = opened(&store, WEST, epoch + 10);
        assert_eq!(arrivals_in(&flushed(&west)), [], "{survival:?}");
        assert_eq!(count_in(loaded(&west, W).as_ref(), KEY), 1, "{survival:?}");
    }
}

// The entity in exactly one place, whatever a crash leaves.

/// Opens `region` and runs its ticks after what it is restored with, up to `last`,
/// each made by `make` from the arrivals sent it and not taken yet; opened again, with a
/// higher epoch, if the handle is lost, as its worker would be, up to three times.
fn run(
    store: &Store,
    region: u32,
    epoch: &mut u64,
    last: u64,
    make: &dyn Fn(&StoreHandle, u64, &[Arrival]),
) {
    for _ in 0..3 {
        *epoch += 1;
        let Ok((handle, restored)) = store.open_region(hello(region, *epoch)) else {
            continue;
        };
        let mut kept: Vec<Arrival> = Vec::new();
        let mut taken: BTreeSet<EffectId> = BTreeSet::new();
        keep(&mut kept, &flushed(&handle), &taken);
        for tick in last_tick(&restored) + 1..=last {
            let handed = kept.clone();
            make(&handle, tick, &handed);
            let (committed, told) = settle(&handle);
            if committed != [tick] {
                break;
            }
            // What a tick hands in, it takes (the tests' ticks take all they are given).
            taken.extend(handed.iter().map(|arrival| arrival.id));
            kept.retain(|arrival| !taken.contains(&arrival.id));
            keep(&mut kept, &told, &taken);
        }
        if !handle.is_lost() {
            return;
        }
    }
}

/// Keeps the arrivals among what a region was told that it has neither kept nor taken.
fn keep(kept: &mut Vec<Arrival>, told: &[StoreReply], taken: &BTreeSet<EffectId>) {
    for arrival in arrivals_in(told) {
        if !taken.contains(&arrival.id) && !kept.iter().any(|kept| kept.id == arrival.id) {
            kept.push(arrival);
        }
    }
}

/// The pinned world: the east puts the item in the origin, sends it west and checkpoints
/// with the origin saved empty, which lets the commits' segment go; the west takes it,
/// saves its chunk with it and checkpoints.
fn across_a_pinned_line(store: &Store) {
    let mut epoch = 0;
    let entity = item(KEY, ORIGIN, 0);
    run(store, EAST, &mut epoch, 2, &|handle, tick, _| match tick {
        1 => put(handle, 1, &entity),
        _ => {
            send(handle, 2, &entity, W);
            save_chunk(handle, ORIGIN, 2, chunk_with(ORIGIN, Vec::new()));
            handle.request(StoreRequest::Checkpoint {
                tick: 2,
                state: b"east 2".to_vec(),
            });
        }
    });
    let mut epoch = 0;
    run(store, WEST, &mut epoch, 1, &|handle, tick, arrivals| {
        take(handle, tick, arrivals);
        let entities = arrivals
            .iter()
            .map(|arrival| taken_in(&arrival.entity))
            .collect();
        save_chunk(handle, W, tick, chunk_with(W, entities));
        handle.request(StoreRequest::Checkpoint {
            tick,
            state: b"west 1".to_vec(),
        });
    });
}

/// Where the item is in the pinned world a store started on `disk` has: filed, in the
/// origin, in the western chunk, with the hops it has in each.
fn places_across_a_pinned_line(disk: Arc<MemoryDisk>, case: &str) -> Vec<(&'static str, u32)> {
    let store = store_on(&disk, &division()).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (west, _) = opened(&store, WEST, 50);
    let (east, restored) = opened(&store, EAST, 50);
    let mut places = Vec::new();
    // Before the sender's first commit is durable there is no item yet.
    if last_tick(&restored) == 0 {
        places.push(("not yet", 0));
    }
    for arrival in arrivals_in(&flushed(&west)) {
        assert_eq!(arrival.chunk, W, "{case}");
        places.push(("filed", arrival.entity.hops));
    }
    for (name, handle, chunk) in [("origin", &east, ORIGIN), ("west", &west, W)] {
        for entity in loaded(handle, chunk).expect("held").entities() {
            if entity.key.0 == KEY {
                places.push((name, entity.hops));
            }
        }
    }
    places
}

fn once_across_a_pinned_line(fault: Fault, survival: Survival) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    if let Ok(store) = store_on(&disk, &division()) {
        across_a_pinned_line(&store);
    }
    let case = format!("{fault:?}, {survival:?}");
    let places = places_across_a_pinned_line(Arc::new(disk.crashed(survival)), &case);
    // Sent with hops 0; filed with them, taken with one more (section 6).
    assert!(
        places == [("not yet", 0)]
            || places == [("origin", 0)]
            || places == [("filed", 0)]
            || places == [("west", 1)],
        "{case}: the item is at {places:?}"
    );
}

fn operations_across_a_pinned_line() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    across_a_pinned_line(&store);
    drop(store);
    assert_eq!(
        places_across_a_pinned_line(Arc::new(disk.crashed(Survival::Nothing)), "no fault"),
        [("west", 1)]
    );
    disk.operations() + 10
}

#[test]
fn an_entity_crossing_a_pinned_line_is_in_exactly_one_place_after_a_stop_at_any_point() {
    for n in 1..=operations_across_a_pinned_line() {
        for survival in SURVIVALS {
            once_across_a_pinned_line(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn an_entity_crossing_a_pinned_line_is_in_exactly_one_place_after_a_failure_at_any_point() {
    for n in 1..=operations_across_a_pinned_line() {
        for survival in SURVIVALS {
            once_across_a_pinned_line(Fault::Fail(n), survival);
        }
    }
}

/// The world with a gap: the home region puts the item in the origin, sends it into land
/// nobody holds, which it is granted with it, and takes it there itself.
fn into_land_nobody_holds(store: &Store) {
    let mut epoch = 0;
    let entity = item(KEY, ORIGIN, 0);
    run(
        store,
        HOME,
        &mut epoch,
        3,
        &|handle, tick, arrivals| match tick {
            1 => put(handle, 1, &entity),
            2 => send(handle, 2, &entity, FREE),
            _ => take(handle, tick, arrivals),
        },
    );
}

fn places_in_land_nobody_holds(disk: Arc<MemoryDisk>, case: &str) -> Vec<(&'static str, u32)> {
    let store = store_on(&disk, &gap()).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (home, restored) = opened(&store, HOME, 50);
    let granted = restored.held.iter().any(|(chunk, _)| *chunk == FREE);
    let mut places = Vec::new();
    // Before the first commit is durable there is no item yet.
    if last_tick(&restored) == 0 {
        places.push(("not yet", 0));
    }
    for arrival in arrivals_in(&flushed(&home)) {
        assert_eq!(arrival.chunk, FREE, "{case}");
        assert!(granted, "{case}: an arrival filed for land nobody holds");
        places.push(("filed", arrival.entity.hops));
    }
    for entity in loaded(&home, ORIGIN).expect("held").entities() {
        if entity.key.0 == KEY {
            places.push(("origin", entity.hops));
        }
    }
    if granted {
        for entity in loaded(&home, FREE).expect("granted").entities() {
            if entity.key.0 == KEY {
                places.push(("free", entity.hops));
            }
        }
    }
    places
}

fn once_into_land_nobody_holds(fault: Fault, survival: Survival) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    if let Ok(store) = store_on(&disk, &gap()) {
        into_land_nobody_holds(&store);
    }
    let case = format!("{fault:?}, {survival:?}");
    let places = places_in_land_nobody_holds(Arc::new(disk.crashed(survival)), &case);
    assert!(
        places == [("not yet", 0)]
            || places == [("origin", 0)]
            || places == [("filed", 0)]
            || places == [("free", 1)],
        "{case}: the item is at {places:?}"
    );
}

fn operations_into_land_nobody_holds() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    into_land_nobody_holds(&store);
    drop(store);
    assert_eq!(
        places_in_land_nobody_holds(Arc::new(disk.crashed(Survival::Nothing)), "no fault"),
        [("free", 1)]
    );
    disk.operations() + 10
}

#[test]
fn an_entity_sent_into_land_nobody_holds_is_in_exactly_one_place_after_a_stop_at_any_point() {
    for n in 1..=operations_into_land_nobody_holds() {
        for survival in SURVIVALS {
            once_into_land_nobody_holds(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn an_entity_sent_into_land_nobody_holds_is_in_exactly_one_place_after_a_failure_at_any_point() {
    for n in 1..=operations_into_land_nobody_holds() {
        for survival in SURVIVALS {
            once_into_land_nobody_holds(Fault::Fail(n), survival);
        }
    }
}

// The fold at a change of division.

/// Three stripes in place of two: a division the pinned world was not made with.
fn three_stripes() -> Division {
    Division::side_by_side(ORIGIN, &[0, 16]).unwrap()
}

/// The pinned world with the item sent west and not taken.
fn sent_and_not_taken() -> Arc<MemoryDisk> {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (east, _) = opened(&store, EAST, 1);
    let entity = item(KEY, ORIGIN, 0);
    put(&east, 1, &entity);
    send(&east, 2, &entity, W);
    flushed(&east);
    drop((east, store));
    disk
}

/// Section 5, "a world made over for another division": the fold writes the untaken
/// arrival into its chunk, whole by key, once; no region is sent it any more, and a store
/// started again finds it there once.
#[test]
fn a_fold_at_a_change_of_division_writes_an_untaken_arrival_into_its_chunk_once() {
    let disk = sent_and_not_taken();
    for start in 0..2 {
        let store = store_on(&disk, &three_stripes()).unwrap();
        for region in 0..3 {
            let (handle, _) = opened(&store, region, 10 + start);
            assert_eq!(
                arrivals_in(&flushed(&handle)),
                [],
                "start {start}, region {region}"
            );
            if region == 0 {
                let west = loaded(&handle, W);
                assert_eq!(count_in(west.as_ref(), KEY), 1, "start {start}");
                assert_eq!(west.unwrap().entities()[0].hops, 0);
            }
            if region == 1 {
                assert_eq!(count_in(loaded(&handle, ORIGIN).as_ref(), KEY), 0);
            }
        }
    }
    // The dropping came before the records that leave the old commits out.
    let records = logged(&disk);
    if let Some(dropped) = records
        .iter()
        .position(|record| matches!(record, LogRecord::ArrivalsDropped { .. }))
    {
        let opened = records
            .iter()
            .rposition(|record| matches!(record, LogRecord::Opened { restored: 0, .. }))
            .unwrap();
        assert!(dropped < opened, "{records:?}");
    }
}

/// The same with the store stopped at every point of the fold and started again: the
/// fold done again writes the same entity by key, and it is there once and filed
/// nowhere.
#[test]
fn a_fold_stopped_at_any_point_and_done_again_leaves_the_arrival_in_its_chunk_once() {
    let operations = {
        let disk = Arc::new(sent_and_not_taken().crashed(Survival::Everything));
        drop(store_on(&disk, &three_stripes()).unwrap());
        disk.operations() + 10
    };
    for n in 1..=operations {
        for survival in SURVIVALS {
            let case = format!("stop {n}, {survival:?}");
            let disk = Arc::new(
                sent_and_not_taken()
                    .crashed(Survival::Everything)
                    .with(Fault::Stop(n)),
            );
            drop(store_on(&disk, &three_stripes()));
            let left = Arc::new(disk.crashed(survival));
            let store = store_on(&left, &three_stripes()).unwrap_or_else(|e| panic!("{case}: {e}"));
            let (west, _) = opened(&store, 0, 20);
            assert_eq!(arrivals_in(&flushed(&west)), [], "{case}");
            assert_eq!(count_in(loaded(&west, W).as_ref(), KEY), 1, "{case}");
            let (middle, _) = opened(&store, 1, 20);
            assert_eq!(arrivals_in(&flushed(&middle)), [], "{case}");
            assert_eq!(count_in(loaded(&middle, ORIGIN).as_ref(), KEY), 0, "{case}");
        }
    }
}

/// The plan, section 6 (2), and section 5: "an arrival whose chunk the fold cannot load
/// or make stays filed", written forward before the `Opened` records that take back
/// every earlier commit's arrivals, and goes to whoever holds the chunk.
#[test]
fn a_fold_that_cannot_read_an_arrivals_chunk_keeps_the_arrival_filed() {
    let disk = sent_and_not_taken();
    let path = FileChunks::new(disk.clone(), Path::new("/world")).chunk_path(W);
    replace(disk.as_ref(), &path, b"not a chunk").unwrap();
    disk.sync_directory(path.parent().unwrap()).unwrap();
    for start in 0..2 {
        let store = store_on(&disk, &three_stripes()).unwrap();
        let (west, _) = opened(&store, 0, 10 + start);
        let arrivals = arrivals_in(&flushed(&west));
        // The chunk cannot be read, so the item is in no chunk: it has to be filed.
        assert!(loaded(&west, W).is_none(), "start {start}");
        assert_eq!(keys(&arrivals), [KEY], "start {start}");
        assert_eq!(arrivals[0].chunk, W);
    }
}
