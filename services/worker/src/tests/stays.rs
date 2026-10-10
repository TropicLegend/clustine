//! Tests of the runner's part of "one stay per player": step R1.3 of
//! `docs/adr/0020-one-stay-per-player.md`. The setting `place_by_store` is on in every
//! region here, and the world store is the real one: in memory, and on disk where a
//! test says so, with the store stopped and started again where a test kills it.
//!
//! A test plays the edges. It steps the runners itself and reads what comes back, so
//! every wait is a loop that steps until a message or a state is there. A gate before
//! a runner's store holds back what the store says of stays, or the answers to
//! commits, where a test has to stand between two steps of a login.
//!
//! Every link's outbox entries are held to ascending numbers as they are read: an edge
//! remembers only the highest number it has seen, so an `Ended` behind a `Departed` of
//! the same tick would be passed over (section 6 of the record).

use clustine_sim::EnteringState;
use clustine_sim::api::{ItemStack, Place, StayNote};

use super::*;

/// The home region of two stripes, and the edges the tests play.
const WEST: RegionId = RegionId(0);
const E: EdgeId = EdgeId(11);
const F: EdgeId = EdgeId(22);

/// A chunk of the gap that nobody holds until somebody claims it, and where a player
/// stands in it on the line the tests walk along.
const OUT: ChunkPos = ChunkPos::new(3, 0);
const OUT_X: f64 = 56.5;

/// Where a player stands in the eastern stripe, and in the western one away from where
/// players enter the world.
const EAST_X: f64 = 40.5;
const HOME_X: f64 = 9.5;

/// How long a region of these tests keeps a chunk nothing uses: as the processes set it.
const KEPT: u64 = DEFAULT_RETURN_AFTER;

/// What a region is made with where the store keeps the players' places.
fn keeping() -> RegionConfig {
    RegionConfig {
        place_by_store: true,
        ..config(KEPT)
    }
}

/// How a test's world is divided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// [`Divided::stripes`].
    Stripes,
    /// [`Divided::gap`].
    Gap,
}

/// A world and where it is kept, if it is kept on disk.
struct Land {
    world: Divided,
    shape: Shape,
    directory: Option<tempfile::TempDir>,
}

impl Land {
    fn new(shape: Shape, on_disk: bool) -> Self {
        let directory = on_disk.then(|| tempfile::tempdir().unwrap());
        let world = Self::started(shape, directory.as_ref());
        Self {
            world,
            shape,
            directory,
        }
    }

    fn started(shape: Shape, directory: Option<&tempfile::TempDir>) -> Divided {
        let division = match shape {
            Shape::Stripes => Divided::line_at_one(),
            Shape::Gap => Division {
                home: ORIGIN,
                pinned: vec![
                    ChunkArea {
                        min_x: None,
                        max_x: Some(0),
                    },
                    ChunkArea {
                        min_x: Some(16),
                        max_x: None,
                    },
                ],
            },
        };
        let generator = Arc::new(FlatGenerator::classic());
        let store = match directory {
            Some(directory) => Store::local_divided(directory.path(), generator, division),
            None => Store::memory_divided(generator, division),
        };
        Divided {
            store: store.unwrap(),
        }
    }

    /// The store dies and is started again from what it wrote, as the crate's other
    /// kills have it: it has done what it was asked before. A store in memory has
    /// nothing to start from, and is the store it was; only its regions' owners die.
    fn restarted(self) -> Self {
        let Self {
            world,
            shape,
            directory,
        } = self;
        let world = match &directory {
            Some(kept) => {
                world.store.flush().unwrap();
                drop(world);
                Self::started(shape, Some(kept))
            }
            None => world,
        };
        Self {
            world,
            shape,
            directory,
        }
    }

    /// The region players enter the world in.
    fn home(&self) -> RegionId {
        match self.shape {
            Shape::Stripes => WEST,
            Shape::Gap => HOME,
        }
    }

    /// A runner for `region` as the store has it, opened with `epoch` and told which
    /// region it runs, with a gate before its store.
    fn runner(&self, region: RegionId, epoch: u64) -> (RegionRunner, Arc<GateControl>) {
        let hello = self.world.hello(region, epoch);
        let (runner, gate) = gated_as(&self.world.store, hello, keeping());
        (runner.with_id(region), gate)
    }
}

/// Runs a test on a store in memory and on one on disk.
fn in_memory_and_on_disk(test: fn(bool)) {
    test(false);
    test(true);
}

/// An edge's link to a region, with everything read from it so far.
struct Seat {
    edge: TestEdge,
    log: Vec<WorkerToEdge>,
    /// The number of the last outbox entry read on any link of the edge to the region.
    seen: u64,
}

impl Seat {
    /// The first link of the edge `id` to the region of `runner`, which has said hello
    /// and asks for `chunks` as a viewer.
    fn first(runner: &RegionRunner, id: EdgeId, chunks: &[ChunkPos]) -> Self {
        let (end, worker_end) = link::in_process(1024);
        let edge = TestEdge::silent(end, id, 1);
        runner.links().attach(worker_end);
        edge.try_send(edge.hello(0, &[], chunks)).unwrap();
        Self {
            edge,
            log: Vec::new(),
            seen: 0,
        }
    }

    /// The next link of the same edge to the region, which `runner` runs now: it says
    /// what it has seen of the outbox and names `players` as those it has there.
    fn again(&self, runner: &RegionRunner, players: &[PlayerId]) -> Self {
        let (end, worker_end) = link::in_process(1024);
        let edge = self.edge.again(end, runner);
        runner.links().attach(worker_end);
        edge.try_send(edge.hello(self.seen, players, &[])).unwrap();
        Self {
            edge,
            log: Vec::new(),
            seen: self.seen,
        }
    }

    /// Sends `body` with the edge's next number for the region, and returns the number.
    fn send(&self, body: EdgeToWorker) -> u64 {
        self.edge.try_send(body).unwrap();
        self.edge.sent.load(Ordering::Relaxed)
    }

    /// Sends `body` again under the number it had.
    fn send_again(&self, number: u64, body: EdgeToWorker) {
        let number = Some(number);
        self.edge
            .end
            .try_send(EdgeMessage { number, body })
            .unwrap();
    }

    /// Reads whatever has arrived.
    fn read(&mut self) {
        for message in self.edge.everything() {
            if let WorkerToEdge::Outbox { number, .. } = &message {
                assert!(
                    *number > self.seen,
                    "entry {number} after entry {}: {:?} after {:#?}",
                    self.seen,
                    message,
                    self.log
                );
                self.seen = *number;
            }
            self.log.push(message);
        }
    }

    /// How far the region has said the edge's messages are applied.
    fn applied(&self) -> u64 {
        let applied = self.log.iter().filter_map(|message| match message {
            WorkerToEdge::Progress { applied, .. } => Some(*applied),
            _ => None,
        });
        applied.max().unwrap_or(0)
    }

    /// What `player` was told when they entered the world, each time they were.
    fn spawned(&self, player: PlayerId) -> Vec<PlayerEvent> {
        let spawned = |message: &WorkerToEdge| match message {
            WorkerToEdge::ToPlayer {
                player: whom,
                event: event @ PlayerEvent::Spawned { .. },
            } if *whom == player => Some(event.clone()),
            _ => None,
        };
        self.log.iter().filter_map(spawned).collect()
    }

    /// The outbox entries read on this link, with their numbers.
    fn entries(&self) -> Vec<(u64, Durable)> {
        entries(&self.log)
    }

    /// The presence answers read on this link, in the order they came.
    fn answers(&self) -> Vec<(PlayerId, Presence)> {
        let answer = |message: &WorkerToEdge| match message {
            WorkerToEdge::Presence { player, answer } => Some((*player, answer.clone())),
            _ => None,
        };
        self.log.iter().filter_map(answer).collect()
    }

    /// Where in the log `found` is first true.
    fn at(&self, found: impl Fn(&WorkerToEdge) -> bool) -> Option<usize> {
        self.log.iter().position(found)
    }
}

/// Steps every runner and reads every link, until `done`.
fn run(
    runners: &mut [&mut RegionRunner],
    seats: &mut [&mut Seat],
    what: &str,
    mut done: impl FnMut(&[&mut RegionRunner], &[&mut Seat]) -> bool,
) {
    for _ in 0..20_000 {
        for runner in runners.iter_mut() {
            runner.step();
        }
        for seat in seats.iter_mut() {
            seat.read();
        }
        if done(runners, seats) {
            return;
        }
        // The store answers on threads of its own.
        thread::sleep(Duration::from_millis(1));
    }
    let logs: Vec<_> = seats.iter().map(|seat| &seat.log).collect();
    panic!("never happened: {what}\n{logs:#?}");
}

/// Waits until `seats[through]` has been told that its messages up to `number` are
/// applied: the tick that applied them is confirmed and published then, and every
/// tick before it.
fn applied(
    runners: &mut [&mut RegionRunner],
    seats: &mut [&mut Seat],
    through: usize,
    number: u64,
) {
    run(runners, seats, "progress up to a number", |_, seats| {
        seats[through].applied() >= number
    });
}

/// A join of `player` on the connection `attempt`.
fn join_as(player: PlayerId, attempt: u64) -> EdgeToWorker {
    EdgeToWorker::PlayerJoin(PlayerJoin {
        player,
        name: format!("player-{}", player.0.as_u128()),
        attempt,
    })
}

/// A leave of the stay `entity`, as an edge sends it that was told the entity.
fn leave_of(player: PlayerId, entity: EntityId) -> EdgeToWorker {
    EdgeToWorker::PlayerLeave {
        player,
        entity: Some(entity),
        attempt: None,
    }
}

/// The input with the number `number` of the stay `entity`.
fn does(player: PlayerId, entity: EntityId, number: u64, input: PlayerInput) -> EdgeToWorker {
    EdgeToWorker::Input {
        player,
        entity,
        number,
        input,
    }
}

/// A step to `x` on the line the tests walk along.
fn to(x: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, -60.0, 0.5)),
        rotation: None,
        on_ground: true,
    }
}

/// A numbered message that changes nothing but how far the edge's messages count as
/// applied: an input of a stay no region has.
fn nothing() -> EdgeToWorker {
    let nobody = PlayerId(Uuid::from_u128(0xdead));
    does(nobody, EntityId(0), 1, to(0.0))
}

/// How somebody stands who has walked to `x`.
fn standing(x: f64) -> Pose {
    Pose {
        on_ground: true,
        ..Pose::at(Vec3::new(x, -60.0, 0.5))
    }
}

/// The place of somebody who has walked to `x` and done nothing else.
fn place_at(x: f64) -> Place {
    Place {
        pose: standing(x),
        flying: false,
        hotbar: [None; HOTBAR_SLOTS],
        selected_slot: 0,
    }
}

/// The place of somebody who has just entered the world for the first time.
fn at_the_spawn_point() -> Place {
    Place {
        pose: Pose::at(SPAWN),
        ..place_at(0.0)
    }
}

fn is_enter(reply: &StoreReply) -> bool {
    matches!(reply, StoreReply::Enter { .. })
}

/// How many stays the store has called dead in what the gate holds back. The store
/// says so behind the answers of the commits that made them so, in one message or in
/// several, so a test waits for the stays and not for a message.
fn dead_kept(gate: &GateControl) -> usize {
    let kept = gate.kept.lock().unwrap();
    let stays = kept.iter().map(|reply| match reply {
        StoreReply::Dead { stays } => stays.len(),
        _ => 0,
    });
    stays.sum()
}

/// What `player` was told when they entered the world on the connection `attempt`:
/// their entity and the place they were put in.
fn told(seat: &Seat, player: PlayerId, attempt: u64) -> Option<(EntityId, Place)> {
    seat.spawned(player)
        .into_iter()
        .find_map(|event| match event {
            PlayerEvent::Spawned {
                attempt: of,
                entity_id,
                pose,
                flying,
                hotbar,
                selected_slot,
            } if of == attempt => {
                let place = Place {
                    pose,
                    flying,
                    hotbar,
                    selected_slot,
                };
                Some((entity_id, place))
            }
            _ => None,
        })
}

/// Has `player` join through `seats[through]` on the connection `attempt`, and waits
/// until they are told that they entered the world; returns their entity and where
/// they were put.
fn enters(
    runners: &mut [&mut RegionRunner],
    seats: &mut [&mut Seat],
    through: usize,
    player: PlayerId,
    attempt: u64,
) -> (EntityId, Place) {
    seats[through].send(join_as(player, attempt));
    run(runners, seats, "a player entering the world", |_, seats| {
        told(seats[through], player, attempt).is_some()
    });
    told(seats[through], player, attempt).unwrap()
}

/// Has the stay `entity` of `player` walk to `x` as its input `number` and waits until
/// the edge has been told that it is applied.
fn walks(
    runners: &mut [&mut RegionRunner],
    seats: &mut [&mut Seat],
    through: usize,
    (player, entity, number): (PlayerId, EntityId, u64),
    x: f64,
) {
    let sent = seats[through].send(does(player, entity, number, to(x)));
    applied(runners, seats, through, sent);
}

/// The first `Departed` of `player` among the entries read on `seat`.
fn departure(seat: &Seat, player: PlayerId) -> Option<(u64, PlayerTransfer, RegionId)> {
    seat.entries()
        .into_iter()
        .find_map(|(number, entry)| match entry {
            Durable::Departed {
                player: who,
                transfer,
                to,
            } if who == player => Some((number, transfer, to)),
            _ => None,
        })
}

/// The `Ended` entries among `entries`, each with the player, the entity and the
/// attempt it names.
fn ended(entries: &[(u64, Durable)]) -> Vec<(PlayerId, EntityId, Option<u64>)> {
    let ended = |(_, entry): &(u64, Durable)| match entry {
        Durable::Ended {
            player,
            entity,
            attempt,
        } => Some((*player, *entity, *attempt)),
        _ => None,
    };
    entries.iter().filter_map(ended).collect()
}

/// The notes of the last commit the gate saw that had any.
fn last_notes(gate: &GateControl) -> Vec<StayNote> {
    gate.noted().last().map(|(_, notes)| notes.clone()).unwrap()
}

// ---------------------------------------------------------------------------------------
// A login, step by step (section 4 of the record)
// ---------------------------------------------------------------------------------------

#[test]
fn a_join_is_held_as_entering_and_named_to_the_store_and_the_player_enters_on_its_answer() {
    in_memory_and_on_disk(
        a_join_is_held_as_entering_and_named_to_the_store_and_the_player_enters_on_its_answer_in,
    );
}

/// Steps 3 to 9, with nothing kept of the player: the first row of step 7.
fn a_join_is_held_as_entering_and_named_to_the_store_and_the_player_enters_on_its_answer_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[ORIGIN]);
    gate.hold_stays();
    let joined = e.send(join_as(player(), 71));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the join being applied and the store answering its note",
        |_, seats| seats[0].applied() >= joined && gate.kept_of(is_enter) == 1,
    );

    // The region has given the stay its entity and holds it; the commit of that tick
    // named it; nobody has been told anything.
    let entity = EntityId(1);
    let entering = EnteringState {
        entity_id: entity,
        name: "player-1".to_owned(),
        edge: E,
        attempt: 71,
    };
    assert_eq!(west.region().entering_state(player()), Some(&entering));
    assert_eq!(west.region().player(player()), None);
    let named = StayNote::Entering {
        player: player(),
        entity,
    };
    let noted = gate.noted();
    assert_eq!(noted.len(), 1, "{noted:?}");
    assert_eq!(noted[0].1, [named]);
    let joined_in = noted[0].0;

    // The region waits for nothing but the answer: it ticks on, and what the edge
    // sends meanwhile is applied.
    let later = e.send(nothing());
    applied(&mut [&mut west], &mut [&mut e], 0, later);
    assert!(west.region().tick_number() > joined_in);
    assert_eq!(e.spawned(player()), []);
    assert_eq!(e.entries(), []);
    let said_of_an_entity =
        |message: &WorkerToEdge| matches!(message, WorkerToEdge::TickDelta { .. });
    assert_eq!(e.at(said_of_an_entity), None, "{:#?}", e.log);
    assert_eq!(west.region().entering_state(player()), Some(&entering));

    gate.release_stays();
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the player entering the world",
        |_, seats| !seats[0].spawned(player()).is_empty(),
    );
    assert_eq!(told(&e, player(), 71), Some((entity, at_the_spawn_point())));
    assert_eq!(e.spawned(player()).len(), 1);
    assert_eq!(west.region().entering_state(player()), None);
    let state = west.region().player_state(player()).unwrap();
    assert_eq!(
        (state.entity_id, state.hops, state.attempt, state.edge),
        (entity, 0, Some(71), E)
    );
    // The tick that placed them said so to the store, in a later commit than the join's.
    let noted = gate.noted();
    assert_eq!(noted.len(), 2, "{noted:?}");
    assert!(noted[1].0 > joined_in);
    let has = StayNote::Has {
        player: player(),
        entity,
        hops: 0,
        place: at_the_spawn_point(),
    };
    assert_eq!(noted[1].1, [has]);
    // Those who watch the chunk are shown the entity only now.
    let shown = e.log.iter().any(|message| {
        matches!(message, WorkerToEdge::TickDelta { events, .. }
            if events.iter().any(|event| matches!(event, RegionEvent::EntitySpawned(state) if state.entity == entity)))
    });
    assert!(shown, "{:#?}", e.log);
}

#[test]
fn a_player_who_comes_back_is_placed_where_they_were_in_the_home_region_as_they_were() {
    in_memory_and_on_disk(
        a_player_who_comes_back_is_placed_where_they_were_in_the_home_region_as_they_were_in,
    );
}

/// The second row of step 7, where the holder the store names is the home region
/// itself: the runner says "nobody" to the region in place of its own id.
fn a_player_who_comes_back_is_placed_where_they_were_in_the_home_region_as_they_were_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 71);

    let stack = ItemStack {
        item: clustine_data::items::DIRT,
        count: 7,
    };
    let turned = PlayerInput::Move {
        position: Some(Vec3::new(HOME_X, -50.0, 0.5)),
        rotation: Some((135.0, -20.0)),
        on_ground: false,
    };
    let does_all = [
        turned,
        PlayerInput::SetFlying { flying: true },
        PlayerInput::SetHotbarSlot {
            slot: 4,
            stack: Some(stack),
        },
        PlayerInput::SelectSlot { slot: 4 },
    ];
    let mut done = 0;
    for (number, input) in (1..).zip(does_all) {
        done = e.send(does(player(), first, number, input));
    }
    // A tick applies who came and went before what anybody did, so the leave is sent
    // when all of it is applied.
    applied(&mut [&mut west], &mut [&mut e], 0, done);
    let left = e.send(leave_of(player(), first));
    applied(&mut [&mut west], &mut [&mut e], 0, left);
    assert_eq!(west.region().player(player()), None);
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[4] = Some(stack);
    let place = Place {
        pose: Pose {
            position: Vec3::new(HOME_X, -50.0, 0.5),
            yaw: 135.0,
            pitch: -20.0,
            on_ground: false,
        },
        flying: true,
        hotbar,
        selected_slot: 4,
    };

    let (second, entered) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 72);
    assert_eq!(second, EntityId(2));
    assert_eq!(entered, place);
    // Placed, and not let go: the chunk is the region's own.
    assert_eq!(e.entries(), []);
    let state = west.region().player_state(player()).unwrap();
    assert_eq!(
        (state.entity_id, state.pose, state.flying, state.hops),
        (second, place.pose, true, 0)
    );
    assert_eq!(state.attempt, Some(72));
    let has = StayNote::Has {
        player: player(),
        entity: second,
        hops: 0,
        place,
    };
    assert_eq!(last_notes(&gate), [has]);
}

/// What [`RegionRunner::with_id`] is for: the store names the home region as the
/// holder of a place in the home region's own land, and a runner that was not told
/// which region it runs passes that on.
#[test]
fn a_runner_that_was_not_told_which_region_it_runs_lets_a_stay_go_to_its_own_region() {
    let land = Land::new(Shape::Stripes, false);
    let hello = land.world.hello(WEST, 1);
    let (mut west, _gate) = gated_as(&land.world.store, hello, keeping());
    let mut e = Seat::first(&west, E, &[]);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 71);
    walks(
        &mut [&mut west],
        &mut [&mut e],
        0,
        (player(), first, 1),
        HOME_X,
    );
    let left = e.send(leave_of(player(), first));
    applied(&mut [&mut west], &mut [&mut e], 0, left);

    e.send(join_as(player(), 72));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the stay being let go",
        |_, seats| departure(seats[0], player()).is_some(),
    );
    let (_, transfer, to) = departure(&e, player()).unwrap();
    assert_eq!(to, WEST);
    assert_eq!((transfer.entity_id, transfer.hops), (EntityId(2), 1));
    assert_eq!(told(&e, player(), 72), None);
}

#[test]
fn a_player_whose_place_another_region_holds_is_let_go_to_it_without_being_placed() {
    in_memory_and_on_disk(
        a_player_whose_place_another_region_holds_is_let_go_to_it_without_being_placed_in,
    );
}

/// The third row of step 7, through to the region that holds the place.
fn a_player_whose_place_another_region_holds_is_let_go_to_it_without_being_placed_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let (mut east, east_gate) = land.runner(EAST, 2);
    let mut e = Seat::first(&west, E, &[]);
    let mut there = Seat::first(&east, E, &[]);
    let (first, _) = enters(&mut [&mut west, &mut east], &mut [&mut e], 0, player(), 71);

    // The player walks over and a step further, and leaves there.
    e.send(does(player(), first, 1, to(EAST_X)));
    run(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut there],
        "the player being let go",
        |_, seats| departure(seats[0], player()).is_some(),
    );
    let (_, walked_over, to_region) = departure(&e, player()).unwrap();
    assert_eq!((to_region, walked_over.hops), (EAST, 1));
    there.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer: walked_over,
    });
    let stepped = there.send(does(player(), first, 2, to(EAST_X + 1.0)));
    applied(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut there],
        1,
        stepped,
    );
    let left = there.send(leave_of(player(), first));
    applied(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut there],
        1,
        left,
    );
    assert_eq!(east.region().player(player()), None);

    let before = e.entries().len();
    e.send(join_as(player(), 72));
    run(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut there],
        "the stay being let go to the region that holds its place",
        |_, seats| seats[0].entries().len() > before,
    );
    let (number, entry) = e.entries().pop().unwrap();
    let second = EntityId(2);
    let transfer = PlayerTransfer {
        entity_id: second,
        name: "player-1".to_owned(),
        pose: standing(EAST_X + 1.0),
        hotbar: [None; HOTBAR_SLOTS],
        selected_slot: 0,
        last_input: 0,
        hops: 1,
        flying: false,
        attempt: Some(72),
    };
    let let_go = Durable::Departed {
        player: player(),
        transfer: transfer.clone(),
        to: EAST,
    };
    assert_eq!((number, &entry), (2, &let_go));
    // Never placed: nobody was told that they entered, no entity was shown, and the
    // region has neither the player nor the stay.
    assert_eq!(told(&e, player(), 72), None);
    assert_eq!(west.region().player(player()), None);
    assert_eq!(west.region().entering_state(player()), None);
    let has = StayNote::Has {
        player: player(),
        entity: second,
        hops: 1,
        place: place_at(EAST_X + 1.0),
    };
    let has = [has];
    assert_eq!(last_notes(&gate), has);

    // The edge passes the stay on, and the region that holds the place takes it in and
    // names it to the store, which finds nothing wrong with it: the stay stays.
    let arrived = there.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer,
    });
    applied(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut there],
        1,
        arrived,
    );
    assert_eq!(last_notes(&east_gate), has);
    let after = there.send(nothing());
    applied(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut there],
        1,
        after,
    );
    let state = east.region().player_state(player()).unwrap();
    assert_eq!(
        (state.entity_id, state.pose, state.hops, state.attempt),
        (second, standing(EAST_X + 1.0), 1, Some(72))
    );
    assert_eq!(ended(&there.entries()), []);
}

#[test]
fn a_player_who_left_below_the_world_enters_at_the_spawn_point_with_what_they_held() {
    in_memory_and_on_disk(
        a_player_who_left_below_the_world_enters_at_the_spawn_point_with_what_they_held_in,
    );
}

/// The first row of step 7 for a place that counts as none, whoever holds it: here
/// the eastern region does.
fn a_player_who_left_below_the_world_enters_at_the_spawn_point_with_what_they_held_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, _gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 71);
    let stack = ItemStack {
        item: clustine_data::items::STONE,
        count: 3,
    };
    let held = PlayerInput::SetHotbarSlot {
        slot: 2,
        stack: Some(stack),
    };
    e.send(does(player(), first, 1, held));
    e.send(does(
        player(),
        first,
        2,
        PlayerInput::SelectSlot { slot: 2 },
    ));
    e.send(does(
        player(),
        first,
        3,
        PlayerInput::SetFlying { flying: true },
    ));
    // Through the floor, east of the line: the region lets them go, and the place it
    // tells the store is below the world's lowest block.
    let fell = PlayerInput::Move {
        position: Some(Vec3::new(EAST_X, -70.0, 0.5)),
        rotation: Some((90.0, 45.0)),
        on_ground: false,
    };
    e.send(does(player(), first, 4, fell));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the player being let go",
        |_, seats| departure(seats[0], player()).is_some(),
    );

    let (second, entered) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 72);
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[2] = Some(stack);
    let place = Place {
        pose: Pose::at(SPAWN),
        flying: false,
        hotbar,
        selected_slot: 2,
    };
    assert_eq!((second, entered), (EntityId(2), place));
    assert_eq!(e.entries().len(), 1, "only the first stay was let go");
    assert!(west.region().player(player()).is_some());
}

#[test]
fn a_player_placed_where_the_region_believes_the_chunk_anothers_is_told_they_entered_and_let_go() {
    in_memory_and_on_disk(
        a_player_placed_where_the_region_believes_the_chunk_anothers_is_told_they_entered_and_let_go_in,
    );
}

/// The second row of step 7 where the store's advice is behind the region's belief:
/// the store says nobody holds the place, the region still believes the chunk
/// another's, places the player and lets them go in the same tick. The player is in
/// the region no longer when the runner looks for their edge.
fn a_player_placed_where_the_region_believes_the_chunk_anothers_is_told_they_entered_and_let_go_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Gap, on_disk);
    let (mut home, gate) = land.runner(land.home(), 1);
    // The eastern region, played by the test, has the chunk for now.
    let (other, _) = land.world.open(EAST, 2);
    assert_eq!(claim(&other, &[OUT]).0, [OUT]);
    let mut e = Seat::first(&home, E, &[ORIGIN, OUT]);
    let elsewhere = |message: &WorkerToEdge| matches!(message, WorkerToEdge::Elsewhere { chunk, region, .. } if *chunk == OUT && *region == EAST);
    run(
        &mut [&mut home],
        &mut [&mut e],
        "the region hearing whose the chunk is",
        |_, seats| seats[0].at(elsewhere).is_some(),
    );
    let (first, _) = enters(&mut [&mut home], &mut [&mut e], 0, player(), 71);
    e.send(does(player(), first, 1, to(OUT_X)));
    run(
        &mut [&mut home],
        &mut [&mut e],
        "the player being let go",
        |_, seats| departure(seats[0], player()).is_some(),
    );
    // The chunk is given back: nobody holds the place when the player comes again.
    other.request(StoreRequest::Return { chunks: vec![OUT] });
    other.flush();
    assert_eq!(home.region().knowledge(OUT), Knowledge::Foreign(EAST));

    let entries_before = e.entries().len();
    let noted_before = gate.noted().len();
    e.send(join_as(player(), 72));
    run(
        &mut [&mut home],
        &mut [&mut e],
        "the second stay being let go",
        |_, seats| seats[0].entries().len() > entries_before,
    );
    let second = EntityId(first.0 + 1);
    assert_eq!(told(&e, player(), 72), Some((second, place_at(OUT_X))));
    let (_, entry) = e.entries().pop().unwrap();
    let Durable::Departed { transfer, to, .. } = &entry else {
        panic!("{entry:?}");
    };
    assert_eq!(*to, EAST);
    assert_eq!(
        (transfer.entity_id, transfer.hops, transfer.attempt),
        (second, 1, Some(72))
    );
    // Told that they entered before they are told that they are another region's.
    let entered = |message: &WorkerToEdge| {
        matches!(
            message,
            WorkerToEdge::ToPlayer {
                event: PlayerEvent::Spawned { attempt: 72, .. },
                ..
            }
        )
    };
    let let_go = |message: &WorkerToEdge| matches!(message, WorkerToEdge::Outbox { entry: Durable::Departed { transfer, .. }, .. } if transfer.entity_id == second);
    assert!(e.at(entered).unwrap() < e.at(let_go).unwrap());
    assert_eq!(home.region().player(player()), None);
    // One tick did both: the join's commit named the stay as entering, and the next
    // one that says anything of a stay says where it went.
    let noted = gate.noted();
    assert_eq!(noted.len(), noted_before + 2, "{noted:?}");
    let has = StayNote::Has {
        player: player(),
        entity: second,
        hops: 1,
        place: place_at(OUT_X),
    };
    assert_eq!(noted[noted_before + 1].1, [has]);
}

// ---------------------------------------------------------------------------------------
// Which stay stays (sections 5 and 6)
// ---------------------------------------------------------------------------------------

#[test]
fn a_second_login_through_another_edge_ends_the_first_stay_and_tells_its_edge() {
    in_memory_and_on_disk(
        a_second_login_through_another_edge_ends_the_first_stay_and_tells_its_edge_in,
    );
}

/// Row 3 of section 10: the earlier stay is in the home region.
fn a_second_login_through_another_edge_ends_the_first_stay_and_tells_its_edge_in(on_disk: bool) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, _gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[ORIGIN]);
    let mut f = Seat::first(&west, F, &[]);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e, &mut f], 0, player(), 71);
    walks(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        0,
        (player(), first, 1),
        HOME_X,
    );

    let (second, entered) = enters(&mut [&mut west], &mut [&mut e, &mut f], 1, player(), 5);
    assert_eq!(second, EntityId(2));
    assert_eq!(entered, place_at(HOME_X));
    // The edge of the first stay is told so on its link, and the other edge is not.
    let synced = e.send(nothing());
    applied(&mut [&mut west], &mut [&mut e, &mut f], 0, synced);
    let gone = Durable::Ended {
        player: player(),
        entity: first,
        // The stay had been heard from as its entity.
        attempt: None,
    };
    assert_eq!(e.entries(), [(1, gone)]);
    assert_eq!(f.entries(), []);
    assert_eq!(e.spawned(player()).len(), 1);
    // Its entity is reported removed to those who watched it.
    let removed = |message: &WorkerToEdge| {
        matches!(message, WorkerToEdge::TickDelta { events, .. }
            if events.iter().any(|event| matches!(event, RegionEvent::EntityRemoved { entity, .. } if *entity == first)))
    };
    let ended_at = |message: &WorkerToEdge| {
        matches!(
            message,
            WorkerToEdge::Outbox {
                entry: Durable::Ended { .. },
                ..
            }
        )
    };
    assert!(e.at(removed).unwrap() < e.at(ended_at).unwrap());
    // One stay, and it is the second edge's.
    let state = west.region().player_state(player()).unwrap();
    assert_eq!((state.entity_id, state.edge), (second, F));
    assert_eq!(west.region().player_count(), 1);
    assert_eq!(west.region().entering_state(player()), None);
}

#[test]
fn a_second_login_before_the_first_was_answered_ends_the_entering_stay_with_its_attempt() {
    // Rows 13 and 14 of section 10: the first join's answer is still held.
    let land = Land::new(Shape::Stripes, false);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let mut f = Seat::first(&west, F, &[]);
    gate.hold_stays();
    let joined = e.send(join_as(player(), 71));
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the first join's note being answered",
        |_, seats| seats[0].applied() >= joined && gate.kept_of(is_enter) == 1,
    );
    f.send(join_as(player(), 5));
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the second join's note being answered",
        |_, _| gate.kept_of(is_enter) == 2,
    );
    gate.release_stays();
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the second stay entering",
        |_, seats| told(seats[1], player(), 5).is_some() && !seats[0].entries().is_empty(),
    );
    let gone = Durable::Ended {
        player: player(),
        entity: EntityId(1),
        attempt: Some(71),
    };
    assert_eq!(e.entries(), [(1, gone)]);
    // The answer to the first join found no such stay and was passed over.
    assert_eq!(e.spawned(player()), []);
    assert_eq!(
        told(&f, player(), 5),
        Some((EntityId(2), at_the_spawn_point()))
    );
    assert_eq!(west.region().player_count(), 1);
}

#[test]
fn the_stores_word_that_a_stay_is_dead_removes_it_where_it_is_and_tells_its_edge() {
    in_memory_and_on_disk(
        the_stores_word_that_a_stay_is_dead_removes_it_where_it_is_and_tells_its_edge_in,
    );
}

/// Row 1 of section 10: the earlier stay is in another region, which runs, and is
/// told at once. The later stay is let go to that region and arrives after.
fn the_stores_word_that_a_stay_is_dead_removes_it_where_it_is_and_tells_its_edge_in(on_disk: bool) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, _gate) = land.runner(WEST, 1);
    let (mut east, _east_gate) = land.runner(EAST, 2);
    let mut e = Seat::first(&west, E, &[]);
    let mut f = Seat::first(&west, F, &[]);
    let mut e_there = Seat::first(&east, E, &[BESIDE, FAR_EAST]);
    let mut f_there = Seat::first(&east, F, &[]);
    let (first, _) = enters(&mut [&mut west, &mut east], &mut [&mut e], 0, player(), 71);
    e.send(does(player(), first, 1, to(EAST_X)));
    run(
        &mut [&mut west, &mut east],
        &mut [&mut e],
        "the player being let go",
        |_, seats| departure(seats[0], player()).is_some(),
    );
    let (_, walked_over, _) = departure(&e, player()).unwrap();
    let arrived = e_there.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer: walked_over,
    });
    applied(&mut [&mut west, &mut east], &mut [&mut e_there], 0, arrived);
    assert_eq!(
        east.region().player(player()).map(|(entity, _)| entity),
        Some(first)
    );

    // The same player joins through the other edge.
    f.send(join_as(player(), 5));
    run(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut f, &mut e_there, &mut f_there],
        "the first stay being ended where it is, and the second let go there",
        |_, seats| !seats[2].entries().is_empty() && departure(seats[1], player()).is_some(),
    );
    let second = EntityId(2);
    let gone = Durable::Ended {
        player: player(),
        entity: first,
        // The stay had been heard from as its entity, by the step that took it over.
        attempt: None,
    };
    assert_eq!(e_there.entries(), [(1, gone)]);
    assert_eq!(east.region().player(player()), None);
    let removed = |message: &WorkerToEdge| {
        matches!(message, WorkerToEdge::TickDelta { events, .. }
            if events.iter().any(|event| matches!(event, RegionEvent::EntityRemoved { entity, .. } if *entity == first)))
    };
    assert!(e_there.at(removed).is_some(), "{:#?}", e_there.log);

    // The second stay arrives and is the only one.
    let (_, transfer, to_region) = departure(&f, player()).unwrap();
    assert_eq!((to_region, transfer.entity_id), (EAST, second));
    let arrived = f_there.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer,
    });
    applied(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut f, &mut e_there, &mut f_there],
        3,
        arrived,
    );
    let after = f_there.send(nothing());
    applied(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut f, &mut e_there, &mut f_there],
        3,
        after,
    );
    let state = east.region().player_state(player()).unwrap();
    assert_eq!((state.entity_id, state.edge), (second, F));
    assert_eq!(state.pose, standing(EAST_X));
    assert_eq!(ended(&f_there.entries()), []);
    assert_eq!(e_there.entries().len(), 1);
    assert_eq!(west.region().player(player()), None);
}

// ---------------------------------------------------------------------------------------
// Presence for a stay that is entering (section 4.2)
// ---------------------------------------------------------------------------------------

#[test]
fn a_link_lost_between_a_join_and_its_entering_is_answered_entering_for_a_player_it_names() {
    let land = Land::new(Shape::Stripes, false);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let mut f = Seat::first(&west, F, &[]);
    let (present, _) = enters(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        0,
        other_player(),
        70,
    );
    gate.hold_stays();
    let joined = e.send(join_as(player(), 71));
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the join being applied",
        |_, seats| seats[0].applied() >= joined && gate.kept_of(is_enter) == 1,
    );

    // The link is lost. A hello that does not name the player who is entering is not
    // answered for them: the edge has ended that connection, and its leave ends the
    // stay.
    let mut unnamed = e.again(&west, &[other_player()]);
    drop(e);
    let resumed = unnamed.send(nothing());
    applied(&mut [&mut west], &mut [&mut unnamed, &mut f], 0, resumed);
    let answers = unnamed.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert!(
        matches!(&answers[0], (who, Presence::Present { entity, .. }) if *who == other_player() && *entity == present),
        "{answers:?}"
    );
    assert!(matches!(
        unnamed.edge.welcomed,
        Some(Welcome::Resumed { presences: 1, .. })
    ));

    // One that names them is answered that they are entering, with the attempt of
    // their join, where it was answered that they are absent.
    let mut named = unnamed.again(&west, &[player(), other_player()]);
    drop(unnamed);
    // Another edge that names the player has no stay of theirs here.
    let mut stranger = f.again(&west, &[player()]);
    drop(f);
    let resumed = named.send(nothing());
    let asked = stranger.send(nothing());
    run(
        &mut [&mut west],
        &mut [&mut named, &mut stranger],
        "both hellos being answered",
        |_, seats| seats[0].applied() >= resumed && seats[1].applied() >= asked,
    );
    let answers = named.answers();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(
        answers[0],
        (player(), Presence::Entering { attempt: 71 }),
        "{answers:?}"
    );
    assert!(
        matches!(&answers[1], (who, Presence::Present { entity, .. }) if *who == other_player() && *entity == present)
    );
    assert_eq!(stranger.answers(), [(player(), Presence::Absent)]);
    assert_eq!(named.spawned(player()), []);

    // And when the store's answer is taken, the player enters on the link the edge
    // has now.
    gate.release_stays();
    run(
        &mut [&mut west],
        &mut [&mut named, &mut stranger],
        "the player entering the world",
        |_, seats| told(seats[0], player(), 71).is_some(),
    );
    assert_eq!(
        told(&named, player(), 71),
        Some((EntityId(2), at_the_spawn_point()))
    );
    assert_eq!(stranger.spawned(player()), []);
}

// ---------------------------------------------------------------------------------------
// Restores, merges and splits (section 9)
// ---------------------------------------------------------------------------------------

#[test]
fn a_restored_home_region_names_every_stay_in_its_first_tick_and_the_entering_one_enters() {
    in_memory_and_on_disk(
        a_restored_home_region_names_every_stay_in_its_first_tick_and_the_entering_one_enters_in,
    );
}

/// Row 18 of section 10. The first tick of the next owner changes nothing of the
/// region and is committed all the same, for its notes.
fn a_restored_home_region_names_every_stay_in_its_first_tick_and_the_entering_one_enters_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let (present, _) = enters(&mut [&mut west], &mut [&mut e], 0, other_player(), 70);
    walks(
        &mut [&mut west],
        &mut [&mut e],
        0,
        (other_player(), present, 1),
        HOME_X,
    );
    gate.hold_stays();
    let joined = e.send(join_as(player(), 71));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the join being applied",
        |_, seats| seats[0].applied() >= joined && gate.kept_of(is_enter) == 1,
    );
    let before = west.region().state();
    // The owner dies with the answer unread, and the store with it where it can.
    drop(west);
    let land = land.restarted();

    let (mut west, gate) = land.runner(WEST, 2);
    assert_eq!(
        RegionState {
            tick: before.tick,
            ..west.region().state()
        },
        before
    );
    let restored_at = west.region().tick_number();
    assert!(gate.noted().is_empty());
    west.step();
    assert_eq!(west.region().tick_number(), restored_at + 1);
    let entering = StayNote::Entering {
        player: player(),
        entity: EntityId(2),
    };
    let has = StayNote::Has {
        player: other_player(),
        entity: present,
        hops: 0,
        place: place_at(HOME_X),
    };
    // In the order of the players, whatever each note is.
    assert_eq!(gate.noted(), [(restored_at + 1, vec![entering, has])]);
    assert_eq!(gate.commits(), [restored_at + 1]);

    // The store answers the naming, and the player enters; the edge that links again
    // is told so.
    let mut again = e.again(&west, &[player(), other_player()]);
    run(
        &mut [&mut west],
        &mut [&mut again],
        "the player entering the world",
        |runners, seats| {
            runners[0].region().player(player()).is_some() && seats[0].applied() >= joined
        },
    );
    let state = west.region().player_state(player()).unwrap();
    assert_eq!(
        (state.entity_id, state.attempt, state.pose),
        (EntityId(2), Some(71), Pose::at(SPAWN))
    );
    assert_eq!(west.region().entering_state(player()), None);
    assert_eq!(west.region().player_count(), 2);
    // Until the tick that placed them is confirmed and published.
    let synced = again.send(nothing());
    applied(&mut [&mut west], &mut [&mut again], 0, synced);
    let answers = again.answers();
    assert_eq!(answers.len(), 2);
    // Told once, by whichever came first: the answer to the hello, made before the
    // tick that placed them, says that they are entering, and the tick says the rest.
    match &answers[0] {
        (who, Presence::Entering { attempt: 71 }) if *who == player() => {
            assert_eq!(
                told(&again, player(), 71),
                Some((EntityId(2), at_the_spawn_point()))
            );
        }
        (
            who,
            Presence::Present {
                entity, attempt, ..
            },
        ) if *who == player() => {
            assert_eq!((*entity, *attempt), (EntityId(2), Some(71)));
            assert_eq!(again.spawned(player()), []);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_restored_region_names_a_stay_that_died_while_it_had_no_owner_and_removes_it() {
    in_memory_and_on_disk(
        a_restored_region_names_a_stay_that_died_while_it_had_no_owner_and_removes_it_in,
    );
}

/// Row 4 of section 10, where the later stay went nowhere yet: the region is told
/// that the stay is dead only because it names it.
fn a_restored_region_names_a_stay_that_died_while_it_had_no_owner_and_removes_it_in(on_disk: bool) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, _gate) = land.runner(WEST, 1);
    let (mut east, _east_gate) = land.runner(EAST, 2);
    let mut e = Seat::first(&west, E, &[]);
    let mut f = Seat::first(&west, F, &[]);
    let mut e_there = Seat::first(&east, E, &[]);
    let (first, _) = enters(&mut [&mut west, &mut east], &mut [&mut e], 0, player(), 71);
    e.send(does(player(), first, 1, to(EAST_X)));
    run(
        &mut [&mut west, &mut east],
        &mut [&mut e],
        "the player being let go",
        |_, seats| departure(seats[0], player()).is_some(),
    );
    let (_, walked_over, _) = departure(&e, player()).unwrap();
    e_there.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer: walked_over,
    });
    let stepped = e_there.send(does(player(), first, 2, to(EAST_X + 1.0)));
    applied(&mut [&mut west, &mut east], &mut [&mut e_there], 0, stepped);
    // The eastern region loses its owner, and nobody runs it.
    drop(east);

    f.send(join_as(player(), 5));
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the second stay being let go to the region without an owner",
        |_, seats| departure(seats[1], player()).is_some(),
    );
    let (_, transfer, to_region) = departure(&f, player()).unwrap();
    assert_eq!((to_region, transfer.entity_id), (EAST, EntityId(2)));
    assert_eq!(transfer.pose, standing(EAST_X + 1.0));

    // The next owner has the first stay, names it in its first tick, which changes
    // nothing else, and removes it on the store's answer.
    let (mut east, east_gate) = land.runner(EAST, 3);
    let restored_at = east.region().tick_number();
    assert_eq!(
        east.region().player(player()).map(|(entity, _)| entity),
        Some(first)
    );
    east.step();
    let has = StayNote::Has {
        player: player(),
        entity: first,
        hops: 1,
        place: place_at(EAST_X + 1.0),
    };
    assert_eq!(east_gate.noted(), [(restored_at + 1, vec![has])]);
    run(
        &mut [&mut west, &mut east],
        &mut [&mut e, &mut f],
        "the dead stay being removed",
        |runners, _| runners[1].region().player(player()).is_none(),
    );
    // Its edge is told when it is back: the entry waits in its outbox.
    let mut back = e_there.again(&east, &[player()]);
    let resumed = back.send(nothing());
    applied(&mut [&mut west, &mut east], &mut [&mut back], 0, resumed);
    assert_eq!(
        ended(&back.entries()),
        [(player(), first, None)],
        "{:#?}",
        back.log
    );
    assert_eq!(back.answers(), [(player(), Presence::Absent)]);
}

#[test]
fn a_merge_drops_what_the_store_said_of_stays_and_the_tick_after_it_names_every_stay_and_is_answered()
 {
    in_memory_and_on_disk(
        a_merge_drops_what_the_store_said_of_stays_and_the_tick_after_it_names_every_stay_and_is_answered_in,
    );
}

/// Rows 2 to 5 of the table of section 9 for a merge. The home region has
/// [`other_player`], and absorbs a region that has an earlier stay of [`player`] and
/// one of [`third_player`]. Before the merge [`third_player`] has joined again and
/// been let go to that region, whose owner is gone; and while the store's answers are
/// held, [`player`] joins again and a fourth player joins for the first time.
fn a_merge_drops_what_the_store_said_of_stays_and_the_tick_after_it_names_every_stay_and_is_answered_in(
    on_disk: bool,
) {
    let fourth = PlayerId(Uuid::from_u128(4));
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let mut f = Seat::first(&west, F, &[]);
    let (stays, _) = enters(&mut [&mut west], &mut [&mut e], 0, other_player(), 70);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 71);
    let (third, _) = enters(&mut [&mut west], &mut [&mut e], 0, third_player(), 73);
    e.send(does(player(), first, 1, to(EAST_X)));
    e.send(does(third_player(), third, 1, to(EAST_X)));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "two players being let go",
        |_, seats| {
            departure(seats[0], player()).is_some() && departure(seats[0], third_player()).is_some()
        },
    );
    let (_, of_first, _) = departure(&e, player()).unwrap();
    let (_, of_third, _) = departure(&e, third_player()).unwrap();

    // Both arrive in the eastern region, which is then released to be absorbed.
    let hello = land.world.hello(EAST, 1);
    let (mut east, _east_gate) = gated_as(&land.world.store, hello, keeping());
    let mut there = Seat::first(&east, E, &[]);
    there.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer: of_first,
    });
    let arrived = there.send(EdgeToWorker::PlayerArrive {
        player: third_player(),
        transfer: of_third,
    });
    applied(&mut [&mut west, &mut east], &mut [&mut there], 0, arrived);
    assert_eq!(east.region().player_count(), 2);
    east.begin_release();
    assert_eq!(released(&mut east), Ended::Released);
    let (handle, restored) = land.world.open(EAST, 2);
    let state = absorbable(&handle, restored).unwrap();
    let east = East {
        handle,
        state,
        stranger: EdgeId::from_name("nobody"),
    };

    // The third player joins again through the other edge, and is let go to the
    // region that holds their place, which nobody runs. The stay they had there is
    // dead from now on, and nobody is there to be told.
    f.send(join_as(third_player(), 6));
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the third player's second stay being let go",
        |_, seats| departure(seats[1], third_player()).is_some(),
    );
    let (_, of_third_again, to_region) = departure(&f, third_player()).unwrap();
    let third_again = of_third_again.entity_id;
    assert_eq!(to_region, EAST);
    // The store says that the stay is dead behind the answer that let the later one
    // go. A commit later the runner has read that too, and nothing of it is held below.
    let synced = f.send(nothing());
    applied(&mut [&mut west], &mut [&mut e, &mut f], 1, synced);

    // From here on the store's answers are held: the first player's second join and
    // the fourth player's join are held as entering, and the store's word that what
    // came before those two stays is dead waits with its answers to them.
    gate.hold_stays();
    f.send(join_as(player(), 5));
    let joined = f.send(join_as(fourth, 74));
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the joins being applied and their notes answered",
        |_, seats| {
            seats[1].applied() >= joined && gate.kept_of(is_enter) == 2 && dead_kept(&gate) == 2
        },
    );
    let second = west.region().entering_state(player()).unwrap().entity_id;
    let of_fourth = west.region().entering_state(fourth).unwrap().entity_id;

    // The merge. Once the region stands still the answers are let through: the
    // runner reads them, and they wait for a tick that the merge takes the place of.
    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    step_to(&mut west, Stage::Settling);
    gate.release_stays();
    for _ in 0..20_000 {
        if west.inputs.entered.len() == 2 && west.inputs.dead.len() == 2 {
            break;
        }
        // Not a step: the merge is not to be taken before the answers are read.
        assert!(west.take_replies());
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(west.inputs.entered.len(), 2);
    assert_eq!(west.inputs.dead.len(), 2);
    assert_eq!(
        reshaped(&mut west, &outcome),
        Reshaped::Absorbed { absorbed: EAST }
    );
    drop(east.handle);
    assert_eq!(west.inputs, TickInputs::default());
    let merged = west.region().state();
    let entering: Vec<_> = merged.entering.keys().copied().collect();
    assert_eq!(entering, [player(), fourth]);
    assert_eq!(merged.players[&player()].entity_id, first);
    assert_eq!(merged.players[&third_player()].entity_id, third);

    // The first tick after it names every stay, and nothing else has changed.
    let noted_before = gate.noted().len();
    west.step();
    assert_eq!(west.region().tick_number(), merged.tick + 1);
    let noted = gate.noted();
    assert_eq!(noted.len(), noted_before + 1);
    let (tick, notes) = &noted[noted_before];
    assert_eq!(*tick, merged.tick + 1);
    let has = |player: PlayerId, entity: EntityId, hops: u32, x: f64| StayNote::Has {
        player,
        entity,
        hops,
        place: if x == 0.5 {
            at_the_spawn_point()
        } else {
            place_at(x)
        },
    };
    let entering = |player: PlayerId, entity: EntityId| StayNote::Entering { player, entity };
    assert_eq!(
        *notes,
        [
            entering(player(), second),
            has(player(), first, 1, EAST_X),
            has(other_player(), stays, 0, 0.5),
            has(third_player(), third, 1, EAST_X),
            entering(fourth, of_fourth),
        ]
    );

    // And is answered: the stays that entered enter, and the stays that are dead go,
    // their edge being told.
    run(
        &mut [&mut west],
        &mut [],
        "the answers to the naming being taken",
        |runners, _| {
            let region = runners[0].region();
            region.player(fourth).is_some()
                && region.player(third_player()).is_none()
                && region
                    .player(player())
                    .is_some_and(|(entity, _)| entity == second)
        },
    );
    let region = west.region();
    assert_eq!(region.state().entering, BTreeMap::new());
    assert_eq!(region.player_count(), 3);
    // The place is the region's own since the merge, so the player is placed in it.
    let state = region.player_state(player()).unwrap();
    assert_eq!(
        (state.pose, state.hops, state.edge),
        (standing(EAST_X), 0, F)
    );
    assert_eq!(
        region.player(fourth).map(|(_, pose)| pose),
        Some(Pose::at(SPAWN))
    );
    let outbox: Vec<_> = region.edge(E).unwrap().outbox.clone().into_iter().collect();
    let mut told = ended(&outbox);
    told.sort();
    assert_eq!(
        told,
        [(player(), first, None), (third_player(), third, None)]
    );

    // The third player's second stay arrives where the edge is told the region went.
    let mut f_again = f.again(&west, &[player(), fourth]);
    let arrived = f_again.send(EdgeToWorker::PlayerArrive {
        player: third_player(),
        transfer: of_third_again,
    });
    applied(&mut [&mut west], &mut [&mut f_again], 0, arrived);
    let after = f_again.send(nothing());
    applied(&mut [&mut west], &mut [&mut f_again], 0, after);
    let state = west.region().player_state(third_player()).unwrap();
    assert_eq!((state.entity_id, state.edge), (third_again, F));
    assert_eq!(ended(&f_again.entries()), []);
}

#[test]
fn a_split_drops_what_the_store_said_of_stays_and_both_regions_name_every_stay_in_their_first_tick()
{
    in_memory_and_on_disk(
        a_split_drops_what_the_store_said_of_stays_and_both_regions_name_every_stay_in_their_first_tick_in,
    );
}

/// The same for a split: the entering stay stays with the home region, the player who
/// goes has one hand-over more, and each region's first tick says what it has.
fn a_split_drops_what_the_store_said_of_stays_and_both_regions_name_every_stay_in_their_first_tick_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[ORIGIN, WEST_OF_HOME, FAR_WEST]);
    let (stays, _) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 71);
    let (goes, _) = enters(&mut [&mut west], &mut [&mut e], 0, other_player(), 72);
    e.send(does(other_player(), goes, 1, to(-20.5)));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the chunks being loaded and the player standing in the far one",
        |runners, _| {
            runners[0].region().loaded_chunk_count() == 3
                && x_of(runners[0], other_player()) == Some(-20.5)
        },
    );
    gate.hold_stays();
    let joined = e.send(join_as(third_player(), 73));
    run(
        &mut [&mut west],
        &mut [&mut e],
        "the join being applied and its note answered",
        |_, seats| {
            seats[0].applied() >= joined && gate.kept_of(is_enter) == 1 && dead_kept(&gate) == 1
        },
    );
    let third = west
        .region()
        .entering_state(third_player())
        .unwrap()
        .entity_id;

    let (done, outcome) = outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    step_to(&mut west, Stage::Settling);
    gate.release_stays();
    for _ in 0..20_000 {
        if west.inputs.entered.len() == 1 && west.inputs.dead.len() == 1 {
            break;
        }
        assert!(west.take_replies());
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(west.inputs.entered.len(), 1);
    assert_eq!(west.inputs.dead.len(), 1);
    let Reshaped::Split {
        region: new,
        as_epoch,
        part,
    } = reshaped(&mut west, &outcome)
    else {
        panic!("no split");
    };
    assert_eq!(west.inputs, TickInputs::default());
    let split_at = west.region().tick_number();
    assert!(west.region().entering_state(third_player()).is_some());
    assert_eq!(part.region.entering_state(third_player()), None);

    // The region that was split: who stayed, and the stay that is entering.
    let noted_before = gate.noted().len();
    west.step();
    let has = |player: PlayerId, entity: EntityId, hops: u32, x: f64| StayNote::Has {
        player,
        entity,
        hops,
        place: if x == 0.5 {
            at_the_spawn_point()
        } else {
            place_at(x)
        },
    };
    let entering = StayNote::Entering {
        player: third_player(),
        entity: third,
    };
    let noted = gate.noted();
    assert_eq!(noted.len(), noted_before + 1);
    assert_eq!(
        noted[noted_before],
        (split_at + 1, vec![has(player(), stays, 0, 0.5), entering])
    );
    run(
        &mut [&mut west],
        &mut [],
        "the entering stay being answered again",
        |runners, _| runners[0].region().player(third_player()).is_some(),
    );
    assert_eq!(west.region().entering_state(third_player()), None);

    // The part: who went, handed on once more by the split. Its first tick changes
    // nothing and is committed for the note.
    let (handle, _) = land.world.open(new, as_epoch);
    let (store, part_gate) = gate_before(handle);
    let mut part = RegionRunner::of_part_with(part, store).with_id(new);
    part.step();
    assert_eq!(
        part_gate.noted(),
        [(split_at + 1, vec![has(other_player(), goes, 1, -20.5)])]
    );
    assert_eq!(part_gate.commits(), [split_at + 1]);

    // The store has that place under that count of hand-overs: when the player joins
    // again they are let go to the part, and the part, which runs, is told at once
    // that the stay it has is dead.
    let mut f = Seat::first(&west, F, &[]);
    f.send(join_as(other_player(), 5));
    run(
        &mut [&mut west, &mut part],
        &mut [&mut f],
        "the second stay being let go to the part, and the first being ended there",
        |runners, seats| {
            departure(seats[0], other_player()).is_some()
                && runners[1].region().player(other_player()).is_none()
        },
    );
    let (_, transfer, to_region) = departure(&f, other_player()).unwrap();
    assert_eq!(to_region, new);
    assert_eq!((transfer.pose, transfer.hops), (standing(-20.5), 1));
    let outbox: Vec<_> = part
        .region()
        .edge(E)
        .unwrap()
        .outbox
        .clone()
        .into_iter()
        .collect();
    assert_eq!(ended(&outbox), [(other_player(), goes, None)]);
}

// ---------------------------------------------------------------------------------------
// Entity ids never go back (section 7)
// ---------------------------------------------------------------------------------------

/// What the store returns for a region whose block is `block`, with `issued` as the
/// highest stay ever given, a state and the changes since.
fn restored_with(
    block: EntityIds,
    issued: i32,
    state: Option<&RegionState>,
    deltas: &[StateDelta],
) -> Restored {
    let tick_state = |tick: u64, state: Vec<u8>| clustine_rpc::TickState { tick, state };
    Restored {
        entity_ids: block,
        state: state.map(|state| tick_state(state.tick, stored(state))),
        deltas: deltas
            .iter()
            .map(|delta| tick_state(delta.tick, stored(delta)))
            .collect(),
        held: Vec::new(),
        pinned: Vec::new(),
        issued: EntityId(issued),
    }
}

/// A state as of tick 5 with `block` as its block and `next` as its next entity id.
fn state_with(block: EntityIds, next: i32) -> RegionState {
    RegionState {
        tick: 5,
        next_entity_id: EntityId(next),
        ..RegionState::new(block)
    }
}

/// The next entity id of the region as it is restored from `restored`, and its block.
fn next_of(restored: Restored) -> (i32, EntityIds) {
    let state = restored_state(restored).unwrap();
    (state.next_entity_id.0, state.entity_ids)
}

#[test]
fn a_restored_region_goes_on_with_the_next_id_of_its_state_if_that_is_of_the_stores_block() {
    let block = EntityIds::block(0).unwrap();
    let state = state_with(block, 7);
    // Nothing was ever given, or the store kept no count: as it was before.
    assert_eq!(
        next_of(restored_with(block, 0, Some(&state), &[])),
        (7, block)
    );
    // What was given is below it.
    assert_eq!(
        next_of(restored_with(block, 6, Some(&state), &[])),
        (7, block)
    );
    assert_eq!(
        next_of(restored_with(block, 3, Some(&state), &[])),
        (7, block)
    );
    // A commit since the state has a later one.
    let delta = StateDelta {
        tick: 6,
        next_entity_id: Some(EntityId(9)),
        ..StateDelta::default()
    };
    let with_a_commit = restored_with(block, 8, Some(&state), std::slice::from_ref(&delta));
    assert_eq!(next_of(with_a_commit), (9, block));
    // A region that never ran begins with the first id of its block.
    assert_eq!(next_of(restored_with(block, 0, None, &[])), (1, block));
}

#[test]
fn a_restored_region_never_gives_an_id_again_that_the_store_was_told_of() {
    let block = EntityIds::block(0).unwrap();
    // The state says less than the store was told: a commit that named a stay is in
    // the log, and the region's state as of it was dropped for another build.
    let state = state_with(block, 7);
    assert_eq!(
        next_of(restored_with(block, 11, Some(&state), &[])),
        (12, block)
    );
    // A state of another build is dropped as a whole, and the region does not begin
    // with the first id of its block again.
    let mut of_another_build = restored_with(block, 11, Some(&state), &[]);
    of_another_build.state.as_mut().unwrap().state = vec![5, 0, 0];
    assert_eq!(next_of(of_another_build), (12, block));
    // A region that has no state at all, in a world that has given stays: the home
    // region of another division, which has this block.
    assert_eq!(next_of(restored_with(block, 11, None, &[])), (12, block));
    // A stay of another block says nothing of this one.
    let other = EntityIds::block(3).unwrap();
    assert_eq!(
        next_of(restored_with(other, 11, None, &[])),
        (other.first.0, other)
    );
}

#[test]
fn a_region_given_a_new_block_has_the_same_next_id_at_every_restore_until_its_next_checkpoint() {
    // The store has given the home region the block `new`: the one it had has no id
    // above the highest stay. The whole state on disk still names the old block.
    let old = EntityIds::block(0).unwrap();
    let new = EntityIds::block(2).unwrap();
    let issued = old.end.0 - 1;
    let state = state_with(old, old.end.0);
    // Before any stay of the new block: its first id, every time.
    for _ in 0..2 {
        let restored = restored_with(new, issued, Some(&state), &[]);
        assert_eq!(next_of(restored), (new.first.0, new));
    }
    // After two joins, whose commits have next ids of the new block: the state's id is
    // kept, and the highest stay is below it.
    let join = |tick: u64, next: i32| StateDelta {
        tick,
        next_entity_id: Some(EntityId(next)),
        ..StateDelta::default()
    };
    let deltas = [join(6, new.first.0 + 1), join(7, new.first.0 + 2)];
    for _ in 0..2 {
        let restored = restored_with(new, new.first.0 + 1, Some(&state), &deltas);
        assert_eq!(next_of(restored), (new.first.0 + 2, new));
    }
    // A next id of the old block that happens to be the new block's first is that.
    let adjacent = EntityIds::block(1).unwrap();
    assert_eq!(old.end, adjacent.first);
    let restored = restored_with(adjacent, issued, Some(&state), &[]);
    assert_eq!(next_of(restored), (adjacent.first.0, adjacent));
}

#[test]
fn a_region_whose_block_is_used_up_stays_so_when_it_is_restored() {
    // The end of the block is the next id of a region that has given out every id of
    // it. Counted as outside the block, it would begin with the first again.
    let block = EntityIds::block(0).unwrap();
    let state = state_with(block, block.end.0);
    assert_eq!(
        next_of(restored_with(block, 0, Some(&state), &[])),
        (block.end.0, block)
    );
    assert_eq!(
        next_of(restored_with(block, block.end.0 - 1, Some(&state), &[])),
        (block.end.0, block)
    );
    // A region that a split made has no ids at all.
    let none = EntityIds {
        first: EntityId(0),
        end: EntityId(0),
    };
    let state = state_with(none, 0);
    assert_eq!(
        next_of(restored_with(none, 40, Some(&state), &[])),
        (0, none)
    );
}

#[test]
fn a_home_region_whose_state_was_dropped_gives_the_next_player_an_id_above_every_stay_given() {
    in_memory_and_on_disk(
        a_home_region_whose_state_was_dropped_gives_the_next_player_an_id_above_every_stay_given_in,
    );
}

/// With the real store: two stays are given, the region's state is replaced by one
/// that no build reads, and the next owner goes on above them.
fn a_home_region_whose_state_was_dropped_gives_the_next_player_an_id_above_every_stay_given_in(
    on_disk: bool,
) {
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, _gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 71);
    let (second, _) = enters(&mut [&mut west], &mut [&mut e], 0, other_player(), 72);
    assert_eq!((first, second), (EntityId(1), EntityId(2)));
    walks(
        &mut [&mut west],
        &mut [&mut e],
        0,
        (player(), first, 1),
        HOME_X,
    );
    let tick = west.region().tick_number();
    drop(west);

    // A checkpoint as a build with another form of state would have left it.
    let (handle, restored) = land.world.open(WEST, 2);
    assert_eq!(restored.issued, second);
    assert!(restored.tick() <= tick);
    handle.request(StoreRequest::Checkpoint {
        tick: restored.tick(),
        state: vec![9, 9, 9],
    });
    handle.flush();
    drop(handle);
    let land = land.restarted();

    for epoch in [3, 4] {
        let (handle, restored) = land.world.open(WEST, epoch);
        assert_eq!(restored.issued, second);
        let state = restored_state(restored).unwrap();
        assert_eq!(state.next_entity_id, EntityId(3));
        assert!(state.players.is_empty());
        drop(handle);
    }
    let (mut west, _gate) = land.runner(WEST, 5);
    let mut e = Seat::first(&west, E, &[]);
    // Everybody joins again, in place, with a higher id.
    let (third, entered) = enters(&mut [&mut west], &mut [&mut e], 0, player(), 81);
    assert_eq!((third, entered), (EntityId(3), place_at(HOME_X)));
}

// ---------------------------------------------------------------------------------------
// A death between any two steps of a login (rows 18 and 19 of section 10)
// ---------------------------------------------------------------------------------------

/// Where the home region's owner dies in the middle of a login, and the store with it
/// where the world is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Death {
    /// Nobody dies: what the others are held to.
    Never,
    /// The join has been sent, and no tick has taken it.
    Sent,
    /// The tick that took the join has run and its commit is asked for; the runner
    /// has read no answer to it.
    Joined,
    /// The commit is confirmed and the store's answer to the note is there, unread.
    Answered,
    /// The tick that took the answer has run and its commit is asked for; nothing of
    /// it was published.
    Taken,
    /// That tick is confirmed and published.
    Published,
}

const DEATHS: [Death; 6] = [
    Death::Never,
    Death::Sent,
    Death::Joined,
    Death::Answered,
    Death::Taken,
    Death::Published,
];

/// Where the player was last when they log in again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    /// In the home region, where their earlier stay still is, through another edge.
    Here,
    /// In the eastern stripe, to which that stay was let go.
    There,
}

/// A second login of [`player`] through edge F, with a death at `death`. Whatever
/// dies, the world ends as it does without a death: one stay of the player, the later
/// one, where the earlier one was last; the later edge told of it, and the earlier
/// edge told that its stay has ended where the region ended it.
fn a_login_with_a_death(on_disk: bool, last: Last, death: Death) {
    let context = format!("{death:?}, {last:?}, on disk: {on_disk}");
    let land = Land::new(Shape::Stripes, on_disk);
    let (mut west, gate) = land.runner(WEST, 1);
    let mut e = Seat::first(&west, E, &[]);
    let mut f = Seat::first(&west, F, &[]);
    let (first, _) = enters(&mut [&mut west], &mut [&mut e, &mut f], 0, player(), 71);
    let x = match last {
        Last::Here => HOME_X,
        Last::There => EAST_X,
    };
    e.send(does(player(), first, 1, to(x)));
    let synced = e.send(nothing());
    applied(&mut [&mut west], &mut [&mut e, &mut f], 0, synced);
    let hello = f.send(nothing());
    applied(&mut [&mut west], &mut [&mut e, &mut f], 1, hello);
    match last {
        Last::Here => assert_eq!(x_of(&west, player()), Some(x), "{context}"),
        // Let go when the store has said whose the chunk is, a tick or two after the
        // step.
        Last::There => run(
            &mut [&mut west],
            &mut [&mut e, &mut f],
            "the first stay being let go",
            |_, seats| departure(seats[0], player()).is_some(),
        ),
    }
    let entries_before = e.entries().len();

    let second = EntityId(2);
    // The edge keeps the join until it is told that it is applied.
    let join = join_as(player(), 5);
    let done = |seat: &Seat| match last {
        Last::Here => told(seat, player(), 5).is_some(),
        Last::There => departure(seat, player()).is_some(),
    };
    match death {
        Death::Sent => {}
        Death::Joined => {
            gate.hold();
            gate.hold_stays();
        }
        Death::Answered | Death::Taken => gate.hold_stays(),
        Death::Never | Death::Published => {}
    }
    let joined = f.send(join.clone());
    match death {
        Death::Sent => {}
        Death::Joined => run(
            &mut [&mut west],
            &mut [&mut e, &mut f],
            "the tick that takes the join",
            |runners, _| runners[0].region().entering_state(player()).is_some(),
        ),
        Death::Answered | Death::Taken => {
            run(
                &mut [&mut west],
                &mut [&mut e, &mut f],
                "the answer to the join's note",
                |_, seats| seats[1].applied() >= joined && gate.kept_of(is_enter) == 1,
            );
            if death == Death::Taken {
                gate.hold();
                gate.release_stays();
                run(
                    &mut [&mut west],
                    &mut [&mut e, &mut f],
                    "the tick that takes the answer",
                    |runners, _| runners[0].region().entering_state(player()).is_none(),
                );
                assert!(!done(&f), "{context}");
            }
        }
        Death::Never | Death::Published => run(
            &mut [&mut west],
            &mut [&mut e, &mut f],
            "the login being through",
            |_, seats| done(seats[1]),
        ),
    }

    let (mut west, land, mut e, mut f) = if death == Death::Never {
        (west, land, e, f)
    } else {
        // The owner dies, and the store where it can. Another owner opens the region,
        // and the edges link again and send again what they were not told is applied.
        drop(west);
        let land = land.restarted();
        let (mut west, _gate) = land.runner(WEST, 2);
        let mut e_again = e.again(&west, &[player()]);
        let mut f_again = f.again(&west, &[player()]);
        run(
            &mut [&mut west],
            &mut [&mut e_again, &mut f_again],
            "both welcomes",
            |_, seats| seats.iter().all(|seat| seat.edge.welcomed.is_some()),
        );
        let (Some(Welcome::Resumed { applied, .. }), Some(Welcome::Resumed { .. })) =
            (f_again.edge.welcomed, e_again.edge.welcomed)
        else {
            panic!("{context}: an edge was not known: {:#?}", f_again.log);
        };
        // What the region had of the join when it answered the hello, by where it
        // died.
        let presence = f_again.answers();
        match death {
            Death::Sent => {
                assert!(applied < joined, "{context}");
                assert_eq!(presence, [(player(), Presence::Absent)], "{context}");
            }
            Death::Joined | Death::Answered => {
                assert_eq!(applied, joined, "{context}");
                let entering = Presence::Entering { attempt: 5 };
                assert_eq!(presence, [(player(), entering)], "{context}");
            }
            Death::Taken | Death::Published => {
                assert_eq!(applied, joined, "{context}");
                match last {
                    Last::Here => assert!(
                        matches!(&presence[..], [(_, Presence::Present { entity, attempt: Some(5), .. })] if *entity == second),
                        "{context}: {presence:?}"
                    ),
                    Last::There => {
                        assert_eq!(presence, [(player(), Presence::Absent)], "{context}");
                    }
                }
            }
            Death::Never => unreachable!(),
        }
        if applied < joined {
            f_again.send_again(joined, join.clone());
        }
        // Everything read before the death is still what the edge has read.
        e_again.log.splice(0..0, e.log.clone());
        f_again.log.splice(0..0, f.log.clone());
        (west, land, e_again, f_again)
    };

    // To the end without a death.
    let through_e = e.send(nothing());
    let through_f = f.send(nothing());
    run(
        &mut [&mut west],
        &mut [&mut e, &mut f],
        "the login being through for everybody",
        |runners, seats| {
            let region = runners[0].region();
            let settled = region.entering_state(player()).is_none()
                && match last {
                    Last::Here => region
                        .player(player())
                        .is_some_and(|(entity, _)| entity == second),
                    // The entry stays in the outbox until the edge says that it has it,
                    // which one that had read it before the death does with its hello.
                    Last::There => departure(seats[1], player()).is_some(),
                };
            settled && seats[0].applied() >= through_e && seats[1].applied() >= through_f
        },
    );
    let through_f = f.send(nothing());
    applied(&mut [&mut west], &mut [&mut e, &mut f], 1, through_f);

    // One stay, the later one, in the right place.
    let region = west.region();
    assert_eq!(region.entering_state(player()), None, "{context}");
    let presences = f.answers();
    let present = presences.iter().find_map(|(_, answer)| match answer {
        Presence::Present {
            entity,
            pose,
            attempt,
            ..
        } => Some((*entity, *pose, *attempt)),
        _ => None,
    });
    match last {
        Last::Here => {
            let state = region.player_state(player()).unwrap();
            assert_eq!(
                (state.entity_id, state.pose, state.edge, state.hops),
                (second, standing(x), F, 0),
                "{context}"
            );
            assert_eq!(region.player_count(), 1, "{context}");
            // The edge was told of the stay of its connection: that it entered, or, where
            // that was lost with the owner, by the answer to its hello.
            let entered = told(&f, player(), 5);
            assert!(
                entered.is_some() || present.is_some(),
                "{context}: {:#?}",
                f.log
            );
            if let Some(entered) = entered {
                assert_eq!(entered, (second, place_at(x)), "{context}");
            }
            if let Some(present) = present {
                assert_eq!(present, (second, standing(x), Some(5)), "{context}");
            }
            assert!(f.spawned(player()).len() <= 1, "{context}");
            assert_eq!(f.entries(), [], "{context}");
            // The earlier edge is told that its stay has ended, once.
            assert_eq!(
                ended(&e.entries()),
                [(player(), first, None)],
                "{context}: {:#?}",
                e.log
            );
        }
        Last::There => {
            assert_eq!(region.player(player()), None, "{context}");
            assert_eq!(told(&f, player(), 5), None, "{context}");
            let entries = f.entries();
            assert_eq!(entries.len(), 1, "{context}: {entries:?}");
            let (_, transfer, to_region) = departure(&f, player()).unwrap();
            assert_eq!(to_region, EAST, "{context}");
            assert_eq!(
                (
                    transfer.entity_id,
                    transfer.pose,
                    transfer.hops,
                    transfer.attempt
                ),
                (second, standing(x), 1, Some(5)),
                "{context}"
            );
            // The earlier stay was let go before, and the home region ended nothing.
            assert_eq!(e.entries().len(), entries_before, "{context}");
        }
    }

    // Whoever joins next has an entity above both, also after one more death.
    drop(west);
    let land = land.restarted();
    let (handle, restored) = land.world.open(WEST, 7);
    assert_eq!(restored.issued, second, "{context}");
    let state = restored_state(restored).unwrap();
    assert_eq!(state.next_entity_id, EntityId(3), "{context}");
    assert!(state.entering.is_empty(), "{context}");
    drop(handle);
}

#[test]
fn a_death_at_any_step_of_a_login_into_the_home_region_leaves_one_stay_in_the_right_place() {
    for on_disk in [false, true] {
        for death in DEATHS {
            a_login_with_a_death(on_disk, Last::Here, death);
        }
    }
}

#[test]
fn a_death_at_any_step_of_a_login_that_is_let_go_leaves_one_stay_on_its_way_to_the_right_place() {
    for on_disk in [false, true] {
        for death in DEATHS {
            a_login_with_a_death(on_disk, Last::There, death);
        }
    }
}
