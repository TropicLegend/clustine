//! What a borrower does with an action on a chunk it has been lent and has not loaded
//! yet (step L6 of `docs/adr/0027-lending-at-a-line.md`, and the hold of
//! `docs/adr/0014-merging-and-splitting.md`, section 3.6): the action waits for the
//! chunk's load, with everything its link sent behind it, and is then taken against the
//! loaded chunk. Before that rule, a click that came in the tick that took the grant was
//! judged against a chunk held and not loaded, read air there, and was acknowledged
//! without effect: a lever placed at the line was lost.
//!
//! The world is that of [`Divided::stripes`]: region 0, the borrower, is pinned west of
//! chunk x = 1 and region 1, the lender, east of it. Both are run by runners of their
//! own on the one store in memory, and the lend is brought about as a player brings it
//! about: the borrower's player stands by the line and places a lever a block from it,
//! in the borrower's own pinned land, which asks for the chunks across within two
//! blocks; the store asks the lender, which lets go (after a wait while it loads them)
//! and the store grants them to the borrower. Nothing is handed to the store by the
//! test. A gate before the borrower's store holds back its loads, and for one test its
//! grants, so that a click arrives before the lent chunk's load, or with its grant.
//!
//! The borrower's edge views only the borrower's own chunks, so that no subscription of
//! the link waits for a lent chunk: what is tested is that the action waits whether or
//! not one does.

use clustine_data::{BlockState, blocks, items};
use clustine_sim::api::{Face, ItemStack, PlayerEvent};

use super::*;

/// The borrower.
const WEST: RegionId = RegionId(0);

/// The chunk across the line from where the builder stands, and the one north of it,
/// which a lever a block from the line reaches within two blocks.
const ACROSS: ChunkPos = ChunkPos::new(1, 0);
const ACROSS_NORTH: ChunkPos = ChunkPos::new(1, -1);

/// Where the builder stands: in the borrower's chunk at the origin, a block and a half
/// from the line, within reach of both sides of it.
const STANDS: Vec3 = Vec3::new(14.5, -60.0, 0.5);

/// The lever that asks for the lend, on the grass a block from the line.
const ASKS: BlockPos = BlockPos::new(15, -60, 0);

/// A stone on the borrower's side of the line, against whose east face a lever is put
/// into the chunk across.
const STONE_AT: BlockPos = BlockPos::new(15, -60, 2);

/// How many steps of both runners a wait for a state may take. A lend takes the
/// lender's wait while it loads the chunks and the borrower's ask again twenty ticks on.
const STEPS: usize = 4000;

/// How many steps a test runs while it holds a load back, to see that nothing waiting
/// for it is taken meanwhile.
const HELD_FOR: usize = 15;

/// The borrower's chunks that its edge views: those around the builder.
fn own() -> Vec<ChunkPos> {
    (-1..=0)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A region's settings in which a player enters with levers in the first slot of the
/// hotbar and stone in the second. Chunks nothing uses are given back late, so that the
/// borrower's own stay loaded throughout.
fn builders_settings() -> RegionSettings {
    let mut settings = settings(1_000);
    settings.starting_inventory.0[0] = Some(ItemStack::new(items::LEVER, 64));
    settings.starting_inventory.0[1] = Some(ItemStack::new(items::STONE, 64));
    settings
}

/// A click on the `face` of the block at `position`, as a player's client sends it.
fn click(position: BlockPos, face: Face, sequence: i32) -> PlayerInput {
    PlayerInput::UseItemOn {
        position,
        face,
        cursor: [0.5; 3],
        sneaking: false,
        sequence,
    }
}

/// A lever standing on the floor, as another region's player places it through the
/// edge.
fn floor_lever() -> BlockState {
    BlockState::parse("minecraft:lever[face=floor,facing=north,powered=false]")
        .expect("a lever's state")
}

/// The two regions by the line, the borrower's builder standing at [`STANDS`] with the
/// stone at [`STONE_AT`] placed.
struct Line {
    /// Kept for the store, which lasts as long as it.
    _world: Divided,
    west: RegionRunner,
    gate: Arc<GateControl>,
    east: RegionRunner,
    /// The builder's edge, and what it has been told.
    edge: TestEdge,
    told: Vec<WorkerToEdge>,
    /// Another edge of the borrower's, through which another region's player acts, and
    /// what it has been told.
    other: Option<TestEdge>,
    other_told: Vec<WorkerToEdge>,
    /// The sequence number of the builder's last click.
    sequence: i32,
}

impl Line {
    fn new() -> Self {
        let world = Divided::stripes();
        let (west, gate) = world.gated(WEST, builders_settings());
        let east = world.runner(EAST, 1);
        let (edge, worker_end) = in_process(4096);
        west.links().attach(worker_end);
        let mut line = Self {
            _world: world,
            west,
            gate,
            east,
            edge,
            told: Vec::new(),
            other: None,
            other_told: Vec::new(),
            sequence: 0,
        };
        line.edge.try_send(join(player(), "Notch")).unwrap();
        line.edge.try_send(asking_for(own())).unwrap();
        line.until("the builder in the world with the chunks around", |line| {
            let region = line.west.region();
            region.player(player()).is_some()
                && own().iter().all(|chunk| region.chunk(*chunk).is_some())
        });
        line.send(PlayerInput::Move {
            position: Some(STANDS),
            rotation: None,
            on_ground: true,
        });
        line.until("the builder by the line", |line| {
            x_of(&line.west, player()) == Some(STANDS.x)
        });
        // Stone is no circuit's part, and asks for no lend.
        line.send(PlayerInput::SelectSlot { slot: 1 });
        line.click(BlockPos::new(15, -61, 2), Face::Top);
        line.send(PlayerInput::SelectSlot { slot: 0 });
        line.until("the stone", |line| {
            line.block(STONE_AT) == Some(blocks::STONE)
        });
        line
    }

    /// Steps the borrower and then the lender once, and reads what the edges were told.
    fn step(&mut self) {
        step(&mut self.west);
        step(&mut self.east);
        self.told.extend(received(&mut self.edge));
        if let Some(other) = &mut self.other {
            self.other_told.extend(received(other));
        }
    }

    /// Steps both runners until `done` holds, at most [`STEPS`] times.
    fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        for _ in 0..STEPS {
            self.step();
            if done(self) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!(
            "{what} never came: the borrower knows {:?} of the chunk across and has {:?} \
             loaded; the lender knows {:?} of it",
            self.west.region().knowledge(ACROSS),
            self.west.region().loaded_chunks(),
            self.east.region().knowledge(ACROSS),
        );
    }

    /// Sends what the builder does, numbered as an edge numbers it.
    fn send(&self, input: PlayerInput) {
        let input = EdgeToWorker::Input {
            player: player(),
            entity: usual_entity(player()),
            number: next_number(),
            input,
        };
        self.edge.try_send(input).unwrap();
    }

    /// Sends a click of the builder's with the next sequence number, and returns that.
    fn click(&mut self, position: BlockPos, face: Face) -> i32 {
        self.sequence += 1;
        self.send(click(position, face, self.sequence));
        self.sequence
    }

    /// The block at `position` as the borrower has it, if it has the chunk loaded.
    fn block(&self, position: BlockPos) -> Option<BlockState> {
        let chunk = self.west.region().chunk(position.chunk())?;
        let (x, z) = position.in_chunk();
        chunk.get(x, position.y, z)
    }

    /// Whether the borrower has a lever at `position`, in whatever state.
    fn lever_at(&self, position: BlockPos) -> bool {
        self.block(position)
            .is_some_and(|state| state.block_id() == blocks::LEVER.block_id())
    }

    /// How many times the builder's edge was told that their actions up to `sequence`
    /// were handled, with that sequence.
    fn acknowledged(&self, sequence: i32) -> usize {
        let told = |message: &&WorkerToEdge| {
            matches!(
                message,
                WorkerToEdge::ToPlayer {
                    player: who,
                    event: PlayerEvent::Acknowledged { sequence: number },
                } if *who == player() && *number == sequence
            )
        };
        self.told.iter().filter(told).count()
    }

    /// The number of the last message of `edge` the borrower has applied.
    fn applied_of(&self, edge: &TestEdge) -> u64 {
        self.west.region().edge(edge.edge).unwrap().applied
    }

    fn applied(&self) -> u64 {
        self.applied_of(&self.edge)
    }

    /// Whether the borrower holds both chunks across by the lend.
    fn lent(&self) -> bool {
        let region = self.west.region();
        !region.pins(ACROSS)
            && region.knowledge(ACROSS) == Knowledge::Held
            && region.knowledge(ACROSS_NORTH) == Knowledge::Held
    }

    /// Has the builder place the lever at [`ASKS`], and waits until the lend it asks
    /// for is granted and taken. The test holds the loads back if the chunks across
    /// are not to be loaded yet.
    fn lend(&mut self) {
        self.click(BlockPos::new(15, -61, 0), Face::Top);
        self.until("the lever that asks", |line| line.lever_at(ASKS));
        self.until("the lend", Self::lent);
    }

    /// Steps both runners [`HELD_FOR`] times while the borrower's loads are held back,
    /// and checks after each step that the lent chunk is still not loaded and that
    /// `still` holds.
    fn while_held(&mut self, mut still: impl FnMut(&Self)) {
        for _ in 0..HELD_FOR {
            self.step();
            assert!(self.west.region().chunk(ACROSS).is_none());
            still(self);
        }
    }
}

/// Has the builder place the lever at [`ASKS`] while the gate holds the borrower's
/// grants and loads back, and waits until the store has granted the lend and the gate
/// holds the grant.
fn grant_held(line: &mut Line) {
    line.gate.hold_loads();
    line.gate.hold_claims();
    line.click(BlockPos::new(15, -61, 0), Face::Top);
    line.until("the lever that asks and the grant", |line| {
        line.lever_at(ASKS) && line.gate.kept_claims() >= 1
    });
    assert_eq!(line.west.region().knowledge(ACROSS), Knowledge::Unknown);
}

/// After the tick that took a grant and a click on the chunk granted: checks that the
/// click waits while the load is held back, then that it places its lever once the
/// load comes and is acknowledged once.
fn placed_once_loaded(line: &mut Line, applied: u64, target: BlockPos, sequence: i32) {
    assert!(line.lent(), "the tick took the grant");
    let waits = |line: &Line| {
        assert_eq!(line.applied(), applied, "the click waits");
        assert!(!line.lever_at(target));
        assert_eq!(line.acknowledged(sequence), 0);
    };
    waits(line);
    line.while_held(waits);

    line.gate.release_loads();
    line.until("the lever across the line", |line| line.lever_at(target));
    assert_eq!(line.applied(), applied + 1);
    line.until("the acknowledgement", |line| {
        line.acknowledged(sequence) > 0
    });
    for _ in 0..HELD_FOR {
        line.step();
    }
    assert_eq!(line.acknowledged(sequence), 1);
}

/// The case the rule was made for: a click on a block of a chunk that comes in the
/// same tick as the store's grant of the chunk by a lend (a `Claimed` the coming tick
/// takes) waits for the chunk's load, then places its lever there and is acknowledged
/// once. The gate holds the grant back until the click is on the link and lets it go
/// just before the step, so that the `Claimed` reaches the runner within the step whose
/// tick takes both.
#[tokio::test]
async fn a_click_that_comes_with_the_grant_of_a_lent_chunk_places_its_lever_once_it_is_loaded() {
    let mut line = Line::new();
    grant_held(&mut line);
    let applied = line.applied();

    // On the grass across the line: both the clicked block and the one above it are
    // in the chunk granted.
    let target = BlockPos::new(16, -60, 0);
    let sequence = line.click(BlockPos::new(16, -61, 0), Face::Top);
    line.gate.release_claims();
    line.step();
    placed_once_loaded(&mut line, applied, target, sequence);
}

/// The same with the `Claimed` taken from the store before the step, as a runner takes
/// answers while it waits between ticks: the grant is among what the coming tick takes,
/// and the click that is there with it waits.
#[tokio::test]
async fn a_click_that_comes_with_a_grant_taken_between_ticks_places_its_lever_once_it_is_loaded() {
    let mut line = Line::new();
    grant_held(&mut line);
    let applied = line.applied();

    let target = BlockPos::new(16, -60, 0);
    let sequence = line.click(BlockPos::new(16, -61, 0), Face::Top);
    line.gate.release_claims();
    for _ in 0..STEPS {
        assert!(line.west.take_replies());
        if line.gate.kept_claims() == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(line.gate.kept_claims(), 0, "the runner has the grant");
    // The tick takes it, not the runner's taking of the answer.
    assert_eq!(line.west.region().knowledge(ACROSS), Knowledge::Unknown);
    line.step();
    placed_once_loaded(&mut line, applied, target, sequence);
}

/// A click on a block of a chunk the borrower was granted by a lend and has taken, which
/// has not been loaded yet, waits for the load and then places its lever there,
/// acknowledged once.
#[tokio::test]
async fn a_click_on_a_lent_chunk_before_its_load_waits_for_it_and_places_its_lever() {
    let mut line = Line::new();
    line.gate.hold_loads();
    line.lend();
    assert!(line.west.region().chunk(ACROSS).is_none());
    let applied = line.applied();

    let target = BlockPos::new(16, -60, 0);
    let sequence = line.click(BlockPos::new(16, -61, 0), Face::Top);
    line.while_held(|line| {
        assert_eq!(line.applied(), applied, "the click waits");
        assert!(!line.lever_at(target));
        assert_eq!(line.acknowledged(sequence), 0);
    });

    line.gate.release_loads();
    line.until("the lever across the line", |line| line.lever_at(target));
    assert_eq!(line.applied(), applied + 1);
    line.until("the acknowledgement", |line| {
        line.acknowledged(sequence) > 0
    });
    for _ in 0..HELD_FOR {
        line.step();
    }
    assert_eq!(line.acknowledged(sequence), 1);
}

/// For a click the chunk that counts is also that of the block beside the clicked face:
/// a lever put against the east face of a stone of the borrower's own, loaded land goes
/// into the lent chunk, and so waits for that chunk's load before it is placed.
#[tokio::test]
async fn a_click_whose_face_is_towards_a_lent_chunk_before_its_load_waits_for_it() {
    let mut line = Line::new();
    line.gate.hold_loads();
    line.lend();
    let applied = line.applied();

    let target = BlockPos::new(16, -60, 2);
    assert_eq!(target.chunk(), ACROSS);
    assert!(
        line.block(STONE_AT).is_some(),
        "the stone's chunk is loaded"
    );
    let sequence = line.click(STONE_AT, Face::East);
    line.while_held(|line| {
        assert_eq!(line.applied(), applied, "the click waits");
        assert!(!line.lever_at(target));
        assert_eq!(line.acknowledged(sequence), 0);
    });

    line.gate.release_loads();
    line.until("the lever on the stone", |line| line.lever_at(target));
    assert_eq!(line.applied(), applied + 1);
    line.until("the acknowledgement", |line| {
        line.acknowledged(sequence) > 0
    });
    for _ in 0..HELD_FOR {
        line.step();
    }
    assert_eq!(line.acknowledged(sequence), 1);
}

/// What another region's player does through the edge (an `EdgeToWorker::Remote`) to a
/// block of a lent chunk that has not been loaded waits like a player's own action, and
/// is done once when the chunk is there.
#[tokio::test]
async fn a_remote_action_on_a_lent_chunk_before_its_load_waits_for_it_and_is_done_once() {
    let mut line = Line::new();
    let (other, other_end) = in_process(4096);
    line.west.links().attach(other_end);
    line.other = Some(other);
    line.gate.hold_loads();
    line.lend();
    let other = line.other.take().expect("attached above");
    let applied = line.applied_of(&other);

    let target = BlockPos::new(16, -60, 1);
    let action = RemoteAction {
        player: other_player(),
        entity: EntityId(4040),
        sequence: 5,
        step: RemoteStep::Place {
            target,
            block: floor_lever(),
            placer: Vec3::new(17.5, -60.0, 4.5),
        },
    };
    other.try_send(EdgeToWorker::Remote(action)).unwrap();
    line.other = Some(other);
    let done = |told: &[WorkerToEdge]| {
        let done = |message: &&WorkerToEdge| {
            matches!(
                message,
                WorkerToEdge::Outbox {
                    entry: Durable::RemoteDone { player, sequence: 5, .. },
                    ..
                } if *player == other_player()
            )
        };
        told.iter().filter(done).count()
    };
    line.while_held(|line| {
        let other = line.other.as_ref().expect("attached above");
        assert_eq!(line.applied_of(other), applied, "the action waits");
        assert!(!line.lever_at(target));
        assert_eq!(done(&line.other_told), 0);
    });

    line.gate.release_loads();
    line.until("the lever across the line", |line| line.lever_at(target));
    let other = line.other.as_ref().expect("attached above");
    assert_eq!(line.applied_of(other), applied + 1);
    line.until("the action done", |line| done(&line.other_told) > 0);
    for _ in 0..HELD_FOR {
        line.step();
    }
    assert_eq!(done(&line.other_told), 1);
}

/// The order of a link holds: what the builder's link sent behind a click that waits
/// for a lent chunk's load, here a click on the borrower's own loaded land, is not
/// taken before that click.
#[tokio::test]
async fn what_a_link_sent_behind_a_click_that_waits_for_a_lent_chunk_is_not_taken_before_it() {
    let mut line = Line::new();
    line.gate.hold_loads();
    line.lend();
    let applied = line.applied();

    let across = BlockPos::new(16, -60, 0);
    // Four blocks from the line: a lever there reaches nothing across it.
    let behind = BlockPos::new(12, -60, 2);
    line.click(BlockPos::new(16, -61, 0), Face::Top);
    let second = line.click(BlockPos::new(12, -61, 2), Face::Top);
    line.while_held(|line| {
        assert_eq!(line.applied(), applied, "both wait");
        assert!(!line.lever_at(behind), "the click behind waits too");
        assert!(!line.lever_at(across));
    });

    line.gate.release_loads();
    for _ in 0..STEPS {
        line.step();
        if line.lever_at(behind) {
            break;
        }
        assert_eq!(line.acknowledged(second), 0);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        line.lever_at(behind),
        "the click behind was taken in the end"
    );
    assert!(line.lever_at(across), "the first click was taken before it");
    assert_eq!(line.applied(), applied + 2);
}

/// A click on a chunk the borrower holds by a lend and has loaded is taken in the tick
/// that takes it: nothing waits.
#[tokio::test]
async fn a_click_on_a_lent_chunk_that_is_loaded_is_taken_at_once() {
    let mut line = Line::new();
    line.lend();
    line.until("the lent chunk's load", |line| {
        line.west.region().chunk(ACROSS).is_some()
    });
    let applied = line.applied();

    let target = BlockPos::new(17, -60, 0);
    let sequence = line.click(BlockPos::new(17, -61, 0), Face::Top);
    line.step();
    assert!(line.lever_at(target));
    assert_eq!(line.applied(), applied + 1);
    assert!(line.west.links[&LinkId(0)].held.is_empty());
    assert_eq!(line.acknowledged(sequence), 1);
}

/// The hold is narrow: while a lent chunk's load is still to come, a click on a chunk
/// of the lender's that is not lent to the borrower (the row south of what the lever
/// reaches) does not wait for anything. It is passed on in the tick that takes it, as
/// before, as an entry of the builder's edge; and so is another region's player's
/// action on that chunk, which another edge passes on.
#[tokio::test]
async fn a_click_on_a_chunk_of_another_region_that_is_not_lent_here_does_not_wait() {
    let mut line = Line::new();
    let (other, other_end) = in_process(4096);
    line.west.links().attach(other_end);
    line.gate.hold_loads();
    line.lend();
    let not_lent = ChunkPos::new(1, 1);
    assert_ne!(line.west.region().knowledge(not_lent), Knowledge::Held);
    // Near the corner of the chunk at the origin, within reach of the chunk south of
    // the one across.
    line.send(PlayerInput::Move {
        position: Some(Vec3::new(14.5, -60.0, 14.5)),
        rotation: None,
        on_ground: true,
    });
    line.until("the builder at the corner", |line| {
        line.west
            .region()
            .player(player())
            .is_some_and(|(_, pose)| pose.position.z == 14.5)
    });
    assert!(line.west.region().chunk(ACROSS).is_none());
    let applied = line.applied();
    let entries = |line: &Line, edge: &TestEdge| -> Vec<Durable> {
        let state = line.west.region().edge(edge.edge).unwrap();
        state.outbox.values().cloned().collect()
    };
    let passed_on =
        |entry: &Durable| matches!(entry, Durable::Remote { .. } | Durable::NotMine { .. });
    let before = entries(&line, &line.edge);

    let clicked = BlockPos::new(16, -61, 17);
    assert_eq!(clicked.chunk(), not_lent);
    line.click(clicked, Face::Top);
    line.step();
    assert_eq!(line.applied(), applied + 1, "the click is taken at once");
    let after = entries(&line, &line.edge);
    assert_eq!(after.len(), before.len() + 1, "{after:?}");
    let entry = after.last().expect("one more");
    assert!(passed_on(entry), "{entry:?}");
    assert!(!line.lever_at(BlockPos::new(16, -60, 17)));

    let applied = line.applied_of(&other);
    let before = entries(&line, &other);
    let action = RemoteAction {
        player: other_player(),
        entity: EntityId(4040),
        sequence: 9,
        step: RemoteStep::Break { position: clicked },
    };
    other.try_send(EdgeToWorker::Remote(action)).unwrap();
    line.step();
    assert_eq!(
        line.applied_of(&other),
        applied + 1,
        "the action is taken at once"
    );
    let after = entries(&line, &other);
    assert_eq!(after.len(), before.len() + 1, "{after:?}");
    let entry = after.last().expect("one more");
    assert!(passed_on(entry), "{entry:?}");
    assert!(line.west.links.values().all(|link| link.held.is_empty()));
}
