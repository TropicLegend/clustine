//! Tests of when a chunk that still has generation's marks is first shown to an edge:
//! step B6, written from section 6 of `docs/adr/0024-block-behaviour-in-a-region.md`
//! ("The first snapshot") and its "Found while building", by someone who did not write
//! the code. They use the double of the tests of generation, which can say of chosen
//! chunks that they are being made, hold back the answer to their load, or fail them.
//!
//! A viewer's subscription of a chunk with marks is not answered while a neighbour is
//! on its way that would let the chunk tick: one the region wants (a link subscribes to
//! it, or it is in a ring), has not loaded, and holds and is loading or making, or is
//! claiming, or will claim as a viewer's. A neighbour that is another region's, or that
//! cannot be read, lets the chunk go as it is. While it waits, the chunk does not hold
//! what its link sends behind a hello. Once its neighbours are there, it is
//! post-processed and then shown.
//!
//! The runners here run without rings unless a test says otherwise, so a neighbour is
//! wanted only through a subscription. Every wait is for a state, with a bound on the
//! steps; that something did not come is shown by something later that did.

use clustine_world::InChunk;

use super::*;

/// The marked chunk most tests watch, in the western stripe with all its neighbours.
const K: ChunkPos = ChunkPos::new(-3, 0);
/// A neighbour of [`K`] to its west, which nothing else needs.
const N: ChunkPos = ChunkPos::new(-4, 0);
/// A marked chunk at the line between the stripes: its eastern neighbours are
/// [`EAST`]'s.
const AT_THE_LINE: ChunkPos = ChunkPos::new(0, 3);

/// Where in a marked chunk generation left a fence, beside stone to its east, and
/// marked it: post-processing connects it.
const FENCE: (usize, i32, usize) = (4, -60, 4);
const STONE: (usize, i32, usize) = (5, -60, 4);

/// `position` and the eight chunks around it.
fn around(position: ChunkPos) -> Vec<ChunkPos> {
    (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| ChunkPos::new(position.x + dx, position.z + dz)))
        .collect()
}

/// The chunk at `position` as the store hands it out through `handle`.
fn made(handle: &StoreHandle, position: ChunkPos) -> Chunk {
    handle.request(StoreRequest::Load { position });
    for _ in 0..20_000 {
        match handle.try_reply() {
            Some(StoreReply::Loaded { chunk, .. }) => return chunk,
            Some(StoreReply::Generating { .. } | StoreReply::Dead { .. }) => {}
            Some(other) => panic!("the store answered a load with {other:?}"),
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer the load");
}

/// The world of [`Divided::stripes`] with each of `marked` stored with a fence beside
/// stone, the fence marked, and each of `plain` stored as made. Its region 0 was
/// opened with epoch 1.
fn world_with(marked: &[ChunkPos], plain: &[ChunkPos]) -> Divided {
    let world = Divided::stripes();
    let (handle, _) = world.open(RegionId(0), 1);
    for position in marked {
        let mut chunk = made(&handle, *position);
        chunk.set(FENCE.0, FENCE.1, FENCE.2, clustine_data::blocks::OAK_FENCE);
        chunk.set(STONE.0, STONE.1, STONE.2, clustine_data::blocks::STONE);
        chunk.set_marks(vec![InChunk::new(FENCE.0, FENCE.1, FENCE.2)]);
        handle.request(StoreRequest::Save {
            position: *position,
            tick: 0,
            chunk,
        });
    }
    for position in plain {
        let chunk = made(&handle, *position);
        handle.request(StoreRequest::Save {
            position: *position,
            tick: 0,
            chunk,
        });
    }
    handle.flush();
    world
}

/// A runner of the western stripe of `world` with the double before its store and an
/// edge linked to it, which has run a tick.
fn western(world: &Divided) -> (RegionRunner, Arc<Maker>, TestEdge) {
    let (mut runner, maker) = making(world, RegionId(0), 2, config(DEFAULT_RETURN_AFTER));
    let (mut edge, worker_end) = in_process(256);
    runner.links().attach(worker_end);
    step(&mut runner);
    edge.everything();
    (runner, maker, edge)
}

/// Whether the fence of a marked chunk is connected to the stone beside it in `chunk`.
fn connected(chunk: &Chunk) -> bool {
    let fence = chunk
        .get(FENCE.0, FENCE.1, FENCE.2)
        .expect("the fence is in the world");
    fence.property("east") == Some("true")
}

/// The snapshots among `messages`, by chunk.
fn snapshots(messages: &[WorkerToEdge]) -> BTreeMap<ChunkPos, Chunk> {
    messages
        .iter()
        .filter_map(|message| match message {
            WorkerToEdge::ChunkSnapshot {
                position, chunk, ..
            } => Some((*position, chunk.clone())),
            _ => None,
        })
        .collect()
}

/// Steps until `edge` has been answered for each of `chunks`, and returns everything
/// it was sent meanwhile.
fn until_answered(
    runner: &mut RegionRunner,
    edge: &mut TestEdge,
    chunks: &[ChunkPos],
) -> Vec<WorkerToEdge> {
    let mut messages = Vec::new();
    for _ in 0..2000 {
        messages.extend(received(edge));
        let answered: BTreeSet<ChunkPos> = messages
            .iter()
            .filter_map(told)
            .map(|(chunk, _, _)| chunk)
            .collect();
        if chunks.iter().all(|chunk| answered.contains(chunk)) {
            return messages;
        }
        step(runner);
        thread::sleep(Duration::from_millis(1));
    }
    panic!("{chunks:?} were not all answered: {messages:?}");
}

/// Whether `messages` answer the subscription of `chunk`.
fn answers_for(messages: &[WorkerToEdge], chunk: ChunkPos) -> bool {
    messages
        .iter()
        .filter_map(told)
        .any(|(answered, _, _)| answered == chunk)
}

/// What the region has of the marks of the loaded chunk at `position`.
fn marks_of(runner: &RegionRunner, position: ChunkPos) -> usize {
    runner
        .region()
        .chunk(position)
        .expect("the chunk is loaded")
        .marks()
        .len()
}

/// The edge subscribes to [`K`] and its neighbours while `delay` keeps [`N`] on its
/// way. Every other chunk is shown, and then more, while [`K`], loaded and still marked,
/// is not; when [`N`] comes, [`K`] is post-processed and shown so.
async fn waits_for_a_neighbour_on_its_way(delay: fn(&Maker, ChunkPos)) {
    let world = world_with(&[K], &[N]);
    let (mut runner, maker, mut edge) = western(&world);
    delay(&maker, N);
    edge.send(asking_for(around(K))).await.unwrap();

    let others: Vec<ChunkPos> = around(K)
        .into_iter()
        .filter(|chunk| *chunk != K && *chunk != N)
        .collect();
    let mut told = until_answered(&mut runner, &mut edge, &others);
    step_until(&mut runner, |runner| runner.region().chunk(K).is_some());
    told.extend(told_until_marked(&mut runner, &mut edge, -10));
    told.extend(received(&mut edge));
    assert!(
        !answers_for(&told, K),
        "shown before it was settled: {told:?}"
    );
    assert!(!answers_for(&told, N));
    assert_eq!(marks_of(&runner, K), 1);
    assert!(runner.region().chunk(N).is_none());

    maker.let_go(N);
    let told = until_answered(&mut runner, &mut edge, &[K, N]);
    let shown = snapshots(&told);
    let k = shown.get(&K).expect("a snapshot of K");
    assert!(connected(k), "shown as post-processing left it");
    assert_eq!(k.marks(), []);
    assert_eq!(marks_of(&runner, K), 0);
}

/// A neighbour that this region is making holds the first snapshot back.
#[tokio::test]
async fn a_marked_chunks_snapshot_waits_while_a_neighbour_is_made_and_comes_settled() {
    waits_for_a_neighbour_on_its_way(Maker::make).await;
}

/// A neighbour that this region is loading holds the first snapshot back.
#[tokio::test]
async fn a_marked_chunks_snapshot_waits_while_a_neighbour_is_loaded_and_comes_settled() {
    waits_for_a_neighbour_on_its_way(Maker::slow_down).await;
}

/// Only a chunk that still has marks waits: one without is shown while a neighbour is
/// still being made.
#[tokio::test]
async fn a_chunk_without_marks_is_shown_while_a_neighbour_is_still_made() {
    let world = world_with(&[], &[K]);
    let (mut runner, maker, mut edge) = western(&world);
    maker.make(N);
    edge.send(asking_for(around(K))).await.unwrap();
    let others: Vec<ChunkPos> = around(K).into_iter().filter(|chunk| *chunk != N).collect();
    let told = until_answered(&mut runner, &mut edge, &others);
    assert!(snapshots(&told).contains_key(&K));
    assert!(maker.keeps_the_load_of(N), "N is still made");
}

/// With rings, a neighbour in the ring of a subscribed chunk is wanted: a marked chunk
/// subscribed to alone waits for the ring around it, and is shown settled.
#[tokio::test]
async fn with_rings_a_marked_chunk_subscribed_alone_waits_for_its_ring() {
    let world = world_with(&[K], &[N]);
    let (mut runner, maker, mut edge) = western(&world);
    runner.with_rings = true;
    maker.slow_down(N);
    edge.send(asking_for(vec![K])).await.unwrap();

    step_until(&mut runner, |runner| {
        runner.region().chunk(K).is_some() && maker.keeps_the_load_of(N)
    });
    let mut told = told_until_marked(&mut runner, &mut edge, -10);
    told.extend(received(&mut edge));
    assert!(
        !answers_for(&told, K),
        "shown before it was settled: {told:?}"
    );
    assert_eq!(marks_of(&runner, K), 1);

    maker.let_go(N);
    let told = until_answered(&mut runner, &mut edge, &[K]);
    let shown = snapshots(&told);
    assert!(connected(&shown[&K]));
    assert_eq!(marks_of(&runner, K), 0);
}

/// A marked chunk whose missing neighbours are another region's is shown as it is,
/// marks and all, to be settled later.
#[tokio::test]
async fn a_marked_chunk_beside_another_regions_land_is_shown_unsettled() {
    let world = world_with(&[AT_THE_LINE], &[]);
    let (mut runner, _maker, mut edge) = western(&world);
    edge.send(asking_for(around(AT_THE_LINE))).await.unwrap();

    let told = until_answered(&mut runner, &mut edge, &around(AT_THE_LINE));
    let elsewhere: Vec<ChunkPos> = told
        .iter()
        .filter_map(told_of)
        .filter(|(_, _, what)| *what == Told::Elsewhere(EAST))
        .map(|(chunk, _, _)| chunk)
        .collect();
    assert_eq!(elsewhere.len(), 3, "{told:?}");
    let shown = snapshots(&told);
    let chunk = shown.get(&AT_THE_LINE).expect("a snapshot of the chunk");
    assert!(!connected(chunk), "it cannot have been post-processed");
    assert_eq!(marks_of(&runner, AT_THE_LINE), 1);
}

/// A marked chunk whose missing neighbour cannot be read is shown as it is.
#[tokio::test]
async fn a_marked_chunk_beside_a_chunk_that_cannot_be_read_is_shown_unsettled() {
    let world = world_with(&[K], &[]);
    let (mut runner, maker, mut edge) = western(&world);
    maker.fail(N);
    edge.send(asking_for(around(K))).await.unwrap();

    let others: Vec<ChunkPos> = around(K).into_iter().filter(|chunk| *chunk != N).collect();
    let told = until_answered(&mut runner, &mut edge, &others);
    assert!(runner.unreadable.contains(&N));
    let shown = snapshots(&told);
    assert!(!connected(&shown[&K]));
    assert_eq!(marks_of(&runner, K), 1);
}

/// While a marked chunk waits for a neighbour that is being made, it does not hold
/// what its link sends behind a hello that names it: the step sent after the hello is
/// taken while the chunk still waits. The chunk is shown settled once the neighbour is
/// made.
#[tokio::test]
async fn while_a_marked_chunk_waits_it_does_not_hold_its_links_other_messages() {
    let world = world_with(&[K], &[]);
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 2, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    maker.make(N);
    edge.send(asking_for(around(K))).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().chunk(K).is_some() && maker.handed(Handed::Generating(N))
    });
    settle(&mut runner);
    edge.everything();

    // The link breaks; the edge comes back on another, names the chunks it watched,
    // and its player steps.
    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &runner);
    link_ends(&mut runner, edge).await;
    runner.links().attach(worker_end);
    let mut watched = vec![ORIGIN];
    watched.extend(around(K));
    again
        .send(again.hello(0, &[player()], &watched))
        .await
        .unwrap();
    again.send(walk(player(), THERE)).await.unwrap();

    step_until(&mut runner, |runner| {
        assert!(runner.region().chunk(N).is_none(), "N is still made");
        x_of(runner, player()) == Some(THERE)
    });
    assert!(runner.region().chunk(K).is_some());
    assert_eq!(marks_of(&runner, K), 1, "K still waits");
    assert!(!holds(&runner));
    let told = received(&mut again);
    assert!(
        !answers_for(&told, K),
        "shown before it was settled: {told:?}"
    );

    maker.let_go(N);
    let told = until_answered(&mut runner, &mut again, &[K]);
    assert!(connected(&snapshots(&told)[&K]));
}

/// [`told`] of the tests module, under a name that does not clash with the variables
/// here.
fn told_of(message: &WorkerToEdge) -> Option<(ChunkPos, u64, Told)> {
    told(message)
}

/// What a tick post-processed, and the seam entries a player's break keeps, are
/// committed: a region whose process ends before it saves either chunk is restored
/// with the chunk post-processed and without marks, and with the entries.
#[tokio::test]
async fn post_processing_and_seams_are_committed_and_outlive_the_runner_without_a_save() {
    let world = world_with(&[K], &[]);
    let (edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 2, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    edge.send(asking_for(around(K))).await.unwrap();
    step_until(&mut runner, |runner| {
        runner
            .region()
            .chunk(K)
            .is_some_and(|chunk| chunk.marks().is_empty())
    });

    // The player walks into `N` and breaks the grass at its western edge, beyond which
    // nothing is loaded.
    edge.send(walk(player(), -60.5)).await.unwrap();
    step_until(&mut runner, |runner| x_of(runner, player()) == Some(-60.5));
    edge.send(dig_by(player(), -64, 1)).await.unwrap();
    step_until(&mut runner, |runner| !runner.region().seams(N).is_empty());
    settle(&mut runner);
    let seams = runner.region().seams(N);
    let saved: Vec<Asked> = maker
        .asked()
        .into_iter()
        .filter(|asked| matches!(asked, Asked::Save(chunk) if *chunk == K || *chunk == N))
        .collect();
    assert_eq!(saved, [], "neither chunk was saved");
    // Both are counted unsaved, to be saved where a stop is near.
    assert!(runner.unsaved.contains(&K) && runner.unsaved.contains(&N));
    drop((runner, edge));

    let (handle, _) = world.open(RegionId(0), 3);
    let k = made(&handle, K);
    assert_eq!(k.marks(), []);
    assert!(connected(&k));
    let n = made(&handle, N);
    assert_eq!(n.seams(), seams);
    handle.flush();
}

/// A marked chunk of the gap between the pinned areas, which the home region holds,
/// stored with its fence marked.
const IN_THE_GAP: ChunkPos = ChunkPos::new(5, 0);

/// A neighbour the region is claiming is on its way: a marked chunk whose missing
/// neighbours are nobody's waits while the store has not answered their claims, and is
/// shown settled once they are granted and loaded.
#[tokio::test]
async fn a_marked_chunks_snapshot_waits_while_its_neighbours_are_claimed() {
    let world = Divided::gap();
    {
        let (handle, _) = world.open(HOME, 1);
        assert_eq!(claim(&handle, &[IN_THE_GAP]).0, [IN_THE_GAP]);
        let mut chunk = made(&handle, IN_THE_GAP);
        chunk.set(FENCE.0, FENCE.1, FENCE.2, clustine_data::blocks::OAK_FENCE);
        chunk.set(STONE.0, STONE.1, STONE.2, clustine_data::blocks::STONE);
        chunk.set_marks(vec![InChunk::new(FENCE.0, FENCE.1, FENCE.2)]);
        handle.request(StoreRequest::Save {
            position: IN_THE_GAP,
            tick: 0,
            chunk,
        });
        handle.flush();
    }
    // A region that gives back what it holds as soon as nothing uses it would give
    // the chunk back before the edge subscribes.
    let settings = settings(DEFAULT_RETURN_AFTER);
    let (runner, gate) = gated_as(&world.store, world.hello(HOME, 2), settings);
    let mut runner = runner.with_id(HOME);
    let (mut edge, worker_end) = in_process(256);
    runner.links().attach(worker_end);
    step(&mut runner);
    edge.everything();
    gate.hold_claims();
    edge.send(asking_for(around(IN_THE_GAP))).await.unwrap();

    step_until(&mut runner, |runner| {
        runner.region().chunk(IN_THE_GAP).is_some() && gate.kept_claims() > 0
    });
    for _ in 0..3 {
        step(&mut runner);
        let told = received(&mut edge);
        assert!(!answers_for(&told, IN_THE_GAP), "shown unsettled: {told:?}");
        assert_eq!(marks_of(&runner, IN_THE_GAP), 1);
    }

    gate.release_claims();
    let told = until_answered(&mut runner, &mut edge, &around(IN_THE_GAP));
    assert!(connected(&snapshots(&told)[&IN_THE_GAP]));
    assert_eq!(marks_of(&runner, IN_THE_GAP), 0);
}
