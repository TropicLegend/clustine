use super::*;

/// Where `p` stands in the scenarios: 19 chunks east of the home chunk.
const P_AT: ChunkPos = ChunkPos::new(19, 0);

/// The time before a return of the regions of these scenarios, as the processes
/// set it: what a split leaves a region is still its own when an edge says hello.
const KEPT_FOR: u64 = DEFAULT_RETURN_AFTER;

/// The epoch a part is to be opened with.
const AS_EPOCH: u64 = 5;

fn at(x: i32, z: i32) -> ChunkPos {
    ChunkPos::new(x, z)
}

/// `chunks` in ascending order, each once.
fn sorted(chunks: impl IntoIterator<Item = ChunkPos>) -> Vec<ChunkPos> {
    let set: BTreeSet<ChunkPos> = chunks.into_iter().collect();
    set.into_iter().collect()
}

/// The seven by seven chunks around `centre`, ascending: what a viewer who
/// stands there sees at a view distance of 3.
fn around(centre: ChunkPos) -> Vec<ChunkPos> {
    let mut chunks = Vec::new();
    for x in -3..=3 {
        for z in -3..=3 {
            chunks.push(at(centre.x + x, centre.z + z));
        }
    }
    sorted(chunks)
}

/// The seven chunks `(x, -3)` to `(x, 3)`: the row that comes into the view of
/// somebody who walks east or west along z = 0.
fn row_at(x: i32) -> Vec<ChunkPos> {
    (-3..=3).map(|z| at(x, z)).collect()
}

/// The chunks of both, ascending.
fn both(one: &[ChunkPos], other: &[ChunkPos]) -> Vec<ChunkPos> {
    sorted(one.iter().chain(other).copied())
}

/// The smallest box that has every chunk of `chunks`, both corners, as the
/// store's list says the bounds of a region.
fn corners(chunks: &[ChunkPos]) -> Option<(ChunkPos, ChunkPos)> {
    let xs = || chunks.iter().map(|chunk| chunk.x);
    let zs = || chunks.iter().map(|chunk| chunk.z);
    let min = at(xs().min()?, zs().min()?);
    let max = at(xs().max()?, zs().max()?);
    Some((min, max))
}

/// A world that is one home region, pinned to nothing: every chunk but the
/// home chunk is nobody's until a region claims it.
fn open_land() -> Divided {
    Divided::new(Pins::none())
}

/// The world of [`open_land`] as it is kept in `directory`.
fn open_land_in(directory: &std::path::Path) -> Divided {
    let generator = Arc::new(FlatGenerator::classic());
    let division = Pins::none();
    let store = Store::local_divided(directory, generator, division).unwrap();
    Divided { store }
}

/// Two regions pinned side by side: `A`, region 0, to x < 30, with the home
/// chunk, and `B`, which is [`EAST`], to the rest.
fn cut_at_thirty() -> Divided {
    Divided::new(Pins::side_by_side(&[30]).unwrap())
}

/// A home region pinned to nothing on open land, with two pinned regions far
/// out on either side, of which [`EAST`] is there to be absorbed. The home
/// region is [`HOME`], as in [`Divided::gap`].
fn with_a_region_far_out() -> Divided {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(-60),
    };
    let east = ChunkArea {
        min_x: Some(60),
        max_x: None,
    };
    Divided::new(Pins {
        areas: vec![west, east],
    })
}

/// Whether the region has had an answer for each of `chunks` and has loaded
/// those of them it holds.
fn answered(runner: &RegionRunner, chunks: &[ChunkPos]) -> bool {
    let region = runner.region();
    chunks.iter().all(|chunk| match region.knowledge(*chunk) {
        Knowledge::Held => region.chunk(*chunk).is_some(),
        Knowledge::Foreign(_) => true,
        Knowledge::Asked | Knowledge::Unknown => false,
    })
}

/// The chunks the runner has claimed of the store since the gate was last
/// asked what it saw.
fn claims_sent(gate: &GateControl) -> BTreeSet<ChunkPos> {
    let mut claimed = BTreeSet::new();
    for asked in gate.asked() {
        if let Asked::Claim(chunks) = asked {
            claimed.extend(chunks);
        }
    }
    claimed
}

/// The lines among `lines` that say that chunks asked for players who went
/// were taken for a part's.
fn lines_of_the_taking(lines: &[String]) -> Vec<&str> {
    let of_it = |line: &&String| {
        line.starts_with("chunks asked for players who went are taken for the part's")
    };
    lines.iter().filter(of_it).map(String::as_str).collect()
}

/// The answers to subscriptions among `heard`, in the order they came.
fn answers_among(heard: &[WorkerToEdge]) -> Vec<(ChunkPos, u64, Told)> {
    heard.iter().filter_map(told).collect()
}

/// Steps `runner` until `edge` has been given `count` answers to subscriptions
/// that it asked for with a message of its own, behind its hello, and returns
/// them in ascending order of their chunks. The answers to the hello's chunks,
/// which carry the number 0, are passed over.
fn answers_to_askings(
    runner: &mut RegionRunner,
    edge: &mut TestEdge,
    count: usize,
) -> Vec<(ChunkPos, u64, Told)> {
    let mut found = Vec::new();
    for _ in 0..2000 {
        let heard = received(edge);
        found.extend(answers_among(&heard).into_iter().filter(|told| told.1 != 0));
        if found.len() >= count {
            found.sort();
            return found;
        }
        step(runner);
        thread::sleep(Duration::from_millis(1));
    }
    panic!("of {count} answers only these came: {found:?}");
}

/// The home region of a world with `s`, who is [`player`], in the home chunk
/// and `p`, who is [`other_player`], in `p_at`, both of one edge, whose link
/// has a viewer's subscription to both views. Every chunk of the views that
/// the region holds is loaded.
struct Scene {
    world: Divided,
    runner: RegionRunner,
    gate: Arc<GateControl>,
    /// The link the edge has before the split, which the split closes.
    edge: TestEdge,
    home: RegionId,
    /// The id the store gives the next region, which a split has to name.
    next: RegionId,
    p_at: ChunkPos,
    /// The entities of `s` and of `p`.
    s: EntityId,
    p: EntityId,
}

impl Scene {
    /// The order to split `p` off.
    fn order(&self) -> Reshape {
        Reshape::SplitOff {
            chunks: vec![self.p_at],
            as_epoch: AS_EPOCH,
            part: self.next,
        }
    }

    /// The two views, ascending.
    fn views(&self) -> Vec<ChunkPos> {
        both(&around(ORIGIN), &around(self.p_at))
    }

    /// What the region's state has in the outbox of the edge: the numbers and
    /// the entries.
    fn outbox(&self) -> Vec<(u64, Durable)> {
        let state = self.runner.region().edge(self.edge.edge).unwrap();
        let entries = state.outbox.iter();
        entries
            .map(|(number, entry)| (*number, entry.clone()))
            .collect()
    }

    /// The number of the entry that tells the edge of the split that made
    /// `part`.
    fn number_of_the_split(&self, part: RegionId) -> u64 {
        let of_it = |(_, entry): &(u64, Durable)| matches!(entry, Durable::SplitOff { region, .. } if *region == part);
        let entry = self.outbox().into_iter().find(of_it);
        entry.expect("the edge is told of the split").0
    }

    /// A new link of the same edge, as it makes one when the split has closed
    /// the one it had, with the hello it says there: `seen` is the number of
    /// the last entry it has read.
    async fn hello_again(&self, seen: u64, players: &[PlayerId], chunks: &[ChunkPos]) -> TestEdge {
        let (end, worker_end) = link::in_process(4096);
        let again = self.edge.again(end, &self.runner);
        self.runner.links().attach(worker_end);
        let hello = again.hello(seen, players, chunks);
        again.send(hello).await.unwrap();
        again
    }
}

/// A [`Scene`] in `world`, with a runner that `make` has had its way with and
/// whose region gives back what nothing has used for `return_after` ticks.
async fn scene_with(
    world: Divided,
    p_at: ChunkPos,
    return_after: u64,
    make: impl FnOnce(RegionRunner) -> RegionRunner,
) -> Scene {
    assert_eq!(p_at.z, 0, "p walks along the row the spawn point is in");
    let list = world.store.regions().unwrap();
    let (home, next) = (list.home, list.next);
    let (runner, gate) = world.gated(home, settings(return_after));
    let mut runner = make(runner);
    let (mut edge, worker_end) = in_process(4096);
    runner.links().attach(worker_end);

    let views = both(&around(ORIGIN), &around(p_at));
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    edge.send(asking_for(views.clone())).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().player_count() == 2 && answered(runner, &views)
    });
    let (s, _) = runner.region().player(player()).unwrap();
    let (p, _) = runner.region().player(other_player()).unwrap();
    let x = f64::from(p_at.x) * 16.0 + 8.5;
    let walked = of_entity(p, walk(other_player(), x));
    edge.send(walked).await.unwrap();
    step_until(&mut runner, |runner| {
        x_of(runner, other_player()) == Some(x)
    });
    step(&mut runner);
    edge.everything();
    gate.asked();
    assert_eq!(runner.region().player_count(), 2, "p is the region's");
    Scene {
        world,
        runner,
        gate,
        edge,
        home,
        next,
        p_at,
        s,
        p,
    }
}

/// A [`Scene`] in `world` whose region keeps what it holds.
async fn scene(world: Divided, p_at: ChunkPos) -> Scene {
    scene_with(world, p_at, KEPT_FOR, |runner| runner).await
}

/// What a split gave.
struct Made {
    /// The new region, and the epoch to open it with.
    region: RegionId,
    as_epoch: u64,
    part: Part,
    /// What the runner wrote to the log from the stop to the outcome.
    lines: Vec<String>,
}

impl Made {
    /// The line of the split, which has to be there.
    fn line(&self) -> &str {
        line_of_the_split(&self.lines).expect("the split is logged")
    }
}

/// Steps the runner of `scene` from where it is with its split to the outcome,
/// which has to be a split.
fn stepped_to_the_split(scene: &mut Scene, outcome: &Receiver<Reshaped>) -> Made {
    let (reshaped, lines) = logged(|| reshaped(&mut scene.runner, outcome));
    let Reshaped::Split {
        region,
        as_epoch,
        part,
    } = reshaped
    else {
        panic!("no split: {reshaped:?}");
    };
    assert_eq!((region, as_epoch), (scene.next, AS_EPOCH));
    Made {
        region,
        as_epoch,
        part,
        lines,
    }
}

/// Has `p` split off the region of `scene` while the store's grants of
/// `waiting` wait: the link asks for them as a viewer, a tick claims them, and
/// the gate keeps the store's answer from the runner until the region has
/// stopped ticking, so that no tick is told of them before the split. With no
/// chunk in `waiting` nothing waits.
async fn split_while_waiting(scene: &mut Scene, waiting: &[ChunkPos]) -> Made {
    if !waiting.is_empty() {
        asked_and_not_told(&mut scene.runner, &scene.gate, &scene.edge, waiting).await;
    }
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    step_to(&mut scene.runner, Stage::Settling);
    let tick = scene.runner.region().tick_number();
    scene.gate.release_claims();
    let made = stepped_to_the_split(scene, &outcome);
    // The split is the tick after the last that ran, and none ran in between.
    assert_eq!(scene.runner.region().tick_number(), tick + 1);
    made
}

// R1 and R2. The first way: a grant that waits.

/// Holds the split of R1 to what the scenario says of it: `seven` are the
/// chunks at `(23, ..)`, whose grants waited.
fn check_the_first_way(scene: &Scene, made: &Made, seven: &[ChunkPos]) {
    let region = scene.runner.region();
    let part = &made.part.region;
    let of_the_part = both(&around(P_AT), seven);
    assert_eq!(of_the_part.len(), 56);

    // The part holds the seven, and the region knows nothing of them.
    for chunk in seven {
        assert_eq!(part.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(region.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
    }
    for chunk in &of_the_part {
        assert_eq!(part.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(region.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
    }
    assert_eq!(part.held_chunk_count(), 56);
    assert_eq!(region.held_chunk_count(), 49);
    assert_eq!(
        part.player(other_player()).map(|(entity, _)| entity),
        Some(scene.p)
    );
    assert_eq!(part.player_count(), 1);
    assert_eq!(
        region.player(player()).map(|(entity, _)| entity),
        Some(scene.s)
    );
    assert_eq!(region.player_count(), 1);
    // What was loaded goes with the land. The seven were never read.
    let loaded: Vec<ChunkPos> = made.part.chunks.iter().map(|(chunk, _)| *chunk).collect();
    assert_eq!(loaded, around(P_AT));

    // The store's list has them within the part's bounds and not within the
    // region's.
    let store = &scene.world.store;
    assert_eq!(bounds_of(store, made.region), corners(&of_the_part));
    assert_eq!(bounds_of(store, made.region), Some((at(16, -3), at(23, 3))));
    assert_eq!(bounds_of(store, scene.home), Some((at(-3, -3), at(3, 3))));

    // The line: how many went, how many chunks the part holds, and how many
    // of those were grants that no tick had been told of, behind the tick and
    // the part.
    let line = made.line();
    let tick = region.tick_number();
    let begins = format!("a part of the region has been split off tick={tick} part=");
    assert!(line.starts_with(&begins), "{line}");
    assert!(line.ends_with(" players=1 chunks=56 waited=7"), "{line}");
    assert_eq!(line.matches('=').count(), 5, "{line}");

    // Whoever opens the part is handed the 56 by the store, and restores the
    // region the runner made (ADR-0014, section 2.6).
    let hello = scene.world.hello(made.region, made.as_epoch);
    let (_handle, restored) = store.open_region(hello).unwrap();
    let held: Vec<ChunkPos> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
    assert_eq!(held, of_the_part);
    let holdings = holdings(&restored);
    let block = restored.entity_ids;
    let config = settings(KEPT_FOR).of_restored(&restored);
    let state = restored_state(restored).unwrap();
    // The part was made with no keys, and takes the block its hello gave, as a
    // runner restored with it does (`docs/adr/0028-entities.md`, section 1).
    let mut restored = Region::restore(config, state, holdings);
    restored.take_keys(block);
    assert_eq!(restored, with_keys_of_region(part, block));
}

/// R1, as the runner's own tests can have it: the gate holds the store's
/// answer to the claim back until the region has stopped ticking.
#[tokio::test]
async fn a_grant_that_waits_on_the_parts_side_is_the_parts_by_the_runner_and_by_the_store() {
    // R1.
    let mut scene = scene(open_land(), P_AT).await;
    let seven = row_at(23);
    let made = split_while_waiting(&mut scene, &seven).await;
    check_the_first_way(&scene, &made, &seven);
}

/// R1, as the record has it, with the store as it is and nothing held back:
/// the claim is asked before the split is ordered, so the store answers it
/// before the flush behind the first checkpoint, and the step that finds the
/// flush answered takes both answers and runs no tick.
#[tokio::test]
async fn a_claim_made_in_the_last_tick_is_granted_before_the_flush_and_its_grant_waits() {
    // R1.
    let mut scene = scene(open_land(), P_AT).await;
    let seven = row_at(23);

    // 1. The row comes into p's view. One step: tick T, which takes the
    // subscription, and behind it the claim.
    scene.edge.send(asking_for(seven.clone())).await.unwrap();
    scene.runner.step();
    let tick = scene.runner.region().tick_number();
    for chunk in &seven {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Asked, "{chunk:?}");
    }
    // 2. The split is ordered at once, with no step in between.
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    assert_eq!(scene.runner.stage(), Some(Stage::Preparing));
    // 3. The test waits for the store, and not for the runner.
    scene.world.store.flush().unwrap();
    // 4. One step takes both answers, finds its flush answered and stops
    // ticking. The grant waits.
    scene.runner.step();
    assert_eq!(scene.runner.stage(), Some(Stage::Settling));
    assert_eq!(scene.runner.region().tick_number(), tick, "no tick ran");
    for chunk in &seven {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Asked, "{chunk:?}");
    }
    assert_eq!(sorted(scene.runner.inputs.granted.clone()), seven);

    let made = stepped_to_the_split(&mut scene, &outcome);
    assert_eq!(scene.runner.region().tick_number(), tick + 1);
    check_the_first_way(&scene, &made, &seven);
}

#[tokio::test]
async fn a_grant_that_waits_behind_the_home_chunk_stays_with_the_region() {
    // R2.
    let mut scene = scene(open_land(), P_AT).await;
    let seven = row_at(-4);
    let made = split_while_waiting(&mut scene, &seven).await;

    let region = scene.runner.region();
    let part = &made.part.region;
    for chunk in &seven {
        assert_eq!(region.knowledge(*chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(part.knowledge(*chunk), Knowledge::Unknown, "{chunk:?}");
    }
    assert_eq!(region.held_chunk_count(), 56);
    assert_eq!(part.held_chunk_count(), 49);
    for chunk in around(P_AT) {
        assert_eq!(part.knowledge(chunk), Knowledge::Held, "{chunk:?}");
        assert_eq!(region.knowledge(chunk), Knowledge::Unknown, "{chunk:?}");
    }
    let line = made.line();
    assert!(line.ends_with(" players=1 chunks=49 waited=0"), "{line}");

    let store = &scene.world.store;
    assert_eq!(bounds_of(store, scene.home), Some((at(-4, -3), at(3, 3))));
    assert_eq!(bounds_of(store, made.region), Some((at(16, -3), at(22, 3))));
}

// R3. Killed.

/// Where the worker of a scenario dies.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kill {
    /// The runner is dropped at the first step with this stage.
    At(Stage),
    /// The runner is dropped after the outcome, and the part is never opened.
    Done,
}

/// What the store has of a region when it is opened anew: the chunks it is
/// granted, and its state.
struct Anew {
    held: Vec<ChunkPos>,
    state: RegionState,
}

/// Opens every region the list of `world` has with a higher epoch than any
/// before, and holds what each is restored holding to what the list grants
/// it. No chunk is held by two. A chunk is its dimension's, as the store keys
/// it: the homes of the Nether and of the End hold their chunks (0, 0), which
/// are not the overworld's.
fn opened_anew(world: &Divided) -> BTreeMap<RegionId, Anew> {
    let list = world.store.regions().unwrap();
    let mut all = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for info in &list.regions {
        let (_handle, restored) = world.open(info.region, 9);
        let held: Vec<ChunkPos> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
        let bounds = info.bounds.map(|bounds| (bounds.min, bounds.max));
        assert_eq!(bounds, corners(&held), "{}", info.region);
        for chunk in &held {
            let chunk = clustine_world::LevelChunk::new(info.dimension, *chunk);
            assert!(seen.insert(chunk), "{chunk:?} is held by two regions");
        }
        let holdings = holdings(&restored);
        let state = restored_state(restored).unwrap();
        let region = Region::restore(config(KEPT_FOR), state.clone(), holdings);
        assert_eq!(region.held_chunk_count(), held.len());
        all.insert(info.region, Anew { held, state });
    }
    all
}

/// R1 with a death: the seven chunks at `(23, ..)` are granted and no tick is
/// told, the runner is dropped where `kill` says, the store is started again
/// from its directory if it has one, and both regions are opened anew where
/// the store has them.
async fn the_first_way_with_a_death(kill: Kill, on_disk: bool) {
    let directory = tempfile::tempdir().unwrap();
    let world = match on_disk {
        true => open_land_in(directory.path()),
        false => open_land(),
    };
    let mut scene = scene(world, P_AT).await;
    let seven = row_at(23);
    asked_and_not_told(&mut scene.runner, &scene.gate, &scene.edge, &seven).await;
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    assert_eq!(scene.runner.stage(), Some(Stage::Preparing));
    // A step advances at most one stage, so each is met at its first step.
    let stages = [
        Stage::Preparing,
        Stage::Settling,
        Stage::Closing,
        Stage::Committing,
    ];
    let last = match kill {
        Kill::At(stage) => stages.iter().position(|at| *at == stage),
        Kill::Done => Some(1),
    };
    for stage in &stages[1..=last.expect("a stage")] {
        step_to(&mut scene.runner, *stage);
        if *stage == Stage::Settling {
            // No tick runs from here on: the grant waits.
            scene.gate.release_claims();
        }
    }
    let made = match kill {
        Kill::Done => Some(stepped_to_the_split(&mut scene, &outcome)),
        Kill::At(_) => None,
    };
    let Scene {
        world,
        runner,
        home,
        next,
        ..
    } = scene;
    drop(runner);
    drop(made);
    if kill != Kill::Done {
        // Whether the store has the record is for the store to say.
        let lost = Reshaped::Off {
            why: Off::StoreLost,
        };
        assert_eq!(outcome.try_recv(), Ok(lost), "{kill:?}");
    }
    let world = match on_disk {
        true => {
            // The store has done with what the runner asked before it died,
            // and is started again from what it wrote.
            world.store.flush().unwrap();
            drop(world);
            open_land_in(directory.path())
        }
        false => world,
    };

    let context = format!("{kill:?}, on disk: {on_disk}");
    let anew = opened_anew(&world);
    let has_the_part = matches!(kill, Kill::At(Stage::Committing) | Kill::Done);
    assert_eq!(anew.contains_key(&next), has_the_part, "{context}");
    // The region, the homes of the Nether and of the End, each holding its
    // dimension's chunk (0, 0) and nothing else, and the part if it was made.
    let homes = world.store.regions().unwrap().homes;
    let mut regions: Vec<RegionId> = homes.iter().map(|(_, region)| *region).collect();
    assert_eq!(regions[0], home, "{context}");
    for (_, other) in &homes[1..] {
        assert_eq!(anew[other].held, [ORIGIN], "{context}");
    }
    if has_the_part {
        regions.push(next);
    }
    assert_eq!(
        anew.keys().copied().collect::<Vec<_>>(),
        regions,
        "{context}"
    );
    let region = &anew[&home];
    let everything = both(&both(&around(ORIGIN), &around(P_AT)), &seven);
    let both_players = [player(), other_player()];
    if has_the_part {
        // The seven are the part's, with what was around p, and p with them.
        let part = &anew[&next];
        assert_eq!(part.held, both(&around(P_AT), &seven), "{context}");
        assert_eq!(region.held, around(ORIGIN), "{context}");
        let players: Vec<PlayerId> = part.state.players.keys().copied().collect();
        assert_eq!(players, [other_player()], "{context}");
        let players: Vec<PlayerId> = region.state.players.keys().copied().collect();
        assert_eq!(players, [player()], "{context}");
    } else {
        // No split: the grants that waited are the region's, with every player.
        assert_eq!(region.held, everything, "{context}");
        let players: Vec<PlayerId> = region.state.players.keys().copied().collect();
        assert_eq!(players, both_players, "{context}");
    }
}

#[tokio::test]
async fn a_runner_that_dies_before_the_split_is_sent_leaves_the_grants_that_waited_with_the_region()
{
    // R3: the runner dropped at the first step of each stage before the commit.
    for on_disk in [false, true] {
        for stage in [Stage::Preparing, Stage::Settling, Stage::Closing] {
            the_first_way_with_a_death(Kill::At(stage), on_disk).await;
        }
    }
}

#[tokio::test]
async fn a_runner_that_dies_when_the_split_is_sent_or_made_leaves_the_grants_that_waited_with_the_part()
 {
    // R3: the runner dropped at the first step of the commit, and after the
    // outcome before the part is opened.
    for on_disk in [false, true] {
        for kill in [Kill::At(Stage::Committing), Kill::Done] {
            the_first_way_with_a_death(kill, on_disk).await;
        }
    }
}

/// The chunks R5's hello names as a viewer's: the 49 around the home chunk, the
/// 49 around `(19, 0)` and the seven at `(23, ..)`, which nobody holds.
fn named_by_the_hello() -> Vec<ChunkPos> {
    both(&both(&around(ORIGIN), &around(P_AT)), &row_at(23))
}

/// The 56 of them on the part's side: the 49 that went, and the seven.
fn on_the_parts_side() -> Vec<ChunkPos> {
    both(&around(P_AT), &row_at(23))
}

/// The one case section 3.6.5 allows: the store has the record of the split
/// and dies before it answers. The runner that made the split is gone with
/// what it knew of where the line is, and the region is opened by one that
/// never held a `Splitting`.
async fn the_hole_of_a_region_restored_between_the_split_and_the_hello(on_disk: bool) {
    let directory = tempfile::tempdir().unwrap();
    let world = match on_disk {
        true => open_land_in(directory.path()),
        false => open_land(),
    };
    let mut scene = scene(world, P_AT).await;
    // R2's split, to where the store has it durable and has answered; the
    // answer does not reach the runner.
    let behind = row_at(-4);
    asked_and_not_told(&mut scene.runner, &scene.gate, &scene.edge, &behind).await;
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    step_to(&mut scene.runner, Stage::Settling);
    scene.gate.release_claims();
    scene.gate.hold_reshapes();
    step_to(&mut scene.runner, Stage::Committing);
    for _ in 0..20_000 {
        if scene.gate.kept_reshapes() >= 1 {
            break;
        }
        scene.runner.step();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(scene.gate.kept_reshapes(), 1, "the store did not answer");
    scene.gate.lose();
    let lost = Reshaped::Off {
        why: Off::StoreLost,
    };
    assert_eq!(reshaped(&mut scene.runner, &outcome), lost);
    assert!(scene.runner.store_is_lost());

    let Scene {
        world,
        runner,
        edge,
        home,
        next,
        s,
        p,
        ..
    } = scene;
    drop(runner);
    let world = match on_disk {
        true => {
            world.store.flush().unwrap();
            drop(world);
            open_land_in(directory.path())
        }
        false => world,
    };

    // The region is opened anew, and is the region after the split.
    let part = next;
    let list = world.store.regions().unwrap();
    assert!(list.regions.iter().any(|info| info.region == part));
    assert_eq!(bounds_of(&world.store, part), Some((at(16, -3), at(22, 3))));
    assert_eq!(bounds_of(&world.store, home), Some((at(-4, -3), at(3, 3))));
    let (mut runner, gate) = gated_as(&world.store, world.hello(home, 2), settings(KEPT_FOR));
    assert_eq!(runner.region().player_count(), 1);
    assert!(runner.region().player(other_player()).is_none());
    let state = runner.region().edge(edge.edge).unwrap();
    let went = Durable::SplitOff {
        region: part,
        players: vec![(other_player(), p, None)],
    };
    assert_eq!(state.outbox.values().collect::<Vec<_>>(), [&went]);

    // A link says R5's hello.
    let (end, worker_end) = link::in_process(4096);
    let mut again = edge.again(end, &runner);
    runner.links().attach(worker_end);
    let named = named_by_the_hello();
    let hello = again.hello(0, &[player(), other_player()], &named);
    again.send(hello).await.unwrap();
    let (told, lines) = logged(|| answers(&mut runner, &mut again, named.len()));

    // The region claims the seven at (23, ..), which nobody holds, and is
    // granted them; and claims the 49 that went, and is told the part's.
    let claimed = claims_sent(&gate);
    for chunk in on_the_parts_side() {
        assert!(claimed.contains(&chunk), "{chunk:?} was not claimed");
    }
    let expected: Vec<(ChunkPos, u64, Told)> = named
        .iter()
        .map(|chunk| match around(P_AT).contains(chunk) {
            true => (*chunk, 0, Told::Elsewhere(part)),
            false => (*chunk, 0, Told::Snapshot),
        })
        .collect();
    assert_eq!(told, expected);
    for chunk in row_at(23) {
        assert_eq!(
            runner.region().knowledge(chunk),
            Knowledge::Held,
            "{chunk:?}"
        );
    }
    // Land of the region that was split, ahead of the player who went.
    assert_eq!(bounds_of(&world.store, home), Some((at(-4, -3), at(23, 3))));
    // It has no `Parted`: nothing was taken for the part's.
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
    // The edge is told of the split all the same.
    assert_eq!(
        presences(&again.aside),
        [(player(), Some(s)), (other_player(), None)]
    );
}

/// This is the hole that section 3.6.5 leaves open and says it does not mend.
/// The test says that it is open, so that whoever closes it finds the test
/// that has to change.
#[tokio::test]
async fn a_region_restored_between_the_split_and_the_hello_claims_the_row_ahead_of_who_went() {
    // R3, its last part.
    for on_disk in [false, true] {
        the_hole_of_a_region_restored_between_the_split_and_the_hello(on_disk).await;
    }
}

// R4.

#[tokio::test]
async fn a_chunk_read_before_it_was_given_back_and_granted_again_goes_to_the_part_unread() {
    // R4. The region gives back at once what nothing uses, so that the chunk
    // can leave it and come back.
    let mut scene = scene_with(open_land(), P_AT, 0, |runner| runner).await;
    let ahead = at(23, 0);
    let (runner, gate, edge) = (&mut scene.runner, &scene.gate, &scene.edge);

    // The chunk is granted and asked of the store, whose answer does not come.
    gate.hold_loads();
    edge.send(asking_for(vec![ahead])).await.unwrap();
    step_until(runner, |runner| runner.loads.contains_key(&ahead));
    wait_for_kept_answers(runner, gate, GateControl::kept_loads, 1);
    // The link lets go, and the region gives the chunk back; then the link
    // asks again, and the store's word that the chunk is the region's does
    // not come either.
    edge.send(done_with(vec![ahead])).await.unwrap();
    step_until(runner, |runner| {
        runner.region().knowledge(ahead) == Knowledge::Unknown
    });
    gate.hold_claims();
    edge.send(asking_for(vec![ahead])).await.unwrap();
    step_until(runner, |runner| {
        runner.region().knowledge(ahead) == Knowledge::Asked
    });
    wait_for_kept_answers(runner, gate, GateControl::kept_claims, 1);

    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    step_to(&mut scene.runner, Stage::Closing);
    scene.gate.release_loads();
    scene.gate.release_claims();
    let made = stepped_to_the_split(&mut scene, &outcome);

    // Granted by the answer that waited, and the part's by where it lies; and
    // not kept from the answer before it, by either of the two.
    assert_eq!(made.part.region.knowledge(ahead), Knowledge::Held);
    assert_eq!(scene.runner.region().knowledge(ahead), Knowledge::Unknown);
    let loaded: Vec<ChunkPos> = made.part.chunks.iter().map(|(chunk, _)| *chunk).collect();
    assert_eq!(loaded, around(P_AT));
    assert!(!loaded.contains(&ahead));
    let warm: Vec<ChunkPos> = scene.runner.warm.keys().copied().collect();
    assert_eq!(warm, around(ORIGIN));
    let line = made.line();
    assert!(line.ends_with(" players=1 chunks=50 waited=1"), "{line}");

    // The part reads it from the store, and nothing else.
    let hello = scene.world.hello(made.region, made.as_epoch);
    let (handle, _) = scene.world.store.open_region(hello).unwrap();
    let (store, part_gate) = gate_before(handle);
    let mut part = RegionRunner::of_part_with(made.part, store).without_rings();
    let (end, worker_end) = link::in_process(4096);
    let mut there = TestEdge::silent(end, scene.edge.edge, scene.edge.start);
    part.links().attach(worker_end);
    let seen = both(&around(P_AT), &[ahead]);
    let hello = there.hello(0, &[other_player()], &seen);
    there.send(hello).await.unwrap();
    let told = answers(&mut part, &mut there, seen.len());
    let expected: Vec<(ChunkPos, u64, Told)> = seen
        .iter()
        .map(|chunk| (*chunk, 0, Told::Snapshot))
        .collect();
    assert_eq!(told, expected);
    let asked = part_gate.asked();
    let loads: Vec<&Asked> = asked
        .iter()
        .filter(|asked| matches!(asked, Asked::Load(_)))
        .collect();
    assert_eq!(loads, [&Asked::Load(ahead)]);
    assert!(!asked.iter().any(|asked| matches!(asked, Asked::Claim(_))));
}

// R5 to R7 and R11. The second way: what an edge asks for before it has heard.

/// R5 as far as the tick that takes the hello: R2's split, and then the edge's
/// new link, which names both players and [`named_by_the_hello`].
struct Unheard {
    scene: Scene,
    /// The part, and the number of the entry that tells the edge of it.
    part: RegionId,
    number: u64,
    /// The new link.
    again: TestEdge,
    /// Everything the link was told by the tick that took its hello, and what
    /// the runner wrote to the log in that step.
    heard: Vec<WorkerToEdge>,
    lines: Vec<String>,
    /// What the runner claimed in that step.
    claimed: BTreeSet<ChunkPos>,
    /// How far the region had applied the edge's messages before that tick.
    applied: u64,
}

async fn unheard() -> Unheard {
    let mut scene = scene(open_land(), P_AT).await;
    let made = split_while_waiting(&mut scene, &row_at(-4)).await;
    let part = made.region;
    let number = scene.number_of_the_split(part);
    let applied = scene.runner.region().edge(scene.edge.edge).unwrap().applied;
    let both_players = [player(), other_player()];
    let mut again = scene
        .hello_again(0, &both_players, &named_by_the_hello())
        .await;
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let heard = again.everything();
    let claimed = claims_sent(&scene.gate);
    Unheard {
        scene,
        part,
        number,
        again,
        heard,
        lines,
        claimed,
        applied,
    }
}

#[tokio::test]
async fn what_an_edge_that_has_not_heard_asks_for_on_the_parts_side_is_taken_for_the_parts() {
    // R5.
    let Unheard {
        mut scene,
        part,
        number,
        mut again,
        heard,
        lines,
        claimed,
        applied,
    } = unheard().await;
    let of_the_part = on_the_parts_side();
    assert_eq!(of_the_part.len(), 56);

    // In the tick that takes the hello the store is sent no claim that names
    // any of the 56,
    for chunk in &of_the_part {
        assert!(!claimed.contains(chunk), "{chunk:?} was claimed");
    }
    // and each of them is answered `Elsewhere` with the part. Nothing else is
    // answered by that tick.
    let expected: Vec<(ChunkPos, u64, Told)> = of_the_part
        .iter()
        .map(|chunk| (*chunk, 0, Told::Elsewhere(part)))
        .collect();
    assert_eq!(answers_among(&heard), expected);
    for chunk in &of_the_part {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Foreign(part), "{chunk:?}");
    }

    // The welcome is followed by the `SplitOff` and by `Absent` for p, and
    // only then by the answers.
    let welcome = Welcome::Resumed {
        entries: 1,
        presences: 2,
        applied,
    };
    assert_eq!(again.welcomed, Some(welcome));
    let went = Durable::SplitOff {
        region: part,
        players: vec![(other_player(), scene.p, None)],
    };
    assert_eq!(entries(&heard), [(number, went)]);
    assert_eq!(
        presences(&heard),
        [(player(), Some(scene.s)), (other_player(), None)]
    );
    let first = |found: fn(&WorkerToEdge) -> bool| {
        let position = heard.iter().position(found);
        position.expect("it was said")
    };
    let last = |found: fn(&WorkerToEdge) -> bool| {
        let position = heard.iter().rposition(found);
        position.expect("it was said")
    };
    let welcomed = first(|message| matches!(message, WorkerToEdge::Welcome(_)));
    let entry: fn(&WorkerToEdge) -> bool = |message| matches!(message, WorkerToEdge::Outbox { .. });
    let presence: fn(&WorkerToEdge) -> bool =
        |message| matches!(message, WorkerToEdge::Presence { .. });
    let answer = first(|message| told(message).is_some());
    assert!(welcomed < first(entry), "the welcome comes first");
    assert!(last(entry) < first(presence), "entries before presence");
    assert!(last(presence) < answer, "presence before the answers");

    // The line, once for the hello.
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=56 free=7"), "{}", taken[0]);

    // The chunks around the home chunk are served by the tick after, from
    // what the runner kept of them at the split.
    step(&mut scene.runner);
    let heard = again.everything();
    let served = sorted(shown(&heard));
    assert_eq!(served, around(ORIGIN));
    let asked = scene.gate.asked();
    assert!(!asked.iter().any(|asked| matches!(asked, Asked::Load(_))));
    assert!(!asked.iter().any(|asked| matches!(asked, Asked::Claim(_))));
    // The seven are nobody's still: the part's by the first claim there is.
    let store = &scene.world.store;
    assert_eq!(bounds_of(store, scene.home), Some((at(-4, -3), at(3, 3))));
    assert_eq!(bounds_of(store, part), Some((at(16, -3), at(22, 3))));
}

#[tokio::test]
async fn when_the_edge_has_made_its_view_a_guests_and_confirmed_the_beliefs_and_the_line_go() {
    // R6.
    let Unheard {
        mut scene,
        number,
        mut again,
        ..
    } = unheard().await;
    let of_the_part = on_the_parts_side();

    // What the edge does when it has read the entry: the view of the player
    // who went is a guest's from here on, and the entry is confirmed.
    again
        .send(asking_as_guest_for(of_the_part.clone()))
        .await
        .unwrap();
    let as_guest = again.asked();
    again.send(EdgeToWorker::Confirm { number }).await.unwrap();
    let mut heard = Vec::new();
    step_until(&mut scene.runner, |_| {
        heard.extend(received(&mut again));
        let not_mine = |told: &(ChunkPos, u64, Told)| told.2 == Told::NotMine;
        answers_among(&heard)
            .iter()
            .filter(|told| not_mine(told))
            .count()
            >= 56
    });

    // 56 times `NotMine`, and nothing else for any of them.
    let about_them: Vec<(ChunkPos, u64, Told)> = answers_among(&heard)
        .into_iter()
        .filter(|told| of_the_part.contains(&told.0))
        .collect();
    let expected: Vec<(ChunkPos, u64, Told)> = of_the_part
        .iter()
        .map(|chunk| (*chunk, as_guest, Told::NotMine))
        .collect();
    assert_eq!(sorted(about_them.iter().map(|told| told.0)), of_the_part);
    assert_eq!(about_them.len(), 56);
    let mut in_order = about_them.clone();
    in_order.sort();
    assert_eq!(in_order, expected);
    // Nothing was claimed for them, and nothing is believed of them any more.
    let claimed = claims_sent(&scene.gate);
    for chunk in &of_the_part {
        assert!(!claimed.contains(chunk), "{chunk:?} was claimed");
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Unknown, "{chunk:?}");
    }
    assert_eq!(scene.outbox(), [], "the entry is confirmed");

    // After that tick a chunk on the part's side that nobody holds is claimed
    // as ever: no edge is left that has not heard.
    let beyond = at(24, 0);
    again.send(asking_for(vec![beyond])).await.unwrap();
    let asking = again.asked();
    let (told, lines) = logged(|| answers_to_askings(&mut scene.runner, &mut again, 1));
    assert_eq!(told, [(beyond, asking, Told::Snapshot)]);
    assert!(claims_sent(&scene.gate).contains(&beyond));
    assert_eq!(scene.runner.region().knowledge(beyond), Knowledge::Held);
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
}

#[tokio::test]
async fn a_chunk_that_was_told_elsewhere_and_is_asked_for_again_is_asked_of_the_store() {
    // R7. After R5, without R6: the edge doubts what it was told.
    let Unheard {
        mut scene,
        part,
        mut again,
        ..
    } = unheard().await;
    let free = at(23, 0);

    again.send(asking_for(vec![free])).await.unwrap();
    let for_the_free = again.asked();
    again.send(asking_for(vec![P_AT])).await.unwrap();
    let for_the_gone = again.asked();
    let (told, lines) = logged(|| answers_to_askings(&mut scene.runner, &mut again, 2));
    // The one nobody holds is granted and served, the one that went is the
    // part's on the store's word.
    assert_eq!(
        told,
        [
            (P_AT, for_the_gone, Told::Elsewhere(part)),
            (free, for_the_free, Told::Snapshot)
        ]
    );
    let claimed = claims_sent(&scene.gate);
    assert!(
        claimed.contains(&free) && claimed.contains(&P_AT),
        "{claimed:?}"
    );
    assert_eq!(scene.runner.region().knowledge(free), Knowledge::Held);
    assert_eq!(
        scene.runner.region().knowledge(P_AT),
        Knowledge::Foreign(part)
    );
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
    // The others are believed the part's as before.
    let known = scene.runner.region().knowledge(at(23, 1));
    assert_eq!(known, Knowledge::Foreign(part));
}

// R8.

#[tokio::test]
async fn a_chunk_of_the_regions_own_area_that_did_not_go_is_claimed_and_served() {
    // R8.
    let mut scene = scene(cut_at_thirty(), P_AT).await;
    let own = at(25, 0);
    assert!(scene.runner.region().pins(own));
    assert_eq!(scene.runner.region().knowledge(own), Knowledge::Unknown);
    let made = split_while_waiting(&mut scene, &[]).await;
    let part = made.region;
    assert_eq!(made.part.region.held_chunk_count(), 49);

    let named = both(&scene.views(), &[own]);
    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &named).await;
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let heard = again.everything();
    let claimed = claims_sent(&scene.gate);

    // The 49 that went are taken for the part's; the chunk of the area is
    // claimed.
    let expected: Vec<(ChunkPos, u64, Told)> = around(P_AT)
        .iter()
        .map(|chunk| (*chunk, 0, Told::Elsewhere(part)))
        .collect();
    assert_eq!(answers_among(&heard), expected);
    assert_eq!(claimed, BTreeSet::from([own]));
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=49 free=0"), "{}", taken[0]);

    // And served, not told elsewhere.
    let told = answers(&mut scene.runner, &mut again, 50);
    let expected: Vec<(ChunkPos, u64, Told)> = both(&around(ORIGIN), &[own])
        .iter()
        .map(|chunk| (*chunk, 0, Told::Snapshot))
        .collect();
    assert_eq!(told, expected);
    assert_eq!(scene.runner.region().knowledge(own), Knowledge::Held);
}

// R9.

#[tokio::test]
async fn after_a_merge_what_lay_on_the_side_of_a_part_is_claimed_as_ever() {
    // R9, its first half: the region absorbs another before any edge has
    // said hello.
    let mut scene = scene(with_a_region_far_out(), P_AT).await;
    assert_eq!(scene.home, HOME);
    let made = split_while_waiting(&mut scene, &[]).await;
    let part = made.region;
    let east = untouched_east(&scene.world);
    let (done, outcome) = outcome();
    scene.runner.reshape(east.absorb(), done);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut scene.runner, &outcome), absorbed);
    drop(east);

    let named = named_by_the_hello();
    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &named).await;
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let claimed = claims_sent(&scene.gate);
    // Every chunk of the hello that the region does not hold is claimed, the
    // row that nobody holds among them, in the tick that takes the hello.
    assert_eq!(claimed, on_the_parts_side().into_iter().collect());
    let (told, more) = logged(|| answers(&mut scene.runner, &mut again, named.len()));
    let expected: Vec<(ChunkPos, u64, Told)> = named
        .iter()
        .map(|chunk| match around(P_AT).contains(chunk) {
            true => (*chunk, 0, Told::Elsewhere(part)),
            false => (*chunk, 0, Told::Snapshot),
        })
        .collect();
    assert_eq!(told, expected);
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
    assert_eq!(lines_of_the_taking(&more), Vec::<&str>::new());
    // The edge is told of both, the split first.
    let told: Vec<Durable> = scene.outbox().into_iter().map(|(_, entry)| entry).collect();
    assert!(matches!(
        told[..],
        [Durable::SplitOff { .. }, Durable::Absorbed { .. }]
    ));
}

#[tokio::test]
async fn a_chunk_on_the_side_of_two_parts_is_told_to_be_the_part_with_the_lower_id() {
    // R9, its second half: two splits made, neither heard of. A third player,
    // q, stands 40 chunks east of the home chunk and is split off second.
    let mut scene = scene(open_land(), P_AT).await;
    let q_at = at(40, 0);
    let (runner, edge) = (&mut scene.runner, &scene.edge);
    edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
    edge.send(asking_for(around(q_at))).await.unwrap();
    step_until(runner, |runner| {
        runner.region().player_count() == 3 && answered(runner, &around(q_at))
    });
    let (q, _) = runner.region().player(third_player()).unwrap();
    let x = f64::from(q_at.x) * 16.0 + 8.5;
    edge.send(of_entity(q, walk(third_player(), x)))
        .await
        .unwrap();
    step_until(runner, |runner| x_of(runner, third_player()) == Some(x));
    step(runner);

    let first = split_while_waiting(&mut scene, &[]).await.region;
    assert_eq!(scene.runner.region().player_count(), 2);
    let second = scene.world.store.regions().unwrap().next;
    let order = Reshape::SplitOff {
        chunks: vec![q_at],
        as_epoch: AS_EPOCH + 1,
        part: second,
    };
    let (done, outcome) = outcome();
    scene.runner.reshape(order, done);
    let reshaped = reshaped(&mut scene.runner, &outcome);
    assert!(
        matches!(reshaped, Reshaped::Split { region, .. } if region == second),
        "{reshaped:?}"
    );
    assert!(first < second);
    assert_eq!(scene.runner.region().player_count(), 1);
    for chunk in [P_AT, q_at] {
        assert_eq!(scene.runner.region().knowledge(chunk), Knowledge::Unknown);
    }

    // (25, 0) is nearer to p than to q and to the home chunk, and nearer to q
    // than to the home chunk: on the side of both. (12, 0) is on the side of
    // the first alone, (33, 0) on that of the second alone. Nobody holds any
    // of the three.
    let of_both = at(25, 0);
    let of_the_first = at(12, 0);
    let of_the_second = at(33, 0);
    let named = [ORIGIN, of_the_first, P_AT, of_both, of_the_second, q_at];
    let all = [player(), other_player(), third_player()];
    let mut again = scene.hello_again(0, &all, &named).await;
    scene.gate.asked();
    step(&mut scene.runner);
    let heard = again.everything();
    assert_eq!(
        answers_among(&heard),
        [
            (of_the_first, 0, Told::Elsewhere(first)),
            (P_AT, 0, Told::Elsewhere(first)),
            (of_both, 0, Told::Elsewhere(first)),
            (of_the_second, 0, Told::Elsewhere(second)),
            (q_at, 0, Told::Elsewhere(second)),
        ]
    );
    assert_eq!(claims_sent(&scene.gate), BTreeSet::new());
    let told: Vec<RegionId> = entries(&heard)
        .into_iter()
        .map(|(_, entry)| match entry {
            Durable::SplitOff { region, .. } => region,
            other => panic!("{other:?} is no split"),
        })
        .collect();
    assert_eq!(told, [first, second]);
}

// R10.

#[tokio::test]
async fn a_third_regions_chunk_is_taken_for_the_parts_until_the_edge_asks_again() {
    // R10. p stands three chunks west of the line, and sees across it.
    let p_at = at(27, 0);
    let mut scene = scene(cut_at_thirty(), p_at).await;
    let theirs = row_at(30);
    for chunk in &theirs {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Foreign(EAST), "{chunk:?}");
    }
    let made = split_while_waiting(&mut scene, &[]).await;
    let part = made.region;
    assert_eq!(made.part.region.held_chunk_count(), 42);
    let line = made.line();
    assert!(line.ends_with(" players=1 chunks=42 waited=0"), "{line}");

    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &scene.views()).await;
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let heard = again.everything();
    let claimed = claims_sent(&scene.gate);

    // The seven are answered `Elsewhere` with the part, with what went, and
    // the store is sent no claim for them.
    let expected: Vec<(ChunkPos, u64, Told)> = around(p_at)
        .iter()
        .map(|chunk| (*chunk, 0, Told::Elsewhere(part)))
        .collect();
    assert_eq!(answers_among(&heard), expected);
    for chunk in &theirs {
        assert!(expected.contains(&(*chunk, 0, Told::Elsewhere(part))));
        assert!(!claimed.contains(chunk), "{chunk:?} was claimed");
    }
    assert_eq!(claimed, BTreeSet::new());
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=49 free=7"), "{}", taken[0]);

    // The edge asks once more, as it does when the part has told it
    // `NotMine`: the region claims the chunk, the store says whose it is, and
    // so does the region.
    let asked_again = at(30, 0);
    again.send(asking_for(vec![asked_again])).await.unwrap();
    let asking = again.asked();
    let told = answers_to_askings(&mut scene.runner, &mut again, 1);
    assert_eq!(told, [(asked_again, asking, Told::Elsewhere(EAST))]);
    assert!(claims_sent(&scene.gate).contains(&asked_again));
    let known = scene.runner.region().knowledge(asked_again);
    assert_eq!(known, Knowledge::Foreign(EAST));
}

// R11. An edge that has heard.

/// R11, its first half. The edge has read the `SplitOff` on the link of R5,
/// and that link is dropped before a tick has taken a confirmation. A new
/// link resumes with the entry's number as `seen` and names, as a viewer's, a
/// chunk that went and one on the part's side that nobody holds.
///
/// `a_tick_between` says whether the runner is stepped between the end of
/// the one link and the hello on the other, so that a tick has given the
/// tickets of the first back before the second asks.
async fn resumed_having_seen_the_entry(a_tick_between: bool) {
    let Unheard {
        mut scene,
        part,
        number,
        again,
        ..
    } = unheard().await;
    drop(again);
    if a_tick_between {
        step(&mut scene.runner);
    }

    let went = at(22, 0);
    let free = at(24, 0);
    let mut resumed = scene.hello_again(number, &[player()], &[went, free]).await;
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    // Both are claimed in the tick that takes the hello.
    let claimed = claims_sent(&scene.gate);
    assert!(
        claimed.contains(&went) && claimed.contains(&free),
        "claimed in the tick that took the hello: {claimed:?}"
    );
    // The first is told the part's by the store, the second is granted and
    // served.
    let (told, more) = logged(|| answers(&mut scene.runner, &mut resumed, 2));
    assert_eq!(
        told,
        [(went, 0, Told::Elsewhere(part)), (free, 0, Told::Snapshot)]
    );
    let welcomed = resumed.welcomed;
    assert!(
        matches!(welcomed, Some(Welcome::Resumed { entries: 0, .. })),
        "{welcomed:?}"
    );
    assert_eq!(scene.runner.region().knowledge(free), Knowledge::Held);
    // No line is written for that hello.
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
    assert_eq!(lines_of_the_taking(&more), Vec::<&str>::new());
}

#[tokio::test]
async fn an_edge_that_resumes_having_seen_the_entry_has_its_chunks_claimed_as_ever() {
    // R11, with a tick between the end of the link that read the entry and
    // the hello of the next.
    resumed_having_seen_the_entry(true).await;
}

/// R11 as it reads, with no step between the end of the one link and the
/// hello of the next: the tick that takes the hello is the tick that gives
/// the first link's tickets back.
///
/// **Finding.** The sequence: R2's split; a link says R5's hello, and the
/// tick that takes it believes the 56 chunks on the part's side the part's
/// and answers the welcome with the `SplitOff`; that link is dropped; a new
/// link of the same edge says hello with the number of the entry as `seen`
/// and names `(22, 0)`, which went, and `(24, 0)`, as a viewer's; the runner
/// is stepped once.
///
/// The record (section 9.5a, R11): "Both are claimed in the tick that takes
/// the hello; the first is told the part's by the store, a tick or two
/// later". Section 3.6.2 says the same of every such edge: "What such an edge
/// names as a viewer's is the view of somebody who stayed, and is claimed as
/// ever", and that a belief ends with "the end of the link, which gives its
/// tickets back".
///
/// What happens: only `(24, 0)` is claimed. `(22, 0)` was named by the hello
/// of the link before as well, the belief that tick put in is still there,
/// and the tick that takes the new hello has the new link's ticket on the
/// chunk when the old link's is taken back, so the belief is wanted
/// throughout and is kept: the chunk is answered `Elsewhere` with the part at
/// once, on the runner's word and not the store's. That is true of a chunk
/// that went. It is the same for a chunk on the part's side that nobody
/// holds, if both hellos name it (`(23, 1)`, tried by hand): an edge that has
/// heard of the split is told that the part holds it, and nothing is claimed,
/// for as long as it looks. That is the second case of "When a belief is
/// wrong" (the chunk arrives a second late, by way of the part's `NotMine`),
/// reached by an edge that had heard. With a tick between the two links the
/// scenario holds:
/// [`an_edge_that_resumes_having_seen_the_entry_has_its_chunks_claimed_as_ever`].
/// The record does not say that R11 needs that tick, and an edge that links
/// again 20 ms after its link ended will often be within the same one.
#[tokio::test]
async fn an_edge_that_resumes_in_the_tick_that_ends_its_link_has_its_chunks_claimed_as_ever() {
    // R11.
    resumed_having_seen_the_entry(false).await;
}

/// R11, its second half, on R5's own link and in place of R6: the link
/// confirms the entry and asks, behind that, for a chunk on the part's side
/// that nobody holds. `apart` says whether a step lies between the two.
async fn confirmed_and_then_asked(apart: bool, behind_the_hello: bool) {
    let Unheard {
        mut scene,
        number,
        mut again,
        ..
    } = unheard().await;
    if !behind_the_hello {
        // The hello's hold is over when the chunks around the home chunk are
        // served.
        let served = answers(&mut scene.runner, &mut again, 49);
        assert!(served.iter().all(|told| told.2 == Told::Snapshot));
    }
    scene.gate.asked();
    let free = at(24, 0);
    again.send(EdgeToWorker::Confirm { number }).await.unwrap();
    if apart {
        step(&mut scene.runner);
    }
    again.send(asking_for(vec![free])).await.unwrap();
    let asking = again.asked();
    let (told, lines) = logged(|| answers_to_askings(&mut scene.runner, &mut again, 1));
    let context = format!("apart: {apart}, behind the hello: {behind_the_hello}");
    assert_eq!(told, [(free, asking, Told::Snapshot)], "{context}");
    assert!(claims_sent(&scene.gate).contains(&free), "{context}");
    let known = scene.runner.region().knowledge(free);
    assert_eq!(known, Knowledge::Held, "{context}");
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new(), "{context}");
}

#[tokio::test]
async fn a_chunk_asked_for_behind_a_confirmation_is_claimed_whether_or_not_one_step_takes_both() {
    // R11.
    for apart in [false, true] {
        confirmed_and_then_asked(apart, false).await;
    }
    // And with both sent while the hello still holds the link, so that one
    // tick takes both for certain.
    confirmed_and_then_asked(false, true).await;
}

// Beyond the list: what sections 3.6.1 and 3.6.2 and N15 say besides.

#[tokio::test]
async fn somebody_who_stands_in_a_chunk_whose_grant_waits_is_split_off_with_it() {
    // N15, and section 3.6.1, item 1: p has walked on into a chunk the region
    // asked for when p came to stand in it. The store has granted it, and no
    // tick has been told.
    let mut scene = scene(open_land(), P_AT).await;
    let ahead = at(23, 0);
    let x = f64::from(ahead.x) * 16.0 + 8.5;
    let claims = scene.gate.kept_claims();
    scene.gate.hold_claims();
    let walked = of_entity(scene.p, walk(other_player(), x));
    scene.edge.send(walked).await.unwrap();
    step_until(&mut scene.runner, |runner| {
        runner.region().knowledge(ahead) == Knowledge::Asked
    });
    let kept = GateControl::kept_claims;
    wait_for_kept_answers(&mut scene.runner, &scene.gate, kept, claims + 1);
    assert_eq!(x_of(&scene.runner, other_player()), Some(x));
    assert_eq!(scene.runner.region().player_count(), 2);

    let order = Reshape::SplitOff {
        chunks: vec![ahead],
        as_epoch: AS_EPOCH,
        part: scene.next,
    };
    let (done, outcome) = outcome();
    scene.runner.reshape(order, done);
    step_to(&mut scene.runner, Stage::Settling);
    scene.gate.release_claims();
    let made = stepped_to_the_split(&mut scene, &outcome);

    // They are a seed and go, and the part holds the chunk they stand in,
    // with what the region held nearer to them than to the home chunk.
    let part = &made.part.region;
    assert_eq!(
        part.player(other_player()).map(|(entity, _)| entity),
        Some(scene.p)
    );
    assert_eq!(part.knowledge(ahead), Knowledge::Held);
    assert_eq!(part.held_chunk_count(), 50);
    assert_eq!(scene.runner.region().player_count(), 1);
    assert_eq!(scene.runner.region().knowledge(ahead), Knowledge::Unknown);
    let line = made.line();
    assert!(line.ends_with(" players=1 chunks=50 waited=1"), "{line}");
    let store = &scene.world.store;
    assert_eq!(bounds_of(store, made.region), Some((at(16, -3), at(23, 3))));
    assert_eq!(bounds_of(store, scene.home), Some((at(-3, -3), at(3, 3))));
}

#[tokio::test]
async fn when_the_split_is_off_the_grants_that_waited_are_taken_by_the_next_tick() {
    // Section 3.6.1, the runner: "The inputs stay where they are: if the split
    // is off or declined, the next tick takes them, as today".
    let mut scene = scene(open_land(), P_AT).await;
    let seven = row_at(23);
    asked_and_not_told(&mut scene.runner, &scene.gate, &scene.edge, &seven).await;
    let asking = scene.edge.asked();
    let nobody_there = Reshape::SplitOff {
        chunks: vec![at(2, 2)],
        as_epoch: AS_EPOCH,
        part: scene.next,
    };
    let (done, outcome) = outcome();
    scene.runner.reshape(nobody_there, done);
    step_to(&mut scene.runner, Stage::Settling);
    scene.gate.release_claims();
    let nobody = Reshaped::Off { why: Off::Nobody };
    assert_eq!(reshaped(&mut scene.runner, &outcome), nobody);
    for chunk in &seven {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Asked, "{chunk:?}");
    }

    scene.runner.step();
    for chunk in &seven {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Held, "{chunk:?}");
    }
    // The link is as it was, and is served them.
    let told = answers(&mut scene.runner, &mut scene.edge, 7);
    let expected: Vec<(ChunkPos, u64, Told)> = seven
        .iter()
        .map(|chunk| (*chunk, asking, Told::Snapshot))
        .collect();
    assert_eq!(told, expected);
    assert_eq!(scene.runner.region().player_count(), 2);
    let store = &scene.world.store;
    assert_eq!(bounds_of(store, scene.home), Some((at(-3, -3), at(23, 3))));
}

#[tokio::test]
async fn a_split_that_is_worked_out_again_under_the_stores_id_is_handed_the_same_grants() {
    // Section 3.6.1, the runner: "if the store has another id for the part,
    // the split is worked out again from the same inputs".
    let mut scene = scene(open_land(), P_AT).await;
    let seven = row_at(23);
    asked_and_not_told(&mut scene.runner, &scene.gate, &scene.edge, &seven).await;
    let order = Reshape::SplitOff {
        chunks: vec![P_AT],
        as_epoch: AS_EPOCH,
        part: RegionId(scene.next.0 + 7),
    };
    let (done, outcome) = outcome();
    scene.runner.reshape(order, done);
    step_to(&mut scene.runner, Stage::Settling);
    scene.gate.release_claims();
    let made = stepped_to_the_split(&mut scene, &outcome);
    check_the_first_way(&scene, &made, &seven);
}

#[tokio::test]
async fn a_viewers_subscription_made_by_a_subscribe_is_taken_for_the_parts_as_a_hellos_is() {
    // Section 3.6.2, "Used": for each chunk of a `Subscribe` the link has no
    // subscription to. The hello names the view of who stayed alone; the
    // edge, which has not read the entry yet, asks for more behind it.
    let mut scene = scene(open_land(), P_AT).await;
    let part = split_while_waiting(&mut scene, &[]).await.region;
    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &around(ORIGIN)).await;
    // One that went, one on the part's side that nobody holds, and one that
    // nobody holds behind the home chunk.
    let free = at(23, 0);
    let behind = at(-5, 0);
    let more = asking_for(vec![P_AT, free, behind]);
    again.send(more).await.unwrap();
    let asking = again.asked();
    scene.gate.asked();
    let (told, lines) = logged(|| answers_to_askings(&mut scene.runner, &mut again, 3));
    assert_eq!(
        told,
        [
            (behind, asking, Told::Snapshot),
            (P_AT, asking, Told::Elsewhere(part)),
            (free, asking, Told::Elsewhere(part)),
        ]
    );
    assert_eq!(claims_sent(&scene.gate), BTreeSet::from([behind]));
    // Once for the message that had any.
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=2 free=1"), "{}", taken[0]);
}

#[tokio::test]
async fn nothing_is_taken_for_the_parts_for_a_guests_subscription() {
    // Section 3.6.2: "Nothing is put in for a guest's subscription". An edge
    // that reached the part first names p's view as a guest's in its hello.
    let mut scene = scene(open_land(), P_AT).await;
    split_while_waiting(&mut scene, &[]).await;
    let (end, worker_end) = link::in_process(4096);
    let mut again = scene.edge.again(end, &scene.runner);
    scene.runner.links().attach(worker_end);
    let of_the_part = on_the_parts_side();
    let hello = EdgeToWorker::Hello {
        edge: again.edge,
        start: again.start,
        since: again.since,
        seen: 0,
        players: vec![player()],
        chunks: around(ORIGIN),
        guests: of_the_part.clone(),
    };
    again.send(hello).await.unwrap();
    scene.gate.asked();
    let (told, lines) = logged(|| answers(&mut scene.runner, &mut again, 49 + 56));
    let expected: Vec<(ChunkPos, u64, Told)> = named_by_the_hello()
        .iter()
        .map(|chunk| match of_the_part.contains(chunk) {
            true => (*chunk, 0, Told::NotMine),
            false => (*chunk, 0, Told::Snapshot),
        })
        .collect();
    assert_eq!(told, expected);
    assert_eq!(claims_sent(&scene.gate), BTreeSet::new());
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
    for chunk in &of_the_part {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Unknown, "{chunk:?}");
    }
}

#[tokio::test]
async fn an_edge_none_of_whose_players_went_has_its_chunks_claimed_as_ever() {
    // Section 3.6.2, "Used": the entry has to be in the outbox of the link's
    // own edge. Another edge's viewer looks at the part's side.
    let mut scene = scene(open_land(), P_AT).await;
    let part = split_while_waiting(&mut scene, &[]).await.region;
    let (mut other, worker_end) = in_process(256);
    scene.runner.links().attach(worker_end);
    let free = at(23, 0);
    other.send(asking_for(vec![P_AT, free])).await.unwrap();
    let asking = other.asked();
    scene.gate.asked();
    let (told, lines) = logged(|| answers_to_askings(&mut scene.runner, &mut other, 2));
    assert_eq!(
        told,
        [
            (P_AT, asking, Told::Elsewhere(part)),
            (free, asking, Told::Snapshot)
        ]
    );
    let claimed = claims_sent(&scene.gate);
    assert!(
        claimed.contains(&P_AT) && claimed.contains(&free),
        "{claimed:?}"
    );
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
}

/// Steps `runner` until `edge` has been told `NotMine` `count` times, and
/// returns those answers in ascending order of their chunks.
fn told_not_mine(
    runner: &mut RegionRunner,
    edge: &mut TestEdge,
    count: usize,
) -> Vec<(ChunkPos, u64, Told)> {
    let mut found = Vec::new();
    step_until(runner, |_| {
        let heard = received(edge);
        let not_mine = |told: &(ChunkPos, u64, Told)| told.2 == Told::NotMine;
        found.extend(answers_among(&heard).into_iter().filter(not_mine));
        found.len() >= count
    });
    found.sort();
    found
}

#[tokio::test]
async fn the_beliefs_last_by_their_tickets_when_the_confirmation_comes_before_the_guests() {
    // Section 3.6.2, "What ends a belief": the end of the viewer's ticket it was
    // put in for, and not the `Parted`. With the `Confirm` before the
    // `SubscribeAsGuest`, the `Parted` goes a tick sooner and the beliefs last
    // by their tickets as before.
    let Unheard {
        mut scene,
        part,
        number,
        mut again,
        ..
    } = unheard().await;
    let of_the_part = on_the_parts_side();
    let edge = again.edge;
    again.send(EdgeToWorker::Confirm { number }).await.unwrap();
    step_until(&mut scene.runner, |runner| {
        let state = runner.region().edge(edge).expect("the edge is known");
        state.outbox.is_empty()
    });
    for _ in 0..3 {
        step(&mut scene.runner);
    }
    // No outbox has the entry any more. The edge looks on, and nothing has
    // changed for what it looks at.
    let claimed = claims_sent(&scene.gate);
    for chunk in &of_the_part {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Foreign(part), "{chunk:?}");
        assert!(!claimed.contains(chunk), "{chunk:?} was claimed");
    }
    let about_them = |told: &(ChunkPos, u64, Told)| of_the_part.contains(&told.0);
    let heard = again.everything();
    assert_eq!(
        answers_among(&heard).into_iter().filter(about_them).count(),
        0
    );

    // Then the view is made a guest's, and the beliefs go with the tickets.
    again
        .send(asking_as_guest_for(of_the_part.clone()))
        .await
        .unwrap();
    let as_guest = again.asked();
    let told = told_not_mine(&mut scene.runner, &mut again, 56);
    let expected: Vec<(ChunkPos, u64, Told)> = of_the_part
        .iter()
        .map(|chunk| (*chunk, as_guest, Told::NotMine))
        .collect();
    assert_eq!(told, expected);
    let claimed = claims_sent(&scene.gate);
    for chunk in &of_the_part {
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Unknown, "{chunk:?}");
        assert!(!claimed.contains(chunk), "{chunk:?} was claimed");
    }
}

#[tokio::test]
async fn a_split_is_kept_for_the_edge_that_has_not_heard_of_it_when_another_has() {
    // Section 3.6.2: a `Parted` is kept while an edge has not heard of the
    // split, and is used for a link whose own edge has the entry and has not
    // said so. Two edges have a player each who goes; one confirms.
    let mut scene = scene(open_land(), P_AT).await;
    let (mut far, worker_end) = in_process(256);
    scene.runner.links().attach(worker_end);
    far.send(join(third_player(), "Dinnerbone")).await.unwrap();
    step_until(&mut scene.runner, |runner| {
        runner.region().player_count() == 3
    });
    let (q, _) = scene.runner.region().player(third_player()).unwrap();
    let x = f64::from(P_AT.x) * 16.0 + 8.5;
    let walked = of_entity(q, walk(third_player(), x));
    far.send(walked).await.unwrap();
    step_until(&mut scene.runner, |runner| {
        x_of(runner, third_player()) == Some(x)
    });
    step(&mut scene.runner);
    far.everything();

    let part = split_while_waiting(&mut scene, &[]).await.region;
    assert_eq!(scene.runner.region().player_count(), 1, "both went");
    let number = scene.number_of_the_split(part);

    // The first edge says hello, reads the entry and confirms it.
    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &around(ORIGIN)).await;
    let edge = again.edge;
    again.send(EdgeToWorker::Confirm { number }).await.unwrap();
    step_until(&mut scene.runner, |runner| {
        let state = runner.region().edge(edge).expect("the edge is known");
        state.outbox.is_empty()
    });
    let theirs = scene.runner.region().edge(far.edge).unwrap();
    assert_eq!(theirs.outbox.len(), 1, "the other edge has not heard");
    // What it asks for from here on is claimed as ever.
    let free = at(24, 0);
    again.send(asking_for(vec![free])).await.unwrap();
    let asking = again.asked();
    scene.gate.asked();
    let told = answers_to_askings(&mut scene.runner, &mut again, 1);
    assert_eq!(told, [(free, asking, Told::Snapshot)]);
    assert!(claims_sent(&scene.gate).contains(&free));

    // The second edge has not heard, and what it asks for on the part's side
    // is taken for the part's still.
    let went = at(19, 1);
    let also_free = at(24, 1);
    let (end, worker_end) = link::in_process(256);
    let mut far_again = far.again(end, &scene.runner);
    scene.runner.links().attach(worker_end);
    let hello = far_again.hello(0, &[third_player()], &[went, also_free]);
    far_again.send(hello).await.unwrap();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let heard = far_again.everything();
    assert_eq!(
        answers_among(&heard),
        [
            (went, 0, Told::Elsewhere(part)),
            (also_free, 0, Told::Elsewhere(part))
        ]
    );
    assert_eq!(claims_sent(&scene.gate), BTreeSet::new());
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=2 free=1"), "{}", taken[0]);
}

#[tokio::test]
async fn nothing_is_taken_for_the_parts_of_a_chunk_the_region_holds_or_has_asked_for() {
    // Section 3.6.2, "Used": the region has to know nothing of the chunk.
    // Another edge's viewer has had it come to hold one chunk on the part's
    // side, and ask for a second.
    let mut scene = scene(open_land(), P_AT).await;
    let part = split_while_waiting(&mut scene, &[]).await.region;
    let (mut other, worker_end) = in_process(256);
    scene.runner.links().attach(worker_end);
    let held = at(23, 0);
    let asked = at(23, 1);
    let free = at(23, 2);
    other.send(asking_for(vec![held])).await.unwrap();
    let told = answers_to_askings(&mut scene.runner, &mut other, 1);
    assert_eq!(told, [(held, other.asked(), Told::Snapshot)]);
    let claims = scene.gate.kept_claims();
    scene.gate.hold_claims();
    other.send(asking_for(vec![asked])).await.unwrap();
    step_until(&mut scene.runner, |runner| {
        runner.region().knowledge(asked) == Knowledge::Asked
    });
    let kept = GateControl::kept_claims;
    wait_for_kept_answers(&mut scene.runner, &scene.gate, kept, claims + 1);

    // The edge whose player went has not heard, and names all three.
    let both_players = [player(), other_player()];
    let named = [held, asked, free];
    let mut again = scene.hello_again(0, &both_players, &named).await;
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let heard = again.everything();
    assert_eq!(
        answers_among(&heard),
        [(held, 0, Told::Snapshot), (free, 0, Told::Elsewhere(part))]
    );
    assert_eq!(scene.runner.region().knowledge(asked), Knowledge::Asked);
    assert_eq!(claims_sent(&scene.gate), BTreeSet::new());
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=1 free=1"), "{}", taken[0]);

    // The store's answer comes, and the chunk that was asked for is served.
    scene.gate.release_claims();
    let told = answers(&mut scene.runner, &mut again, 1);
    assert_eq!(told, [(asked, 0, Told::Snapshot)]);
}

#[tokio::test]
async fn nothing_is_taken_for_the_parts_of_a_chunk_the_region_believes_anothers() {
    // Section 3.6.2, "Used", in the world of R10: another edge's viewer has
    // had the region ask the store about a chunk across the line since the
    // split, so that it believes it the neighbour's again.
    let p_at = at(27, 0);
    let mut scene = scene(cut_at_thirty(), p_at).await;
    let part = split_while_waiting(&mut scene, &[]).await.region;
    let (mut other, worker_end) = in_process(256);
    scene.runner.links().attach(worker_end);
    let known = at(30, 0);
    let forgotten = at(30, 1);
    other.send(asking_for(vec![known])).await.unwrap();
    let told = answers_to_askings(&mut scene.runner, &mut other, 1);
    assert_eq!(told, [(known, other.asked(), Told::Elsewhere(EAST))]);

    let both_players = [player(), other_player()];
    let mut again = scene
        .hello_again(0, &both_players, &[known, forgotten])
        .await;
    scene.gate.asked();
    step(&mut scene.runner);
    let heard = again.everything();
    assert_eq!(
        answers_among(&heard),
        [
            (known, 0, Told::Elsewhere(EAST)),
            (forgotten, 0, Told::Elsewhere(part))
        ]
    );
    assert_eq!(claims_sent(&scene.gate), BTreeSet::new());
}

#[tokio::test]
async fn a_chunk_a_hello_names_as_a_viewers_and_as_a_guests_is_taken_as_a_viewers_is() {
    // ADR-0012, section 4.5: a chunk in both lists of a hello is a viewer's.
    let mut scene = scene(open_land(), P_AT).await;
    let part = split_while_waiting(&mut scene, &[]).await.region;
    let (end, worker_end) = link::in_process(256);
    let mut again = scene.edge.again(end, &scene.runner);
    scene.runner.links().attach(worker_end);
    let in_both = at(23, 0);
    let a_guests = at(23, 1);
    let hello = EdgeToWorker::Hello {
        edge: again.edge,
        start: again.start,
        since: again.since,
        seen: 0,
        players: vec![player(), other_player()],
        chunks: vec![in_both],
        guests: vec![in_both, a_guests],
    };
    again.send(hello).await.unwrap();
    scene.gate.asked();
    let ((), lines) = logged(|| step(&mut scene.runner));
    let heard = again.everything();
    assert_eq!(
        answers_among(&heard),
        [
            (in_both, 0, Told::Elsewhere(part)),
            (a_guests, 0, Told::NotMine)
        ]
    );
    assert_eq!(claims_sent(&scene.gate), BTreeSet::new());
    let taken = lines_of_the_taking(&lines);
    assert_eq!(taken.len(), 1, "{lines:?}");
    assert!(taken[0].ends_with(" chunks=1 free=1"), "{}", taken[0]);
}

#[tokio::test]
async fn an_edge_the_region_has_forgotten_since_the_split_has_its_chunks_claimed_as_ever() {
    // Section 3.6.2, "Dropped": every edge that had a player go has confirmed
    // the entry, or was forgotten, or has started anew. The edge stays away
    // until the region forgets it, and its outbox with it.
    let forgets = |runner: RegionRunner| runner.with_gone_after(5);
    let mut scene = scene_with(open_land(), P_AT, KEPT_FOR, forgets).await;
    let part = split_while_waiting(&mut scene, &[]).await.region;
    let edge = scene.edge.edge;
    step_until(&mut scene.runner, |runner| {
        runner.region().edge(edge).is_none()
    });
    assert_eq!(scene.runner.region().player_count(), 0);

    let free = at(23, 0);
    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &[P_AT, free]).await;
    scene.gate.asked();
    let (told, lines) = logged(|| answers(&mut scene.runner, &mut again, 2));
    assert_eq!(
        told,
        [(P_AT, 0, Told::Elsewhere(part)), (free, 0, Told::Snapshot)]
    );
    let claimed = claims_sent(&scene.gate);
    assert!(
        claimed.contains(&P_AT) && claimed.contains(&free),
        "{claimed:?}"
    );
    assert_eq!(lines_of_the_taking(&lines), Vec::<&str>::new());
}

#[tokio::test]
async fn a_player_who_walks_on_through_their_split_has_no_land_of_the_region_they_left_ahead() {
    // N17 and statement L, with both runners on the store. The row at
    // (23, ..) was granted in the last tick before the stop (the first way);
    // the row at (24, ..) came into view in a tick whose `Subscribe` no tick
    // took, and the edge names it in its hello (the second way).
    let mut scene = scene(open_land(), P_AT).await;
    let made = split_while_waiting(&mut scene, &row_at(23)).await;
    let part = made.region;
    let number = scene.number_of_the_split(part);
    let hello = scene.world.hello(part, made.as_epoch);
    let (handle, _) = scene.world.store.open_region(hello).unwrap();
    let mut there = RegionRunner::of_part(made.part, handle).without_rings();

    // The edge has p under the region they left, links to it again and names
    // all that p sees. None of it is claimed.
    let ahead = both(&both(&around(P_AT), &row_at(23)), &row_at(24));
    let named = both(&around(ORIGIN), &ahead);
    let both_players = [player(), other_player()];
    let mut again = scene.hello_again(0, &both_players, &named).await;
    scene.gate.asked();
    let expected: Vec<(ChunkPos, u64, Told)> = ahead
        .iter()
        .map(|chunk| (*chunk, 0, Told::Elsewhere(part)))
        .collect();
    // The hello reaches the runner when it reaches it: under load a step can
    // come first, so the runner steps until it has answered all it names.
    let mut heard = Vec::new();
    for _ in 0..10_000 {
        step(&mut scene.runner);
        heard.extend(again.everything());
        if answers_among(&heard).len() >= expected.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(answers_among(&heard), expected);

    // It reads of the split and asks the part, which serves what it holds and
    // is granted the row that nobody held, by the first claim there is.
    let (end, worker_end) = link::in_process(4096);
    let mut to_part = TestEdge::silent(end, scene.edge.edge, scene.edge.start);
    there.links().attach(worker_end);
    let hello = to_part.hello(0, &[other_player()], &ahead);
    to_part.send(hello).await.unwrap();
    let told = answers(&mut there, &mut to_part, ahead.len());
    let expected: Vec<(ChunkPos, u64, Told)> = ahead
        .iter()
        .map(|chunk| (*chunk, 0, Told::Snapshot))
        .collect();
    assert_eq!(told, expected);
    for chunk in &ahead {
        assert_eq!(
            there.region().knowledge(*chunk),
            Knowledge::Held,
            "{chunk:?}"
        );
    }
    assert_eq!(presences(&to_part.aside), [(other_player(), Some(scene.p))]);

    // It makes p's view at the region they left a guest's and confirms the
    // entry: nothing wants the beliefs any more.
    again
        .send(asking_as_guest_for(ahead.clone()))
        .await
        .unwrap();
    let as_guest = again.asked();
    again.send(EdgeToWorker::Confirm { number }).await.unwrap();
    let told = told_not_mine(&mut scene.runner, &mut again, ahead.len());
    let expected: Vec<(ChunkPos, u64, Told)> = ahead
        .iter()
        .map(|chunk| (*chunk, as_guest, Told::NotMine))
        .collect();
    assert_eq!(told, expected);

    // p flies on, and is the part's player in the part's land.
    let x = 22.0 * 16.0 + 8.5;
    let walked = of_entity(scene.p, walk(other_player(), x));
    to_part.send(walked).await.unwrap();
    step_until(&mut there, |runner| x_of(runner, other_player()) == Some(x));
    assert_eq!(there.region().player_count(), 1);

    // From the split on, the region they left was granted nothing on the
    // part's side: it has claimed none of it, and the store's list has its
    // land where it was.
    let claimed = claims_sent(&scene.gate);
    for chunk in &ahead {
        assert!(!claimed.contains(chunk), "{chunk:?} was claimed");
        let known = scene.runner.region().knowledge(*chunk);
        assert_eq!(known, Knowledge::Unknown, "{chunk:?}");
    }
    let store = &scene.world.store;
    assert_eq!(bounds_of(store, scene.home), Some((at(-3, -3), at(3, 3))));
    assert_eq!(bounds_of(store, part), Some((at(16, -3), at(24, 3))));
}

// R12. The standstill.

/// A [`Scene`] in `world` whose runner says what it is told of standstills
/// into the channel returned.
async fn scene_that_tells(world: Divided) -> (Scene, Receiver<Standstill>) {
    let (said, told) = mpsc::channel();
    let tell = move |standstill| {
        // Whoever listened may have stopped.
        let _ = said.send(standstill);
    };
    let make = |runner: RegionRunner| runner.with_standstills(Box::new(tell));
    let scene = scene_with(world, P_AT, KEPT_FOR, make).await;
    (scene, told)
}

/// How many players the region has and how many chunks it holds by what its
/// ticks were told, which is what the runner's status says as well.
fn players_and_chunks(runner: &RegionRunner) -> (u64, u64) {
    let players = runner.region().player_count() as u64;
    let held = runner.region().held_chunk_count() as u64;
    assert_eq!(runner.status().held.load(Ordering::Relaxed), held);
    (players, held)
}

#[tokio::test]
async fn a_region_that_was_split_tells_once_how_long_it_stood_still_at_its_next_tick() {
    // R12.
    let (mut scene, told) = scene_that_tells(open_land()).await;
    let before = players_and_chunks(&scene.runner);
    assert_eq!(before, (2, 98));

    // R2's split, stepped to the outcome: nothing is told yet.
    split_while_waiting(&mut scene, &row_at(-4)).await;
    assert_eq!(told.try_iter().count(), 0, "told at the outcome");
    let tick = scene.runner.region().tick_number();

    // One step more, which runs a tick.
    scene.runner.step();
    assert_eq!(scene.runner.region().tick_number(), tick + 1);
    let all: Vec<Standstill> = told.try_iter().collect();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!((all[0].players, all[0].held), (2, 98));

    // No second one, however many ticks follow.
    for _ in 0..10 {
        step(&mut scene.runner);
    }
    assert!(scene.runner.region().tick_number() >= tick + 6);
    assert_eq!(told.try_iter().count(), 0);
}

#[tokio::test]
async fn the_standstill_is_at_least_as_long_as_the_region_did_not_tick() {
    // R12, and section 5.6: from when it stopped ticking until its next tick
    // had run. The one wait for time here: the region is left standing for a
    // while, and what it says cannot be less.
    let stood = Duration::from_millis(40);
    let (mut scene, told) = scene_that_tells(open_land()).await;
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    step_to(&mut scene.runner, Stage::Settling);
    thread::sleep(stood);
    stepped_to_the_split(&mut scene, &outcome);
    scene.runner.step();
    let all: Vec<Standstill> = told.try_iter().collect();
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(
        u128::from(all[0].milliseconds) >= stood.as_millis(),
        "{:?}",
        all[0]
    );
}

#[tokio::test]
async fn a_split_that_comes_to_nothing_after_the_region_stopped_is_a_standstill_as_well() {
    // R12: the chunks named have nobody in them.
    let (mut scene, told) = scene_that_tells(open_land()).await;
    let before = players_and_chunks(&scene.runner);
    let empty = Reshape::SplitOff {
        chunks: vec![at(2, 2), at(40, 40)],
        as_epoch: AS_EPOCH,
        part: scene.next,
    };
    let (done, outcome) = outcome();
    scene.runner.reshape(empty, done);
    step_to(&mut scene.runner, Stage::Settling);
    let tick = scene.runner.region().tick_number();
    let nobody = Reshaped::Off { why: Off::Nobody };
    assert_eq!(reshaped(&mut scene.runner, &outcome), nobody);
    assert_eq!(
        scene.runner.region().tick_number(),
        tick,
        "no tick was used"
    );
    assert_eq!(told.try_iter().count(), 0, "told at the outcome");

    // One, at the first tick after.
    scene.runner.step();
    assert_eq!(scene.runner.region().tick_number(), tick + 1);
    let all: Vec<Standstill> = told.try_iter().collect();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!((all[0].players, all[0].held), before);
    for _ in 0..5 {
        step(&mut scene.runner);
    }
    assert_eq!(told.try_iter().count(), 0);
}

#[tokio::test]
async fn a_region_that_absorbed_another_tells_its_standstill_with_what_it_was_before() {
    // R12: an absorption.
    let (mut scene, told) = scene_that_tells(with_a_region_far_out()).await;
    let before = players_and_chunks(&scene.runner);
    assert_eq!(before, (2, 98));
    let east = untouched_east(&scene.world);
    let (done, outcome) = outcome();
    scene.runner.reshape(east.absorb(), done);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut scene.runner, &outcome), absorbed);
    drop(east);
    assert_eq!(told.try_iter().count(), 0, "told at the outcome");

    scene.runner.step();
    let all: Vec<Standstill> = told.try_iter().collect();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!((all[0].players, all[0].held), before);
    for _ in 0..5 {
        step(&mut scene.runner);
    }
    assert_eq!(told.try_iter().count(), 0);
}

#[tokio::test]
async fn a_merge_that_the_store_declines_is_a_standstill_as_well() {
    // Section 5.6: whether the merge or the split was made or came to
    // nothing. The store declines a merge whose absorbed region has no owner
    // with the epoch named.
    let (mut scene, told) = scene_that_tells(with_a_region_far_out()).await;
    let before = players_and_chunks(&scene.runner);
    let east = untouched_east(&scene.world);
    let order = Reshape::Absorb {
        absorbed: EAST,
        absorbed_epoch: 3,
        state: east.state.clone(),
    };
    let (done, outcome) = outcome();
    scene.runner.reshape(order, done);
    let reshaped = reshaped(&mut scene.runner, &outcome);
    let declined = matches!(
        reshaped,
        Reshaped::Off {
            why: Off::Declined(_)
        }
    );
    assert!(declined, "{reshaped:?}");
    drop(east);
    assert_eq!(told.try_iter().count(), 0, "told at the outcome");

    scene.runner.step();
    let all: Vec<Standstill> = told.try_iter().collect();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!((all[0].players, all[0].held), before);
    for _ in 0..5 {
        step(&mut scene.runner);
    }
    assert_eq!(told.try_iter().count(), 0);
}

#[tokio::test]
async fn a_region_that_is_released_tells_no_standstill() {
    // R12: after a release the region never ticks on.
    let (mut scene, told) = scene_that_tells(open_land()).await;
    scene.runner.begin_release();
    assert_eq!(released(&mut scene.runner), Ended::Released);
    scene.runner.step();
    assert_eq!(told.try_iter().count(), 0);
}

#[tokio::test]
async fn a_runner_that_loses_the_store_in_the_middle_of_a_split_tells_no_standstill() {
    // R12.
    let (mut scene, told) = scene_that_tells(open_land()).await;
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    step_to(&mut scene.runner, Stage::Settling);
    scene.gate.lose();
    let lost = Reshaped::Off {
        why: Off::StoreLost,
    };
    assert_eq!(reshaped(&mut scene.runner, &outcome), lost);
    for _ in 0..3 {
        scene.runner.step();
    }
    assert_eq!(scene.runner.ended(), Some(Ended::StoreLost));
    assert_eq!(told.try_iter().count(), 0);
}

#[tokio::test]
async fn a_runner_that_is_stopped_in_the_middle_of_a_split_tells_no_standstill() {
    // R12.
    let (mut scene, told) = scene_that_tells(open_land()).await;
    let (done, outcome) = outcome();
    scene.runner.reshape(scene.order(), done);
    step_to(&mut scene.runner, Stage::Settling);
    assert_eq!(scene.runner.run(&AtomicBool::new(true)), Ended::Abandoned);
    let lost = Reshaped::Off {
        why: Off::StoreLost,
    };
    assert_eq!(outcome.try_recv(), Ok(lost));
    scene.runner.step();
    assert_eq!(told.try_iter().count(), 0);
}
