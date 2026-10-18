//! Tests of what the runner makes durable of a region's scheduled ticks: written from
//! `docs/adr/0024-block-behaviour-in-a-region.md`, sections 3 ("Time: delays left")
//! and 4 ("What is durable, and how"), by someone who did not write the code.
//!
//! A chunk's ticks are kept as the delay left: written at the end of tick T, a tick
//! due in D is kept as D − T − 1, and a chunk that starts ticking in L with a tick
//! kept as v has it due in L + v. A tick whose only change is to ticks is committed,
//! with the ticks of each chunk whose ticks changed, whole; such a chunk is unsaved.
//! A chunk that ticks with ticks pending ages without a commit, and is saved where a
//! stop is near: when it is let go, at the checkpoint where the region stops ticking
//! for a release, a merge or a split, and at the end of `run`; it counts among the few
//! unsaved chunks of a prepared region. A chunk that stops ticking keeps its delays
//! from then.
//!
//! The runners here run without rings, so a chunk ticks only where it and its eight
//! neighbours are loaded by an edge's subscriptions. The chunk the tests watch, `K`,
//! is stored before the region opens with sand on the grass and ticks of the sand
//! kept; supported sand does nothing at its tick, so its tick only takes it out of
//! the chunk's ticks. Every wait is for a state, with a bound on the steps.

use clustine_world::{InChunk, ScheduledTick, Ticked};

use super::*;

/// The chunk whose ticks the tests watch, in the western stripe with all of its
/// neighbours, and away from the home chunk, which stays loaded whoever needs it.
const K: ChunkPos = ChunkPos::new(-3, 0);

/// A neighbour of [`K`] to its west, which nothing else needs.
const NEIGHBOUR: ChunkPos = ChunkPos::new(-4, 0);

/// A delay so long that the tick never runs in a test.
const LONG: i32 = 1000;

/// `position` and the eight chunks around it.
fn around(position: ChunkPos) -> Vec<ChunkPos> {
    (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| ChunkPos::new(position.x + dx, position.z + dz)))
        .collect()
}

/// A tick of the sand in column `x` of the row the tests use in [`K`], kept with
/// `delay` left.
fn sand_tick(x: usize, delay: i32) -> ScheduledTick {
    let id = clustine_data::blocks::SAND
        .block_id()
        .expect("sand is a block");
    ScheduledTick {
        at: InChunk::new(x, -60, 5),
        of: Ticked::Block(u16::try_from(id).expect("a block id fits in 16 bits")),
        delay,
        priority: 0,
    }
}

/// The world of [`Divided::stripes`] with [`K`] stored with sand at the place of each
/// of `ticks` and those ticks kept.
fn world_with(ticks: &[ScheduledTick]) -> Divided {
    let world = Divided::stripes();
    let (handle, _) = world.open(RegionId(0), 1);
    let mut chunk = made(&handle, K);
    for tick in ticks {
        let (x, z) = (usize::from(tick.at.x), usize::from(tick.at.z));
        chunk.set(x, i32::from(tick.at.y), z, clustine_data::blocks::SAND);
    }
    chunk.set_ticks(ticks.to_vec());
    handle.request(StoreRequest::Save {
        position: K,
        tick: 0,
        chunk,
        parts: clustine_rpc::Parts::ALL,
    });
    handle.flush();
    world
}

/// The chunk at `position` as the store hands it out through `handle`, made by the
/// generator if it was never stored.
fn made(handle: &StoreHandle, position: ChunkPos) -> Chunk {
    handle.request(StoreRequest::Load { position });
    for _ in 0..20_000 {
        match handle.try_reply() {
            Some(StoreReply::Loaded { chunk, .. }) => return chunk,
            Some(StoreReply::Generating { .. }) => {}
            Some(other) => panic!("the store answered a load with {other:?}"),
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer the load");
}

/// A gated runner of the western stripe of `world`, opened after [`world_with`] stored
/// [`K`], with an edge linked to it.
fn western(world: &Divided) -> (RegionRunner, Arc<GateControl>, TestEdge) {
    let (runner, gate) = gated_as(&world.store, world.hello(RegionId(0), 2), settings(0));
    let runner = runner.with_id(RegionId(0));
    let (edge, worker_end) = in_process(256);
    runner.links().attach(worker_end);
    // The runner checkpoints only where a test asks it to.
    let mut runner = runner;
    runner.checkpoint_interval = 1_000_000;
    (runner, gate, edge)
}

/// Has `edge` subscribe to [`K`] and its neighbours, and steps until all nine are
/// loaded. Returns the tick in which the last of them was, which is the tick `K`
/// starts ticking in.
async fn ticking(runner: &mut RegionRunner, edge: &TestEdge) -> u64 {
    edge.send(asking_for(around(K))).await.unwrap();
    step_until(runner, |runner| {
        around(K)
            .iter()
            .all(|position| runner.region().chunk(*position).is_some())
    });
    runner.region().tick_number()
}

/// The delay left at the end of tick `at` of a tick kept as `kept` in a chunk that
/// started ticking in `started` and has ticked since.
fn left(kept: i32, started: u64, at: u64) -> i32 {
    let due = i64::try_from(started).unwrap() + i64::from(kept);
    i32::try_from(due - i64::try_from(at).unwrap() - 1).unwrap()
}

/// The saves of `position` the runner asked for so far: the tick of each and the
/// ticks the chunk was saved with.
fn saves_of(gate: &GateControl, position: ChunkPos) -> Vec<(u64, Vec<ScheduledTick>)> {
    let saved = gate.saved.lock().unwrap();
    saved
        .iter()
        .filter(|(at, _, _)| *at == position)
        .map(|(_, tick, chunk)| (*tick, chunk.ticks().to_vec()))
        .collect()
}

/// Whether the saves asked for so far include one of [`K`].
fn saved_k(gate: &GateControl) -> bool {
    !saves_of(gate, K).is_empty()
}

/// A tick in which a kept tick runs and does nothing else changes only the chunk's
/// ticks. It is committed, and the commit carries the chunk's ticks whole, as the
/// delays left at the tick's end, without the one that ran; the chunk is unsaved
/// from then. A region opened again from the store after that, without a save in
/// between, has the chunk with those ticks.
#[tokio::test]
async fn a_tick_in_which_only_ticks_change_is_committed_with_the_delays_left() {
    let world = world_with(&[sand_tick(5, 3), sand_tick(6, LONG)]);
    let (mut runner, gate, edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    step_until(&mut runner, |_| !gate.ticked.lock().unwrap().is_empty());

    let ran = started + 3;
    assert_eq!(runner.region().tick_number(), ran);
    let ticked = gate.ticked.lock().unwrap().clone();
    let expected = TickedCommit {
        tick: ran,
        changes: 0,
        ticks: vec![(K, vec![sand_tick(6, left(LONG, started, ran))])],
    };
    assert_eq!(ticked, [expected]);
    assert_eq!(left(LONG, started, ran), LONG - 4);
    assert!(gate.commits().contains(&ran));
    assert!(runner.unsaved.contains(&K));
    assert!(!saved_k(&gate));

    drop(runner);
    let (handle, _) = world.open(RegionId(0), 3);
    let stored = stored_chunk(&handle, K);
    assert_eq!(stored.ticks(), [sand_tick(6, LONG - 4)]);
}

/// A commit lists the ticks of a chunk as they are at the end of its tick, also where
/// they are empty: once the last kept tick has run, the commit says the chunk has
/// none.
#[tokio::test]
async fn a_chunk_whose_last_tick_ran_is_committed_with_no_ticks() {
    let world = world_with(&[sand_tick(5, 2)]);
    let (mut runner, gate, edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    step_until(&mut runner, |_| !gate.ticked.lock().unwrap().is_empty());
    let ticked = gate.ticked.lock().unwrap().clone();
    let expected = TickedCommit {
        tick: started + 2,
        changes: 0,
        ticks: vec![(K, Vec::new())],
    };
    assert_eq!(ticked, [expected]);
}

/// While the chunk ticks with a tick pending and nothing else happens, nothing of it
/// is committed, and the checkpoints made while the region ticks do not save it,
/// though its saved form ages with every tick.
#[tokio::test]
async fn checkpoints_made_while_the_region_ticks_do_not_save_a_chunk_whose_ticks_only_age() {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, edge) = western(&world);
    ticking(&mut runner, &edge).await;
    gate.asked();
    runner.checkpoint_interval = 1;
    for _ in 0..5 {
        step(&mut runner);
    }
    let asked = asked_beside_commits(&gate);
    assert!(
        asked
            .iter()
            .all(|asked| matches!(asked, Asked::Checkpoint(_))),
        "{asked:?}"
    );
    assert!(asked.len() >= 5, "{asked:?}");
    assert!(!saved_k(&gate));
    assert!(gate.ticked.lock().unwrap().is_empty());
}

/// A chunk let go while it ticks with a tick pending is saved as it is let go, with
/// the delay left at the end of the last tick it ticked in, though nothing of it was
/// ever committed.
#[tokio::test]
async fn a_chunk_with_ticks_pending_is_saved_with_its_delays_left_when_it_is_let_go() {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, mut edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    for _ in 0..4 {
        step(&mut runner);
    }
    assert!(!saved_k(&gate));
    edge.send(done_with(vec![K])).await.unwrap();
    let before = runner.region().tick_number();
    step_until(&mut runner, |runner| runner.region().chunk(K).is_none());
    let saves = saves_of(&gate, K);
    assert_eq!(
        saves,
        [(before, vec![sand_tick(5, left(LONG, started, before))])]
    );
    assert!(gate.ticked.lock().unwrap().is_empty());
    edge.everything();
}

/// When the region stops ticking for a merge, the checkpoint it stops with saves the
/// chunk with a tick pending, before the checkpoint, with the delay left at the end
/// of the last tick; the store then has that, and not what the chunk was stored
/// with.
#[tokio::test]
async fn a_chunk_with_ticks_pending_is_saved_with_its_delays_left_when_the_region_stops_for_a_merge()
 {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, edge) = western(&world);
    let east = untouched_east(&world);
    let started = ticking(&mut runner, &edge).await;
    for _ in 0..3 {
        step(&mut runner);
    }
    gate.asked();

    let (done, outcome) = outcome();
    runner.reshape(east.absorb(), done);
    step_to(&mut runner, Stage::Settling);
    let stop = runner.region().tick_number();
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut runner, &outcome), absorbed);

    let asked = asked_beside_commits(&gate);
    let save = asked.iter().rposition(|asked| *asked == Asked::Save(K));
    let checkpoint = asked.iter().position(|a| *a == Asked::Checkpoint(stop));
    assert!(save.is_some() && save < checkpoint, "{asked:?}");
    let saves = saves_of(&gate, K);
    let last = saves.last().expect("the chunk was saved");
    assert_eq!(*last, (stop, vec![sand_tick(5, left(LONG, started, stop))]));
    assert!(left(LONG, started, stop) < LONG - 3);
}

/// So too where the region stops for a split, here one that finds nobody in the
/// chunk it names and so ticks on after it.
#[tokio::test]
async fn a_chunk_with_ticks_pending_is_saved_with_its_delays_left_when_the_region_stops_for_a_split()
 {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    for _ in 0..3 {
        step(&mut runner);
    }
    gate.asked();

    let (done, outcome) = outcome();
    runner.reshape(split_off(&[ChunkPos::new(-20, 0)]), done);
    step_to(&mut runner, Stage::Settling);
    let stop = runner.region().tick_number();
    assert_eq!(
        reshaped(&mut runner, &outcome),
        Reshaped::Off { why: Off::Nobody }
    );
    let saves = saves_of(&gate, K);
    let last = saves.last().expect("the chunk was saved");
    assert_eq!(*last, (stop, vec![sand_tick(5, left(LONG, started, stop))]));
    let asked = asked_beside_commits(&gate);
    let save = asked.iter().rposition(|asked| *asked == Asked::Save(K));
    // The stop's checkpoint is the last of its tick: the one the split prepared with
    // can be of the same tick, where the region did not tick between the two, as
    // under load.
    let checkpoint = asked.iter().rposition(|a| *a == Asked::Checkpoint(stop));
    assert!(save.is_some() && save < checkpoint, "{asked:?}");
}

/// A release stops the region once the store has its first checkpoint, and the region
/// ticks on until then; the checkpoint it stops with saves the chunk with a tick
/// pending with the delay left at the end of the last tick, not at that of the first
/// checkpoint. The store has it so when the region is opened again.
#[tokio::test]
async fn a_chunk_with_ticks_pending_is_saved_with_its_delays_left_when_the_region_is_released() {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    gate.hold_flushes();
    runner.begin_release();
    let first = runner.region().tick_number();
    for _ in 0..4 {
        runner.step();
    }
    assert!(runner.region().tick_number() > first);
    gate.release_flushes();
    assert_eq!(released(&mut runner), Ended::Released);
    let stop = runner.region().tick_number();
    let saves = saves_of(&gate, K);
    let last = saves.last().expect("the chunk was saved");
    assert_eq!(*last, (stop, vec![sand_tick(5, left(LONG, started, stop))]));

    let (handle, _) = world.open(RegionId(0), 3);
    let stored = stored_chunk(&handle, K);
    assert_eq!(stored.ticks(), [sand_tick(5, left(LONG, started, stop))]);
}

/// A runner that is stopped saves the chunk with a tick pending as it ends, with the
/// delay left at the end of its last tick, and the store has it so.
#[tokio::test]
async fn a_chunk_with_ticks_pending_is_saved_with_its_delays_left_when_the_runner_is_stopped() {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    for _ in 0..2 {
        step(&mut runner);
    }
    let stop = runner.region().tick_number();
    assert_eq!(runner.run(&AtomicBool::new(true)), Ended::Stopped);
    let saves = saves_of(&gate, K);
    assert_eq!(
        saves,
        [(stop, vec![sand_tick(5, left(LONG, started, stop))])]
    );
    drop(runner);
    let (handle, _) = world.open(RegionId(0), 3);
    let stored = stored_chunk(&handle, K);
    assert_eq!(stored.ticks(), [sand_tick(5, left(LONG, started, stop))]);
}

/// A chunk that ticks with a tick pending counts among the chunks changed since the
/// order to prepare: where more than a few count, the region makes a checkpoint of
/// its own before it stops for a merge, as where blocks changed; where they are few
/// it stops at once.
#[tokio::test]
async fn a_chunk_whose_ticks_age_counts_among_the_few_unsaved_chunks_of_a_prepared_region() {
    for few in [0, 1] {
        let world = world_with(&[sand_tick(5, LONG)]);
        let (mut runner, gate, edge) = western(&world);
        let east = untouched_east(&world);
        ticking(&mut runner, &edge).await;
        runner.few_unsaved = few;
        runner.fresh_for = 0;
        runner.reshape(Reshape::Prepare, no_outcome());
        step_until(&mut runner, |runner| {
            runner.flushes_answered == runner.flushes_asked
        });
        step(&mut runner);
        assert!(runner.unsaved.is_empty(), "{few}");
        gate.asked();

        gate.hold_flushes();
        let (done, outcome) = outcome();
        runner.reshape(east.absorb(), done);
        runner.step();
        let asked = asked_beside_commits(&gate);
        if few == 0 {
            assert_eq!(runner.stage(), Some(Stage::Preparing), "{asked:?}");
            assert!(asked.contains(&Asked::Flush), "{asked:?}");
            assert!(
                asked.iter().any(|a| matches!(a, Asked::Checkpoint(_))),
                "{asked:?}"
            );
        } else {
            assert_eq!(runner.stage(), Some(Stage::Settling), "{asked:?}");
        }
        gate.release_flushes();
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut runner, &outcome), absorbed, "{few}");
    }
}

/// A chunk that stops ticking because a neighbour is let go keeps its ticks with the
/// delays they had when it stopped (section 3), and so does the store once the chunk
/// is let go itself: the save as it is let go has those delays, and not those it was
/// last stored or committed with, which would have the tick run later than the game
/// runs it.
#[tokio::test]
async fn a_chunk_that_stopped_ticking_when_a_neighbour_went_is_saved_with_the_delays_it_stopped_with()
 {
    let world = world_with(&[sand_tick(5, LONG)]);
    let (mut runner, gate, mut edge) = western(&world);
    let started = ticking(&mut runner, &edge).await;
    for _ in 0..3 {
        step(&mut runner);
    }
    edge.send(done_with(vec![NEIGHBOUR])).await.unwrap();
    let asked = runner.region().tick_number();
    step_until(&mut runner, |runner| {
        runner.region().chunk(NEIGHBOUR).is_none()
    });
    step(&mut runner);
    // The chunk does not tick, and its delays wait.
    let kept = runner.region().scheduled_ticks(K);
    let [tick] = kept[..] else {
        panic!("the chunk keeps its one tick: {kept:?}")
    };
    for _ in 0..5 {
        step(&mut runner);
        assert_eq!(runner.region().scheduled_ticks(K), kept);
    }
    // It ticked on until about when the neighbour went, so its delay is not what it
    // was stored with.
    let gone = runner.region().tick_number() - 5;
    assert!(tick.delay <= left(LONG, started, asked), "{tick:?}");
    assert!(tick.delay >= left(LONG, started, gone), "{tick:?}");

    edge.send(done_with(vec![K])).await.unwrap();
    step_until(&mut runner, |runner| runner.region().chunk(K).is_none());
    let saves = saves_of(&gate, K);
    let ticks = saves.last().map(|(_, ticks)| ticks.clone());
    assert_eq!(ticks, Some(kept), "the saves of the chunk: {saves:?}");
    edge.everything();
}
