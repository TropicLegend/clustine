use clustine_rpc::link::WorkerEnd;
use clustine_sim::api::{RemoteAction, RemoteStep};
use clustine_world::Vec3;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

#[test]
fn a_client_s_entity_id_names_the_entity_it_was_given_for_until_the_edge_forgets_it() {
    let mut wire = WireIds::default();
    let (first, second) = (EntityId(5 << 32), EntityId(3));
    let (one, two) = (wire.of(first), wire.of(second));
    assert_eq!(
        (wire.key_of(one), wire.key_of(two), wire.key_of(two + 1)),
        (Some(first), Some(second), None)
    );
    wire.forget(first);
    assert_eq!(wire.key_of(one), None);
    // An id is not given again: the entity shown anew has another.
    assert_ne!(wire.of(first), one);
}

/// The plan, section 19.5 item 10: an entity of several parts is given its parts'
/// ids right after its own, as the game numbers a dragon's, and a client's hit on a
/// part's id names the entity and the part, until the edge forgets the entity.
#[test]
fn an_entity_of_parts_has_its_parts_ids_after_its_own_and_a_hit_on_one_names_the_part() {
    let mut wire = WireIds::default();
    let (dragon, after) = (EntityId(9), EntityId(10));
    let id = wire.run(dragon, 8);
    assert_eq!(wire.run(dragon, 8), id, "given once");
    assert_eq!(wire.part_of(id), Some((dragon, None)));
    assert_eq!(wire.part_of(id + 1), Some((dragon, Some(0))));
    assert_eq!(wire.part_of(id + 8), Some((dragon, Some(7))));
    assert_eq!(
        wire.of(after),
        id + 9,
        "the next entity comes after the parts"
    );
    wire.forget(dragon);
    assert_eq!(wire.part_of(id + 3), None);
}

use super::*;
use crate::Relinks;

/// The flat world's spawn, which these tests' edges tell their clients.
const FLAT_SPAWN: BlockPos = BlockPos::new(0, -60, 0);

/// How long a test waits for something that has to happen.
const SOON: Duration = Duration::from_secs(10);

/// The world of these tests: a western region up to the chunks with x = 4, where
/// players enter the world, and an eastern one from there.
const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);

const IDENTITY: EdgeIdentity = EdgeIdentity {
    edge: clustine_world::EdgeId(7),
    start: 100,
};

/// A fan-out task with the regions played by the test.
struct Harness {
    commands: mpsc::Sender<Command>,
    relinks: Relinks,
    /// The workers' ends of the links to the two regions.
    regions: Vec<WorkerEnd>,
    task: JoinHandle<Stopped>,
    sessions: u64,
    /// The session of each player's latest connection, which is the attempt of
    /// its join.
    attempts: BTreeMap<PlayerId, u64>,
    /// The number of the last outbox entry each region has made: the two the edge
    /// starts with, and one that a split makes.
    outbox: [u64; 3],
}

/// A link to `region` whose owner has `epoch`, and the worker's end of it.
fn link_to(region: RegionId, epoch: u64) -> (RegionLink, WorkerEnd) {
    let (end, worker) = link::in_process(1024);
    (
        RegionLink {
            dimension: Default::default(),
            region,
            epoch,
            end,
        },
        worker,
    )
}

impl Harness {
    /// An edge linked to both regions, each of which has been said hello to and has
    /// answered that it does not know the edge, as at the start of everything.
    async fn start() -> Self {
        Self::start_with_patience(Duration::from_secs(3600)).await
    }

    async fn start_with_patience(region_patience: Duration) -> Self {
        let (west, west_end) = link_to(WEST, 1);
        let (east, east_end) = link_to(EAST, 1);
        // Players enter the world in the west.
        let (routing, relinks) = Routing::new(WEST, FLAT_SPAWN, IDENTITY, vec![west, east]);
        let config = FanoutConfig {
            max_players: 20,
            view_distance: 2,
            online: Arc::default(),
            region_patience,
            login: crate::LoginWorld::FLAT,
            light: LightSettings::AT_ONCE,
            wire_ledger: WireLedger::default(),
        };
        let (commands, receiver) = mpsc::channel(64);
        // A job is done in the turn that dispatches it, so that everything a turn
        // marks is lit before the turn ends.
        let task = tokio::spawn(Fanout::new(config, routing, receiver, Lighting::here()).run());
        let mut harness = Self {
            commands,
            relinks,
            regions: vec![west_end, east_end],
            task,
            sessions: 0,
            attempts: BTreeMap::new(),
            outbox: [0; 3],
        };
        for region in [WEST, EAST] {
            let hello = harness.next(region).await;
            assert!(
                matches!(
                    &hello.body,
                    EdgeToWorker::Hello { seen: 0, players, chunks, .. }
                        if players.is_empty() && chunks.is_empty()
                ),
                "{hello:?}"
            );
            harness.tell(
                region,
                WorkerToEdge::Welcome(Welcome::Unknown {
                    since: 1,
                    entries: 0,
                    presences: 0,
                    applied: 0,
                }),
            );
        }
        harness
    }

    /// What the edge sends `region` next.
    async fn next(&mut self, region: RegionId) -> EdgeMessage {
        timeout(SOON, self.regions[region.0 as usize].recv())
            .await
            .expect("the edge sent nothing")
            .expect("the edge closed the link")
    }

    /// What the edge sends `region` next that is numbered, with its number.
    async fn next_numbered(&mut self, region: RegionId) -> (u64, EdgeToWorker) {
        loop {
            let EdgeMessage { number, body } = self.next(region).await;
            if let Some(number) = number {
                return (number, body);
            }
        }
    }

    /// Says something as `region`.
    fn tell(&self, region: RegionId, message: WorkerToEdge) {
        self.regions[region.0 as usize].try_send(message).unwrap();
    }

    /// Says `entry` as the next entry of the outbox of `region`. Returns its number.
    fn say(&mut self, region: RegionId, entry: Durable) -> u64 {
        let number = &mut self.outbox[region.0 as usize];
        *number += 1;
        let number = *number;
        self.tell(region, WorkerToEdge::Outbox { number, entry });
        number
    }

    /// Waits until the edge has handled everything `region` has said so far. The
    /// edge takes what regions say, what players do and new links from different
    /// queues, in no order between them, so a test that depends on the edge having
    /// heard something makes sure of it with this.
    async fn settle(&mut self, region: RegionId) {
        // An entry about nobody, which the edge confirms like any other.
        let nobody = Durable::RemoteDone {
            player: player(u128::MAX),
            entity: EntityId(0),
            sequence: 0,
            shown: Vec::new(),
        };
        let number = self.say(region, nobody);
        loop {
            let message = self.next(region).await;
            if message.body == (EdgeToWorker::Confirm { number }) {
                return;
            }
        }
    }

    /// The word of a region that it has placed `player` as `entity`, in answer to
    /// the join of the player's latest connection.
    fn spawned(&self, player: PlayerId, entity: EntityId) -> WorkerToEdge {
        let attempt = self.attempts.get(&player).copied().unwrap_or_default();
        spawned(attempt, player, entity)
    }

    /// A player connects. Returns what their connection is sent.
    async fn join(&mut self, player: PlayerId) -> mpsc::Receiver<Bytes> {
        self.sessions += 1;
        self.attempts.insert(player, self.sessions);
        let (outbound, packets) = mpsc::channel(4096);
        let join = Command::Join {
            appearance: Default::default(),
            session: SessionId(self.sessions),
            profile: Profile {
                uuid: player.0,
                name: format!("Player{}", self.sessions),
            },
            requested_view_distance: None,
            outbound,
            awaiting_teleport: Arc::new(AtomicI32::new(NO_TELEPORT)),
        };
        self.commands.send(join).await.unwrap();
        packets
    }

    /// A player joins and the western region places them as `entity`. Everything
    /// the edge sends the region for that has been received when this returns.
    async fn joined(&mut self, player: PlayerId, entity: EntityId) -> mpsc::Receiver<Bytes> {
        let packets = self.join(player).await;
        let (_, join) = self.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        self.tell(WEST, self.spawned(player, entity));
        self.settle(WEST).await;
        packets
    }

    /// Something the player with the latest session did.
    async fn input(&mut self, player: PlayerId, input: PlayerInput) {
        let command = Command::Input {
            session: SessionId(self.sessions),
            player,
            input,
        };
        self.commands.send(command).await.unwrap();
    }

    /// Replaces the edge's link to `region` and returns the hello the edge says on
    /// the new one.
    async fn relink(&mut self, region: RegionId, epoch: u64) -> EdgeToWorker {
        let (link, worker) = link_to(region, epoch);
        self.regions[region.0 as usize] = worker;
        assert!(self.relinks.replace(link).await);
        self.next(region).await.body
    }

    /// Gives the edge its first link to the region a split made, and returns the
    /// hello it says there.
    async fn link_part(&mut self) -> EdgeToWorker {
        assert_eq!(self.regions.len(), PART.0 as usize);
        let (link, worker) = link_to(PART, 1);
        self.regions.push(worker);
        assert!(self.relinks.replace(link).await);
        self.next(PART).await.body
    }

    /// What the edge sends `region` up to its next numbered message: the
    /// messages without a number, and then that one with its number.
    async fn up_to_numbered(
        &mut self,
        region: RegionId,
    ) -> (Vec<EdgeToWorker>, (u64, EdgeToWorker)) {
        let mut before = Vec::new();
        loop {
            let EdgeMessage { number, body } = self.next(region).await;
            match number {
                Some(number) => return (before, (number, body)),
                None => before.push(body),
            }
        }
    }
}

/// The region a split of these tests makes.
const PART: RegionId = RegionId(2);

fn player(number: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(number))
}

/// The word of a region that it has placed `player` as `entity`, in answer to the
/// join with `attempt`.
fn spawned(attempt: u64, player: PlayerId, entity: EntityId) -> WorkerToEdge {
    WorkerToEdge::ToPlayer {
        player,
        event: PlayerEvent::Spawned {
            last_death: None,
            mode: Default::default(),
            riding: None,
            attempt,
            entity_id: entity,
            pose: Pose::at(Vec3::new(0.5, -60.0, 0.5)),
            flying: false,
            inventory: Inventory::default(),
            selected_slot: 0,
        },
    }
}

fn step(x: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, -60.0, 0.5)),
        rotation: None,
        on_ground: true,
    }
}

/// The player as the western region lets them go, standing in the eastern one.
fn transfer(entity: EntityId, last_input: u64) -> PlayerTransfer {
    PlayerTransfer {
        digging: Default::default(),
        appearance: Default::default(),
        posture: Default::default(),
        forwards: 0,
        health: 20.0,
        dead: false,
        dimension: Default::default(),
        travel: None,
        seen_credits: false,
        portal: Default::default(),
        unit: None,
        sleeping: None,
        respawn: None,
        last_death: None,
        mode: Default::default(),
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id: entity,
        name: "Player1".to_owned(),
        pose: Pose::at(Vec3::new(64.5, -60.0, 0.5)),
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        last_input,
        hops: 0,
        flying: false,
        attempt: None,
        riding: None,
        keys: Default::default(),
        kept: Vec::new(),
        raid_omen_position: None,
        crafting: Default::default(),
        vitals: Default::default(),
    }
}

fn present(entity: EntityId, last_input: u64) -> Presence {
    Presence::Present {
        riding: None,
        entity,
        pose: Pose::at(Vec3::new(0.5, -60.0, 0.5)),
        inventory: Inventory::default(),
        selected_slot: 0,
        last_input,
        handled: None,
        flying: false,
        attempt: None,
        last_death: None,
        mode: Default::default(),
    }
}

/// A region's answer that it has the stay of the join with `attempt`, as
/// `entity`, of which it has applied no input yet.
fn present_of_join(entity: EntityId, attempt: u64) -> Presence {
    let Presence::Present {
        entity,
        pose,
        inventory,
        selected_slot,
        last_input,
        handled,
        flying,
        ..
    } = present(entity, 0)
    else {
        unreachable!("`present` makes a `Present`");
    };
    Presence::Present {
        riding: None,
        entity,
        pose,
        inventory,
        selected_slot,
        last_input,
        handled,
        flying,
        attempt: Some(attempt),
        last_death: None,
        mode: Default::default(),
    }
}

/// Waits until the player's connection is ended by the edge.
async fn disconnected(packets: &mut mpsc::Receiver<Bytes>) {
    let ended = async { while packets.recv().await.is_some() {} };
    timeout(SOON, ended)
        .await
        .expect("the player was not disconnected");
}

/// Whether the edge still serves the player: their queue is open.
fn connected(packets: &mut mpsc::Receiver<Bytes>) -> bool {
    loop {
        match packets.try_recv() {
            Ok(_) => {}
            Err(mpsc::error::TryRecvError::Empty) => return true,
            Err(mpsc::error::TryRecvError::Disconnected) => return false,
        }
    }
}

/// A region takes the subscription messages of a link only in ascending order of
/// their numbers, which the edge counts per link, from 1. Everything a player sees
/// is asked of the player's region; another region is asked as a guest when the
/// player's region names it.
#[tokio::test]
async fn subscription_messages_are_numbered_from_one_on_each_link() {
    /// What the edge says next to `region` about its subscriptions.
    async fn next_asked(edge: &mut Harness, region: RegionId) -> EdgeToWorker {
        loop {
            let body = edge.next(region).await.body;
            if let EdgeToWorker::Subscribe { .. }
            | EdgeToWorker::SubscribeAsGuest { .. }
            | EdgeToWorker::Unsubscribe { .. } = body
            {
                return body;
            }
        }
    }
    // The region says that the player has walked a chunk east, to `x`.
    let walked = |x: f64| WorkerToEdge::TickDelta {
        tick: 1,
        events: vec![RegionEvent::EntityMoved {
            entity: EntityId(5),
            pose: Pose::at(Vec3::new(x, -60.0, 0.5)),
            previous_chunk: ChunkPos::containing(x - 16.0, 0.5),
        }],
    };

    // Entering the world makes the edge ask the player's region for the chunks
    // around.
    let mut edge = Harness::start().await;
    let _packets = edge.join(player(1)).await;
    // Only once the edge has taken the join can it be told where the player is.
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    let asked = next_asked(&mut edge, WEST).await;
    assert!(
        matches!(asked, EdgeToWorker::Subscribe { ask: 1, .. }),
        "{asked:?}"
    );

    // A column of chunks leaves the view and another comes into it: both are said
    // to the player's region, whoever holds the chunks.
    edge.tell(WEST, walked(16.5));
    let asked = next_asked(&mut edge, WEST).await;
    assert!(
        matches!(asked, EdgeToWorker::Unsubscribe { ask: 2, .. }),
        "{asked:?}"
    );
    let asked = next_asked(&mut edge, WEST).await;
    let EdgeToWorker::Subscribe { ask: 3, chunks } = asked else {
        panic!("{asked:?}");
    };
    // The region names the eastern one as the holder of one of them: the edge asks
    // there as a guest, with that link's own count.
    let chunk = chunks[0];
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk,
        ask: 3,
        region: EAST,
    };
    edge.tell(WEST, elsewhere);
    let asked = next_asked(&mut edge, EAST).await;
    assert_eq!(
        asked,
        EdgeToWorker::SubscribeAsGuest {
            ask: 1,
            chunks: vec![chunk]
        }
    );

    // On a new link the count begins anew; on the other it goes on.
    let hello = edge.relink(WEST, 2).await;
    let EdgeToWorker::Hello {
        chunks: named,
        guests,
        ..
    } = hello
    else {
        panic!("{hello:?}");
    };
    // The hello names what the player sees, the chunk held elsewhere among it.
    assert!(
        named.contains(&chunk) && guests.is_empty(),
        "{named:?} {guests:?}"
    );
    edge.tell(WEST, walked(32.5));
    let asked = next_asked(&mut edge, WEST).await;
    assert!(
        matches!(asked, EdgeToWorker::Unsubscribe { ask: 1, .. }),
        "{asked:?}"
    );
    let asked = next_asked(&mut edge, WEST).await;
    let EdgeToWorker::Subscribe { ask: 2, chunks } = asked else {
        panic!("{asked:?}");
    };
    let next = chunks[0];
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk: next,
        ask: 2,
        region: EAST,
    };
    edge.tell(WEST, elsewhere);
    let asked = next_asked(&mut edge, EAST).await;
    assert_eq!(
        asked,
        EdgeToWorker::SubscribeAsGuest {
            ask: 2,
            chunks: vec![next]
        }
    );
}

/// The inputs `region` is sent next, as many as `count`, by number.
async fn next_inputs(
    edge: &mut Harness,
    region: RegionId,
    count: usize,
) -> Vec<(u64, PlayerInput)> {
    let mut inputs = Vec::new();
    while inputs.len() < count {
        if let (_, EdgeToWorker::Input { number, input, .. }) = edge.next_numbered(region).await {
            inputs.push((number, input));
        }
    }
    inputs
}

/// `handleSetCreativeModeSlot @16` and `handlePlayerAbilities @13-44`: of a survival
/// player the edge passes on no creative slot or throw, and flying as not flying.
/// The client's `player_loaded` goes on as an input.
#[tokio::test]
async fn a_survival_player_s_creative_slots_are_dropped_and_their_flying_passed_on_as_none() {
    let mut edge = Harness::start().await;
    let _packets = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    let mut spawned = edge.spawned(player(1), EntityId(5));
    if let WorkerToEdge::ToPlayer {
        event: PlayerEvent::Spawned { mode, .. },
        ..
    } = &mut spawned
    {
        *mode = clustine_world::PlayerMode::new(clustine_world::GameMode::Survival);
    }
    edge.tell(WEST, spawned);
    edge.settle(WEST).await;

    let stone = clustine_world::ItemStack::new(1, 1);
    let slot = PlayerInput::SetInventorySlot {
        slot: 36,
        stack: Some(stone.clone()),
    };
    let throw = PlayerInput::Throw { stack: stone };
    for input in [
        slot,
        throw,
        PlayerInput::SetFlying { flying: true },
        PlayerInput::Loaded,
    ] {
        edge.input(player(1), input).await;
    }
    assert_eq!(
        next_inputs(&mut edge, WEST, 2).await,
        [
            (1, PlayerInput::SetFlying { flying: false }),
            (2, PlayerInput::Loaded)
        ]
    );
}

/// Of a creative player the same are passed on as said.
#[tokio::test]
async fn a_creative_player_s_creative_slots_and_flying_are_passed_on() {
    let mut edge = Harness::start().await;
    let _packets = edge.joined(player(1), EntityId(5)).await;
    let slot = PlayerInput::SetInventorySlot {
        slot: 36,
        stack: Some(clustine_world::ItemStack::new(1, 1)),
    };
    let fly = PlayerInput::SetFlying { flying: true };
    edge.input(player(1), slot.clone()).await;
    edge.input(player(1), fly.clone()).await;
    assert_eq!(next_inputs(&mut edge, WEST, 2).await, [(1, slot), (2, fly)]);
}

#[tokio::test]
async fn what_changes_a_region_is_numbered_from_one_per_region() {
    let mut edge = Harness::start().await;
    let _packets = edge.joined(player(1), EntityId(5)).await;

    edge.input(player(1), step(1.5)).await;
    let (number, body) = edge.next_numbered(WEST).await;
    assert_eq!(number, 2);
    assert!(
        matches!(body, EdgeToWorker::Input { number: 1, .. }),
        "{body:?}"
    );

    // The eastern region has been sent nothing numbered yet, so its first is 1.
    let departed = Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 1),
        to: EAST,
    };
    edge.say(WEST, departed);
    let (number, body) = edge.next_numbered(EAST).await;
    assert_eq!(number, 1);
    assert!(
        matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
}

/// The heart of resuming: the players stay, the hello says what the edge believes
/// and has seen, and what the region has not reported applied is sent again with the
/// numbers it had, once the region has answered.
#[tokio::test]
async fn a_lost_link_keeps_the_players_and_a_new_one_resumes() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    // The join is applied and durable; two inputs follow that are not yet.
    edge.tell(
        WEST,
        WorkerToEdge::Progress {
            applied: 1,
            inputs: Vec::new(),
        },
    );
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    edge.input(player(1), step(2.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
    assert_eq!(edge.next_numbered(WEST).await.0, 3);

    // The worker dies. An input made meanwhile is kept like the others.
    let hello = {
        let (link, worker) = link_to(WEST, 2);
        drop(std::mem::replace(&mut edge.regions[0], worker));
        edge.input(player(1), step(3.5)).await;
        assert!(edge.relinks.replace(link).await);
        edge.next(WEST).await.body
    };
    let EdgeToWorker::Hello {
        edge: id,
        start,
        since,
        seen,
        players,
        chunks,
        guests,
    } = hello
    else {
        panic!("expected a hello, got {hello:?}");
    };
    // What it has seen of the region's outbox are the entries that settled it.
    assert_eq!(
        (id, start, seen),
        (IDENTITY.edge, IDENTITY.start, edge.outbox[0])
    );
    // And it says since when the region knows it, as the region's welcome told it.
    assert_eq!(since, 1);
    assert_eq!(players, [player(1)]);
    // Every chunk the region's player sees, whoever serves it: the region has not
    // said of any that another holds it. And the edge is a guest nowhere.
    let seen: Vec<_> = view_area(ChunkPos::new(0, 0), 2).into_iter().collect();
    assert_eq!(chunks, seen);
    assert!(guests.is_empty());
    assert!(connected(&mut packets));

    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 1,
        }),
    );
    for expected in [2, 3, 4] {
        let (number, body) = edge.next_numbered(WEST).await;
        assert_eq!(number, expected);
        assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
    }
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: present(EntityId(5), 0),
        },
    );

    // What comes next carries on from there, and the player has stayed throughout.
    edge.input(player(1), step(4.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 5);
    assert!(connected(&mut packets));
}

#[tokio::test]
async fn what_a_region_reported_applied_is_not_sent_again() {
    let mut edge = Harness::start().await;
    let _packets = edge.joined(player(1), EntityId(5)).await;
    edge.input(player(1), step(1.5)).await;
    edge.input(player(1), step(2.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    let progress = WorkerToEdge::Progress {
        applied: 2,
        inputs: vec![(player(1), 1)],
    };
    edge.tell(WEST, progress);
    edge.settle(WEST).await;

    edge.input(player(1), step(3.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 4);
    let hello = edge.relink(WEST, 2).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 2,
        }),
    );
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    assert_eq!(edge.next_numbered(WEST).await.0, 4);
}

/// A region that has forgotten the edge expects its messages numbered from 1. The
/// edge must not send what it kept with the old numbers, which would close the
/// link, and the players it believed to be there are not.
#[tokio::test]
async fn a_region_that_forgot_the_edge_gets_nothing_that_was_kept() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    edge.tell(
        WEST,
        WorkerToEdge::Progress {
            applied: 1,
            inputs: Vec::new(),
        },
    );
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    let hello = edge.relink(WEST, 2).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Unknown {
            since: 1,
            entries: 0,
            presences: 1,
            applied: 0,
        }),
    );
    disconnected(&mut packets).await;

    // All the region hears is that the player is gone, numbered from 1 again.
    let (number, body) = edge.next_numbered(WEST).await;
    assert_eq!(number, 1);
    assert!(matches!(body, EdgeToWorker::PlayerLeave { .. }), "{body:?}");
    // And a later hello claims nothing of the past.
    let hello = edge.relink(WEST, 3).await;
    assert!(
        matches!(&hello, EdgeToWorker::Hello { seen: 0, players, .. } if players.is_empty()),
        "{hello:?}"
    );
}

/// An entry of a region's outbox is handled once, however often it is sent, and
/// confirmed after what it led to is kept for the region it concerns.
#[tokio::test]
async fn an_outbox_entry_is_handled_once_and_confirmed() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    let departed = || Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 0),
        to: EAST,
    };

    let number = edge.say(WEST, departed());
    let (arrival, body) = edge.next_numbered(EAST).await;
    assert_eq!(arrival, 1);
    assert!(
        matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
    let confirm = loop {
        let message = edge.next(WEST).await;
        if matches!(message.body, EdgeToWorker::Confirm { .. }) {
            break message;
        }
    };
    assert_eq!(
        confirm,
        EdgeMessage::unnumbered(EdgeToWorker::Confirm { number })
    );

    // Sent again, as after a lost confirmation: nobody arrives a second time. The
    // next numbered message the east gets is the player's next input.
    edge.tell(
        WEST,
        WorkerToEdge::Outbox {
            number,
            entry: departed(),
        },
    );
    edge.settle(WEST).await;
    edge.input(player(1), step(65.5)).await;
    let (number, body) = edge.next_numbered(EAST).await;
    assert_eq!(number, 2);
    assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
    assert!(connected(&mut packets));
}

/// The order of a resume matters: the outbox comes before the presence answers, so
/// a player the region let go while the link was down has been handed on by the
/// time the region says that it does not have them.
#[tokio::test]
async fn a_departure_missed_with_the_link_hands_the_player_on_before_presence_is_judged() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    edge.tell(
        WEST,
        WorkerToEdge::Progress {
            applied: 1,
            inputs: Vec::new(),
        },
    );
    edge.settle(WEST).await;
    edge.input(player(1), step(64.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    let hello = edge.relink(WEST, 2).await;
    assert!(
        matches!(&hello, EdgeToWorker::Hello { players, .. } if *players == [player(1)]),
        "{hello:?}"
    );
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 1,
        }),
    );
    let departed = Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 1),
        to: EAST,
    };
    edge.say(WEST, departed);
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );

    let (_, body) = edge.next_numbered(EAST).await;
    assert!(
        matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
    edge.settle(WEST).await;
    assert!(connected(&mut packets));
}

#[tokio::test]
async fn a_player_a_region_does_not_have_is_disconnected_unless_they_are_on_their_way_there() {
    let mut edge = Harness::start().await;
    // The first is in the region as far as the edge knows, and the region has
    // reported the join applied. The second has asked to join, which is still kept.
    let mut settled = edge.joined(player(1), EntityId(5)).await;
    edge.tell(
        WEST,
        WorkerToEdge::Progress {
            applied: 1,
            inputs: Vec::new(),
        },
    );
    edge.settle(WEST).await;
    let mut joining = edge.join(player(2)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    let hello = edge.relink(WEST, 2).await;
    assert!(
        matches!(&hello, EdgeToWorker::Hello { players, .. } if players.len() == 2),
        "{hello:?}"
    );
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 2,
            applied: 1,
        }),
    );
    for absent in [player(1), player(2)] {
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: absent,
                answer: Presence::Absent,
            },
        );
    }
    disconnected(&mut settled).await;
    edge.settle(WEST).await;
    assert!(connected(&mut joining));
}

/// A player the region placed while the link was down is told so on resuming, from
/// what the region says of them.
#[tokio::test]
async fn a_player_placed_unnoticed_enters_the_world_on_resuming() {
    let mut edge = Harness::start().await;
    let mut packets = edge.join(player(1)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 1);

    let hello = edge.relink(WEST, 2).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 1,
        }),
    );
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: present_of_join(EntityId(5), 1),
        },
    );

    // Entering the world makes the edge ask for the chunks around.
    loop {
        let message = edge.next(WEST).await;
        if matches!(message.body, EdgeToWorker::Subscribe { .. }) {
            break;
        }
    }
    assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
    // The spawn the region may still report for them changes nothing.
    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
    assert!(connected(&mut packets));
}

/// A player leaves and joins again while the region has not yet applied their
/// leaving, as when its worker has just died. What the region then says of the
/// player is about who they were; the one who joined again is placed by the join.
#[tokio::test]
async fn what_a_region_says_of_a_player_who_left_and_came_back_since_is_not_taken_for_them() {
    let mut edge = Harness::start().await;
    let _old = edge.joined(player(1), EntityId(5)).await;
    let leave = Command::Leave {
        session: SessionId(edge.sessions),
        player: player(1),
    };
    edge.commands.send(leave).await.unwrap();
    let (number, left) = edge.next_numbered(WEST).await;
    assert_eq!(
        (number, left),
        (
            2,
            EdgeToWorker::PlayerLeave {
                player: player(1),
                entity: Some(EntityId(5)),
                attempt: None,
            }
        )
    );
    let mut packets = edge.join(player(1)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    // The first join is durable; the leaving and the second join are not yet.
    edge.tell(
        WEST,
        WorkerToEdge::Progress {
            applied: 1,
            inputs: Vec::new(),
        },
    );
    edge.settle(WEST).await;

    // The region's owner is replaced by one that has applied neither, and still
    // has the player as they were.
    let hello = edge.relink(WEST, 2).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 1,
        }),
    );
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: present(EntityId(5), 40),
        },
    );
    let (number, left) = edge.next_numbered(WEST).await;
    assert_eq!(
        (number, left),
        (
            2,
            EdgeToWorker::PlayerLeave {
                player: player(1),
                entity: Some(EntityId(5)),
                attempt: None,
            }
        )
    );
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(
        number == 3 && matches!(join, EdgeToWorker::PlayerJoin(_)),
        "{join:?}"
    );
    edge.settle(WEST).await;
    // They have not been put into the world as who they were.
    assert_eq!(
        packets.try_recv().err(),
        Some(mpsc::error::TryRecvError::Empty)
    );

    // The region applies both and places them anew.
    edge.tell(WEST, edge.spawned(player(1), EntityId(6)));
    assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
    edge.tell(
        WEST,
        WorkerToEdge::Progress {
            applied: 3,
            inputs: Vec::new(),
        },
    );
    edge.settle(WEST).await;

    // And it is as that entity that the edge knows them from then on.
    let hello = edge.relink(WEST, 3).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 3,
        }),
    );
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: present(EntityId(6), 0),
        },
    );
    edge.settle(WEST).await;
    assert!(connected(&mut packets));
}

/// Gives `chunk` the light of its blocks, as a generator's chunk comes with it.
fn lit(mut chunk: Chunk) -> Chunk {
    let mut light = clustine_light::light(&clustine_light::Around {
        centre: &chunk,
        neighbours: [None; 8],
        sky: true,
    });
    light.beside = 1;
    chunk.set_light(light);
    chunk
}

/// An empty chunk, as a region sends it in answer to a subscription numbered `ask`.
fn snapshot(position: ChunkPos, ask: u64) -> WorkerToEdge {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == OVERWORLD)
        .expect("the overworld is a dimension");
    WorkerToEdge::ChunkSnapshot {
        position,
        ask,
        tick: 1,
        chunk: lit(Chunk::empty(overworld, clustine_world::Biome(0))),
        entities: Vec::new(),
    }
}

/// What the edge says next to `region` about its subscriptions.
async fn next_asked(edge: &mut Harness, region: RegionId) -> EdgeToWorker {
    loop {
        let body = edge.next(region).await.body;
        if let EdgeToWorker::Subscribe { .. }
        | EdgeToWorker::SubscribeAsGuest { .. }
        | EdgeToWorker::Unsubscribe { .. } = body
        {
            return body;
        }
    }
}

/// A chunk that someone at the spawn point sees, and still sees from the first
/// chunk of the eastern region.
const SHARED: ChunkPos = ChunkPos::new(2, 0);

/// The number under which the edge asked the western region for the view of a
/// player who has just entered the world.
async fn first_asked(edge: &mut Harness) -> u64 {
    let EdgeToWorker::Subscribe { ask, chunks } = next_asked(edge, WEST).await else {
        panic!("the edge did not ask for the view");
    };
    assert!(chunks.contains(&SHARED), "{chunks:?}");
    ask
}

/// The outbox entry with which the western region lets a player go to the eastern.
fn departing_to_east(player: PlayerId, entity: EntityId) -> Durable {
    Durable::Departed {
        player,
        transfer: transfer(entity, 0),
        to: EAST,
    }
}

/// A snapshot answers the subscription it was made for, whatever message has changed
/// the subscription's kind since: a region that has made the snapshot does not
/// answer the change, so an edge that took only an answer with its latest number
/// would wait for ever. This is the edge as a guest at the east, and its player
/// handed there while the snapshot is in flight.
#[tokio::test]
async fn a_snapshot_in_flight_is_taken_when_the_subscriptions_kind_has_changed_since() {
    let mut edge = Harness::start().await;
    let mut packets = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    let ask = first_asked(&mut edge).await;
    let chunk = SHARED;

    // The west says that the east holds the chunk; the edge asks there as a guest.
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk,
        ask,
        region: EAST,
    };
    edge.tell(WEST, elsewhere);
    let asked = next_asked(&mut edge, EAST).await;
    let as_guest = EdgeToWorker::SubscribeAsGuest {
        ask: 1,
        chunks: vec![chunk],
    };
    assert_eq!(asked, as_guest);

    // The player is handed to the east before the east's snapshot is read: the
    // guest's subscription becomes a viewer's, under a later number.
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    let asked = next_asked(&mut edge, EAST).await;
    let EdgeToWorker::Subscribe { ask: later, chunks } = asked else {
        panic!("{asked:?}");
    };
    assert!(later > 1 && chunks.contains(&chunk), "{later} {chunks:?}");

    // The snapshot made for the guest's subscription arrives, and is shown.
    while packets.try_recv().is_ok() {}
    edge.tell(EAST, snapshot(chunk, 1));
    edge.settle(EAST).await;
    assert!(packets.try_recv().is_ok(), "the chunk was not sent on");
}

/// Two players of two regions see one chunk, which the first one's region serves
/// and the second one's region says is held there. When the first player goes, the
/// subscription at their region becomes a guest's instead of ending: it is what
/// the second player's chunk comes from.
#[tokio::test]
async fn a_chunk_another_regions_viewer_still_sees_stays_subscribed_as_a_guests() {
    let mut edge = Harness::start().await;
    // The first enters the world in the west and is handed to the east.
    let _first = edge.joined(player(1), EntityId(5)).await;
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    let asked = next_asked(&mut edge, EAST).await;
    let EdgeToWorker::Subscribe { ask, chunks } = asked else {
        panic!("{asked:?}");
    };
    let chunk = SHARED;
    assert!(chunks.contains(&chunk), "{chunks:?}");
    edge.tell(EAST, snapshot(chunk, ask));
    edge.settle(EAST).await;

    // The second enters in the west and sees the same chunk; the west names the
    // east, where the edge is subscribed already and asks nothing more.
    let mut second = edge.join(player(2)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.tell(WEST, edge.spawned(player(2), EntityId(6)));
    let asked = loop {
        // Past what the hand-over of the first left for the west to hear.
        match next_asked(&mut edge, WEST).await {
            EdgeToWorker::Subscribe { ask, chunks } if chunks.contains(&chunk) => break ask,
            _ => {}
        }
    };
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk,
        ask: asked,
        region: EAST,
    };
    edge.tell(WEST, elsewhere);
    edge.settle(WEST).await;

    // The first leaves. The east is not told to forget the chunk.
    let leave = Command::Leave {
        session: SessionId(1),
        player: player(1),
    };
    edge.commands.send(leave).await.unwrap();
    loop {
        match next_asked(&mut edge, EAST).await {
            EdgeToWorker::SubscribeAsGuest { chunks, .. } if chunks.contains(&chunk) => break,
            EdgeToWorker::Unsubscribe { chunks, .. } => {
                assert!(!chunks.contains(&chunk), "the chunk was given up");
            }
            _ => {}
        }
    }
    assert!(connected(&mut second));
}

/// A region the edge has no link to yet is kept for like any other: what is to go
/// to it waits for its link, and is not dropped.
#[tokio::test]
async fn what_is_sent_to_a_region_without_a_link_yet_is_kept_for_it() {
    let mut edge = Harness::start().await;
    let _packets = edge.joined(player(1), EntityId(5)).await;
    let third = RegionId(2);
    let action = RemoteAction {
        player: player(1),
        entity: EntityId(5),
        sequence: 3,
        step: RemoteStep::Break {
            position: BlockPos::new(640, -61, 0),
        },
    };
    let entry = Durable::Remote {
        action: action.clone(),
        to: Some(third),
    };
    edge.say(WEST, entry);
    edge.settle(WEST).await;

    // The routing table brings the region: after its welcome it is sent what waited.
    let (link, mut worker) = link_to(third, 1);
    assert!(edge.relinks.replace(link).await);
    let hello = timeout(SOON, worker.recv()).await.unwrap().unwrap();
    assert!(
        matches!(hello.body, EdgeToWorker::Hello { .. }),
        "{hello:?}"
    );
    let welcome = Welcome::Unknown {
        since: 1,
        entries: 0,
        presences: 0,
        applied: 0,
    };
    worker.try_send(WorkerToEdge::Welcome(welcome)).unwrap();
    let kept = timeout(SOON, worker.recv()).await.unwrap().unwrap();
    assert_eq!(kept.number, Some(1));
    assert_eq!(kept.body, EdgeToWorker::Remote(action));
}

/// What was kept for a region is sent again only when the outbox entries that the
/// region's welcome announced have been handled: one of them can change what is
/// kept.
#[tokio::test]
async fn what_was_kept_is_sent_after_the_entries_the_welcome_announced() {
    let mut edge = Harness::start().await;
    let _packets = edge.joined(player(1), EntityId(5)).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    let hello = edge.relink(WEST, 2).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 2,
            presences: 1,
            applied: 0,
        }),
    );
    let done = |sequence| Durable::RemoteDone {
        player: player(u128::MAX),
        entity: EntityId(0),
        sequence,
        shown: Vec::new(),
    };
    // The first entry is confirmed, and nothing that was kept has been sent.
    let first = edge.say(WEST, done(1));
    let message = edge.next(WEST).await;
    assert_eq!(message.body, EdgeToWorker::Confirm { number: first });
    // With the second, what was kept follows its confirmation, in order.
    let second = edge.say(WEST, done(2));
    let message = edge.next(WEST).await;
    assert_eq!(message.body, EdgeToWorker::Confirm { number: second });
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(
        number == 1 && matches!(join, EdgeToWorker::PlayerJoin(_)),
        "{join:?}"
    );
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
}

#[tokio::test]
async fn an_edge_that_has_been_superseded_stops() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    let hello = edge.relink(EAST, 2).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(EAST, WorkerToEdge::Welcome(Welcome::Superseded));
    assert_eq!(
        timeout(SOON, &mut edge.task).await.unwrap().unwrap(),
        Stopped::Superseded
    );
    disconnected(&mut packets).await;
}

#[tokio::test]
async fn a_link_to_a_replaced_owner_is_not_taken() {
    let mut edge = Harness::start().await;
    let hello = edge.relink(WEST, 5).await;
    assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Unknown {
            since: 1,
            entries: 0,
            presences: 0,
            applied: 0,
        }),
    );

    // The link of an owner with a lower epoch is dropped, which closes it.
    let (stale, mut worker) = link_to(WEST, 4);
    assert!(edge.relinks.replace(stale).await);
    assert_eq!(timeout(SOON, worker.recv()).await.unwrap(), None);
    // The link the edge has is still the one it uses.
    let _packets = edge.join(player(1)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 1);
}

/// What is left of a player's action on blocks goes to the region the entry names.
/// Where it names none, the region did not know who holds the chunk, and the edge
/// sends it to the region that serves it the chunk; with nobody serving it, the
/// action ends at the edge.
#[tokio::test]
async fn an_action_on_blocks_goes_to_the_region_named_or_to_the_one_that_serves_the_chunk() {
    let mut edge = Harness::start().await;
    let mut packets = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    let ask = first_asked(&mut edge).await;
    // About a block of the chunk both regions' players can see.
    let action = |sequence| RemoteAction {
        player: player(1),
        entity: EntityId(5),
        sequence,
        step: RemoteStep::Break {
            position: BlockPos::new(40, -61, 0),
        },
    };
    let without_a_region = |sequence| Durable::Remote {
        action: action(sequence),
        to: None,
    };

    // The region names the one that takes the next step. The edge is not asking
    // that one for the chunk yet, and does so first, as a guest: a region that
    // holds the chunk and has yet to load it then judges the action only when it
    // has.
    let named = Durable::Remote {
        action: action(3),
        to: Some(EAST),
    };
    edge.say(WEST, named);
    let EdgeMessage { number, body } = edge.next(EAST).await;
    assert!(
        number.is_none()
            && matches!(&body, EdgeToWorker::SubscribeAsGuest { chunks, .. } if chunks == &[SHARED]),
        "{body:?}"
    );
    let (number, body) = edge.next_numbered(EAST).await;
    assert_eq!((number, body), (1, EdgeToWorker::Remote(action(3))));

    // It names none, and no region serves the edge that chunk: the action ends
    // here, and nothing goes to the east (the next message there is numbered 2).
    edge.say(WEST, without_a_region(4));
    edge.settle(WEST).await;

    // The east serves the chunk: the west says so, and the east, which the edge is
    // asking as a guest already, sends it.
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk: SHARED,
        ask,
        region: EAST,
    };
    edge.tell(WEST, elsewhere);
    edge.settle(WEST).await;
    edge.tell(EAST, snapshot(SHARED, 1));
    edge.settle(EAST).await;
    edge.say(WEST, without_a_region(5));
    let (number, body) = edge.next_numbered(EAST).await;
    assert_eq!((number, body), (2, EdgeToWorker::Remote(action(5))));

    // A region that was passed the action and does not hold the chunk names who
    // does. Had that been the west itself, it would have ended at the edge.
    let not_mine = Durable::NotMine {
        what: Misdirected::Remote(action(6)),
        holder: EAST,
    };
    edge.say(WEST, not_mine);
    let (number, body) = edge.next_numbered(EAST).await;
    assert_eq!((number, body), (3, EdgeToWorker::Remote(action(6))));
    assert!(connected(&mut packets));
}

/// A player who is let go to a region that believes another to hold the chunk they
/// stand in is sent on to that one, as if the region had let them go, also back to
/// where they came from. One who is sent from region to region without end is
/// disconnected.
#[tokio::test]
async fn a_player_a_region_sends_on_goes_to_the_region_it_names() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    let (number, body) = edge.next_numbered(EAST).await;
    assert!(
        number == 1 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
    let sends_on = |holder| Durable::NotMine {
        what: Misdirected::Arrival {
            player: player(1),
            transfer: transfer(EntityId(5), 0),
        },
        holder,
    };

    // The east sends them back; the west is sent the arrival, behind the join.
    edge.say(EAST, sends_on(WEST));
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
    assert!(connected(&mut packets));

    // The two go on sending them to each other: after as many times as there are
    // regions, the edge gives up.
    edge.say(WEST, sends_on(EAST));
    let (_, body) = edge.next_numbered(EAST).await;
    assert!(
        matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
    edge.say(EAST, sends_on(WEST));
    disconnected(&mut packets).await;
}

/// A welcome that resumes, with `presences` answers behind it and the region
/// having applied this edge's messages up to `applied`.
fn resumed(presences: u32, applied: u64) -> WorkerToEdge {
    WorkerToEdge::Welcome(Welcome::Resumed {
        entries: 0,
        presences,
        applied,
    })
}

fn says_present(player: PlayerId, entity: EntityId) -> WorkerToEdge {
    WorkerToEdge::Presence {
        player,
        answer: present(entity, 0),
    }
}

/// A region says every stay it has for the edge, also those the hello did not
/// name. One the edge does not have, of a player it has given up or has as another
/// entity, is ended by a leave that names it; the player the edge has is not
/// touched by that.
#[tokio::test]
async fn a_stay_a_region_has_and_the_edge_does_not_is_ended_by_a_leave_that_names_it() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed(2, 1));
    edge.tell(WEST, says_present(player(1), EntityId(5)));
    edge.tell(WEST, says_present(player(2), EntityId(9)));
    let leave = EdgeToWorker::PlayerLeave {
        player: player(2),
        entity: Some(EntityId(9)),
        attempt: None,
    };
    assert_eq!(edge.next_numbered(WEST).await, (2, leave));

    // The region has the player the edge has, as someone they were before.
    edge.relink(WEST, 3).await;
    edge.tell(WEST, resumed(1, 2));
    edge.tell(WEST, says_present(player(1), EntityId(4)));
    let leave = EdgeToWorker::PlayerLeave {
        player: player(1),
        entity: Some(EntityId(4)),
        attempt: None,
    };
    assert_eq!(edge.next_numbered(WEST).await, (3, leave));
    edge.settle(WEST).await;
    assert!(connected(&mut packets));
}

/// A region that says it has a stay the edge has under another region has come by
/// it through a merge or a split. The stay is that region's from then on: it is
/// asked for what the player sees before it is sent anything they did, there is no
/// arrival, and what they do next goes there.
#[tokio::test]
async fn a_stay_the_edge_has_under_another_region_moves_to_the_one_that_says_it_has_it() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    let hello = edge.relink(EAST, 2).await;
    let EdgeToWorker::Hello { players, .. } = hello else {
        panic!("{hello:?}");
    };
    assert_eq!(players, []);
    let welcome = Welcome::Unknown {
        since: 7,
        entries: 0,
        presences: 1,
        applied: 0,
    };
    edge.tell(EAST, WorkerToEdge::Welcome(welcome));
    edge.tell(EAST, says_present(player(1), EntityId(5)));

    // What they see, as a viewer's, and only then what they did.
    let EdgeMessage { number, body } = edge.next(EAST).await;
    let EdgeToWorker::Subscribe { chunks, .. } = &body else {
        panic!("{body:?}");
    };
    assert!(number.is_none() && chunks.contains(&SHARED), "{body:?}");
    let (number, body) = edge.next_numbered(EAST).await;
    let again = EdgeToWorker::Input {
        player: player(1),
        entity: EntityId(5),
        number: 1,
        input: step(1.5),
    };
    assert_eq!((number, body), (1, again));
    // The region they were under is left a guest's subscription for what they
    // still see.
    let left = next_asked(&mut edge, WEST).await;
    assert!(
        matches!(&left, EdgeToWorker::SubscribeAsGuest { chunks, .. } if chunks.contains(&SHARED)),
        "{left:?}"
    );

    edge.input(player(1), step(2.5)).await;
    let (number, body) = edge.next_numbered(EAST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::Input { number: 2, .. }),
        "{body:?}"
    );
    assert!(connected(&mut packets));
}

/// A region's answer about a player who is entering the world is about the stay
/// their join made only if the region had applied the join, which its welcome
/// says. Otherwise it is about who they were before, and the join, sent again,
/// places them.
#[tokio::test]
async fn a_player_who_is_entering_the_world_is_placed_by_an_answer_only_if_their_join_was_applied()
{
    let mut edge = Harness::start().await;
    let mut packets = edge.join(player(1)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 1);
    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed(1, 1));
    let of_join = WorkerToEdge::Presence {
        player: player(1),
        answer: present_of_join(EntityId(5), 1),
    };
    edge.tell(WEST, of_join);
    assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
    // The join is not sent again, and what they do names the stay they were told.
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2
            && matches!(
                body,
                EdgeToWorker::Input {
                    entity: EntityId(5),
                    ..
                }
            ),
        "{number} {body:?}"
    );

    let mut edge = Harness::start().await;
    let mut packets = edge.join(player(1)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 1);
    edge.relink(WEST, 2).await;
    // The region has them from before, by a way that left no leave with the edge.
    edge.tell(WEST, resumed(1, 0));
    edge.tell(WEST, says_present(player(1), EntityId(4)));
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(
        number == 1 && matches!(join, EdgeToWorker::PlayerJoin(_)),
        "{join:?}"
    );
    edge.settle(WEST).await;
    assert_eq!(
        packets.try_recv().err(),
        Some(mpsc::error::TryRecvError::Empty)
    );
    edge.tell(WEST, edge.spawned(player(1), EntityId(6)));
    assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(
            body,
            EdgeToWorker::Input {
                entity: EntityId(6),
                ..
            }
        ),
        "{body:?}"
    );
}

/// What a welcome says the region had applied is not sent to it again.
#[tokio::test]
async fn what_a_welcome_says_was_applied_is_not_sent_again() {
    let mut edge = Harness::start().await;
    let _packets = edge.joined(player(1), EntityId(5)).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed(1, 1));
    edge.tell(WEST, says_present(player(1), EntityId(5)));
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::Input { .. }),
        "{number} {body:?}"
    );
}

/// A welcome that resumes with `entries` outbox entries and `presences` answers
/// behind it, from a region that had applied this edge's messages up to `applied`.
fn resumed_with(entries: u32, presences: u32, applied: u64) -> WorkerToEdge {
    WorkerToEdge::Welcome(Welcome::Resumed {
        entries,
        presences,
        applied,
    })
}

/// The east as absorbed by the region that says this: it knew the edge since the
/// welcome of these tests' start, had applied its messages up to `applied`, and
/// had entries for it under `numbers`, which follow.
fn east_absorbed(applied: u64, numbers: Vec<u64>) -> Durable {
    Durable::Absorbed {
        region: EAST,
        since: 1,
        applied,
        numbers,
    }
}

fn applied_up_to(applied: u64) -> WorkerToEdge {
    WorkerToEdge::Progress {
        applied,
        inputs: Vec::new(),
    }
}

/// A player joins as entity 5, is let go to the east, which applies their arrival,
/// and takes a step there that the east has not applied.
async fn stepping_in_the_east(edge: &mut Harness) -> mpsc::Receiver<Bytes> {
    let packets = edge.joined(player(1), EntityId(5)).await;
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    let (number, body) = edge.next_numbered(EAST).await;
    assert!(
        number == 1 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{body:?}"
    );
    edge.tell(EAST, applied_up_to(1));
    edge.settle(EAST).await;
    edge.settle(WEST).await;
    edge.input(player(1), step(65.5)).await;
    assert_eq!(edge.next_numbered(EAST).await.0, 2);
    packets
}

fn step_in_the_east() -> EdgeToWorker {
    EdgeToWorker::Input {
        player: player(1),
        entity: EntityId(5),
        number: 1,
        input: step(65.5),
    }
}

/// A region that was absorbed is the survivor's from the moment the survivor says
/// so, among the entries of a welcome: its players are the survivor's, what they
/// see is asked of the survivor, and what was kept for it and not applied goes to
/// the survivor under the survivor's numbers, behind those subscriptions.
#[tokio::test]
async fn what_the_edge_had_at_an_absorbed_region_is_the_survivors_from_its_word_on() {
    let mut edge = Harness::start().await;
    let mut packets = stepping_in_the_east(&mut edge).await;

    let hello = edge.relink(WEST, 2).await;
    let EdgeToWorker::Hello { players, .. } = hello else {
        panic!("{hello:?}");
    };
    assert_eq!(players, []);
    edge.tell(WEST, resumed_with(1, 1, 1));
    let entry = edge.say(WEST, east_absorbed(1, Vec::new()));
    edge.tell(WEST, says_present(player(1), EntityId(5)));

    let (before, numbered) = edge.up_to_numbered(WEST).await;
    // What they see, as a viewer's; the entry confirmed; then what the east had
    // not applied, under the west's next number: its join was applied.
    let eastern = ChunkPos::new(4, 0);
    assert!(
        before.iter().any(|body| matches!(
            body,
            EdgeToWorker::Subscribe { chunks, .. } if chunks.contains(&eastern)
        )),
        "{before:?}"
    );
    assert!(
        before.contains(&EdgeToWorker::Confirm { number: entry }),
        "{before:?}"
    );
    assert_eq!(numbered, (2, step_in_the_east()));

    edge.settle(WEST).await;
    edge.input(player(1), step(66.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    assert!(connected(&mut packets));
}

/// A player the survivor's welcome brought with an absorbed region, and of whom
/// its presence answers then say nothing, is not there: they are told so, and the
/// leave names their stay.
#[tokio::test]
async fn a_player_who_came_with_an_absorbed_region_and_is_not_there_is_disconnected() {
    let mut edge = Harness::start().await;
    let mut packets = stepping_in_the_east(&mut edge).await;

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(1, 0, 1));
    edge.say(WEST, east_absorbed(1, Vec::new()));
    assert_eq!(edge.next_numbered(WEST).await, (2, step_in_the_east()));
    let leave = EdgeToWorker::PlayerLeave {
        player: player(1),
        entity: Some(EntityId(5)),
        attempt: None,
    };
    assert_eq!(edge.next_numbered(WEST).await, (3, leave));
    disconnected(&mut packets).await;
}

/// The entries an absorbed region had for the edge follow the survivor's word of
/// the merge under the survivor's numbers. Those the edge had handled as the
/// absorbed region's are passed over; the others are handled, and one that lets a
/// player go to the survivor, which until now a region could not say of itself,
/// is an arrival there.
#[tokio::test]
async fn entries_that_came_with_a_merge_are_handled_unless_they_were_before() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    assert_eq!(edge.next_numbered(EAST).await.0, 1);
    edge.tell(EAST, applied_up_to(1));
    edge.settle(WEST).await;
    // The edge has seen five entries of the east.
    for _ in 0..5 {
        edge.settle(EAST).await;
    }
    assert_eq!(edge.outbox[EAST.0 as usize], 5);

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(4, 0, 1));
    edge.say(WEST, east_absorbed(1, vec![4, 5, 6]));
    // Two the edge had seen as the east's, which would send an action on to the
    // west if they were handled again; and one it had not: the east let the
    // player go back to the west.
    let action = |sequence| RemoteAction {
        player: player(1),
        entity: EntityId(5),
        sequence,
        step: RemoteStep::Break {
            position: BlockPos::new(40, -61, 0),
        },
    };
    for sequence in [3, 4] {
        let seen = Durable::Remote {
            action: action(sequence),
            to: Some(WEST),
        };
        edge.say(WEST, seen);
    }
    let back = Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 0),
        to: WEST,
    };
    edge.say(WEST, back);

    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{number} {body:?}"
    );
    // Nothing else was sent the west: what the player does next has the next
    // number. And they are there by the arrival that is on its way, whatever the
    // welcome, which announced no answers, did not say of them.
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    assert!(connected(&mut packets));
}

/// A survivor's own word from before the merge that names the absorbed region is
/// read while that region is still itself to the edge: the player is put under it
/// and their arrival kept for it, and the word of the merge behind it brings both
/// to the survivor.
#[tokio::test]
async fn a_player_let_go_to_a_region_that_was_then_absorbed_arrives_at_the_survivor() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(2, 1, 1));
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    edge.say(WEST, east_absorbed(0, Vec::new()));
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{number} {body:?}"
    );
    edge.settle(WEST).await;
    edge.input(player(1), step(65.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    assert!(connected(&mut packets));
}

/// The routing table says that a region the edge still has something of went into
/// another. The edge acts on the survivor's word, not on the table's; a link to
/// the survivor that is through its welcome was answered without that word, and
/// the edge ends it to say a hello that is answered from after the merge.
#[tokio::test]
async fn a_merge_the_routing_table_tells_first_is_acted_on_when_the_survivor_tells_it() {
    let mut edge = Harness::start().await;
    let mut packets = stepping_in_the_east(&mut edge).await;

    assert!(edge.relinks.absorbed(vec![(EAST, WEST)]).await);
    let ended = timeout(SOON, edge.relinks.ended()).await.unwrap();
    assert_eq!(ended, Some((WEST, 1)));
    // Nothing was moved on the table's word: what the player does still goes east.
    edge.input(player(1), step(66.5)).await;
    assert_eq!(edge.next_numbered(EAST).await.0, 3);

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(1, 1, 1));
    edge.say(WEST, east_absorbed(1, Vec::new()));
    edge.tell(WEST, says_present(player(1), EntityId(5)));
    assert_eq!(edge.next_numbered(WEST).await, (2, step_in_the_east()));
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    edge.settle(WEST).await;
    assert!(connected(&mut packets));
}

/// The survivor answers a hello said after the table told of the merge without a
/// word of it: the absorbed region had forgotten the edge, or the survivor has
/// since. What was kept for the absorbed region is given up, and its players are
/// the survivor's to answer for.
#[tokio::test]
async fn a_merge_the_survivor_never_tells_of_gives_up_what_was_kept_for_the_absorbed_region() {
    let mut edge = Harness::start().await;
    let mut packets = stepping_in_the_east(&mut edge).await;

    assert!(edge.relinks.absorbed(vec![(EAST, WEST)]).await);
    let ended = timeout(SOON, edge.relinks.ended()).await.unwrap();
    assert_eq!(ended, Some((WEST, 1)));
    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(0, 0, 1));
    // Not the step the east never applied: only the leave of the player the
    // survivor did not answer for.
    let leave = EdgeToWorker::PlayerLeave {
        player: player(1),
        entity: Some(EntityId(5)),
        attempt: None,
    };
    assert_eq!(edge.next_numbered(WEST).await, (2, leave));
    disconnected(&mut packets).await;
}

/// A region of which the edge has nothing stands for the one it went into on the
/// routing table's word: there is nothing a word of the survivor could move. What
/// another region then sends there goes to the survivor.
#[tokio::test]
async fn a_region_the_edge_has_nothing_of_stands_for_its_survivor_on_the_tables_word() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    assert!(edge.relinks.absorbed(vec![(EAST, WEST)]).await);
    let ended = timeout(SOON, edge.relinks.ended()).await.unwrap();
    assert_eq!(ended, Some((EAST, 1)));

    // The west's own word from before the merge: an arrival at the west itself.
    edge.say(WEST, departing_to_east(player(1), EntityId(5)));
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{number} {body:?}"
    );
    edge.settle(WEST).await;
    assert!(connected(&mut packets));
}

/// The stays carry no attempt, as stays do of which an input was applied.
fn split_off(players: Vec<(PlayerId, EntityId)>) -> Durable {
    Durable::SplitOff {
        region: PART,
        players: players
            .into_iter()
            .map(|(player, entity)| (player, entity, None))
            .collect(),
    }
}

/// A region that was split says which stays went into the part. Those the edge has
/// under the region are the part's: the part is asked for what they see in its
/// first hello, and sent what they did after its welcome, without an arrival. A
/// stay the edge does not have is ended when the part says it has it.
#[tokio::test]
async fn the_stays_a_split_took_are_the_parts_from_the_word_of_it() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(1, 1, 1));
    let gone = vec![(player(1), EntityId(5)), (player(2), EntityId(9))];
    edge.say(WEST, split_off(gone));
    // The split region no longer has them, which costs them nothing.
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    edge.settle(WEST).await;
    assert!(connected(&mut packets));

    let hello = edge.link_part().await;
    let EdgeToWorker::Hello {
        players, chunks, ..
    } = hello
    else {
        panic!("{hello:?}");
    };
    assert_eq!(players, [player(1)]);
    assert!(chunks.contains(&SHARED), "{chunks:?}");
    let welcome = Welcome::Unknown {
        since: 3,
        entries: 0,
        presences: 2,
        applied: 0,
    };
    edge.tell(PART, WorkerToEdge::Welcome(welcome));
    edge.tell(PART, says_present(player(1), EntityId(5)));
    edge.tell(PART, says_present(player(2), EntityId(9)));
    let again = EdgeToWorker::Input {
        player: player(1),
        entity: EntityId(5),
        number: 1,
        input: step(1.5),
    };
    assert_eq!(edge.next_numbered(PART).await, (1, again));
    let leave = EdgeToWorker::PlayerLeave {
        player: player(2),
        entity: Some(EntityId(9)),
        attempt: None,
    };
    assert_eq!(edge.next_numbered(PART).await, (2, leave));
    assert!(connected(&mut packets));
}

/// The word of a split is as old as the split. The edge can have heard from the
/// part first, moved the stay there on its answer, and seen the player walk back:
/// the split region's word, read then, does not take them to the part again.
#[tokio::test]
async fn the_word_of_a_split_does_not_take_back_a_stay_that_has_returned() {
    let mut edge = Harness::start().await;
    let mut packets = edge.joined(player(1), EntityId(5)).await;

    let hello = edge.link_part().await;
    let EdgeToWorker::Hello { players, .. } = hello else {
        panic!("{hello:?}");
    };
    assert_eq!(players, []);
    let welcome = Welcome::Unknown {
        since: 3,
        entries: 0,
        presences: 1,
        applied: 0,
    };
    edge.tell(PART, WorkerToEdge::Welcome(welcome));
    edge.tell(PART, says_present(player(1), EntityId(5)));
    edge.settle(PART).await;

    // They walk back into the region that was split.
    let back = Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 0),
        to: WEST,
    };
    edge.say(PART, back);
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{number} {body:?}"
    );

    edge.relink(WEST, 2).await;
    edge.tell(WEST, resumed_with(1, 1, 1));
    edge.say(WEST, split_off(vec![(player(1), EntityId(5))]));
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    // The arrival is sent again, and what they do goes to the west.
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
        "{number} {body:?}"
    );
    edge.settle(WEST).await;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 3);
    assert!(connected(&mut packets));
}

/// A player whose region does not confirm what they do is not kept for ever.
#[tokio::test(start_paused = true)]
async fn a_player_whose_region_stays_silent_is_disconnected_after_the_patience_is_over() {
    let patience = Duration::from_secs(20);
    let mut edge = Harness::start_with_patience(patience).await;
    let mut waiting = edge.joined(player(1), EntityId(5)).await;
    let mut content = edge.joined(player(2), EntityId(6)).await;
    // One of them does something the region never reports applied.
    edge.sessions = 1;
    edge.input(player(1), step(1.5)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 3);

    tokio::time::sleep(patience / 2).await;
    assert!(connected(&mut waiting));
    tokio::time::sleep(patience).await;
    disconnected(&mut waiting).await;
    // Who has nothing outstanding stays, however long the region is silent.
    assert!(connected(&mut content));
}

#[test]
fn view_area_is_a_rounded_square() {
    // The official server sends 329 chunks at view distance 8.
    let area = view_area(ChunkPos::new(10, -3), 8);
    assert_eq!(area.len(), 329);
    // Straight ahead it reaches one chunk beyond the view distance.
    assert!(area.contains(&ChunkPos::new(19, -3)));
    assert!(!area.contains(&ChunkPos::new(20, -3)));
    // The corners are cut.
    assert!(!area.contains(&ChunkPos::new(19, 6)));
    assert!(area.contains(&ChunkPos::new(17, 4)));
}

#[test]
fn small_view_areas_are_full_squares() {
    assert_eq!(view_area(ChunkPos::new(0, 0), 2).len(), 49);
    assert_eq!(view_area(ChunkPos::new(0, 0), 3).len(), 81);
}
