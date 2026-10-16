//! Tests of the runner's part of generation off the thread for chunks: item 11 of
//! section 9 of `docs/adr/0021-generation-is-a-function.md`, from section 7 of that
//! record ("What waits for a load in the worker and at the edge", "`Unreadable`, and
//! how it ends" and "The two new answers").
//!
//! The world store does not say `Generating` or `Withdrawn` yet, so a double stands
//! before the real one and says them as section 7 has the store say them: of a chunk
//! the test names as being made, a load is answered `Generating` at once and `Loaded`
//! when the test lets the chunk go; a return that names such a chunk while its load
//! waits answers that load `Withdrawn` in its place; a chunk the test names as failing
//! is answered `Generating` and then `Unreadable`. The double can also hold back the
//! answer to a load of a stored chunk, as the gate of the other tests does, and the
//! word `Generating` itself, so that a test can stand between the load and that word.
//!
//! Every wait is a loop that steps until a message or a state is there. To show that
//! something did not happen, a test waits for something later that could only come
//! after the point at which it would have.

use super::*;

mod first_snapshot;

/// Where an edge's player stands in these tests, and where a step takes them: both in
/// the chunk at the origin, which is stored once the player has joined.
const THERE: f64 = 3.0;

/// What the double handed the runner of what section 7 is about, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handed {
    Generating(ChunkPos),
    Withdrawn(ChunkPos),
    Unreadable(ChunkPos),
    Loaded(ChunkPos),
}

impl Handed {
    fn of(reply: &StoreReply) -> Option<Self> {
        match reply {
            StoreReply::Generating { position } => Some(Self::Generating(*position)),
            StoreReply::Withdrawn { position } => Some(Self::Withdrawn(*position)),
            StoreReply::Unreadable { position } => Some(Self::Unreadable(*position)),
            StoreReply::Loaded { position, .. } => Some(Self::Loaded(*position)),
            _ => None,
        }
    }
}

/// A store's handle that says of chosen chunks what the store says of chunks it makes.
struct Making {
    inner: StoreHandle,
    control: Arc<Maker>,
}

/// The test's side of [`Making`].
#[derive(Default)]
struct Maker {
    state: Mutex<MakerState>,
}

#[derive(Default)]
struct MakerState {
    /// Chunks being made: a load is answered `Generating`, and `Loaded` when let go.
    made: BTreeSet<ChunkPos>,
    /// Chunks of [`MakerState::made`] whose `Generating` is held back too, until spoken.
    unspoken: BTreeSet<ChunkPos>,
    /// Chunks the generator fails for: `Generating`, then `Unreadable`.
    failing: BTreeSet<ChunkPos>,
    /// Stored chunks whose `Loaded` is held back, without a word before it.
    slow: BTreeSet<ChunkPos>,
    /// What the runner is handed next, in that order.
    ready: VecDeque<StoreReply>,
    /// What is held back, in the order the store gave it.
    kept: Vec<StoreReply>,
    /// What the runner asked for, in that order.
    asked: Vec<Asked>,
    /// What the runner was handed of loads, in that order.
    handed: Vec<Handed>,
}

impl MakerState {
    /// Takes an answer of the real store and puts what the runner is to be told of it
    /// where it belongs.
    fn take(&mut self, reply: StoreReply) {
        // The double says itself which chunks are being made; the store behind it
        // says so of every chunk it has not stored, which here would be all of them.
        if matches!(reply, StoreReply::Generating { .. }) {
            return;
        }
        let StoreReply::Loaded { position, .. } = &reply else {
            self.ready.push_back(reply);
            return;
        };
        let position = *position;
        if self.failing.contains(&position) {
            self.ready.push_back(StoreReply::Generating { position });
            self.ready.push_back(StoreReply::Unreadable { position });
        } else if self.made.contains(&position) {
            let word = StoreReply::Generating { position };
            if self.unspoken.contains(&position) {
                self.kept.push(word);
            } else {
                self.ready.push_back(word);
            }
            self.kept.push(reply);
        } else if self.slow.contains(&position) {
            self.kept.push(reply);
        } else {
            self.ready.push_back(reply);
        }
    }

    /// Hands on, in the order they were kept, the kept answers that `free` picks.
    fn free(&mut self, free: impl Fn(&StoreReply) -> bool) {
        let (freed, kept) = mem::take(&mut self.kept).into_iter().partition(free);
        self.kept = kept;
        self.ready.extend::<Vec<_>>(freed);
    }
}

impl Maker {
    fn state(&self) -> std::sync::MutexGuard<'_, MakerState> {
        self.state.lock().unwrap()
    }

    /// From now on the store is making `chunk` when it is loaded.
    fn make(&self, chunk: ChunkPos) {
        self.state().made.insert(chunk);
    }

    /// As [`Maker::make`], and the word that it is being made is held back too.
    fn make_silently(&self, chunk: ChunkPos) {
        let mut state = self.state();
        state.made.insert(chunk);
        state.unspoken.insert(chunk);
    }

    /// From now on the generator fails for `chunk`.
    fn fail(&self, chunk: ChunkPos) {
        self.state().failing.insert(chunk);
    }

    /// From now on the answer to a load of the stored `chunk` is held back.
    fn slow_down(&self, chunk: ChunkPos) {
        self.state().slow.insert(chunk);
    }

    /// Hands on the word that `chunk` is being made.
    fn speak(&self, chunk: ChunkPos) {
        let mut state = self.state();
        state.unspoken.remove(&chunk);
        state.free(|reply| *reply == StoreReply::Generating { position: chunk });
    }

    /// The generator is done with `chunk`, and the store answers every load of it as
    /// it does a stored one: what was held back of it is handed on.
    fn let_go(&self, chunk: ChunkPos) {
        let mut state = self.state();
        let state = &mut *state;
        for set in [
            &mut state.made,
            &mut state.unspoken,
            &mut state.failing,
            &mut state.slow,
        ] {
            set.remove(&chunk);
        }
        state.free(|reply| Handed::of(reply).is_some_and(|handed| handed.chunk() == chunk));
    }

    /// Whether the store's answer to a load of `chunk` is held back, as `Loaded`.
    fn keeps_the_load_of(&self, chunk: ChunkPos) -> bool {
        let state = self.state();
        state
            .kept
            .iter()
            .any(|reply| Handed::of(reply) == Some(Handed::Loaded(chunk)))
    }

    /// Whether the word that `chunk` is being made is held back.
    fn keeps_the_word_of(&self, chunk: ChunkPos) -> bool {
        let state = self.state();
        state
            .kept
            .contains(&StoreReply::Generating { position: chunk })
    }

    /// Whether the runner was handed `handed`.
    fn handed(&self, handed: Handed) -> bool {
        self.state().handed.contains(&handed)
    }

    /// What the runner was handed of loads so far, but for the home chunk's, which a
    /// region that holds it loads whoever needs it (docs/groundwork/w6-plan.md,
    /// section 2), and which these tests do not look at.
    fn all_handed(&self) -> Vec<Handed> {
        let handed = self.state().handed.clone();
        handed
            .into_iter()
            .filter(|handed| handed.chunk() != ChunkPos::containing(SPAWN.x, SPAWN.z))
            .collect()
    }

    /// What the runner asked for since this was last called, in that order.
    fn asked(&self) -> Vec<Asked> {
        mem::take(&mut self.state().asked)
    }
}

impl Handed {
    fn chunk(self) -> ChunkPos {
        match self {
            Self::Generating(chunk)
            | Self::Withdrawn(chunk)
            | Self::Unreadable(chunk)
            | Self::Loaded(chunk) => chunk,
        }
    }
}

impl RegionStore for Making {
    fn request(&self, request: StoreRequest) {
        {
            let mut state = self.control.state();
            state.asked.extend(Asked::of(&request));
            // Section 7, rule 5: a return of a chunk whose load waits for the generator
            // takes that load out, and answers it `Withdrawn`.
            if let StoreRequest::Return { chunks } = &request {
                for &chunk in chunks {
                    if !state.made.contains(&chunk) {
                        continue;
                    }
                    let waiting = |reply: &StoreReply| {
                        Handed::of(reply) == Some(Handed::Loaded(chunk))
                            || *reply == StoreReply::Generating { position: chunk }
                    };
                    let before = state.kept.len();
                    state.kept.retain(|reply| !waiting(reply));
                    let loads = (before - state.kept.len()).min(1);
                    for _ in 0..loads {
                        state
                            .ready
                            .push_back(StoreReply::Withdrawn { position: chunk });
                    }
                }
            }
        }
        self.inner.request(request);
    }

    fn try_reply(&self) -> Option<StoreReply> {
        let mut state = self.control.state();
        loop {
            if let Some(reply) = state.ready.pop_front() {
                if let Some(handed) = Handed::of(&reply) {
                    state.handed.push(handed);
                }
                return Some(reply);
            }
            let reply = self.inner.try_reply()?;
            state.take(reply);
        }
    }

    fn is_lost(&self) -> bool {
        self.inner.is_lost()
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// A runner for `region` of `world`, opened with `epoch`, with [`Making`] before its
/// store, that is told which region it runs.
fn making(
    world: &Divided,
    region: RegionId,
    epoch: u64,
    config: RegionConfig,
) -> (RegionRunner, Arc<Maker>) {
    let (inner, restored) = world.open(region, epoch);
    let control = Arc::new(Maker::default());
    let store = Making {
        inner,
        control: Arc::clone(&control),
    };
    let holdings = holdings(&restored);
    let state = restored_state(restored).unwrap();
    let region_now = Region::restore(config, state, holdings);
    let runner = RegionRunner::with_store(region_now, Box::new(store))
        .without_rings()
        .with_id(region);
    (runner, control)
}

/// The squared distance in chunks from `chunk` to the nearest of `players`' chunks, or
/// none if there are no players.
fn nearness(chunk: ChunkPos, players: &[ChunkPos]) -> Option<i64> {
    players
        .iter()
        .map(|player| {
            let dx = i64::from(chunk.x - player.x);
            let dz = i64::from(chunk.z - player.z);
            dx * dx + dz * dz
        })
        .min()
}

/// The loads among `asked`, in runs: those asked one after another with nothing else
/// between, which are what one tick asked for.
fn runs_of_loads(asked: &[Asked]) -> Vec<Vec<ChunkPos>> {
    let mut runs: Vec<Vec<ChunkPos>> = Vec::new();
    let mut in_run = false;
    for asked in asked {
        match asked {
            Asked::Load(chunk) if in_run => runs.last_mut().unwrap().push(*chunk),
            Asked::Load(chunk) => {
                runs.push(vec![*chunk]);
                in_run = true;
            }
            _ => in_run = false,
        }
    }
    runs
}

/// Steps `runner` until the store has been asked to load every one of `chunks`, and
/// returns what it was asked for meanwhile.
fn asked_until_loaded(
    runner: &mut RegionRunner,
    maker: &Maker,
    chunks: &BTreeSet<ChunkPos>,
) -> Vec<Asked> {
    let mut asked = Vec::new();
    step_until(runner, |_| {
        asked.extend(maker.asked());
        let loaded: BTreeSet<_> = asked
            .iter()
            .filter_map(|asked| match asked {
                Asked::Load(chunk) => Some(*chunk),
                _ => None,
            })
            .collect();
        chunks.is_subset(&loaded)
    });
    asked
}

/// The chunks of the western stripe from x = -5 to 0 and z = -2 to 2, which region 0
/// of [`Divided::stripes`] holds once it claims them.
fn western_block() -> Vec<ChunkPos> {
    (-5..=0)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A tick's loads go out nearest the region's players first: by the squared distance
/// in chunks to the nearest player's chunk, ties by position.
#[tokio::test]
async fn a_ticks_loads_go_out_nearest_the_regions_players_first() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 1, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    step_in(&mut runner);
    // One player stays in the chunk at the origin, the other steps three chunks west.
    edge.send(walk(other_player(), -40.5)).await.unwrap();
    step_until(&mut runner, |runner| {
        x_of(runner, other_player()) == Some(-40.5)
    });
    settle(&mut runner);
    edge.everything();
    let players = [ORIGIN, ChunkPos::new(-3, 0)];

    let block = western_block();
    let wanted: BTreeSet<_> = block
        .iter()
        .copied()
        .filter(|chunk| runner.region().chunk(*chunk).is_none())
        .collect();
    maker.asked();
    edge.send(asking_for(block)).await.unwrap();
    let asked = asked_until_loaded(&mut runner, &maker, &wanted);

    let runs = runs_of_loads(&asked);
    let all: Vec<_> = runs.iter().flatten().copied().collect();
    assert_eq!(all.iter().copied().collect::<BTreeSet<_>>(), wanted);
    assert!(
        runs.iter().any(|run| run.len() > 1),
        "no tick asked for more than one load: {runs:?}"
    );
    for run in &runs {
        let mut sorted = run.clone();
        sorted.sort_by_key(|chunk| (nearness(*chunk, &players), *chunk));
        assert_eq!(*run, sorted, "a tick's loads, of {runs:?}");
    }
}

/// Without players a tick's loads go out in the order of their positions.
#[tokio::test]
async fn a_ticks_loads_go_out_in_position_order_when_the_region_has_no_players() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 1, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    step(&mut runner);
    edge.everything();
    assert_eq!(runner.region().player_count(), 0);

    let block = western_block();
    let wanted: BTreeSet<_> = block.iter().copied().collect();
    maker.asked();
    edge.send(asking_for(block)).await.unwrap();
    let asked = asked_until_loaded(&mut runner, &maker, &wanted);

    let runs = runs_of_loads(&asked);
    let all: Vec<_> = runs.iter().flatten().copied().collect();
    assert_eq!(all.iter().copied().collect::<BTreeSet<_>>(), wanted);
    assert!(
        runs.iter().any(|run| run.len() > 1),
        "no tick asked for more than one load: {runs:?}"
    );
    for run in &runs {
        let mut sorted = run.clone();
        sorted.sort();
        assert_eq!(*run, sorted, "a tick's loads, of {runs:?}");
    }
}

/// A chunk answered `Unreadable` is forgotten as such when the region gives it back,
/// and is loaded again, and shown, when the region is granted it again.
#[tokio::test]
async fn an_unreadable_chunk_is_forgotten_when_given_back_and_loaded_again_when_granted_again() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, HOME, 1, config(0));
    runner.links().attach(worker_end);
    maker.fail(BESIDE);

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |runner| runner.unreadable.contains(&BESIDE));
    assert_eq!(
        maker.all_handed(),
        [Handed::Generating(BESIDE), Handed::Unreadable(BESIDE)]
    );

    // The link lets go of it, and the region gives it back once nothing uses it.
    maker.asked();
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    let mut returned = false;
    step_until(&mut runner, |_| {
        returned |= maker
            .asked()
            .iter()
            .any(|asked| matches!(asked, Asked::Return(chunks) if chunks.contains(&BESIDE)));
        returned
    });
    assert!(
        !runner.unreadable.contains(&BESIDE),
        "the region gave the chunk back and still has it as unreadable"
    );
    received(&mut edge);

    // The generator no longer fails there. Granted again, the chunk is asked for again.
    maker.let_go(BESIDE);
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    let ask = edge.asked();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, ask, Told::Snapshot)]
    );
    assert!(runner.region().chunk(BESIDE).is_some());
    assert!(!runner.unreadable.contains(&BESIDE));
    assert_eq!(
        maker.all_handed()[2..],
        [Handed::Loaded(BESIDE)],
        "the chunk was loaded again"
    );
}

/// `Withdrawn` answers one load of its chunk: the answer to the next load of that
/// chunk is taken for the region, and not thrown away as one to a load it dropped.
#[tokio::test]
async fn withdrawn_counts_as_the_answer_to_one_load_and_the_next_answer_reaches_the_region() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, HOME, 1, config(0));
    runner.links().attach(worker_end);
    maker.make(BESIDE);

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |_| maker.handed(Handed::Generating(BESIDE)));
    assert!(maker.keeps_the_load_of(BESIDE));

    // The link lets go of the chunk while it is made; the region gives it back, which
    // withdraws the load.
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |_| maker.handed(Handed::Withdrawn(BESIDE)));
    assert!(!maker.keeps_the_load_of(BESIDE));
    // The step after the one that took the word, so that it has been counted.
    step(&mut runner);
    received(&mut edge);

    // The chunk is made by now. Asked for again, it is granted again and loaded.
    maker.let_go(BESIDE);
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    let ask = edge.asked();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, ask, Told::Snapshot)]
    );
    assert!(runner.region().chunk(BESIDE).is_some());
    assert_eq!(
        maker.all_handed(),
        [
            Handed::Generating(BESIDE),
            Handed::Withdrawn(BESIDE),
            Handed::Loaded(BESIDE),
        ]
    );
    assert!(runner.loads.is_empty(), "{:?}", runner.loads);
}

/// Whether a link of `runner` holds what it was sent behind a hello.
fn holds(runner: &RegionRunner) -> bool {
    runner.links.values().any(|link| !link.held.is_empty())
}

/// Has the link of `edge` end, and waits until the runner has noticed.
async fn link_ends(runner: &mut RegionRunner, edge: TestEdge) {
    drop(edge);
    step_until(runner, |runner| runner.links.is_empty());
}

/// W5h: a hello that names a chunk the store has already answered `Generating` for is
/// welcomed, and its link's next input reaches a tick while the chunk is still made.
#[tokio::test]
async fn a_hello_that_names_a_chunk_being_made_is_welcomed_and_its_next_input_is_taken() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 1, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    maker.make(WEST_OF_HOME);

    // The edge's viewer looks west, where the chunk is being made.
    edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
    step_until(&mut runner, |_| {
        maker.handed(Handed::Generating(WEST_OF_HOME))
    });
    settle(&mut runner);
    edge.everything();

    // The link breaks; the edge comes back on another, and its player steps.
    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &runner);
    link_ends(&mut runner, edge).await;
    runner.links().attach(worker_end);
    again
        .send(again.hello(0, &[player()], &[ORIGIN, WEST_OF_HOME]))
        .await
        .unwrap();
    again.send(walk(player(), THERE)).await.unwrap();

    step_until(&mut runner, |runner| {
        assert!(runner.region().chunk(WEST_OF_HOME).is_none());
        x_of(runner, player()) == Some(THERE)
    });
    assert!(
        maker.keeps_the_load_of(WEST_OF_HOME),
        "the chunk is still made"
    );
    let applied = runner.region().edge(again.edge).unwrap().applied;
    assert_eq!(applied, again.sent.load(Ordering::Relaxed));
    settle(&mut runner);
    again.everything();
    assert!(
        matches!(again.welcomed, Some(Welcome::Resumed { .. })),
        "{:?}",
        again.welcomed
    );

    // When it is made, the link is shown it.
    maker.let_go(WEST_OF_HOME);
    step_until(&mut runner, |runner| {
        runner.region().chunk(WEST_OF_HOME).is_some()
    });
    settle(&mut runner);
    let shown = again.everything().into_iter().any(|message| {
        matches!(message, WorkerToEdge::ChunkSnapshot { position, .. } if position == WEST_OF_HOME)
    });
    assert!(shown);
}

/// W5h: what an edge sends again after a restore that digs in a chunk the store is
/// making does not reach a tick until the chunk is loaded, and then takes effect. What
/// the link sent behind it waits with it.
#[tokio::test]
async fn a_dig_sent_again_after_a_restore_into_a_chunk_being_made_waits_for_it_and_takes_effect() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut first, _) = making(&world, RegionId(0), 1, config(DEFAULT_RETURN_AFTER));
    first.links().attach(worker_end);
    joined(&edge, &mut first).await;
    settle(&mut first);
    let applied = first.region().edge(edge.edge).unwrap().applied;
    assert_eq!(applied, edge.sent.load(Ordering::Relaxed));

    // The edge digs to the west and steps, and the region's process ends before it
    // takes either.
    let (dig, step_aside) = (dig(-1), walk(player(), THERE));
    edge.send(dig.clone()).await.unwrap();
    edge.send(step_aside.clone()).await.unwrap();
    drop(first);
    assert!(closed(&mut edge).await);

    // The region is restored. The chunk west of the player was never stored: it is
    // being made when the edge comes back, says hello and sends both again.
    let (mut second, maker) = making(&world, RegionId(0), 2, config(DEFAULT_RETURN_AFTER));
    maker.make(WEST_OF_HOME);
    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &second);
    second.links().attach(worker_end);
    again
        .send(again.hello(0, &[player()], &[ORIGIN, WEST_OF_HOME]))
        .await
        .unwrap();
    again.send_as(applied + 1, dig).await;
    again.send_as(applied + 2, step_aside).await;

    // The chunk the player stands in is loaded and shown, and the store has said that
    // the other is being made; the dig still waits.
    step_until(&mut second, |runner| {
        runner.region().chunk(ORIGIN).is_some() && maker.handed(Handed::Generating(WEST_OF_HOME))
    });
    for _ in 0..3 {
        step(&mut second);
        assert_eq!(second.region().edge(again.edge).unwrap().applied, applied);
        assert_eq!(dug(&second, -1), None);
        assert_eq!(x_of(&second, player()), Some(SPAWN.x));
    }
    assert!(maker.keeps_the_load_of(WEST_OF_HOME));

    // Made, it is loaded, and the dig takes effect; then the step behind it.
    maker.let_go(WEST_OF_HOME);
    step_until(&mut second, |runner| x_of(runner, player()) == Some(THERE));
    assert_eq!(dug(&second, -1), Some(true));
    assert_eq!(
        second.region().edge(again.edge).unwrap().applied,
        applied + 2
    );
    assert!(!holds(&second));
    settle(&mut second);
    assert!(shown_before_it_changed(&mut again, WEST_OF_HOME).await);
}

/// W5h: a chunk that is stored holds the link as it did before: what follows the hello
/// waits for its snapshot.
#[tokio::test]
async fn a_hello_that_names_a_stored_chunk_holds_its_link_until_the_chunk_is_shown() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 1, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    settle(&mut runner);
    edge.everything();
    maker.slow_down(WEST_OF_HOME);

    let (edge_end, worker_end) = link::in_process(256);
    let again = edge.again(edge_end, &runner);
    link_ends(&mut runner, edge).await;
    runner.links().attach(worker_end);
    again
        .send(again.hello(0, &[player()], &[ORIGIN, WEST_OF_HOME]))
        .await
        .unwrap();
    again.send(walk(player(), THERE)).await.unwrap();

    // The store has answered the load, and the answer is on its way; the link holds.
    step_until(&mut runner, |_| maker.keeps_the_load_of(WEST_OF_HOME));
    for _ in 0..3 {
        step(&mut runner);
        assert_eq!(x_of(&runner, player()), Some(SPAWN.x));
        assert!(holds(&runner));
    }

    maker.let_go(WEST_OF_HOME);
    let mut loaded_before_the_step = false;
    step_until(&mut runner, |runner| {
        let moved = x_of(runner, player()) == Some(THERE);
        loaded_before_the_step |= !moved && runner.region().chunk(WEST_OF_HOME).is_some();
        moved
    });
    assert!(loaded_before_the_step);
    assert!(!holds(&runner));
}

/// A link that already holds on a chunk is let go of when the store says the chunk is
/// being made, before the chunk is there.
#[tokio::test]
async fn a_link_holding_on_a_chunk_is_let_go_when_the_store_says_it_is_being_made() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, maker) = making(&world, RegionId(0), 1, config(DEFAULT_RETURN_AFTER));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    settle(&mut runner);
    edge.everything();
    maker.make_silently(WEST_OF_HOME);

    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &runner);
    link_ends(&mut runner, edge).await;
    runner.links().attach(worker_end);
    again
        .send(again.hello(0, &[player()], &[ORIGIN, WEST_OF_HOME]))
        .await
        .unwrap();
    again.send(walk(player(), THERE)).await.unwrap();

    // The store has answered, and its word that it makes the chunk is on its way.
    step_until(&mut runner, |_| maker.keeps_the_word_of(WEST_OF_HOME));
    for _ in 0..3 {
        step(&mut runner);
        assert_eq!(x_of(&runner, player()), Some(SPAWN.x));
        assert!(holds(&runner));
    }

    // The word comes; the chunk is not there, and the step is taken.
    maker.speak(WEST_OF_HOME);
    step_until(&mut runner, |runner| {
        assert!(runner.region().chunk(WEST_OF_HOME).is_none());
        x_of(runner, player()) == Some(THERE)
    });
    assert!(maker.keeps_the_load_of(WEST_OF_HOME));
    assert!(!holds(&runner));

    maker.let_go(WEST_OF_HOME);
    step_until(&mut runner, |runner| {
        runner.region().chunk(WEST_OF_HOME).is_some()
    });
    settle(&mut runner);
    let shown = again.everything().into_iter().any(|message| {
        matches!(message, WorkerToEdge::ChunkSnapshot { position, .. } if position == WEST_OF_HOME)
    });
    assert!(shown);
}
