//! The scenarios of ADR-0013, section 8, and more of their kind, written from that
//! record, from the contract of ADR-0012, section 5, and from ADR-0008 by someone who
//! had not read the code above. What a test expects is what the records say. A test
//! marked as a finding is one the code does not pass.
use std::cell::RefCell;

use clustine_protocol::chunk::decode_sections;
use clustine_protocol::packets::play::{ClientboundPlay, LightData};
use clustine_rpc::link::WorkerEnd;
use clustine_sim::api::{RemoteAction, RemoteStep};
use clustine_world::Vec3;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

use super::*;
use crate::Relinks;

mod block_destroyed;
mod break_across_spec;
mod crossing_specification;
mod dragon;
mod end_mobs_specification;
mod explosions;
mod gateways_specification;
mod hanging;
mod inventory_screen;
mod items;
mod items_specification;
mod leashes;
mod menus;
mod n9_mobs;
mod physics_specification;
mod projectiles;
mod redstone;
mod respawn_specification;
mod riding;
mod riding_specification;
mod survival;
mod tracking;

/// The flat world's spawn, which these tests' edges tell their clients.
const FLAT_SPAWN: BlockPos = BlockPos::new(0, -60, 0);

/// How long a test waits for something that has to happen.
const SOON: Duration = Duration::from_secs(10);

/// The three regions of these tests. Players enter the world in the western one.
/// Which of them holds a chunk is whatever the test has them say: the edge knows
/// nothing else about it.
const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);
const NORTH: RegionId = RegionId(2);
const REGIONS: [RegionId; 3] = [WEST, EAST, NORTH];
/// A region of the Nether, linked only where a test asks for it.
const NETHER: RegionId = RegionId(10);
/// Regions that come and go in the tests of merges and splits: one more that is
/// there from the start of the world, and the parts that are split off.
const SOUTH: RegionId = RegionId(3);
const PART: RegionId = RegionId(4);
const SECOND_PART: RegionId = RegionId(5);
/// Where the witness of `Harness::witnessed` is kept. No test gives it a link.
const ASIDE: RegionId = RegionId(11);
/// How many regions a test can play: the ids below this.
const PORTS: usize = 12;

const IDENTITY: EdgeIdentity = EdgeIdentity {
    edge: clustine_world::EdgeId(7),
    start: 100,
};

/// Where players enter the world, which is in the chunk `HOME`.
const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);
const HOME: ChunkPos = ChunkPos::new(0, 0);
/// The view distance of every player in these tests: a view is seven chunks wide.
const VIEW: i32 = 2;

/// What a region takes a subscription for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Viewer,
    Guest,
}

/// What the edge has said on its current link to a region.
#[derive(Debug, Default)]
struct Heard {
    /// The number of the last subscription message.
    asked: u64,
    /// What the region is subscribed to by those messages and the hello, each with
    /// the number of the last message that named it.
    subscriptions: BTreeMap<ChunkPos, (Role, u64)>,
    /// What the edge said that no test has looked at yet, in order.
    said: VecDeque<EdgeMessage>,
    /// How many players the hello of the link named: as many presence answers as
    /// a welcome on it announces.
    named: u32,
    /// The highest number of a numbered message the edge has sent on the link.
    numbered: u64,
}

impl Heard {
    /// The chunks the region is subscribed to as `role`.
    fn chunks(&self, role: Role) -> BTreeSet<ChunkPos> {
        self.subscriptions
            .iter()
            .filter(|(_, (kind, _))| *kind == role)
            .map(|(chunk, _)| *chunk)
            .collect()
    }

    /// Takes a message of the edge into what the region is subscribed to. A region
    /// ends a link whose subscription messages do not ascend, so that is checked
    /// for everything any test reads.
    fn note(&mut self, region: RegionId, message: &EdgeMessage) {
        if let Some(number) = message.number {
            self.numbered = self.numbered.max(number);
        }
        let (ask, chunks, role) = match &message.body {
            EdgeToWorker::Subscribe { ask, chunks } => (*ask, chunks, Some(Role::Viewer)),
            EdgeToWorker::SubscribeAsGuest { ask, chunks } => (*ask, chunks, Some(Role::Guest)),
            EdgeToWorker::Unsubscribe { ask, chunks } => (*ask, chunks, None),
            _ => return,
        };
        assert!(
            ask > self.asked,
            "{region:?} was sent the subscription message {ask} after {}: {message:?}",
            self.asked
        );
        assert!(!chunks.is_empty(), "{region:?} was sent {message:?}");
        self.asked = ask;
        for chunk in chunks {
            match role {
                Some(role) => self.subscriptions.insert(*chunk, (role, ask)),
                None => self.subscriptions.remove(chunk),
            };
        }
    }
}

/// What a hello of the edge says.
#[derive(Debug)]
struct Hello {
    since: u64,
    seen: u64,
    players: Vec<PlayerId>,
    chunks: Vec<ChunkPos>,
    guests: Vec<ChunkPos>,
}

/// A fan-out task with three regions played by the test.
struct Harness {
    commands: mpsc::Sender<Command>,
    relinks: Relinks,
    /// The keys of the wire ids the edge gives.
    ledger: WireLedger,
    /// The workers' ends of the links to the regions; none where the test has
    /// given the edge no link, or has dropped it.
    links: Vec<Option<WorkerEnd>>,
    heard: Vec<Heard>,
    task: JoinHandle<Stopped>,
    sessions: u64,
    /// The session of each player's latest connection, which is the attempt of
    /// its join.
    attempts: BTreeMap<PlayerId, u64>,
    /// The entity each player was last placed as by a test.
    placed: RefCell<BTreeMap<PlayerId, EntityId>>,
    /// The number of the last outbox entry each region has made.
    outbox: [u64; PORTS],
    /// The epoch of the owner each region's last link went to.
    epochs: [u64; PORTS],
    /// The last sequence number used to find out that the edge has got to a
    /// player's commands.
    sequences: i32,
    /// The witness of `Harness::witnessed`, if the test has one.
    witness: Option<Client>,
    /// The hello the edge said last to each region on a link that a step of a
    /// script made.
    hellos: Vec<Option<Hello>>,
    /// The links the edge has said have ended, of which no test has asked.
    ended: Vec<(RegionId, u64)>,
    /// The regions whose link the edge closed while a test waited for the edge
    /// to read what the region had said on it.
    gone: BTreeSet<RegionId>,
}

/// A link to `region` whose owner has `epoch`, and the worker's end of it: of the
/// Nether for [`NETHER`], of the overworld for every other.
fn link_to(region: RegionId, epoch: u64) -> (RegionLink, WorkerEnd) {
    let (end, worker) = link::in_process(1024);
    let dimension = if region == NETHER {
        Dimension::Nether
    } else {
        Dimension::Overworld
    };
    (
        RegionLink {
            dimension,
            region,
            epoch,
            end,
        },
        worker,
    )
}

impl Harness {
    /// An edge linked to all three regions, each of which has been said hello to
    /// and has answered that it does not know the edge, as at the start of
    /// everything.
    async fn start() -> Self {
        Self::start_with(&REGIONS, Duration::from_secs(1_000_000)).await
    }

    /// An edge that starts with links to `linked` only.
    async fn start_with(linked: &[RegionId], region_patience: Duration) -> Self {
        Self::start_limited(linked, region_patience, VIEW).await
    }

    /// An edge linked to all three regions that grants view distances up to
    /// `limit`, for the tests of a distance that changes in play: with the limit
    /// of the other tests there is no distance but the one a player has.
    async fn start_with_limit(limit: i32) -> Self {
        Self::start_limited(&REGIONS, Duration::from_secs(1_000_000), limit).await
    }

    async fn start_limited(linked: &[RegionId], region_patience: Duration, limit: i32) -> Self {
        let mut links = Vec::new();
        let mut ends: Vec<Option<WorkerEnd>> = (0..PORTS).map(|_| None).collect();
        for region in linked {
            let (link, end) = link_to(*region, 1);
            links.push(link);
            ends[region.0 as usize] = Some(end);
        }
        let (routing, relinks) = Routing::new(WEST, FLAT_SPAWN, IDENTITY, links);
        let config = FanoutConfig {
            max_players: 20,
            view_distance: limit,
            online: Arc::default(),
            region_patience,
            login: crate::LoginWorld::FLAT,
            light: LightSettings::AT_ONCE,
            wire_ledger: WireLedger::default(),
        };
        let (commands, receiver) = mpsc::channel(64);
        let ledger = Arc::clone(&config.wire_ledger);
        // A job is done in the turn that dispatches it, so that everything a turn
        // marks is lit before the turn ends.
        let task = tokio::spawn(Fanout::new(config, routing, receiver, Lighting::here()).run());
        let mut harness = Self {
            commands,
            relinks,
            ledger,
            links: ends,
            heard: (0..PORTS).map(|_| Heard::default()).collect(),
            task,
            sessions: 0,
            attempts: BTreeMap::new(),
            placed: RefCell::new(BTreeMap::new()),
            outbox: [0; PORTS],
            epochs: [1; PORTS],
            sequences: 0,
            witness: None,
            hellos: (0..PORTS).map(|_| None).collect(),
            ended: Vec::new(),
            gone: BTreeSet::new(),
        };
        for region in linked {
            let hello = harness.read(*region).await;
            assert!(
                matches!(
                    &hello.body,
                    EdgeToWorker::Hello { seen: 0, players, chunks, guests, .. }
                        if players.is_empty() && chunks.is_empty() && guests.is_empty()
                ),
                "{hello:?}"
            );
            harness.tell(
                *region,
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

    /// Reads what the edge sends `region` next. The edge checks what it holds
    /// about subscriptions at the end of its turns and panics if it is wrong, so
    /// a task that has ended is what a test finds here.
    async fn read(&mut self, region: RegionId) -> EdgeMessage {
        let index = region.0 as usize;
        let link = self.links[index].as_mut().expect("the region has a link");
        let message = tokio::select! {
            biased;
            ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
            message = timeout(SOON, link.recv()) => message,
        };
        let message = message.unwrap_or_else(|_| panic!("the edge sent {region:?} nothing"));
        let Some(message) = message else {
            let ended = timeout(SOON, &mut self.task).await;
            panic!("the edge closed its link to {region:?}: {ended:?}");
        };
        self.heard[index].note(region, &message);
        message
    }

    /// What the edge said to `region` next that no test has looked at.
    async fn next(&mut self, region: RegionId) -> EdgeMessage {
        match self.heard[region.0 as usize].said.pop_front() {
            Some(message) => message,
            None => self.read(region).await,
        }
    }

    /// What the edge said to `region` next that is numbered, with its number.
    async fn next_numbered(&mut self, region: RegionId) -> (u64, EdgeToWorker) {
        loop {
            let EdgeMessage { number, body } = self.next(region).await;
            if let Some(number) = number {
                return (number, body);
            }
        }
    }

    /// What the edge said to `region` next about its subscriptions.
    async fn next_asked(&mut self, region: RegionId) -> EdgeToWorker {
        loop {
            let body = self.next(region).await.body;
            if is_about_subscriptions(&body) {
                return body;
            }
        }
    }

    /// Says something as `region`.
    fn tell(&mut self, region: RegionId, message: WorkerToEdge) {
        let index = region.0 as usize;
        // A region that says `NotMine` to a guest has ended that subscription.
        if let WorkerToEdge::NotMine { chunk, ask } = &message {
            let subscriptions = &mut self.heard[index].subscriptions;
            if subscriptions.get(chunk) == Some(&(Role::Guest, *ask)) {
                subscriptions.remove(chunk);
            }
        }
        let sent = self.links[index]
            .as_ref()
            .expect("the region has a link")
            .try_send(message);
        if let Err(error) = sent {
            let ended = self.task.is_finished();
            panic!("{region:?} cannot say anything: {error:?}; the edge's task ended: {ended}");
        }
    }

    /// Says `entry` as the next entry of the outbox of `region`. Returns its number.
    fn say(&mut self, region: RegionId, entry: Durable) -> u64 {
        let number = &mut self.outbox[region.0 as usize];
        *number += 1;
        let number = *number;
        self.tell(region, WorkerToEdge::Outbox { number, entry });
        number
    }

    /// Waits until the edge has handled everything `region` has said so far, by an
    /// entry about nobody, which the edge confirms like any other. What the edge
    /// said to the region meanwhile is kept for the test to look at. The edge
    /// takes what regions say, what players do and new links from different
    /// queues, in no order between them, so a test that depends on the edge having
    /// heard something makes sure of it with this.
    async fn settle(&mut self, region: RegionId) {
        let nobody = Durable::RemoteDone {
            player: player(u128::MAX),
            entity: EntityId(0),
            sequence: 0,
            shown: Vec::new(),
        };
        let number = self.say(region, nobody);
        loop {
            let message = self.read(region).await;
            if message.body == (EdgeToWorker::Confirm { number }) {
                return;
            }
            self.heard[region.0 as usize].said.push_back(message);
        }
    }

    /// Everything the edge has said to `region` that no test has looked at, once
    /// it has handled what the region has said so far.
    async fn said(&mut self, region: RegionId) -> Vec<EdgeMessage> {
        self.settle(region).await;
        self.heard[region.0 as usize].said.drain(..).collect()
    }

    /// What the edge has said to `region` about subscriptions that no test has
    /// looked at, once it has handled what the region has said so far.
    async fn asked(&mut self, region: RegionId) -> Vec<EdgeToWorker> {
        let said = self.said(region).await;
        said.into_iter()
            .map(|message| message.body)
            .filter(is_about_subscriptions)
            .collect()
    }

    /// The number of the edge's last message to `region` that named `chunk`.
    fn ask(&self, region: RegionId, chunk: ChunkPos) -> u64 {
        let subscription = self.heard[region.0 as usize].subscriptions.get(&chunk);
        subscription
            .expect("the region is subscribed to the chunk")
            .1
    }

    /// Waits until the edge has taken every command sent to it. It handles one
    /// thing at a time, so what a region is told after this is handled after the
    /// commands.
    async fn drained(&mut self) {
        let waited = async {
            while self.commands.capacity() < self.commands.max_capacity() {
                tokio::task::yield_now().await;
            }
        };
        timeout(SOON, waited)
            .await
            .expect("the edge takes its commands");
    }

    /// The word of a region that it has placed `player` as `entity`, in answer to
    /// the join of the player's latest connection.
    fn spawned(&self, player: PlayerId, entity: EntityId) -> WorkerToEdge {
        let attempt = self.attempts.get(&player).copied().unwrap_or_default();
        self.placed.borrow_mut().insert(player, entity);
        spawned(attempt, player, entity)
    }

    /// The entity a test last placed `player` as, which is the stay their
    /// actions are of.
    fn entity_of(&self, player: PlayerId) -> EntityId {
        let placed = self.placed.borrow();
        *placed.get(&player).expect("the test placed the player")
    }

    /// A player connects. Returns their client.
    async fn join(&mut self, player: PlayerId) -> Client {
        self.join_asking(player, None).await
    }

    /// A player connects whose client asks for `requested` as its view distance.
    async fn join_asking(&mut self, player: PlayerId, requested: Option<i32>) -> Client {
        self.sessions += 1;
        self.attempts.insert(player, self.sessions);
        let session = SessionId(self.sessions);
        let (outbound, packets) = mpsc::channel(4096);
        let awaiting = Arc::new(AtomicI32::new(NO_TELEPORT));
        let join = Command::Join {
            appearance: Default::default(),
            session,
            profile: Profile {
                uuid: player.0,
                name: format!("Player{}", self.sessions),
            },
            requested_view_distance: requested,
            outbound,
            awaiting_teleport: Arc::clone(&awaiting),
        };
        self.commands.send(join).await.unwrap();
        Client::new(player, session, packets, Arc::clone(&self.ledger), awaiting)
    }

    /// A player joins and the western region places them as `entity` where
    /// players enter the world. The edge has asked for their view when this
    /// returns, and no test has looked at that yet.
    async fn joined(&mut self, player: PlayerId, entity: EntityId) -> Client {
        self.joined_asking(player, entity, None).await
    }

    /// The same for a client that asks for `requested` as its view distance.
    async fn joined_asking(
        &mut self,
        player: PlayerId,
        entity: EntityId,
        requested: Option<i32>,
    ) -> Client {
        let client = self.join_asking(player, requested).await;
        let (_, join) = self.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        self.tell(WEST, self.spawned(player, entity));
        self.settle(WEST).await;
        client
    }

    /// The player's connection ends.
    async fn leave(&mut self, client: &Client) {
        let leave = Command::Leave {
            session: client.session,
            player: client.player,
        };
        self.commands.send(leave).await.unwrap();
        self.drained().await;
    }

    /// The client asks for `requested` as its view distance in play, as the
    /// connection passes on from the settings packet, on the connection
    /// `session`. The edge has taken the command when this returns.
    async fn ask_view_distance(&mut self, client: &Client, session: SessionId, requested: i32) {
        let command = Command::ViewDistance {
            session,
            player: client.player,
            requested,
        };
        self.commands.send(command).await.unwrap();
        self.drained().await;
    }

    /// Something the player did.
    /// The client clicks in, or closes, a menu's window.
    async fn menu(&mut self, client: &Client, what: MenuCommand) {
        let command = Command::Menu {
            session: client.session,
            player: client.player,
            what,
        };
        self.commands.send(command).await.unwrap();
    }

    async fn input(&mut self, client: &Client, input: PlayerInput) {
        let command = Command::Input {
            session: client.session,
            player: client.player,
            input,
        };
        self.commands.send(command).await.unwrap();
    }

    /// The next packet the edge sends the client; `None` once it has ended the
    /// connection.
    async fn packet(&mut self, client: &mut Client) -> Option<Bytes> {
        tokio::select! {
            biased;
            ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
            packet = timeout(SOON, client.packets.recv()) => {
                packet.expect("the edge sent the client nothing")
            }
        }
    }

    /// Brings `client` up to date with everything the edge has sent it and will
    /// send it without anything else happening: chunks come in batches, each of
    /// which the client has to confirm before the next. The edge is asked to
    /// acknowledge an action that no region hears of, and what it sent before the
    /// acknowledgement is what it had to send.
    async fn sync(&mut self, client: &mut Client) {
        loop {
            client.drain();
            let confirmed = client.batches > 0;
            while client.batches > 0 {
                client.batches -= 1;
                let received = Command::ChunkBatchReceived {
                    session: client.session,
                    player: client.player,
                    chunks_per_tick: 64.0,
                };
                self.commands.send(received).await.unwrap();
            }
            let sequence = self.sequence();
            let handled = Command::Handled {
                session: client.session,
                player: client.player,
                sequence,
            };
            self.commands.send(handled).await.unwrap();
            while client.acknowledged.last() != Some(&sequence) {
                let packet = self.packet(client).await;
                client.take(packet.expect("the edge ended the connection"));
            }
            if !confirmed && client.batches == 0 {
                return;
            }
        }
    }

    /// Whether the client has `chunk`, once the edge has handled everything
    /// `region` has said so far.
    async fn shows(&mut self, client: &mut Client, region: RegionId, chunk: ChunkPos) -> bool {
        self.settle(region).await;
        self.sync(client).await;
        client.chunks.contains_key(&chunk)
    }

    /// Gives the edge a new link to `region`, in place of the one it has if it
    /// has one, and returns the hello the edge says on it.
    async fn relink(&mut self, region: RegionId) -> Hello {
        let index = region.0 as usize;
        self.epochs[index] += 1;
        let (link, worker) = link_to(region, self.epochs[index]);
        self.links[index] = Some(worker);
        self.heard[index] = Heard::default();
        assert!(self.relinks.replace(link).await);
        let hello = self.read(region).await;
        let EdgeToWorker::Hello {
            edge,
            start,
            since,
            seen,
            players,
            chunks,
            guests,
        } = hello.body
        else {
            panic!("expected a hello, got {hello:?}");
        };
        assert_eq!((edge, start), (IDENTITY.edge, IDENTITY.start));
        let heard = &mut self.heard[index];
        heard.named = players.len() as u32;
        for chunk in &guests {
            heard.subscriptions.insert(*chunk, (Role::Guest, 0));
        }
        // A chunk in both lists is a viewer's.
        for chunk in &chunks {
            heard.subscriptions.insert(*chunk, (Role::Viewer, 0));
        }
        Hello {
            since,
            seen,
            players,
            chunks,
            guests,
        }
    }

    /// The region's link ends, and the edge has found that out when this returns.
    async fn lose(&mut self, region: RegionId) {
        let index = region.0 as usize;
        drop(self.links[index].take().expect("the region has a link"));
        self.heard[index] = Heard::default();
        // Links that were replaced have ended as well, earlier.
        loop {
            let ended = timeout(SOON, self.relinks.ended())
                .await
                .expect("the edge notices that a link ended")
                .expect("the edge is there");
            if ended == (region, self.epochs[index]) {
                return;
            }
            self.ended.push(ended);
        }
    }

    /// The region answers a hello as one that knows the edge, with nothing in its
    /// outbox that the edge has not seen.
    fn resume(&mut self, region: RegionId) {
        // No region of these tests has told the edge of any progress.
        let welcome = Welcome::Resumed {
            entries: 0,
            presences: self.at(region).named,
            applied: 0,
        };
        self.tell(region, WorkerToEdge::Welcome(welcome));
    }

    /// What the edge has said on its current link to `region`, as far as it was read.
    fn at(&self, region: RegionId) -> &Heard {
        &self.heard[region.0 as usize]
    }

    /// `from` lets the player go to `to`, into `chunk`. The edge has handled that
    /// when this returns.
    async fn hand(
        &mut self,
        player: PlayerId,
        entity: EntityId,
        from: RegionId,
        to: RegionId,
        chunk: ChunkPos,
    ) {
        self.say(from, departed(player, entity, to, chunk));
        self.settle(from).await;
    }

    /// Waits until the edge has handled everything the regions have said so far,
    /// reads everything it has said to them, and has the test look at none of it.
    async fn quiet(&mut self) {
        // Twice, as what one region said can make the edge say something to a
        // region that was settled before.
        for _ in 0..2 {
            for region in self.linked() {
                self.settle(region).await;
            }
        }
        for heard in &mut self.heard {
            heard.said.clear();
        }
    }

    /// What the edge has said to `region` that no test has looked at, as far as it
    /// is on the link now: nothing is waited for.
    fn waiting(&mut self, region: RegionId) -> Vec<EdgeMessage> {
        let index = region.0 as usize;
        let mut waiting: Vec<_> = self.heard[index].said.drain(..).collect();
        let link = self.links[index].as_mut().expect("the region has a link");
        while let Ok(Some(message)) = link.try_recv() {
            self.heard[index].note(region, &message);
            waiting.push(message);
        }
        waiting
    }

    /// Looks through what the edge has said to `region` that no test has looked
    /// at for a player who arrives. Returns the number of the arrival and the
    /// chunks the region was asked for as a viewer's before it.
    async fn arrived(&mut self, region: RegionId) -> (u64, BTreeSet<ChunkPos>) {
        let said = self.said(region).await;
        let arrival = said
            .iter()
            .position(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
            .unwrap_or_else(|| panic!("nobody arrived in {region:?}: {said:?}"));
        let mut before = BTreeSet::new();
        for message in &said[..arrival] {
            if let EdgeToWorker::Subscribe { chunks, .. } = &message.body {
                before.extend(chunks.iter().copied());
            }
        }
        let number = said[arrival].number.expect("an arrival is numbered");
        (number, before)
    }

    /// Makes sure that the edge has said nothing numbered to any region that no
    /// test has looked at.
    async fn nothing_numbered(&mut self) {
        for region in self.linked() {
            let said = numbered(&self.said(region).await);
            assert!(said.is_empty(), "{region:?} was sent {said:?}");
        }
    }

    /// The regions the test plays a link of, in ascending order.
    fn linked(&self) -> Vec<RegionId> {
        let linked = |index: &usize| self.links[*index].is_some();
        let regions = (0..PORTS).filter(linked);
        regions.map(|index| RegionId(index as u32)).collect()
    }

    /// A sequence number for an action of a player, above every one used before.
    fn sequence(&mut self) -> i32 {
        self.sequences += 1;
        self.sequences
    }

    /// Waits until the client is told that its action `sequence` was handled.
    async fn acknowledged(&mut self, client: &mut Client, sequence: i32) {
        while !client.acknowledged.contains(&sequence) {
            let packet = self.packet(client).await;
            client.take(packet.expect("the edge ended the connection"));
        }
    }

    /// Makes sure that the edge has got through everything said to it without
    /// finding its own account of subscriptions wrong.
    async fn end(mut self) {
        self.drained().await;
        // A link the edge has closed by itself is not one to settle.
        if self.witness.is_some() {
            for region in self.linked() {
                self.handled(region).await;
            }
        }
        self.quiet().await;
        assert!(!self.task.is_finished(), "the edge's task ended");
    }
}

fn is_about_subscriptions(body: &EdgeToWorker) -> bool {
    matches!(
        body,
        EdgeToWorker::Subscribe { .. }
            | EdgeToWorker::SubscribeAsGuest { .. }
            | EdgeToWorker::Unsubscribe { .. }
    )
}

/// What a player's client has been sent.
/// Something a client was told of a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfChunk {
    Sent(ChunkPos),
    Unloaded(ChunkPos),
    /// A block of the chunk has changed.
    Block(ChunkPos),
    /// A light update for the chunk.
    Light(ChunkPos),
}

struct Client {
    player: PlayerId,
    session: SessionId,
    packets: mpsc::Receiver<Bytes>,
    /// The chunks the client has, each with what was at `block_of` it when it was
    /// sent last, if that is one of the few states these tests put there.
    chunks: BTreeMap<ChunkPos, Option<BlockState>>,
    /// Every chunk the client was sent, in order.
    sent: Vec<ChunkPos>,
    /// What it was told of chunks, in order: sent, unloaded, and a block of one
    /// changed.
    of_chunks: Vec<OfChunk>,
    /// The light each chunk it has was sent with.
    light: BTreeMap<ChunkPos, LightData>,
    /// Every light update it was sent, in order.
    light_updates: Vec<(ChunkPos, LightData)>,
    /// The blocks it was told have changed since their chunk was sent, each as it
    /// was told last.
    blocks: BTreeMap<BlockPos, i32>,
    /// The entities it shows, by their keys.
    entities: BTreeSet<u64>,
    /// The keys of the edge's wire ids.
    ledger: WireLedger,
    /// Every action it was told was handled, in order.
    acknowledged: Vec<i32>,
    /// Batches of chunks it has been sent and has not confirmed.
    batches: u32,
    /// Why the edge said it disconnects the player, if it did.
    disconnected: Option<String>,
    /// Whether the edge has ended the connection.
    closed: bool,
    /// What it was told of entities and of the player list, in order, for the
    /// tests that ask what a client showed at any moment and not only at the end.
    told: Vec<Told>,
    /// Every chunk it was told its view is centred on, in order.
    centres: Vec<ChunkPos>,
    /// Every packet it was sent that names an entity, with the wire id it names,
    /// in order.
    named: Vec<(Naming, i32)>,
    /// How many packets it has taken, of whatever kind.
    taken: usize,
    /// Every level event it was told of (a block breaking), in order.
    level_events: Vec<LevelEvent>,
    /// The blocks it was told changed one at a time, the block events and the
    /// sounds, in the order told.
    heard: Vec<Received>,
    /// Every entity it was told to spawn, and every entity's data and arm swing
    /// it was told, in order.
    spawns: Vec<SpawnEntity>,
    entity_data: Vec<SetEntityData>,
    swings: Vec<SwingAnimation>,
    /// Every damage event and attribute update it was sent, in order.
    living: Vec<ClientboundPlay>,
    /// Every list of a vehicle's passengers and every teleport it was sent, in order.
    passengers: Vec<SetPassengers>,
    /// Every leash it was told of, in order.
    links: Vec<SetEntityLink>,
    teleports: Vec<SynchronizePlayerPosition>,
    /// Every packet it was sent of an entity's tracking (its moves, relative and
    /// whole, its motion, its being picked up) and every transient block, in order.
    tracking: Vec<ClientboundPlay>,
    /// The id of every packet it was sent, in order.
    ids: Vec<i32>,
    /// Every packet of a menu's window it was sent, in order: openings, contents,
    /// slots, the cursor and closes.
    menus: Vec<ClientboundPlay>,
    /// Every whole of window 0, the player's own inventory screen, it was sent, in
    /// order (`container_set_content` of window 0, which `menus` leaves out).
    inventories: Vec<clustine_protocol::packets::play::SetContainerContent>,
    /// Every explosion it was sent, in order.
    explosions: Vec<clustine_protocol::packets::play::Explode>,
    /// The teleport its connection waits for the client to accept, as the edge sets
    /// it: moves are held back while it is not [`NO_TELEPORT`].
    awaiting: Arc<AtomicI32>,
    /// Every change of level it was sent, in order.
    respawns: Vec<Respawn>,
    /// Every login it was sent, in order.
    logins: Vec<Login>,
    /// Every packet it was sent that none of the fields above keeps, in order (the
    /// health, a death, the game's events, a hurt's shake of the view).
    others: Vec<ClientboundPlay>,
}

/// One of what a client was told, in [`Client::heard`].
#[derive(Debug, Clone, PartialEq)]
enum Received {
    Block(BlockPos),
    Event(BlockEvent),
    Sound(Sound),
}

/// A packet that names an entity by its wire id, in [`Client::named`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    /// The login, which names the client's own entity.
    Login,
    Spawn,
    Position,
    HeadRotation,
    Removal,
    Data,
    Swing,
    Motion,
    Taken,
}

/// Something a client was told that changes whom it shows or lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Told {
    /// It was put into the world: the login packet.
    Entered,
    Shown(u64),
    Hidden(u64),
    Listed(Uuid),
    Unlisted(Uuid),
}

impl Client {
    fn new(
        player: PlayerId,
        session: SessionId,
        packets: mpsc::Receiver<Bytes>,
        ledger: WireLedger,
        awaiting: Arc<AtomicI32>,
    ) -> Self {
        Self {
            player,
            session,
            packets,
            ledger,
            awaiting,
            respawns: Vec::new(),
            logins: Vec::new(),
            others: Vec::new(),
            chunks: BTreeMap::new(),
            sent: Vec::new(),
            of_chunks: Vec::new(),
            light: BTreeMap::new(),
            light_updates: Vec::new(),
            blocks: BTreeMap::new(),
            entities: BTreeSet::new(),
            acknowledged: Vec::new(),
            batches: 0,
            disconnected: None,
            closed: false,
            told: Vec::new(),
            centres: Vec::new(),
            named: Vec::new(),
            taken: 0,
            level_events: Vec::new(),
            heard: Vec::new(),
            spawns: Vec::new(),
            entity_data: Vec::new(),
            swings: Vec::new(),
            living: Vec::new(),
            passengers: Vec::new(),
            links: Vec::new(),
            teleports: Vec::new(),
            tracking: Vec::new(),
            menus: Vec::new(),
            inventories: Vec::new(),
            explosions: Vec::new(),
            ids: Vec::new(),
        }
    }

    /// The key of the entity the edge gave the wire id `id`.
    fn key(&self, id: i32) -> u64 {
        let ledger = self
            .ledger
            .lock()
            .expect("nothing panics holding the ledger");
        ledger.get(&id).expect("the edge gave the id").0
    }

    /// The wire id the edge gave the key `key`.
    fn ledger_id(&self, key: u64) -> i32 {
        let ledger = self
            .ledger
            .lock()
            .expect("nothing panics holding the ledger");
        *ledger
            .iter()
            .find(|(_, of)| of.0 == key)
            .expect("the edge gave the key an id")
            .0
    }

    /// The wire id its login gave its own entity.
    fn own_id(&self) -> i32 {
        let mut logins = self
            .named
            .iter()
            .filter(|(naming, _)| *naming == Naming::Login);
        let (_, id) = logins.next().expect("the client was put into the world");
        assert!(logins.next().is_none(), "the client was logged in twice");
        *id
    }

    /// The wire ids the client was told the entity `key` by, in order, each with
    /// the packet that named it.
    fn naming(&self, key: u64) -> Vec<(Naming, i32)> {
        self.named
            .iter()
            .filter(|(_, id)| self.key(*id) == key)
            .copied()
            .collect()
    }

    /// Takes a packet as a client would.
    fn take(&mut self, packet: Bytes) {
        let decoded = ClientboundPlay::decode(&packet).expect("the edge sends packets");
        self.taken += 1;
        let id = clustine_protocol::codec::Reader::new(&packet).var_int();
        self.ids.push(id.expect("a packet starts with its id"));
        match decoded {
            ClientboundPlay::SetCenterChunk(centre) => {
                self.centres
                    .push(ChunkPos::new(centre.chunk_x, centre.chunk_z));
            }
            ClientboundPlay::LevelChunkWithLight(chunk) => {
                let position = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
                // By the block that tells these tests' chunks apart, read as a
                // client reads it, and not by the packet's bytes: those have the
                // chunk's light in them, which the edge may make itself.
                let sections = decode_sections(
                    &chunk.sections,
                    SECTIONS,
                    crate::encode::BLOCK_PALETTE,
                    crate::encode::biome_palette(),
                )
                .expect("the edge sends sections a client can read");
                let told = sections[TELLING.0].blocks.get(TELLING.1);
                let state = STATES.into_iter().find(|state| i32::from(state.0) == told);
                self.chunks.insert(position, state);
                self.light.insert(position, chunk.light);
                self.sent.push(position);
                self.of_chunks.push(OfChunk::Sent(position));
                self.blocks.retain(|block, _| block.chunk() != position);
            }
            ClientboundPlay::UnloadChunk(chunk) => {
                let position = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
                self.chunks.remove(&position);
                self.of_chunks.push(OfChunk::Unloaded(position));
                self.blocks.retain(|block, _| block.chunk() != position);
            }
            ClientboundPlay::BlockUpdate(update) => {
                let Position { x, y, z } = update.position;
                let block = BlockPos::new(x, y, z);
                self.of_chunks.push(OfChunk::Block(block.chunk()));
                self.blocks.insert(block, update.state);
                self.heard.push(Received::Block(block));
            }
            ClientboundPlay::BlockEvent(event) => self.heard.push(Received::Event(event)),
            ClientboundPlay::Sound(sound) => self.heard.push(Received::Sound(sound)),
            ClientboundPlay::LightUpdate(update) => {
                let position = ChunkPos::new(update.chunk_x, update.chunk_z);
                self.of_chunks.push(OfChunk::Light(position));
                self.light_updates.push((position, update.light));
            }
            ClientboundPlay::SpawnEntity(entity) => {
                self.named.push((Naming::Spawn, entity.entity_id));
                let key = self.key(entity.entity_id);
                self.entities.insert(key);
                self.told.push(Told::Shown(key));
                self.spawns.push(entity);
            }
            ClientboundPlay::SetEntityData(data) => {
                self.named.push((Naming::Data, data.entity_id));
                self.entity_data.push(data);
            }
            ClientboundPlay::SwingAnimation(swing) => {
                self.named.push((Naming::Swing, swing.entity_id));
                self.swings.push(swing);
            }
            packet @ (ClientboundPlay::DamageEvent(_)
            | ClientboundPlay::UpdateAttributes(_)
            | ClientboundPlay::SetEquipment(_)) => {
                self.living.push(packet);
            }
            ClientboundPlay::RemoveEntities(removed) => {
                for entity in removed.entity_ids {
                    self.named.push((Naming::Removal, entity));
                    let key = self.key(entity);
                    self.entities.remove(&key);
                    self.told.push(Told::Hidden(key));
                }
            }
            ClientboundPlay::AcknowledgeBlockChange(acknowledged) => {
                self.acknowledged.push(acknowledged.sequence);
            }
            ClientboundPlay::ChunkBatchFinished(_) => self.batches += 1,
            ClientboundPlay::LevelEvent(event) => self.level_events.push(event),
            ClientboundPlay::Disconnect(disconnect) => {
                self.disconnected = Some(format!("{:?}", disconnect.reason));
            }
            ClientboundPlay::Login(login) => {
                self.named.push((Naming::Login, login.entity_id));
                self.told.push(Told::Entered);
                self.logins.push(login);
            }
            ClientboundPlay::SyncEntityPosition(moved) => {
                self.named.push((Naming::Position, moved.entity_id));
                self.tracking
                    .push(ClientboundPlay::SyncEntityPosition(moved));
            }
            ClientboundPlay::MoveEntityPos(ref moved) => {
                self.named.push((Naming::Position, moved.entity_id));
                self.tracking.push(decoded);
            }
            ClientboundPlay::MoveEntityPosRot(ref moved) => {
                self.named.push((Naming::Position, moved.entity_id));
                self.tracking.push(decoded);
            }
            ClientboundPlay::MoveEntityRot(ref moved) => {
                self.named.push((Naming::Position, moved.entity_id));
                self.tracking.push(decoded);
            }
            ClientboundPlay::SetEntityMotion(ref motion) => {
                self.named.push((Naming::Motion, motion.entity_id));
                self.tracking.push(decoded);
            }
            ClientboundPlay::TakeItemEntity(ref taken) => {
                self.named.push((Naming::Taken, taken.item_id));
                self.tracking.push(decoded);
            }
            ClientboundPlay::AddTransientBlock(_) => self.tracking.push(decoded),
            ClientboundPlay::Explode(explosion) => self.explosions.push(explosion),
            ClientboundPlay::OpenScreen(_)
            | ClientboundPlay::ContainerSetSlot(_)
            | ClientboundPlay::SetCursorItem(_)
            | ClientboundPlay::ClientboundContainerClose(_) => self.menus.push(decoded),
            ClientboundPlay::SetContainerContent(ref content)
                if content.window_id
                    != clustine_protocol::packets::play::inventory::PLAYER_WINDOW =>
            {
                self.menus.push(decoded);
            }
            ClientboundPlay::SetContainerContent(content) => self.inventories.push(content),
            ClientboundPlay::SetPassengers(passengers) => self.passengers.push(passengers),
            ClientboundPlay::SetEntityLink(link) => self.links.push(link),
            ClientboundPlay::SynchronizePlayerPosition(teleport) => {
                self.teleports.push(teleport);
            }
            // A client drops the level it shows, its chunks and its entities.
            ClientboundPlay::Respawn(respawn) => {
                self.chunks.clear();
                self.entities.clear();
                self.respawns.push(respawn);
            }
            ClientboundPlay::SetHeadRotation(head) => {
                self.named.push((Naming::HeadRotation, head.entity_id));
            }
            ClientboundPlay::PlayerInfoUpdate(update) => {
                if update.actions & player_info::ADD_PLAYER != 0 {
                    let listed = update.entries.iter().map(|entry| Told::Listed(entry.uuid));
                    self.told.extend(listed);
                }
            }
            ClientboundPlay::PlayerInfoRemove(removed) => {
                self.told
                    .extend(removed.players.into_iter().map(Told::Unlisted));
            }
            other => self.others.push(other),
        }
    }

    /// Takes everything the edge has sent so far.
    fn drain(&mut self) {
        loop {
            match self.packets.try_recv() {
                Ok(packet) => self.take(packet),
                Err(mpsc::error::TryRecvError::Empty) => return,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.closed = true;
                    return;
                }
            }
        }
    }

    /// Whether the edge still serves the player: their queue is open.
    fn connected(&mut self) -> bool {
        self.drain();
        !self.closed
    }

    /// What the client shows at `block_of(chunk)`, if it has the chunk and that is
    /// one of the few states these tests put there: by the chunk as it was sent
    /// last, or by what the client was told of the block since.
    fn state(&self, chunk: ChunkPos) -> Option<BlockState> {
        let sent = self.chunks.get(&chunk)?;
        match self.blocks.get(&block_of(chunk)) {
            Some(state) => Some(BlockState(*state as u16)),
            None => *sent,
        }
    }

    /// How often the client was sent `chunk`.
    fn times_sent(&self, chunk: ChunkPos) -> usize {
        self.sent.iter().filter(|sent| **sent == chunk).count()
    }

    /// Waits until the connection is ended by the edge.
    async fn disconnected(&mut self) {
        let ended = async {
            while let Some(packet) = self.packets.recv().await {
                self.take(packet);
            }
        };
        timeout(SOON, ended)
            .await
            .expect("the player was not disconnected");
        self.closed = true;
    }
}

fn player(number: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(number))
}

/// The chunks someone in `centre` sees.
fn view(centre: ChunkPos) -> BTreeSet<ChunkPos> {
    view_area(centre, VIEW)
}

/// A place to stand in `chunk`.
fn within(chunk: ChunkPos) -> Vec3 {
    Vec3::new(
        f64::from(chunk.x) * 16.0 + 0.5,
        -60.0,
        f64::from(chunk.z) * 16.0 + 0.5,
    )
}

/// A block of `chunk`.
fn block_of(chunk: ChunkPos) -> BlockPos {
    BlockPos::new(chunk.x * 16 + 3, -60, chunk.z * 16 + 5)
}

/// How many sections a chunk of these tests has, and where [`block_of`] a chunk
/// is in a chunk's packet: the section, and the place in it.
const SECTIONS: usize = 24;
const TELLING: (usize, usize) = (0, 4 << 8 | 5 << 4 | 3);

/// Gives `chunk` the light of its blocks, as a generator's chunk comes with it:
/// made by the light rules beside chunks of one generator. Said after the last
/// change to the chunk, which would take it away.
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
            pose: Pose::at(SPAWN),
            flying: false,
            inventory: Inventory::default(),
            selected_slot: 0,
        },
    }
}

/// A player as a region lets them go, standing in `chunk`.
fn transfer(entity: EntityId, last_input: u64, chunk: ChunkPos) -> PlayerTransfer {
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
        name: "Player".to_owned(),
        pose: Pose::at(within(chunk)),
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

/// The outbox entry with which a region lets a player go to `to`, into `chunk`.
fn departed(player: PlayerId, entity: EntityId, to: RegionId, chunk: ChunkPos) -> Durable {
    Durable::Departed {
        player,
        transfer: transfer(entity, 0, chunk),
        to,
    }
}

/// The outbox entry with which a region sends an arrival on to `holder`.
fn sent_on(player: PlayerId, entity: EntityId, holder: RegionId, chunk: ChunkPos) -> Durable {
    Durable::NotMine {
        what: Misdirected::Arrival {
            player,
            transfer: transfer(entity, 0, chunk),
        },
        holder,
    }
}

/// A region's word that `entity` has walked from one chunk into another.
fn walked(entity: EntityId, from: ChunkPos, to: ChunkPos) -> WorkerToEdge {
    WorkerToEdge::TickDelta {
        tick: 1,
        events: vec![RegionEvent::EntityMoved {
            entity,
            pose: Pose::at(within(to)),
            previous_chunk: from,
        }],
    }
}

fn empty_chunk() -> Chunk {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == OVERWORLD)
        .expect("the overworld is a dimension");
    // The light of an empty chunk is the same every time, and these tests make
    // thousands of them.
    static EMPTY: std::sync::OnceLock<Chunk> = std::sync::OnceLock::new();
    EMPTY
        .get_or_init(|| lit(Chunk::empty(overworld, clustine_world::Biome(0))))
        .clone()
}

/// A chunk that is empty but for `state` at `block_of` it.
fn chunk_with(position: ChunkPos, state: BlockState) -> Chunk {
    // Where the block is in its chunk is the same for every chunk, so the light
    // is one for each state.
    static WITH: std::sync::Mutex<BTreeMap<BlockState, Chunk>> =
        std::sync::Mutex::new(BTreeMap::new());
    let mut made = WITH.lock().expect("no test panics while it holds this");
    let chunk = made.entry(state).or_insert_with(|| {
        let mut chunk = empty_chunk();
        let block = block_of(position);
        let (x, z) = (block.x.rem_euclid(16), block.z.rem_euclid(16));
        chunk
            .set(x as usize, block.y, z as usize, state)
            .expect("the block is within the chunk");
        lit(chunk)
    });
    chunk.clone()
}

/// An empty chunk, as a region sends it in answer to a subscription numbered `ask`.
fn snapshot(position: ChunkPos, ask: u64) -> WorkerToEdge {
    snapshot_of(position, ask, empty_chunk(), Vec::new())
}

fn snapshot_of(
    position: ChunkPos,
    ask: u64,
    chunk: Chunk,
    entities: Vec<EntityState>,
) -> WorkerToEdge {
    WorkerToEdge::ChunkSnapshot {
        position,
        ask,
        tick: 1,
        chunk,
        entities,
    }
}

fn elsewhere(chunk: ChunkPos, ask: u64, region: RegionId) -> WorkerToEdge {
    WorkerToEdge::Elsewhere { chunk, ask, region }
}

fn not_mine(chunk: ChunkPos, ask: u64) -> WorkerToEdge {
    WorkerToEdge::NotMine { chunk, ask }
}

/// A region's word that the block at `block_of(chunk)` has become `state`.
fn changed(chunk: ChunkPos, state: BlockState) -> WorkerToEdge {
    WorkerToEdge::TickDelta {
        tick: 2,
        events: vec![RegionEvent::BlockChanged {
            position: block_of(chunk),
            state,
            seen: true,
        }],
    }
}

/// A region's word that the player is in it, as `entity`.
fn present(player: PlayerId, entity: EntityId) -> WorkerToEdge {
    WorkerToEdge::Presence {
        player,
        answer: Presence::Present {
            last_death: None,
            mode: Default::default(),
            riding: None,
            entity,
            pose: Pose::at(SPAWN),
            inventory: Inventory::default(),
            selected_slot: 0,
            last_input: 0,
            handled: None,
            flying: false,
            attempt: None,
        },
    }
}

fn ordered(chunks: impl IntoIterator<Item = ChunkPos>) -> Vec<ChunkPos> {
    let mut chunks: Vec<_> = chunks.into_iter().collect();
    chunks.sort();
    chunks
}

fn subscribe(ask: u64, chunks: impl IntoIterator<Item = ChunkPos>) -> EdgeToWorker {
    let chunks = ordered(chunks);
    EdgeToWorker::Subscribe { ask, chunks }
}

fn as_guest(ask: u64, chunks: impl IntoIterator<Item = ChunkPos>) -> EdgeToWorker {
    let chunks = ordered(chunks);
    EdgeToWorker::SubscribeAsGuest { ask, chunks }
}

fn unsubscribe(ask: u64, chunks: impl IntoIterator<Item = ChunkPos>) -> EdgeToWorker {
    let chunks = ordered(chunks);
    EdgeToWorker::Unsubscribe { ask, chunks }
}

/// What a subscription message makes of the chunks it names (nothing, if it ends
/// their subscriptions), and those chunks.
fn meaning(body: &EdgeToWorker) -> (Option<Role>, BTreeSet<ChunkPos>) {
    let (role, chunks) = match body {
        EdgeToWorker::Subscribe { chunks, .. } => (Some(Role::Viewer), chunks),
        EdgeToWorker::SubscribeAsGuest { chunks, .. } => (Some(Role::Guest), chunks),
        EdgeToWorker::Unsubscribe { chunks, .. } => (None, chunks),
        other => panic!("{other:?} is not about subscriptions"),
    };
    (role, chunks.iter().copied().collect())
}

/// What some subscription messages make of the chunks they name, without the order
/// and the numbers they came with.
fn meanings(asked: &[EdgeToWorker]) -> BTreeSet<(Option<Role>, BTreeSet<ChunkPos>)> {
    asked.iter().map(meaning).collect()
}

/// Whether a subscription message names `chunk`.
fn names(body: &EdgeToWorker, chunk: ChunkPos) -> bool {
    is_about_subscriptions(body) && meaning(body).1.contains(&chunk)
}

/// The numbered messages among `said`, each with its number.
fn numbered(said: &[EdgeMessage]) -> Vec<(u64, EdgeToWorker)> {
    said.iter()
        .filter_map(|message| Some((message.number?, message.body.clone())))
        .collect()
}

/// A player's step into `chunk`.
fn step_into(chunk: ChunkPos) -> PlayerInput {
    PlayerInput::Move {
        position: Some(within(chunk)),
        rotation: None,
        on_ground: true,
    }
}

/// What is left of the breaking of the block at `block_of(chunk)` by the stay
/// `entity` of `player`.
fn breaking(player: PlayerId, entity: EntityId, sequence: i32, chunk: ChunkPos) -> RemoteAction {
    RemoteAction {
        player,
        entity,
        sequence,
        step: RemoteStep::Break {
            position: block_of(chunk),
        },
    }
}

/// The outbox entry with which a region passes an action on, to a region or to
/// whoever serves the edge the chunk.
fn remote(action: &RemoteAction, to: Option<RegionId>) -> Durable {
    Durable::Remote {
        action: action.clone(),
        to,
    }
}

/// Someone who is not this edge's, standing in `chunk`.
fn stranger(entity: EntityId, chunk: ChunkPos) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(entity.0 as u128 + 1000),
            name: format!("Stranger{}", entity.0),
        },
        pose: Pose::at(within(chunk)),
    }
}

fn both(one: &BTreeSet<ChunkPos>, other: &BTreeSet<ChunkPos>) -> BTreeSet<ChunkPos> {
    one.intersection(other).copied().collect()
}

fn minus(one: &BTreeSet<ChunkPos>, other: &BTreeSet<ChunkPos>) -> BTreeSet<ChunkPos> {
    one.difference(other).copied().collect()
}

fn set(chunks: &[ChunkPos]) -> BTreeSet<ChunkPos> {
    chunks.iter().copied().collect()
}

/// The few states these tests put at `block_of` a chunk.
const AIR: BlockState = BlockState(0);
const STONE: BlockState = BlockState(1);
const GRANITE: BlockState = BlockState(2);
const STATES: [BlockState; 3] = [AIR, STONE, GRANITE];

/// Where a player stands whom these tests hand to the eastern region, and one they
/// hand to the northern.
const EASTERN: ChunkPos = ChunkPos::new(4, 0);
const NORTHERN: ChunkPos = ChunkPos::new(0, -4);
/// A chunk that is seen from `HOME`, from `EASTERN` and from `NORTHERN`.
const COMMON: ChunkPos = ChunkPos::new(2, -2);
/// A chunk that is seen from `HOME` and from `NORTHERN`, and that someone who walks
/// a chunk east from `HOME` sees no more.
const FAR: ChunkPos = ChunkPos::new(-3, -1);
/// The chunk east of `HOME`.
const STEP_EAST: ChunkPos = ChunkPos::new(1, 0);

/// Scenario 1. Everything a player sees is asked of the player's region, which for
/// one who joins is the home region, in one message.
#[tokio::test]
async fn a_player_who_joins_has_their_view_asked_of_the_home_region_and_of_no_other() {
    let mut edge = Harness::start().await;
    let _client = edge.join(player(1)).await;
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(
        number == 1 && matches!(join, EdgeToWorker::PlayerJoin(_)),
        "{join:?}"
    );
    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    assert_eq!(edge.asked(WEST).await, [subscribe(1, view(HOME))]);
    for region in [EAST, NORTH] {
        let said = edge.said(region).await;
        assert!(said.is_empty(), "{region:?} was sent {said:?}");
    }
    edge.end().await;
}

/// Scenario 2. The edge becomes a guest where a region names another, once: a
/// third region that names the same holder finds the subscription made.
#[tokio::test]
async fn a_region_named_for_a_chunk_by_two_others_is_asked_as_a_guest_once() {
    let mut edge = Harness::start().await;
    let _first = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, COMMON);
    edge.tell(WEST, elsewhere(COMMON, ask, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    // A second player becomes the northern region's and sees the chunk from there.
    let _second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
        .await;
    edge.quiet().await;
    assert_eq!(edge.at(NORTH).chunks(Role::Viewer), view(NORTHERN));
    let ask = edge.ask(NORTH, COMMON);
    edge.tell(NORTH, elsewhere(COMMON, ask, EAST));
    edge.settle(NORTH).await;
    let asked = edge.asked(EAST).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[COMMON]));
    edge.end().await;
}

/// Scenario 3. A snapshot with the number its subscription began with is the
/// region's word that it serves the chunk.
#[tokio::test]
async fn a_snapshot_with_the_number_its_subscription_began_with_is_shown() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    assert!(!edge.shows(&mut client, WEST, HOME).await);
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    assert_eq!(client.state(HOME), Some(AIR));
    edge.end().await;
}

/// Scenario 3. A viewer's subscription that waits becomes a guest's when its last
/// viewer goes and another region's player still sees the chunk. The region had
/// made its snapshot before it heard of that, and does not answer the change.
#[tokio::test]
async fn a_snapshot_is_shown_when_its_viewers_subscription_has_become_a_guests_since() {
    let mut edge = Harness::start().await;
    let first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
        .await;
    edge.quiet().await;
    let began = edge.ask(WEST, COMMON);

    // The first goes. The second, who is the northern region's, still sees it.
    edge.leave(&first).await;
    edge.quiet().await;
    let (role, later) = edge.at(WEST).subscriptions[&COMMON];
    assert_eq!(role, Role::Guest);
    assert!(later > began, "{later} {began}");
    assert!(!edge.shows(&mut second, WEST, COMMON).await);
    edge.tell(WEST, snapshot(COMMON, began));
    assert!(edge.shows(&mut second, WEST, COMMON).await);
    edge.end().await;
}

/// Scenario 3, and several changes to one subscription that the edge reads in one
/// go: a chunk that leaves a view and comes back is an `Unsubscribe` and a
/// `Subscribe`, and begins anew. A snapshot numbered below that is of the
/// subscription that was ended.
#[tokio::test]
async fn a_chunk_that_leaves_a_view_and_comes_back_is_asked_for_anew() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let before = edge.ask(WEST, FAR);
    edge.quiet().await;

    // Both steps are on the link before the edge reads either.
    edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
    edge.tell(WEST, walked(EntityId(5), STEP_EAST, HOME));
    let asked = edge.asked(WEST).await;
    let about: Vec<_> = asked.iter().filter(|body| names(body, FAR)).collect();
    let after = edge.ask(WEST, FAR);
    let [
        EdgeToWorker::Unsubscribe { ask: ended, .. },
        EdgeToWorker::Subscribe { ask: begun, .. },
    ] = about[..]
    else {
        panic!("{about:?}");
    };
    assert!(
        before < *ended && ended < begun && *begun == after,
        "{about:?}"
    );
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));

    edge.tell(WEST, snapshot(FAR, before));
    assert!(!edge.shows(&mut client, WEST, FAR).await);
    edge.tell(WEST, snapshot(FAR, after));
    assert!(edge.shows(&mut client, WEST, FAR).await);
    edge.end().await;
}

/// Scenario 3. An answer for a chunk the edge has no subscription for at that
/// region is passed over.
#[tokio::test]
async fn a_snapshot_from_a_region_the_edge_has_no_subscription_at_is_not_shown() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    for number in [0, ask, ask + 1] {
        edge.tell(EAST, snapshot(HOME, number));
    }
    assert!(!edge.shows(&mut client, EAST, HOME).await);
    // The region that was asked is still listened to.
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    edge.end().await;
}

/// Scenario 4. The edge is a guest at the east and waits; the east makes its
/// snapshot; the player is handed to the east, which makes the subscription a
/// viewer's under a later number; then the snapshot is read, with the guest's
/// number. The east does not answer the change, so this is the only answer there
/// will be.
#[tokio::test]
async fn a_guests_snapshot_in_flight_is_shown_when_the_player_has_been_handed_there_since() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, COMMON);
    edge.tell(WEST, elsewhere(COMMON, ask, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    let (role, later) = edge.at(EAST).subscriptions[&COMMON];
    assert_eq!(role, Role::Viewer);
    assert!(later > 1, "{later}");
    assert!(!edge.shows(&mut client, EAST, COMMON).await);

    edge.tell(EAST, snapshot(COMMON, 1));
    assert!(edge.shows(&mut client, EAST, COMMON).await);
    // And nothing more is asked of anyone.
    edge.settle(WEST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.end().await;
}

/// The edge as a guest at the east for `COMMON`, on a link the east has just been
/// said hello on, with its player handed to the east since: `Subscribe` 1 is out
/// while the region still holds back everything behind the hello.
async fn handed_into_a_hold() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, COMMON);
    edge.tell(WEST, elsewhere(COMMON, ask, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    let hello = edge.relink(EAST).await;
    assert!(hello.chunks.is_empty(), "{hello:?}");
    assert_eq!(hello.guests, [COMMON]);
    edge.resume(EAST);
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    assert_eq!(edge.at(EAST).subscriptions[&COMMON], (Role::Viewer, 1));
    (edge, client)
}

/// Scenario 5. During a resume's hold a region answers the hello's chunks before it
/// looks at anything behind the hello, so its answers are numbered 0 whatever the
/// edge has sent meanwhile.
#[tokio::test]
async fn a_snapshot_numbered_0_is_shown_although_subscribe_1_was_sent_meanwhile() {
    let (mut edge, mut client) = handed_into_a_hold().await;
    assert!(!edge.shows(&mut client, EAST, COMMON).await);
    edge.tell(EAST, snapshot(COMMON, 0));
    assert!(edge.shows(&mut client, EAST, COMMON).await);
    edge.settle(WEST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.end().await;
}

/// Scenario 5, the other answer a guest gets. The east said `NotMine` to the
/// hello's guest before it read `Subscribe` 1, which then made a viewer's
/// subscription of its own there. The `NotMine` is about what the edge has changed
/// since, and the answer to the viewer's subscription is still to come.
#[tokio::test]
async fn a_not_mine_numbered_0_is_passed_over_when_subscribe_1_was_sent_meanwhile() {
    let (mut edge, mut client) = handed_into_a_hold().await;
    edge.tell(EAST, not_mine(COMMON, 0));
    edge.settle(EAST).await;
    edge.settle(WEST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.tell(EAST, snapshot(COMMON, 1));
    assert!(edge.shows(&mut client, EAST, COMMON).await);
    edge.end().await;
}

/// Scenario 6: the edge is a guest at the east for `FAR`, which the west named, and
/// its one player walks a chunk east, out of sight of it.
async fn a_chunk_nobody_sees_any_more_is_ended_everywhere(served: bool) {
    let mut edge = Harness::start().await;
    let _client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, FAR);
    edge.tell(WEST, elsewhere(FAR, ask, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [FAR])]);
    if served {
        edge.tell(EAST, snapshot(FAR, 1));
    }
    edge.quiet().await;

    edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [unsubscribe(2, [FAR])]);
    let asked = edge.asked(WEST).await;
    let expected = BTreeSet::from([
        (None, minus(&view(HOME), &view(STEP_EAST))),
        (Some(Role::Viewer), minus(&view(STEP_EAST), &view(HOME))),
    ]);
    assert_eq!(meanings(&asked), expected);
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(STEP_EAST));
    assert!(edge.at(EAST).subscriptions.is_empty());
    edge.end().await;
}

/// Scenario 6.
#[tokio::test]
async fn a_chunk_that_leaves_the_last_view_is_ended_at_the_region_that_was_to_serve_it() {
    a_chunk_nobody_sees_any_more_is_ended_everywhere(false).await;
}

/// Scenario 6.
#[tokio::test]
async fn a_chunk_that_leaves_the_last_view_is_ended_at_the_region_that_served_it() {
    a_chunk_nobody_sees_any_more_is_ended_everywhere(true).await;
}

/// Scenario 6, the other way round: a guest's subscription is ended only when
/// nobody sees the chunk. Here a player of a third region still does.
#[tokio::test]
async fn a_chunk_that_leaves_one_view_stays_subscribed_where_another_regions_player_sees_it() {
    let mut edge = Harness::start().await;
    let _first = edge.joined(player(1), EntityId(5)).await;
    let _second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
        .await;
    edge.quiet().await;
    // Both regions name the east, which serves the chunk.
    edge.tell(WEST, elsewhere(FAR, edge.ask(WEST, FAR), EAST));
    edge.tell(NORTH, elsewhere(FAR, edge.ask(NORTH, FAR), EAST));
    edge.settle(WEST).await;
    edge.settle(NORTH).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [FAR])]);
    edge.quiet().await;

    // The first walks out of sight of it. Their region's subscription carried
    // nothing and is ended; the guest's at the east is what the second sees.
    edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
    edge.settle(WEST).await;
    let asked = edge.asked(EAST).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert!(!edge.at(WEST).subscriptions.contains_key(&FAR));
    assert_eq!(edge.at(NORTH).subscriptions[&FAR].0, Role::Viewer);
    assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[FAR]));
    edge.end().await;
}

/// Scenario 7. Two players of two regions see one chunk, which the first one's
/// region serves; the second one's region says so. When the first player goes,
/// their region's subscriptions become guest's for what the second still sees,
/// whatever they wait for, and are ended for the rest.
#[tokio::test]
async fn what_another_regions_player_still_sees_stays_subscribed_as_a_guests_and_shown() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
    edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
    edge.settle(EAST).await;
    // The edge is subscribed at the west already.
    let asked = edge.asked(WEST).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert!(edge.shows(&mut first, WEST, COMMON).await);
    assert!(edge.shows(&mut second, WEST, COMMON).await);
    let sent = second.times_sent(COMMON);
    edge.quiet().await;

    edge.leave(&first).await;
    let asked = edge.asked(WEST).await;
    let still_seen = both(&view(HOME), &view(EASTERN));
    let expected = BTreeSet::from([
        (Some(Role::Guest), still_seen.clone()),
        (None, minus(&view(HOME), &view(EASTERN))),
    ]);
    assert_eq!(meanings(&asked), expected);
    assert_eq!(edge.at(WEST).chunks(Role::Guest), still_seen);
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    // The east was a guest's region too, for what the second player saw when they
    // were handed over and the first went on seeing. Nobody sees that now.
    let asked = edge.asked(EAST).await;
    let unseen = BTreeSet::from([(None, minus(&view(HOME), &view(EASTERN)))]);
    assert_eq!(meanings(&asked), unseen);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
    assert!(edge.at(EAST).chunks(Role::Guest).is_empty());

    // The chunk stays on the second player's screen as it is, and what happens
    // in it is still shown.
    edge.sync(&mut second).await;
    assert!(second.chunks.contains_key(&COMMON));
    assert_eq!(second.times_sent(COMMON), sent);
    edge.tell(WEST, changed(COMMON, STONE));
    edge.settle(WEST).await;
    edge.sync(&mut second).await;
    assert_eq!(second.state(COMMON), Some(STONE));
    edge.end().await;
}

/// Scenario 8: the same while the first region has no link. Its subscriptions wait
/// then, whatever they were, and are not ended because they wait: the hello of the
/// next link names what the second player still sees among the guests'.
async fn the_last_player_of_a_region_without_a_link_goes(served: bool) {
    let mut edge = Harness::start().await;
    let first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
    if served {
        edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
    }
    edge.quiet().await;
    edge.sync(&mut second).await;
    assert_eq!(second.chunks.contains_key(&COMMON), served);

    edge.lose(WEST).await;
    edge.leave(&first).await;
    // Of what the east was asked for, nobody sees now what only the first saw.
    let asked = edge.asked(EAST).await;
    let unseen = BTreeSet::from([(None, minus(&view(HOME), &view(EASTERN)))]);
    assert_eq!(meanings(&asked), unseen);
    let hello = edge.relink(WEST).await;
    assert!(hello.players.is_empty(), "{hello:?}");
    assert!(hello.chunks.is_empty(), "{hello:?}");
    assert_eq!(set(&hello.guests), both(&view(HOME), &view(EASTERN)));
    // What was on the second player's screen has stayed there.
    edge.sync(&mut second).await;
    assert_eq!(second.chunks.contains_key(&COMMON), served);

    edge.resume(WEST);
    let chunk = chunk_with(COMMON, STONE);
    edge.tell(WEST, snapshot_of(COMMON, 0, chunk, Vec::new()));
    edge.settle(WEST).await;
    edge.sync(&mut second).await;
    assert_eq!(second.state(COMMON), Some(STONE));
    let asked = edge.asked(EAST).await;
    assert!(asked.is_empty(), "{asked:?}");
    edge.end().await;
}

/// Scenario 8, with a chunk the first region had not served yet.
#[tokio::test]
async fn a_chunk_waited_for_at_a_region_without_a_link_is_a_guests_in_its_next_hello() {
    the_last_player_of_a_region_without_a_link_goes(false).await;
}

/// Scenario 8, with a chunk the first region had served: the snapshot on the new
/// link is reconciled with what is shown.
#[tokio::test]
async fn a_chunk_served_by_a_region_without_a_link_is_a_guests_in_its_next_hello() {
    the_last_player_of_a_region_without_a_link_goes(true).await;
}

/// The edge with one player in the west, who sees `COMMON`. The west has named the
/// east for it twice and the east has said `NotMine` twice, the second time less
/// than a second after the west was asked again: another asking is due.
async fn asked_again_and_due() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    // The east does not hold it: the west is asked again, under a new number.
    edge.tell(EAST, not_mine(COMMON, 1));
    edge.settle(EAST).await;
    assert_eq!(edge.asked(WEST).await, [subscribe(2, [COMMON])]);
    assert!(edge.at(EAST).subscriptions.is_empty());

    // The west names the east once more, and the east says the same again.
    edge.tell(WEST, elsewhere(COMMON, 2, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(2, [COMMON])]);
    edge.tell(EAST, not_mine(COMMON, 2));
    edge.settle(EAST).await;
    let asked = edge.asked(WEST).await;
    assert!(asked.is_empty(), "asked again within a second: {asked:?}");
    (edge, client)
}

/// Scenario 9. The clock of this test stands still until the test moves it.
#[tokio::test(start_paused = true)]
async fn a_guest_told_not_mine_has_the_viewers_region_asked_again_at_most_once_a_second() {
    let (mut edge, mut client) = asked_again_and_due().await;
    tokio::time::advance(Duration::from_millis(1100)).await;
    assert_eq!(edge.next_asked(WEST).await, subscribe(3, [COMMON]));
    // The asking is one like any other: its answer is taken.
    edge.tell(WEST, snapshot(COMMON, 3));
    assert!(edge.shows(&mut client, WEST, COMMON).await);
    edge.end().await;
}

/// Scenario 9. An asking that is due is dropped with its subscription.
#[tokio::test(start_paused = true)]
async fn a_subscription_that_ends_while_an_asking_is_due_is_not_asked_again() {
    let (mut edge, _client) = asked_again_and_due().await;
    // The player walks two chunks south, out of sight of the chunk.
    let (south, further) = (ChunkPos::new(0, 1), ChunkPos::new(0, 2));
    edge.tell(WEST, walked(EntityId(5), HOME, south));
    edge.tell(WEST, walked(EntityId(5), south, further));
    edge.quiet().await;
    assert!(!edge.at(WEST).subscriptions.contains_key(&COMMON));
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(1100)).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
    }
    edge.end().await;
}

/// Scenario 9. An asking that is due is dropped when the link is replaced: the
/// hello asks.
#[tokio::test(start_paused = true)]
async fn a_hello_takes_the_place_of_an_asking_that_was_due() {
    let (mut edge, _client) = asked_again_and_due().await;
    let hello = edge.relink(WEST).await;
    assert_eq!(set(&hello.chunks), view(HOME));
    edge.resume(WEST);
    edge.tell(WEST, present(player(1), EntityId(5)));
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(1100)).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
    }
    // The answer to the hello is taken like the answer to an asking.
    edge.tell(WEST, elsewhere(COMMON, 0, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(3, [COMMON])]);
    edge.end().await;
}

/// Scenario 10. An `Elsewhere` is taken only with the number of the edge's last
/// message about the chunk: an older one is about an asking the edge has made
/// again since.
#[tokio::test]
async fn an_elsewhere_with_an_old_number_changes_nothing() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);
    edge.tell(EAST, not_mine(COMMON, 1));
    edge.settle(EAST).await;
    assert_eq!(edge.asked(WEST).await, [subscribe(2, [COMMON])]);

    // Two answers to the first asking, read late.
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.tell(WEST, elsewhere(COMMON, 1, NORTH));
    edge.settle(WEST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    // The subscription waits as before: the answer to the second asking is taken.
    edge.tell(WEST, snapshot(COMMON, 2));
    assert!(edge.shows(&mut client, WEST, COMMON).await);
    edge.end().await;
}

/// Scenario 10. `NotMine` answers a guest's subscription; for one the edge holds as
/// a viewer's it changes nothing.
#[tokio::test]
async fn a_not_mine_for_a_viewers_subscription_changes_nothing() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, not_mine(COMMON, 1));
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.tell(WEST, snapshot(COMMON, 1));
    assert!(edge.shows(&mut client, WEST, COMMON).await);
    edge.end().await;
}

/// Scenario 10. `Elsewhere` answers a viewer's subscription; for one the edge
/// holds as a guest's it changes nothing, and the region it names is not asked.
#[tokio::test]
async fn an_elsewhere_for_a_guests_subscription_changes_nothing() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    edge.tell(EAST, elsewhere(COMMON, 1, NORTH));
    edge.settle(EAST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.tell(EAST, snapshot(COMMON, 1));
    assert!(edge.shows(&mut client, EAST, COMMON).await);
    edge.end().await;
}

/// Scenario 10. A region that names itself as the holder elsewhere has made a
/// mistake, which is passed over.
#[tokio::test]
async fn an_elsewhere_that_names_the_region_that_says_it_changes_nothing() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, WEST));
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    assert_eq!(edge.at(WEST).subscriptions[&COMMON], (Role::Viewer, 1));
    edge.tell(WEST, snapshot(COMMON, 1));
    assert!(edge.shows(&mut client, WEST, COMMON).await);
    edge.end().await;
}

/// Scenario 10. A `NotMine` is taken only with the number of the edge's last
/// message about the chunk. Here the east said it to the first asking as a guest;
/// since then a player of the east has come and gone, so the east has been asked
/// as a viewer and as a guest again, and has yet to answer that.
#[tokio::test]
async fn a_not_mine_with_an_old_number_does_not_end_a_guests_subscription() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    let second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    assert_eq!(edge.at(EAST).subscriptions[&COMMON].0, Role::Viewer);
    edge.leave(&second).await;
    edge.quiet().await;
    let (role, last) = edge.at(EAST).subscriptions[&COMMON];
    assert_eq!(role, Role::Guest);

    edge.tell(EAST, not_mine(COMMON, 1));
    edge.settle(EAST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.tell(EAST, snapshot(COMMON, last));
    assert!(edge.shows(&mut first, EAST, COMMON).await);
    edge.end().await;
}

/// Scenario 11: a player in the west and one in the east. The west has named the
/// east for `told`, which both see, and serves `lent` to the east's player as a
/// guest. Then the west's link is replaced, after it ended or while it stands.
async fn a_hello_names_the_subscriptions_by_kind(ended: bool) {
    let told = ChunkPos::new(3, 0);
    let lent = ChunkPos::new(5, 0);
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(told, edge.ask(WEST, told), EAST));
    edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
    edge.tell(EAST, elsewhere(lent, edge.ask(EAST, lent), WEST));
    edge.settle(EAST).await;
    assert_eq!(edge.asked(WEST).await, [as_guest(2, [lent])]);
    edge.tell(WEST, snapshot(lent, 2));
    edge.quiet().await;
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut second, WEST, lent).await);
    let asked = edge.asked(EAST).await;
    assert!(asked.is_empty(), "{asked:?}");

    if ended {
        edge.lose(WEST).await;
    }
    let hello = edge.relink(WEST).await;
    assert_eq!(hello.players, [player(1)]);
    // The viewer's subscriptions, the one told elsewhere among them, and the
    // guest's.
    assert_eq!(set(&hello.chunks), view(HOME));
    assert_eq!(hello.chunks.len(), view(HOME).len());
    assert_eq!(hello.guests, [lent]);
    // Nothing leaves a screen because a link was replaced.
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert!(first.chunks.contains_key(&HOME));
    assert!(second.chunks.contains_key(&lent));

    // The answers to the hello are numbered 0.
    edge.resume(WEST);
    edge.tell(WEST, present(player(1), EntityId(5)));
    let chunk = chunk_with(HOME, STONE);
    edge.tell(WEST, snapshot_of(HOME, 0, chunk, Vec::new()));
    let chunk = chunk_with(lent, GRANITE);
    edge.tell(WEST, snapshot_of(lent, 0, chunk, Vec::new()));
    edge.tell(WEST, elsewhere(told, 0, EAST));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert_eq!(first.state(HOME), Some(STONE));
    assert_eq!(second.state(lent), Some(GRANITE));
    // The east is subscribed to what the west names it for.
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }

    // The numbers of the new link begin at 1.
    edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
    let asked = edge.asked(WEST).await;
    let numbers: Vec<_> = asked
        .iter()
        .map(|body| match body {
            EdgeToWorker::Subscribe { ask, .. }
            | EdgeToWorker::SubscribeAsGuest { ask, .. }
            | EdgeToWorker::Unsubscribe { ask, .. } => *ask,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(numbers, [1, 2], "{asked:?}");
    edge.end().await;
}

/// Scenario 11.
#[tokio::test]
async fn the_hello_on_a_link_that_replaces_one_that_stands_names_the_subscriptions_by_kind() {
    a_hello_names_the_subscriptions_by_kind(false).await;
}

/// Scenario 11.
#[tokio::test]
async fn the_hello_on_a_link_after_one_that_ended_names_the_subscriptions_by_kind() {
    a_hello_names_the_subscriptions_by_kind(true).await;
}

/// An edge whose first player is the west's and whose second is the east's, the
/// west serving both of them `COMMON`. Then the west answers a hello as one that
/// has forgotten the edge.
async fn forgotten_by_the_west() -> (Harness, Client, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
    edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
    edge.quiet().await;
    assert!(edge.shows(&mut second, WEST, COMMON).await);
    edge.quiet().await;

    let hello = edge.relink(WEST).await;
    assert_eq!(hello.players, [player(1)]);
    assert_eq!(hello.since, 1);
    let unknown = Welcome::Unknown {
        since: 2,
        entries: 0,
        presences: 1,
        applied: 0,
    };
    edge.tell(WEST, WorkerToEdge::Welcome(unknown));
    first.disconnected().await;
    // The region's outbox begins anew with the edge it did not know.
    edge.outbox[WEST.0 as usize] = 0;
    (edge, first, second)
}

/// Scenario 12. The players of a region that has forgotten the edge are gone, and
/// their chunks are given up as for anyone who goes: what a player of another
/// region still sees stays subscribed, as a guest's.
#[tokio::test]
async fn a_region_that_forgot_the_edge_keeps_as_guests_what_other_regions_players_see() {
    let (mut edge, _first, mut second) = forgotten_by_the_west().await;
    let said = edge.said(WEST).await;
    let left = EdgeToWorker::PlayerLeave {
        player: player(1),
        entity: Some(EntityId(5)),
        attempt: None,
    };
    assert_eq!(numbered(&said), [(1, left)]);
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    let still_seen = both(&view(HOME), &view(EASTERN));
    assert_eq!(edge.at(WEST).chunks(Role::Guest), still_seen);
    // The east loses only what nobody sees any more: its player sees what they saw.
    let asked = edge.asked(EAST).await;
    let unseen = BTreeSet::from([(None, minus(&view(HOME), &view(EASTERN)))]);
    assert_eq!(meanings(&asked), unseen);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
    edge.sync(&mut second).await;
    assert!(second.chunks.contains_key(&COMMON));

    // The edge says the region's new word for their numbering from now on.
    let hello = edge.relink(WEST).await;
    assert_eq!((hello.since, hello.seen), (2, edge.outbox[0]));
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(set(&hello.guests), still_seen);
    edge.resume(WEST);
    edge.end().await;
}

/// Scenario 12, and what a region that has forgotten the edge still serves it.
/// The replica has the west's snapshot of the chunk and the edge is still
/// subscribed there, so the west is the region that serves the chunk, as after any
/// link that ended. (The records do not say this of `Unknown` in so many words.)
#[tokio::test]
async fn a_region_that_forgot_the_edge_still_serves_the_chunks_it_served() {
    let (mut edge, _first, mut second) = forgotten_by_the_west().await;
    edge.quiet().await;
    let action = breaking(
        player(2),
        edge.entity_of(player(2)),
        edge.sequence(),
        COMMON,
    );
    edge.say(EAST, remote(&action, None));
    // Behind the leaving of the player who is gone, numbered from 1 again.
    let passed_on = EdgeToWorker::Remote(action);
    assert_eq!(edge.next_numbered(WEST).await, (2, passed_on));
    assert!(second.connected());
    edge.end().await;
}

/// Scenario 13. A player walks east until the next chunk is one their region says
/// the east holds, and is let go into it. On the link to the east the viewer's
/// subscriptions come before the arrival, so that one claim covers the chunk the
/// player arrives in and their view. The west goes on being asked, as a guest, for
/// what the player still sees of it, and for nothing else.
#[tokio::test]
async fn a_player_let_go_is_asked_for_at_the_new_region_before_they_arrive_there() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let mut at = HOME;
    for x in 1..=3 {
        let next = ChunkPos::new(x, 0);
        edge.tell(WEST, walked(EntityId(5), at, next));
        at = next;
    }
    edge.quiet().await;
    let old = view(at);
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), old);
    // Of what the player sees, the west says that the east holds the chunks from
    // x = 4 on, has served some of its own and has yet to serve the others.
    let theirs: BTreeSet<_> = old.iter().copied().filter(|chunk| chunk.x >= 4).collect();
    for chunk in &old {
        let ask = edge.ask(WEST, *chunk);
        if theirs.contains(chunk) {
            edge.tell(WEST, elsewhere(*chunk, ask, EAST));
        } else if chunk.x >= 2 {
            edge.tell(WEST, snapshot(*chunk, ask));
        }
    }
    edge.quiet().await;
    assert_eq!(edge.at(EAST).chunks(Role::Guest), theirs);

    edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
    edge.settle(WEST).await;
    let said = edge.said(EAST).await;
    let arrival = said
        .iter()
        .position(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
        .expect("the player is passed on");
    let mut before = BTreeSet::new();
    for message in &said[..arrival] {
        if let EdgeToWorker::Subscribe { chunks, .. } = &message.body {
            before.extend(chunks.iter().copied());
        }
    }
    assert!(before.is_superset(&old), "{before:?}");
    assert!(before.contains(&EASTERN));
    let passed_on = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, EASTERN),
    };
    assert_eq!(numbered(&said), [(1, passed_on)]);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
    assert!(edge.at(EAST).chunks(Role::Guest).is_empty());

    let asked = edge.asked(WEST).await;
    let still_seen: BTreeSet<_> = both(&old, &view(EASTERN))
        .into_iter()
        .filter(|chunk| !theirs.contains(chunk))
        .collect();
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    assert_eq!(edge.at(WEST).chunks(Role::Guest), still_seen);
    // A subscription that was told elsewhere carried nothing: it is ended, and
    // never made a guest's.
    for body in &asked {
        if let EdgeToWorker::SubscribeAsGuest { chunks, .. } = body {
            assert!(
                chunks.iter().all(|chunk| !theirs.contains(chunk)),
                "{body:?}"
            );
        }
    }
    assert!(client.connected());
    edge.end().await;
}

/// Where a portal in the west sends player 1 in the Nether.
const IN_THE_NETHER: ChunkPos = ChunkPos::new(-3, 2);

/// Player 1's transfer as the west lets them go through a Nether portal: in the
/// Nether's coordinates, with the travel (`docs/groundwork/p-plan.md`, section
/// 15.4).
fn through_a_portal(last_input: u64) -> PlayerTransfer {
    let mut moved = transfer(EntityId(5), last_input, IN_THE_NETHER);
    moved.dimension = Dimension::Nether;
    moved.travel = Some(clustine_world::Travel {
        to: Dimension::Nether,
        target: block_of(IN_THE_NETHER),
        way: clustine_world::Way::NetherPortal {
            axis: Some(clustine_world::PortalAxis::X),
            relative: [clustine_world::Double(0.5); 3],
        },
    });
    moved
}

/// An edge with the Nether's region linked, and player 1 in the west as entity 5,
/// their view served, let go through a portal to the Nether.
async fn sent_to_the_nether() -> (Harness, Client) {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    for chunk in view(HOME) {
        let ask = edge.ask(WEST, chunk);
        edge.tell(WEST, snapshot(chunk, ask));
    }
    edge.quiet().await;
    client.drain();
    assert!(!client.chunks.is_empty());
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: through_a_portal(0),
            to: NETHER,
        },
    );
    edge.settle(WEST).await;
    (edge, client)
}

/// Section 4.2, the first half: the region the player goes to is asked for their
/// new view and told of them before the client hears anything; the client is told
/// `Respawn` in the Nether, keeping its own entity's data, and the world's
/// difficulty, drops its level, and is sent nothing of the new one, chunks least
/// of all, until it is placed; its moves are held back as while a teleport waits.
#[tokio::test]
async fn a_player_sent_to_another_dimension_is_respawned_there_and_shown_nothing_before_they_are_placed()
 {
    let (mut edge, mut client) = sent_to_the_nether().await;
    let said = edge.said(NETHER).await;
    let arrival = said
        .iter()
        .position(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
        .expect("the player is passed on");
    let mut before = BTreeSet::new();
    for message in &said[..arrival] {
        if let EdgeToWorker::Subscribe { chunks, .. } = &message.body {
            before.extend(chunks.iter().copied());
        }
    }
    assert_eq!(before, view(IN_THE_NETHER));
    // Nothing of the overworld's view is asked of the west any more.
    edge.quiet().await;
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());

    client.drain();
    assert_eq!(client.respawns.len(), 1);
    let respawn = &client.respawns[0];
    assert_eq!(respawn.spawn.dimension_name, "minecraft:the_nether");
    assert_eq!(respawn.data_to_keep, Respawn::KEEP_ALL_DATA);
    assert!(
        respawn.spawn.is_flat,
        "a flat world is flat in every dimension, as the official server says"
    );
    assert_eq!(respawn.spawn.sea_level, 32);
    assert!(client.chunks.is_empty() && client.entities.is_empty());
    assert_eq!(client.awaiting.load(Ordering::Relaxed), TRAVELLING);

    // The Nether's chunks come, and are not sent before the player is placed.
    let sent = client.sent.len();
    for chunk in view(IN_THE_NETHER) {
        let ask = edge.ask(NETHER, chunk);
        edge.tell(NETHER, snapshot(chunk, ask));
    }
    edge.quiet().await;
    client.drain();
    assert_eq!(client.sent.len(), sent, "no chunk before the placing");
    edge.end().await;
}

/// An edge with player 1 in the west as entity 5, their view served.
async fn standing_in_the_west() -> (Harness, Client) {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    for chunk in view(HOME) {
        let ask = edge.ask(WEST, chunk);
        edge.tell(WEST, snapshot(chunk, ask));
    }
    edge.quiet().await;
    client.drain();
    (edge, client)
}

/// Where the tests of the last death have the player die: in the End, below it.
const LAST_DEATH: (Dimension, BlockPos) = (Dimension::End, BlockPos::new(-3, -81, 3));

/// The login's word of the last death, as the client reads it.
fn told_last_death(client: &Client) -> Option<clustine_protocol::packets::play::DeathLocation> {
    let [login] = &client.logins[..] else {
        panic!("one login: {:?}", client.logins);
    };
    login.spawn.death_location.clone()
}

/// The death of `LAST_DEATH` as the client is told it.
fn last_death_on_the_wire() -> clustine_protocol::packets::play::DeathLocation {
    clustine_protocol::packets::play::DeathLocation {
        dimension: "minecraft:the_end".to_owned(),
        position: Position {
            x: -3,
            y: -81,
            z: 3,
        },
    }
}

/// Section 19.9: a player who died before is put into the world by a login that
/// says where, as the game's `CommonPlayerSpawnInfo` does from
/// `ServerPlayer.getLastDeathLocation`.
#[tokio::test]
async fn a_login_says_where_the_player_last_died() {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    let mut spawned = edge.spawned(player(1), EntityId(5));
    let WorkerToEdge::ToPlayer {
        event: PlayerEvent::Spawned { last_death, .. },
        ..
    } = &mut spawned
    else {
        unreachable!("the word of a placing");
    };
    *last_death = Some(LAST_DEATH);
    edge.tell(WEST, spawned);
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(told_last_death(&client), Some(last_death_on_the_wire()));
    edge.end().await;
}

/// The same for a join whose stay its region let go before the edge was told its
/// entity (`docs/adr/0020-one-stay-per-player.md`, section 4.3, path 3): the
/// login is made from the transfer.
#[tokio::test]
async fn a_login_by_an_arrival_says_where_the_player_last_died() {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    let mut moved = transfer(EntityId(5), 0, EASTERN);
    moved.attempt = Some(edge.attempts[&player(1)]);
    moved.last_death = Some(LAST_DEATH);
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: moved,
            to: EAST,
        },
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(told_last_death(&client), Some(last_death_on_the_wire()));
    edge.end().await;
}

/// The packets the client was sent from `from` on, by id.
fn ids_from(client: &Client, from: usize) -> Vec<i32> {
    client.ids[from..].to_vec()
}

/// Section 19.7: a death owes the client its respawn. The departure the respawn
/// makes sends no `Respawn` at the hand-over, and what the region tells the player
/// meanwhile is held; the placing sends the abilities, the missing block's event,
/// `Respawn` with keep 0 and the difficulty, the place absolutely and still, the
/// level's information; and only then what was held.
#[tokio::test]
async fn a_respawn_after_a_death_is_sent_with_its_placing_and_what_was_told_meanwhile_after_it() {
    use clustine_protocol::packet_ids::play::clientbound as ids;
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.say(
        WEST,
        Durable::Died {
            player: player(1),
            entity: EntityId(5),
            message: clustine_sim::api::DeathMessage::plain("death.attack.outOfWorld"),
            at: (Dimension::Overworld, BlockPos::new(0, -130, 0)),
        },
    );
    edge.settle(WEST).await;
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Health {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            },
        },
    );
    let mut moved = transfer(EntityId(5), 0, EASTERN);
    moved.travel = Some(clustine_world::Travel {
        to: Dimension::Overworld,
        target: block_of(EASTERN),
        way: clustine_world::Way::Respawn,
    });
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: moved,
            to: EAST,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    client.drain();
    assert!(client.respawns.is_empty(), "no Respawn at the hand-over");
    assert!(
        !client.ids.contains(&ids::SET_HEALTH),
        "what the region told is held"
    );
    let from = client.ids.len();
    for chunk in view(EASTERN) {
        let ask = edge.ask(EAST, chunk);
        edge.tell(EAST, snapshot(chunk, ask));
    }
    let at = within(EASTERN);
    edge.say(
        EAST,
        Durable::Placed {
            player: player(1),
            entity: EntityId(5),
            dimension: Dimension::Overworld,
            pose: Pose {
                position: at,
                yaw: 45.0,
                pitch: 0.0,
                on_ground: true,
            },
            flying: false,
            respawn: Some(Respawned {
                keep: 0,
                no_respawn_block: true,
                anchor_sound: None,
            }),
            gateway: false,
        },
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let after = ids_from(&client, from);
    let order = [
        ids::PLAYER_ABILITIES,
        ids::GAME_EVENT,
        ids::RESPAWN,
        ids::PLAYER_POSITION,
        ids::SET_DEFAULT_SPAWN_POSITION,
        ids::CHANGE_DIFFICULTY,
    ];
    assert_eq!(after[..order.len()], order, "{after:?}");
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.data_to_keep, 0);
    assert_eq!(respawn.spawn.dimension_name, "minecraft:overworld");
    let teleport = client.teleports.last().expect("placed");
    assert_eq!((teleport.x, teleport.y, teleport.z), (at.x, at.y, at.z));
    assert_eq!((teleport.yaw, teleport.relative_flags), (45.0, 0));
    let spawn = after
        .iter()
        .rposition(|id| *id == ids::GAME_EVENT)
        .expect("the level's information, ending with the wait for chunks");
    let health = after
        .iter()
        .position(|id| *id == ids::SET_HEALTH)
        .expect("the held health is told");
    assert!(spawn < health, "{after:?}");
    assert_eq!(
        client.chunks.keys().copied().collect::<BTreeSet<_>>(),
        view(EASTERN)
    );
    edge.end().await;
}

/// Section 19.7: the credits' respawn placed by the region the player is in, with
/// no hand-over: `Respawn` with keep 1 at the placing, and the view sent again,
/// which the `Respawn` made the client drop.
#[tokio::test]
async fn a_respawn_after_the_credits_placed_where_the_player_is_sends_their_view_again() {
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.say(
        WEST,
        Durable::WonGame {
            player: player(1),
            entity: EntityId(5),
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    client.drain();
    let sent = client.sent.len();
    let at = within(HOME);
    edge.say(
        WEST,
        Durable::Placed {
            player: player(1),
            entity: EntityId(5),
            dimension: Dimension::Overworld,
            pose: Pose::at(at),
            flying: false,
            respawn: Some(Respawned {
                keep: Respawn::KEEP_ATTRIBUTE_MODIFIERS,
                no_respawn_block: false,
                anchor_sound: None,
            }),
            gateway: false,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    // The view is asked of the region again, and served anew.
    for chunk in view(HOME) {
        let ask = edge.ask(WEST, chunk);
        edge.tell(WEST, snapshot(chunk, ask));
    }
    edge.sync(&mut client).await;
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.data_to_keep, Respawn::KEEP_ATTRIBUTE_MODIFIERS);
    assert!(
        client.sent.len() > sent,
        "the view is sent again after the Respawn"
    );
    assert_eq!(
        client.chunks.keys().copied().collect::<BTreeSet<_>>(),
        view(HOME)
    );
    edge.end().await;
}

/// Section 4.2, the second half: once the Nether's region says it placed the
/// player, the client is sent where they are, as a teleport it must accept, their
/// abilities, the level's information ending with the wait for its chunks, and
/// the portal's sound; then its view and its chunks. The region is asked for the
/// inventory whole.
#[tokio::test]
async fn a_player_placed_in_the_nether_is_sent_where_they_are_and_then_the_nether() {
    let (mut edge, mut client) = sent_to_the_nether().await;
    edge.quiet().await;
    for chunk in view(IN_THE_NETHER) {
        let ask = edge.ask(NETHER, chunk);
        edge.tell(NETHER, snapshot(chunk, ask));
    }
    edge.quiet().await;
    client.drain();
    let (teleports, ids) = (client.teleports.len(), client.ids.len());

    let placed_at = within(IN_THE_NETHER);
    edge.say(
        NETHER,
        Durable::Placed {
            player: player(1),
            entity: EntityId(5),
            dimension: Dimension::Nether,
            pose: Pose::at(placed_at),
            flying: false,
            respawn: None,
            gateway: false,
        },
    );
    edge.settle(NETHER).await;
    // Confirming each batch of chunks, as a client does.
    edge.sync(&mut client).await;
    let teleport = &client.teleports[teleports];
    assert_eq!(
        (teleport.x, teleport.y, teleport.z),
        (placed_at.x, placed_at.y, placed_at.z)
    );
    assert_eq!(
        client.awaiting.load(Ordering::Relaxed),
        teleport.teleport_id,
        "the moves wait for the client to accept the placing"
    );
    assert_ne!(teleport.teleport_id, TRAVELLING);
    // In the game's order: the position, the abilities, the spawn's position, the
    // wait for chunks, the portal's sound; then the centre and the chunks.
    let after: Vec<i32> = client.ids[ids..].to_vec();
    let order = [
        clustine_protocol::packet_ids::play::clientbound::PLAYER_POSITION,
        clustine_protocol::packet_ids::play::clientbound::PLAYER_ABILITIES,
        clustine_protocol::packet_ids::play::clientbound::SET_DEFAULT_SPAWN_POSITION,
        clustine_protocol::packet_ids::play::clientbound::GAME_EVENT,
        clustine_protocol::packet_ids::play::clientbound::LEVEL_EVENT,
        clustine_protocol::packet_ids::play::clientbound::SET_CHUNK_CACHE_CENTER,
    ];
    assert_eq!(after[..order.len()], order, "{after:?}");
    assert!(
        client
            .level_events
            .iter()
            .any(|event| event.event == level_event::PORTAL_TRAVEL)
    );
    assert_eq!(
        client.chunks.keys().copied().collect::<BTreeSet<_>>(),
        view(IN_THE_NETHER),
        "the Nether's chunks follow"
    );
    let resync = edge.said(NETHER).await.into_iter().any(|message| {
        matches!(
            message.body,
            EdgeToWorker::Input {
                input: PlayerInput::MenuResync,
                ..
            }
        )
    });
    assert!(resync, "the inventory is asked of the Nether's region");
    edge.end().await;
}

/// Section 4.2, "Inputs": of what the client did before the portal and no region
/// applied, only what has nothing to do with where they are is passed on; and what
/// it does in the gap that is aimed at the old level is dropped.
#[tokio::test]
async fn only_inputs_that_have_no_place_cross_a_change_of_dimension() {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let client = edge.joined(player(1), EntityId(5)).await;
    edge.input(&client, PlayerInput::SelectSlot { slot: 3 })
        .await;
    edge.input(&client, PlayerInput::SwapHands).await;
    edge.input(
        &client,
        PlayerInput::Dig {
            position: block_of(HOME),
            sequence: 1,
        },
    )
    .await;
    edge.quiet().await;
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: through_a_portal(0),
            to: NETHER,
        },
    );
    edge.settle(WEST).await;
    edge.input(&client, PlayerInput::SelectSlot { slot: 5 })
        .await;
    edge.input(
        &client,
        PlayerInput::Dig {
            position: block_of(HOME),
            sequence: 2,
        },
    )
    .await;
    let inputs: Vec<PlayerInput> = edge
        .said(NETHER)
        .await
        .into_iter()
        .filter_map(|message| match message.body {
            EdgeToWorker::Input { input, .. } => Some(input),
            _ => None,
        })
        .collect();
    assert_eq!(
        inputs,
        [
            PlayerInput::SelectSlot { slot: 3 },
            PlayerInput::SwapHands,
            PlayerInput::SelectSlot { slot: 5 }
        ]
    );
    edge.end().await;
}

/// Section 4.2: a travelling view has a patience, as a join has. A player the
/// destination does not place in it is put out.
#[tokio::test]
async fn a_player_nobody_places_after_a_portal_is_put_out_in_time() {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_millis(300)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: through_a_portal(0),
            to: NETHER,
        },
    );
    edge.settle(WEST).await;
    client.disconnected().await;
    assert!(client.disconnected.is_some());
    edge.end().await;
}

/// Scenario 14. The west lets its player go while it has no link, and the edge
/// reads that among the entries of the next welcome. A player of the east sees
/// chunks the east says the west holds. The west's subscriptions wait, as on every
/// new link, and those the east's players see must not be ended because they wait.
#[tokio::test]
async fn a_hand_over_read_from_a_welcomes_entries_leaves_what_is_seen_subscribed() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
    edge.quiet().await;

    edge.lose(WEST).await;
    let hello = edge.relink(WEST).await;
    assert_eq!(hello.players, [player(1)]);
    assert_eq!(set(&hello.chunks), view(HOME));
    assert!(hello.guests.is_empty(), "{hello:?}");
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 1,
            presences: 1,
            applied: 0,
        }),
    );
    edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    edge.quiet().await;
    assert!(first.connected());

    // Everything anyone sees is asked of their region, and the west is still
    // asked for what it holds of it.
    let seen = view(EASTERN);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), seen);
    assert!(edge.at(EAST).chunks(Role::Guest).is_empty());
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    assert_eq!(edge.at(WEST).chunks(Role::Guest), both(&view(HOME), &seen));
    // Its answer to the hello is shown to both.
    edge.tell(WEST, snapshot(COMMON, 0));
    assert!(edge.shows(&mut first, WEST, COMMON).await);
    assert!(edge.shows(&mut second, WEST, COMMON).await);
    edge.end().await;
}

/// A hand-over into a region that has no link: nothing is sent into nothing. The
/// hello of its next link names the player and their view, and the arrival that
/// was kept follows the welcome.
#[tokio::test]
async fn a_player_let_go_to_a_region_without_a_link_is_asked_for_in_its_next_hello() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let _second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    edge.lose(EAST).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    // The second player sees all that the first saw from where they were.
    let asked = edge.asked(WEST).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));

    // The east is the first player's region now. It was asked for what they saw
    // when they were let go; what they see no more and the second still does it
    // is asked for as a guest, by the letter of section 2.
    let hello = edge.relink(EAST).await;
    assert_eq!(hello.players, [player(1)]);
    assert_eq!(set(&hello.chunks), view(EASTERN));
    assert_eq!(set(&hello.guests), minus(&view(HOME), &view(EASTERN)));
    edge.resume(EAST);
    // The east does not have the player yet, who is on their way to it.
    edge.tell(
        EAST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, EASTERN),
    };
    assert_eq!(edge.next_numbered(EAST).await, (1, arrival));
    edge.tell(EAST, snapshot(EASTERN, 0));
    assert!(edge.shows(&mut first, EAST, EASTERN).await);
    assert!(first.connected());
    edge.end().await;
}

/// Scenario 15. `NotMine` for an arrival is a `Departed` to the holder it names:
/// the view is asked of the holder before the arrival, the region that said it
/// stays a guest's for what the player sees, and the holder can be the region the
/// player came from.
#[tokio::test]
async fn a_player_a_region_sends_on_goes_to_the_holder_it_names_with_their_view() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;

    edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
    edge.settle(EAST).await;
    let (number, before) = edge.arrived(NORTH).await;
    assert_eq!(number, 1);
    assert!(before.is_superset(&view(EASTERN)), "{before:?}");
    assert_eq!(edge.at(NORTH).chunks(Role::Viewer), view(EASTERN));
    edge.quiet().await;
    // The east waits for all of it, and the player still sees it.
    assert!(edge.at(EAST).chunks(Role::Viewer).is_empty());
    assert_eq!(edge.at(EAST).chunks(Role::Guest), view(EASTERN));

    // The north sends them back to where they came from two steps ago.
    edge.say(NORTH, sent_on(player(1), EntityId(5), WEST, EASTERN));
    edge.settle(NORTH).await;
    let (number, before) = edge.arrived(WEST).await;
    // Behind the join.
    assert_eq!(number, 2);
    assert!(before.is_superset(&view(EASTERN)), "{before:?}");
    edge.quiet().await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(EASTERN));
    assert!(edge.at(NORTH).chunks(Role::Viewer).is_empty());
    assert!(client.connected());
    // What the player does goes to the west from now on.
    edge.input(&client, step_into(EASTERN)).await;
    let (number, body) = edge.next_numbered(WEST).await;
    assert!(
        number == 3 && matches!(body, EdgeToWorker::Input { .. }),
        "{body:?}"
    );
    edge.end().await;
}

/// Scenario 15. A player is passed on by `NotMine` at most as often as the edge
/// has heard of regions. (Read as: that many times they are passed on, and the
/// next `NotMine` disconnects them.)
#[tokio::test]
async fn a_player_sent_on_more_often_than_there_are_regions_is_disconnected() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    for (from, to) in [(EAST, NORTH), (NORTH, WEST), (WEST, EAST)] {
        edge.say(from, sent_on(player(1), EntityId(5), to, EASTERN));
        edge.settle(from).await;
        edge.arrived(to).await;
        assert!(client.connected(), "after {from:?} sent them on");
    }
    edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
    client.disconnected().await;
    let reason = client.disconnected.clone().unwrap_or_default();
    assert!(
        reason.contains("The server lost track of where you are."),
        "{reason}"
    );
    edge.quiet().await;
    for region in REGIONS {
        let left = &edge.at(region).subscriptions;
        assert!(left.is_empty(), "{region:?} is still asked for {left:?}");
    }
    edge.end().await;
}

/// Scenario 15. The count begins anew when a region confirms an input of the
/// player: they have been somewhere since.
#[tokio::test]
async fn a_player_whose_input_a_region_confirmed_can_be_sent_on_as_often_again() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.input(&client, step_into(HOME)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    for (from, to) in [(EAST, NORTH), (NORTH, WEST), (WEST, EAST)] {
        edge.say(from, sent_on(player(1), EntityId(5), to, EASTERN));
        edge.settle(from).await;
        edge.arrived(to).await;
    }
    // The east takes them in this time and applies what they did.
    edge.tell(
        EAST,
        WorkerToEdge::Progress {
            applied: 4,
            inputs: vec![(player(1), 1)],
        },
    );
    edge.quiet().await;
    let mut gone = transfer(EntityId(5), 1, EASTERN);
    gone.pose = Pose::at(within(EASTERN));
    for (from, to) in [(EAST, NORTH), (NORTH, WEST), (WEST, EAST)] {
        let entry = Durable::NotMine {
            what: Misdirected::Arrival {
                player: player(1),
                transfer: gone.clone(),
            },
            holder: to,
        };
        edge.say(from, entry);
        edge.settle(from).await;
        edge.arrived(to).await;
        assert!(client.connected(), "after {from:?} sent them on");
    }
    edge.end().await;
}

/// Scenario 15, and section 4, step 1: a region that names itself as where a
/// player goes has made a mistake, and the player is disconnected.
#[tokio::test]
async fn a_player_let_go_to_the_region_that_says_so_is_disconnected() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.say(WEST, departed(player(1), EntityId(5), WEST, STEP_EAST));
    first.disconnected().await;
    edge.say(EAST, sent_on(player(2), EntityId(6), EAST, EASTERN));
    second.disconnected().await;
    edge.end().await;
}

/// Scenario 16. The edge no longer has the player a region lets go: the entity
/// that is on its way is discarded where it was sent, and nothing else is done.
#[tokio::test]
async fn a_player_who_left_before_their_region_let_them_go_is_discarded_where_they_were_sent() {
    let mut edge = Harness::start().await;
    let client = edge.joined(player(1), EntityId(5)).await;
    edge.leave(&client).await;
    edge.quiet().await;
    edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
    edge.settle(WEST).await;
    let discard = EdgeToWorker::Discard {
        player: player(1),
        entity: EntityId(5),
        hops: 0,
        chunk: EASTERN,
    };
    let said = edge.said(EAST).await;
    assert_eq!(
        said,
        [EdgeMessage {
            number: Some(1),
            body: discard
        }]
    );
    edge.quiet().await;
    for region in REGIONS {
        let left = &edge.at(region).subscriptions;
        assert!(left.is_empty(), "{region:?} is asked for {left:?}");
    }
    edge.end().await;
}

/// Scenario 16: a player leaves and joins again, and then their region lets go who
/// they were before. The one who joined again is the home region's, whether or not
/// that region has placed them yet.
async fn a_region_lets_go_who_a_player_was_before(placed: bool) {
    let mut edge = Harness::start().await;
    let before = edge.joined(player(1), EntityId(5)).await;
    edge.leave(&before).await;
    let mut again = edge.join(player(1)).await;
    edge.drained().await;
    if placed {
        edge.tell(WEST, edge.spawned(player(1), EntityId(6)));
    }
    edge.quiet().await;

    edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
    edge.settle(WEST).await;
    let discard = EdgeToWorker::Discard {
        player: player(1),
        entity: EntityId(5),
        hops: 0,
        chunk: EASTERN,
    };
    let said = edge.said(EAST).await;
    assert_eq!(
        said,
        [EdgeMessage {
            number: Some(1),
            body: discard
        }]
    );
    if !placed {
        edge.tell(WEST, edge.spawned(player(1), EntityId(6)));
    }
    edge.quiet().await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    assert!(edge.at(EAST).subscriptions.is_empty());
    edge.input(&again, step_into(HOME)).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(&body, EdgeToWorker::Input { player: who, .. } if *who == player(1)),
        "{body:?}"
    );
    assert!(again.connected());
    edge.nothing_numbered().await;
    edge.end().await;
}

/// Scenario 16.
#[tokio::test]
async fn a_departure_of_who_a_player_was_before_joining_again_is_discarded() {
    a_region_lets_go_who_a_player_was_before(false).await;
}

/// Scenario 16.
#[tokio::test]
async fn a_departure_of_who_a_player_was_before_being_placed_again_is_discarded() {
    a_region_lets_go_who_a_player_was_before(true).await;
}

/// Scenario 16: a player is handed to the east and leaves, or leaves and joins
/// again, and then the east sends on who they were to the north.
async fn a_region_sends_on_who_a_player_was_before(again: bool) {
    let mut edge = Harness::start().await;
    let before = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.leave(&before).await;
    let mut now = None;
    if again {
        now = Some(edge.join(player(1)).await);
        edge.drained().await;
        edge.tell(WEST, edge.spawned(player(1), EntityId(6)));
    }
    edge.quiet().await;

    edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
    edge.settle(EAST).await;
    let discard = EdgeToWorker::Discard {
        player: player(1),
        entity: EntityId(5),
        hops: 0,
        chunk: EASTERN,
    };
    let said = edge.said(NORTH).await;
    assert_eq!(
        said,
        [EdgeMessage {
            number: Some(1),
            body: discard
        }]
    );
    edge.quiet().await;
    assert!(edge.at(EAST).subscriptions.is_empty());
    assert!(edge.at(NORTH).subscriptions.is_empty());
    match &mut now {
        Some(client) => {
            assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
            edge.input(client, step_into(HOME)).await;
            let (_, body) = edge.next_numbered(WEST).await;
            assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
            assert!(client.connected());
            edge.nothing_numbered().await;
        }
        None => assert!(edge.at(WEST).subscriptions.is_empty()),
    }
    edge.end().await;
}

/// Scenario 16.
#[tokio::test]
async fn an_arrival_sent_on_for_a_player_who_left_is_discarded_at_the_holder() {
    a_region_sends_on_who_a_player_was_before(false).await;
}

/// Scenario 16.
#[tokio::test]
async fn an_arrival_sent_on_for_who_a_player_was_before_joining_again_is_discarded() {
    a_region_sends_on_who_a_player_was_before(true).await;
}

/// Scenario 16, where the region alone does not tell who is meant. A player is let
/// go to the east and leaves; they join again, are placed anew and are let go to
/// the east once more. Then the east's word on the arrival of who they were
/// before is read: it names their region, and is not about them.
#[tokio::test]
async fn an_arrival_sent_on_for_who_a_player_was_before_does_not_move_who_they_are_now() {
    let mut edge = Harness::start().await;
    let before = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    edge.leave(&before).await;
    edge.quiet().await;
    let mut now = edge.joined(player(1), EntityId(6)).await;
    edge.hand(player(1), EntityId(6), WEST, EAST, EASTERN).await;
    edge.quiet().await;
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));

    edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
    edge.settle(EAST).await;
    let discard = EdgeToWorker::Discard {
        player: player(1),
        entity: EntityId(5),
        hops: 0,
        chunk: EASTERN,
    };
    let said = edge.said(NORTH).await;
    assert_eq!(
        said,
        [EdgeMessage {
            number: Some(1),
            body: discard
        }]
    );
    edge.quiet().await;
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
    assert!(edge.at(NORTH).subscriptions.is_empty());
    edge.input(&now, step_into(EASTERN)).await;
    let (_, body) = edge.next_numbered(EAST).await;
    assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
    assert!(now.connected());
    edge.end().await;
}

/// Section 4, step 2. A player is handed east and back. A second word of the east
/// that it lets them go is about their stay there, which is over.
#[tokio::test]
async fn an_entry_about_an_earlier_stay_of_a_player_is_passed_over() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.hand(player(1), EntityId(5), EAST, WEST, HOME).await;
    edge.quiet().await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));

    edge.say(EAST, departed(player(1), EntityId(5), NORTH, NORTHERN));
    edge.settle(EAST).await;
    let said = edge.said(NORTH).await;
    assert!(said.is_empty(), "{said:?}");
    edge.quiet().await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    edge.input(&client, step_into(HOME)).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
    assert!(client.connected());
    edge.end().await;
}

/// A player is handed east and back without anything being answered in between.
/// By the letter of section 2 the east is left a guest's region for what the
/// player still sees, whatever it waits for, until it says `NotMine`; then nothing
/// is asked again, as the west was not told elsewhere.
#[tokio::test]
async fn a_player_handed_over_and_back_leaves_the_other_region_a_guests_until_it_says_not_mine() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.hand(player(1), EntityId(5), EAST, WEST, HOME).await;
    edge.quiet().await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    assert!(edge.at(WEST).chunks(Role::Guest).is_empty());
    let still_seen = both(&view(HOME), &view(EASTERN));
    assert!(edge.at(EAST).chunks(Role::Viewer).is_empty());
    assert_eq!(edge.at(EAST).chunks(Role::Guest), still_seen);

    for chunk in &still_seen {
        edge.tell(EAST, not_mine(*chunk, edge.ask(EAST, *chunk)));
    }
    edge.settle(EAST).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
    assert!(edge.shows(&mut client, WEST, COMMON).await);
    edge.end().await;
}

/// Two players who see the same chunks are handed east one after the other and
/// back again. After each step every region is asked as a viewer for exactly what
/// its players see, and as a guest for what section 2 leaves it.
#[tokio::test]
async fn two_players_handed_over_and_back_leave_each_region_what_its_players_see() {
    let mut edge = Harness::start().await;
    let _first = edge.joined(player(1), EntityId(5)).await;
    let _second = edge.joined(player(2), EntityId(6)).await;
    let (home, east) = (view(HOME), view(EASTERN));
    let nothing = BTreeSet::new();
    // What each region is asked for as a viewer and as a guest: the west, then
    // the east.
    let steps = [
        // The first goes east. Of what they saw, the second still sees what the
        // first sees no more: the east waits for that as a guest.
        (1, EntityId(5), WEST, EAST, EASTERN),
        (2, EntityId(6), WEST, EAST, EASTERN),
        (1, EntityId(5), EAST, WEST, HOME),
        (2, EntityId(6), EAST, WEST, HOME),
    ];
    let expected = [
        [&home, &nothing, &east, &minus(&home, &east)],
        [&nothing, &both(&home, &east), &east, &nothing],
        [&home, &minus(&east, &home), &east, &nothing],
        [&home, &nothing, &nothing, &both(&home, &east)],
    ];
    for ((who, entity, from, to, chunk), expected) in steps.into_iter().zip(expected) {
        edge.hand(player(who), entity, from, to, chunk).await;
        edge.quiet().await;
        let found = [
            edge.at(WEST).chunks(Role::Viewer),
            edge.at(WEST).chunks(Role::Guest),
            edge.at(EAST).chunks(Role::Viewer),
            edge.at(EAST).chunks(Role::Guest),
        ];
        for (index, (found, expected)) in found.iter().zip(expected).enumerate() {
            assert_eq!(
                found, expected,
                "after player {who} went to {to:?}: {index}"
            );
        }
    }
    edge.end().await;
}

/// Scenario 17. What is left of an action goes to the region its entry names;
/// where it names none, to the region that serves the edge the chunk, also while
/// that region has no link: it is kept for it then.
#[tokio::test]
async fn an_action_goes_to_the_region_named_or_to_the_one_that_serves_the_chunk() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let first = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&first, Some(NORTH)));
    let passed_on = EdgeToWorker::Remote(first.clone());
    assert_eq!(edge.next_numbered(NORTH).await, (1, passed_on));

    // The east serves the chunk.
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    edge.tell(EAST, snapshot(COMMON, 1));
    edge.quiet().await;
    let second = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&second, None));
    let passed_on = EdgeToWorker::Remote(second.clone());
    assert_eq!(edge.next_numbered(EAST).await, (1, passed_on.clone()));

    // It still does for this purpose when its link has ended.
    edge.lose(EAST).await;
    let third = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&third, None));
    edge.settle(WEST).await;
    edge.nothing_numbered().await;
    let hello = edge.relink(EAST).await;
    assert_eq!(hello.guests, [COMMON]);
    edge.resume(EAST);
    // What the east has not reported applied is sent again, and then what waited.
    assert_eq!(edge.next_numbered(EAST).await, (1, passed_on));
    let passed_on = EdgeToWorker::Remote(third.clone());
    assert_eq!(edge.next_numbered(EAST).await, (2, passed_on));
    // None of them has ended, so none is acknowledged to the player.
    client.drain();
    for action in [first, second, third] {
        assert!(!client.acknowledged.contains(&action.sequence));
    }
    edge.end().await;
}

/// Scenario 17. An action whose entry names no region ends at the edge when no
/// region serves the chunk or only the region the entry came from does, and so
/// does one that a region passes on to itself: the player is told it was handled
/// and sees the block as it is.
#[tokio::test]
async fn an_action_nobody_else_can_take_is_acknowledged_to_the_player() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    // Nobody serves the chunk.
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&action, None));
    edge.acknowledged(&mut client, action.sequence).await;
    // The region the entry comes from serves it.
    edge.tell(WEST, snapshot(COMMON, 1));
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&action, None));
    edge.acknowledged(&mut client, action.sequence).await;
    // The entry names the region it comes from.
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&action, Some(WEST)));
    edge.acknowledged(&mut client, action.sequence).await;
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    let to_itself = Durable::NotMine {
        what: Misdirected::Remote(action.clone()),
        holder: WEST,
    };
    edge.say(WEST, to_itself);
    edge.acknowledged(&mut client, action.sequence).await;
    edge.nothing_numbered().await;
    edge.end().await;
}

/// A slot the region changed of its own accord, as the swap of the hands does it,
/// reaches the client as the game's set-player-inventory packet, which names the
/// slot as the inventory numbers it.
#[tokio::test]
async fn a_slot_the_region_changed_is_told_to_the_client_by_its_inventory_number() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let stick = ItemStack::new(1, 2);
    for (slot, stack) in [(3, None), (40, Some(stick))] {
        let event = PlayerEvent::InventorySlot {
            slot,
            stack: stack.clone(),
        };
        edge.tell(
            WEST,
            WorkerToEdge::ToPlayer {
                player: player(1),
                event,
            },
        );
        let told = loop {
            let packet = edge.packet(&mut client).await;
            let packet = packet.expect("the edge ended the connection");
            if let Ok(ClientboundPlay::SetPlayerInventory(told)) = ClientboundPlay::decode(&packet)
            {
                break told;
            }
            client.take(packet);
        };
        let expected = SetPlayerInventory {
            slot: i32::from(slot),
            stack: stack.as_ref().map(protocol_stack),
        };
        assert_eq!(told, expected);
    }
    edge.end().await;
}

/// A move of the driver's boat the region refused reaches the driver as the
/// clientbound `move_vehicle`, with where the region has the boat and how it turns.
#[tokio::test]
async fn a_refused_vehicle_move_reaches_the_driver_as_the_clientbound_move_vehicle() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let event = PlayerEvent::VehicleCorrected {
        position: clustine_world::Vec3::new(8.5, -60.0, 3.25),
        yaw: 45.0,
        pitch: -10.0,
    };
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event,
        },
    );
    let told = loop {
        let packet = edge.packet(&mut client).await;
        let packet = packet.expect("the edge ended the connection");
        if let Ok(ClientboundPlay::ClientboundMoveVehicle(told)) = ClientboundPlay::decode(&packet)
        {
            break told;
        }
        client.take(packet);
    };
    let expected = ClientboundMoveVehicle {
        x: 8.5,
        y: -60.0,
        z: 3.25,
        yaw: 45.0,
        pitch: -10.0,
    };
    assert_eq!(told, expected);
    edge.end().await;
}

/// A player the region puts out for holding a vehicle in the air is sent the game's
/// sentence for it and disconnected, and their region is told they left.
#[tokio::test]
async fn a_player_put_out_for_flying_a_vehicle_is_disconnected_with_the_games_sentence() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Kicked {
                reason: "multiplayer.disconnect.flying".to_owned(),
            },
        },
    );
    let reason = loop {
        let packet = edge.packet(&mut client).await;
        let packet = packet.expect("the edge ended the connection before saying why");
        if let Ok(ClientboundPlay::Disconnect(disconnect)) = ClientboundPlay::decode(&packet) {
            break format!("{:?}", disconnect.reason);
        }
        client.take(packet);
    };
    assert!(reason.contains("multiplayer.disconnect.flying"), "{reason}");
    assert!(edge.packet(&mut client).await.is_none());
    edge.end().await;
}

#[test]
fn a_joining_client_is_told_every_slot_of_the_inventory_where_its_screen_shows_it() {
    let mut inventory = Inventory::default();
    for slot in 0..clustine_world::INVENTORY_SLOTS {
        let n = i32::try_from(slot).unwrap() + 1;
        inventory.set(slot, Some(ItemStack::new(n, n)));
    }
    let content = crate::menus::Menus::default().entered(&inventory);
    let Ok(ClientboundPlay::SetContainerContent(content)) = ClientboundPlay::decode(&content)
    else {
        panic!("window 0 is sent whole");
    };
    // `initInventoryMenu` of a new player: window 0, state id 1, nothing carried.
    assert_eq!((content.window_id, content.state_id), (0, 1));
    assert!(content.carried.is_none());
    let item = |container: usize| content.slots[container].as_ref().map(|stack| stack.item);
    assert_eq!(content.slots.len(), 46);
    // The crafting grid and its result are empty.
    assert!(content.slots[..5].iter().all(Option::is_none));
    // The armour from the head down: the helmet is the inventory's slot 39.
    assert_eq!([5, 6, 7, 8].map(item), [40, 39, 38, 37].map(Some));
    assert_eq!(item(9), Some(10));
    assert_eq!(item(35), Some(36));
    // The hotbar at the bottom, and the off hand last.
    assert_eq!(item(36), Some(1));
    assert_eq!(item(44), Some(9));
    assert_eq!(item(45), Some(41));
    assert_eq!(
        content.slots[45].as_ref().map(|stack| stack.count),
        Some(41)
    );
}

/// `PlayerList.placeNewPlayer` @407-424 (V3c): the recipes the client needs are sent
/// after the abilities and before the player's place, once.
#[tokio::test]
async fn a_joining_client_is_sent_the_recipes_after_its_abilities_and_before_its_place() {
    use clustine_protocol::packet_ids::play::clientbound as ids;
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.sync(&mut client).await;
    let at = |id: i32| client.ids.iter().position(|sent| *sent == id);
    let recipes = at(ids::UPDATE_RECIPES).expect("the recipes are sent");
    assert!(at(ids::PLAYER_ABILITIES).is_some_and(|abilities| abilities < recipes));
    assert!(at(ids::PLAYER_POSITION).is_some_and(|place| recipes < place));
    let sent = client
        .ids
        .iter()
        .filter(|id| **id == ids::UPDATE_RECIPES)
        .count();
    assert_eq!(sent, 1);
    edge.end().await;
}

/// V4: a knock the region reports is sent to the player's own client as the motion
/// of its own entity (`ServerEntity.sendChanges @960-993`), and their own entity data
/// with it: air, burning, ticks frozen, arrows and absorption
/// (`ServerEntity.sendDirtyEntityData @44`, to the player too).
#[tokio::test]
async fn a_knock_and_the_players_own_data_are_sent_to_their_own_entity() {
    use clustine_protocol::packets::play::entity_data;
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.sync(&mut client).await;
    let own = client.logins[0].entity_id;
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Knocked {
                motion: [0.25, 0.4, -0.5],
            },
        },
    );
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::OwnData(clustine_sim::api::OwnData {
                air: 120,
                absorption: 4.0,
                on_fire: true,
                frozen: 30,
                arrows: 2,
            }),
        },
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    // The wire carries a motion to about 1/32768 (`LpVec3`).
    let knocked = client.tracking.iter().any(|packet| {
        matches!(
            packet,
            ClientboundPlay::SetEntityMotion(motion)
                if motion.entity_id == own
                    && motion
                        .motion
                        .iter()
                        .zip([0.25, 0.4, -0.5])
                        .all(|(sent, meant)| (sent - meant).abs() < 1e-4)
        )
    });
    assert!(knocked, "{:?}", client.tracking);
    let data = client
        .entity_data
        .iter()
        .rev()
        .find(|data| data.entity_id == own)
        .expect("the player's own data");
    assert!(
        data.values
            .contains(&(entity_data::SHARED_FLAGS, EntityDataValue::Byte(1)))
    );
    assert!(
        data.values
            .contains(&(entity_data::AIR_SUPPLY, EntityDataValue::Int(120)))
    );
    assert!(
        data.values
            .contains(&(entity_data::TICKS_FROZEN, EntityDataValue::Int(30)))
    );
    assert!(
        data.values
            .contains(&(entity_data::ARROW_COUNT, EntityDataValue::Int(2)))
    );
    assert!(
        data.values
            .contains(&(entity_data::PLAYER_ABSORPTION, EntityDataValue::Float(4.0)))
    );
    edge.end().await;
}

/// A death message names its killer by their own name where they have one, else by
/// their type's translation (`Entity.getDisplayName`).
#[test]
fn a_killer_is_named_by_their_name_or_their_type() {
    let zombie = clustine_sim::api::Killer {
        entity: EntityId(9),
        kind: u16::try_from(clustine_data::entity_types::ZOMBIE).unwrap(),
        name: None,
        player: false,
        weapon: None,
    };
    assert_eq!(
        killer_name(&zombie),
        Nbt::Compound(vec![(
            "translate".to_owned(),
            Nbt::String("entity.minecraft.zombie".to_owned())
        )])
    );
    let named = clustine_sim::api::Killer {
        name: Some("Steve".to_owned()),
        player: true,
        ..zombie
    };
    assert_eq!(
        killer_name(&named),
        Nbt::Compound(vec![("text".to_owned(), Nbt::String("Steve".to_owned()))])
    );
}

/// The recipe packet reads back as the game writes it: the property sets the
/// client's furnaces and smithing table ask for, and the stonecutter's recipes.
#[test]
fn the_recipes_packet_has_the_property_sets_and_the_stonecutters_recipes() {
    let Ok(ClientboundPlay::UpdateRecipes(recipes)) =
        ClientboundPlay::decode(&update_recipes_packet())
    else {
        panic!("the recipes packet");
    };
    let keys: Vec<&str> = recipes
        .property_sets
        .iter()
        .map(|set| set.key.as_str())
        .collect();
    for key in [
        "minecraft:furnace_input",
        "minecraft:smithing_base",
        "minecraft:smithing_template",
    ] {
        assert!(keys.contains(&key), "{keys:?}");
    }
    let furnace = recipes
        .property_sets
        .iter()
        .find(|set| set.key == "minecraft:furnace_input")
        .expect("named above");
    assert!(furnace.items.contains(&clustine_data::items::IRON_ORE));
    assert!(!recipes.stonecutter.is_empty());
}

/// Scenario 17. A region that was passed an action and believes another to hold
/// the chunk names it, and the action goes there.
#[tokio::test]
async fn an_action_a_region_says_is_not_its_own_goes_to_the_holder_it_names() {
    let mut edge = Harness::start().await;
    let _client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    let not_mine = Durable::NotMine {
        what: Misdirected::Remote(action.clone()),
        holder: EAST,
    };
    edge.say(NORTH, not_mine);
    let passed_on = EdgeToWorker::Remote(action);
    assert_eq!(edge.next_numbered(EAST).await, (1, passed_on));
    edge.end().await;
}

/// Scenario 17. The edge keeps for a region it has heard of and has no link to
/// yet: an action that is to go there, and a guest's subscription it was named
/// for, which the hello of its first link names.
#[tokio::test]
async fn a_region_the_edge_has_no_link_to_yet_is_kept_for() {
    let patience = Duration::from_secs(1_000_000);
    let mut edge = Harness::start_with(&[WEST, EAST], patience).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&action, Some(NORTH)));
    edge.tell(WEST, elsewhere(COMMON, 1, NORTH));
    edge.quiet().await;

    edge.epochs[NORTH.0 as usize] = 0;
    let hello = edge.relink(NORTH).await;
    assert_eq!((hello.since, hello.seen), (0, 0));
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(hello.guests, [COMMON]);
    let unknown = Welcome::Unknown {
        since: 1,
        entries: 0,
        presences: 0,
        applied: 0,
    };
    edge.tell(NORTH, WorkerToEdge::Welcome(unknown));
    let passed_on = EdgeToWorker::Remote(action.clone());
    assert_eq!(edge.next_numbered(NORTH).await, (1, passed_on));
    let done = Durable::RemoteDone {
        player: player(1),
        entity: action.entity,
        sequence: action.sequence,
        shown: Vec::new(),
    };
    edge.say(NORTH, done);
    edge.tell(NORTH, snapshot(COMMON, 0));
    assert!(edge.shows(&mut client, NORTH, COMMON).await);
    edge.end().await;
}

/// A player is let go to a region the edge has heard of and has no link to yet.
/// The edge keeps their arrival and their view for it, as for a region whose link
/// ended, and the hello of its first link names both.
#[tokio::test]
async fn a_player_let_go_to_a_region_without_a_link_yet_is_named_in_its_first_hello() {
    let patience = Duration::from_secs(1_000_000);
    let mut edge = Harness::start_with(&[WEST, EAST], patience).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, NORTH, NORTHERN)
        .await;
    edge.quiet().await;
    // The west is asked as a guest for what the player still sees, which is all
    // it waits for.
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    assert_eq!(
        edge.at(WEST).chunks(Role::Guest),
        both(&view(HOME), &view(NORTHERN))
    );

    edge.epochs[NORTH.0 as usize] = 0;
    let hello = edge.relink(NORTH).await;
    assert_eq!(hello.players, [player(1)]);
    assert_eq!(set(&hello.chunks), view(NORTHERN));
    assert!(hello.guests.is_empty(), "{hello:?}");
    let unknown = Welcome::Unknown {
        since: 1,
        entries: 0,
        presences: 1,
        applied: 0,
    };
    edge.tell(NORTH, WorkerToEdge::Welcome(unknown));
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, NORTHERN),
    };
    assert_eq!(edge.next_numbered(NORTH).await, (1, arrival));
    edge.tell(NORTH, snapshot(NORTHERN, 0));
    assert!(edge.shows(&mut client, NORTH, NORTHERN).await);
    assert!(client.connected());
    edge.end().await;
}

/// Scenario 17, rule 26. An entry without a region for an action of a player the
/// edge no longer has is dropped, also when a region serves the chunk.
#[tokio::test]
async fn an_action_without_a_region_of_a_player_who_left_is_dropped() {
    let mut edge = Harness::start().await;
    let first = edge.joined(player(1), EntityId(5)).await;
    let _second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    edge.tell(EAST, snapshot(COMMON, 1));
    edge.quiet().await;

    edge.leave(&first).await;
    edge.quiet().await;
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&action, None));
    edge.settle(WEST).await;
    edge.nothing_numbered().await;
    edge.end().await;
}

/// Which region serves a chunk, after `Elsewhere`: a region that says `Elsewhere`
/// for a chunk it had sent a snapshot of (which step C3 makes possible) serves it
/// no more, and the region it names does once its snapshot is taken.
#[tokio::test]
async fn a_region_that_says_elsewhere_for_a_chunk_it_served_serves_it_no_more() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(COMMON, 1));
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

    // A third region passes an action on without knowing who holds the chunk.
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(NORTH, remote(&action, None));
    edge.acknowledged(&mut client, action.sequence).await;
    edge.nothing_numbered().await;
    // The chunk stays on the screen meanwhile, and the next snapshot is
    // reconciled with it.
    edge.sync(&mut client).await;
    assert_eq!(client.state(COMMON), Some(AIR));

    let chunk = chunk_with(COMMON, STONE);
    edge.tell(EAST, snapshot_of(COMMON, 1, chunk, Vec::new()));
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.state(COMMON), Some(STONE));
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(NORTH, remote(&action, None));
    let passed_on = EdgeToWorker::Remote(action);
    assert_eq!(edge.next_numbered(EAST).await, (1, passed_on));
    edge.end().await;
}

/// Which region serves a chunk, after `NotMine`: a guest's region that says it for
/// a chunk it had sent a snapshot of serves it no more.
#[tokio::test]
async fn a_region_that_says_not_mine_for_a_chunk_it_served_serves_it_no_more() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, elsewhere(COMMON, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);
    edge.tell(EAST, snapshot(COMMON, 1));
    assert!(edge.shows(&mut client, EAST, COMMON).await);

    edge.tell(EAST, not_mine(COMMON, 1));
    edge.settle(EAST).await;
    assert_eq!(edge.asked(WEST).await, [subscribe(2, [COMMON])]);
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&action, None));
    edge.acknowledged(&mut client, action.sequence).await;
    edge.nothing_numbered().await;
    // The chunk stays on the screen while the west is asked again.
    edge.sync(&mut client).await;
    assert!(client.chunks.contains_key(&COMMON));
    edge.end().await;
}

/// Which region serves a chunk, after its subscription ended: a chunk that nobody
/// saw for a moment is served by nobody until a snapshot of it is taken again.
#[tokio::test]
async fn a_chunk_that_was_out_of_sight_is_served_by_nobody_until_its_next_snapshot() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(FAR, 1));
    assert!(edge.shows(&mut client, WEST, FAR).await);
    edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
    edge.tell(WEST, walked(EntityId(5), STEP_EAST, HOME));
    edge.quiet().await;

    let action = breaking(player(1), edge.entity_of(player(1)), edge.sequence(), FAR);
    edge.say(NORTH, remote(&action, None));
    edge.acknowledged(&mut client, action.sequence).await;
    edge.nothing_numbered().await;

    edge.tell(WEST, snapshot(FAR, edge.ask(WEST, FAR)));
    edge.settle(WEST).await;
    let action = breaking(player(1), edge.entity_of(player(1)), edge.sequence(), FAR);
    edge.say(NORTH, remote(&action, None));
    // Behind the join.
    let passed_on = EdgeToWorker::Remote(action);
    assert_eq!(edge.next_numbered(WEST).await, (2, passed_on));
    edge.end().await;
}

/// Scenario 18. What was kept for a region goes out when as many `Outbox` messages
/// as its welcome announced have come, also when one of them is an entry the edge
/// has handled before, and not earlier.
#[tokio::test]
async fn what_was_kept_is_sent_after_as_many_entries_as_the_welcome_announced() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.input(&client, step_into(HOME)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
    edge.quiet().await;
    let seen = edge.outbox[0];

    let hello = edge.relink(WEST).await;
    assert_eq!(hello.seen, seen);
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 2,
            presences: 1,
            applied: 0,
        }),
    );
    // The first of the two is one the edge has handled.
    let nobody = Durable::RemoteDone {
        player: player(u128::MAX),
        entity: EntityId(0),
        sequence: 0,
        shown: Vec::new(),
    };
    let again = WorkerToEdge::Outbox {
        number: seen,
        entry: nobody,
    };
    edge.tell(WEST, again);
    // Something behind it that the edge passes on to the player shows that the
    // edge has read it. Nothing that was kept has been sent by then.
    let sequence = edge.sequence();
    let acknowledged = WorkerToEdge::ToPlayer {
        player: player(1),
        event: PlayerEvent::Acknowledged { sequence },
    };
    edge.tell(WEST, acknowledged);
    edge.acknowledged(&mut client, sequence).await;
    let sent = numbered(&edge.waiting(WEST));
    assert!(sent.is_empty(), "{sent:?}");

    // With the second entry, everything that was kept follows in order.
    edge.settle(WEST).await;
    let sent = numbered(&edge.waiting(WEST));
    assert!(
        matches!(
            &sent[..],
            [
                (1, EdgeToWorker::PlayerJoin(_)),
                (2, EdgeToWorker::Input { .. })
            ]
        ),
        "{sent:?}"
    );
    edge.end().await;
}

/// Scenario 18. An entry among those a welcome announces can change what is kept:
/// here the player is let go, so what they did and the west has not applied goes
/// to the east behind their arrival. The west is sent what was kept for it when
/// the entry has been handled.
#[tokio::test]
async fn a_hand_over_among_a_welcomes_entries_is_done_before_what_was_kept_is_sent() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.input(&client, step_into(HOME)).await;
    assert_eq!(edge.next_numbered(WEST).await.0, 2);
    edge.quiet().await;

    let hello = edge.relink(WEST).await;
    assert_eq!(hello.players, [player(1)]);
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 1,
            presences: 1,
            applied: 0,
        }),
    );
    edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    edge.settle(WEST).await;
    let sent = numbered(&edge.said(EAST).await);
    assert!(
        matches!(
            &sent[..],
            [
                (1, EdgeToWorker::PlayerArrive { .. }),
                (2, EdgeToWorker::Input { number: 1, .. })
            ]
        ),
        "{sent:?}"
    );
    let sent = numbered(&edge.said(WEST).await);
    assert!(
        matches!(
            &sent[..],
            [
                (1, EdgeToWorker::PlayerJoin(_)),
                (2, EdgeToWorker::Input { .. })
            ]
        ),
        "{sent:?}"
    );
    assert!(client.connected());
    edge.end().await;
}

/// Scenario 19. A snapshot that is reconciled with a chunk the edge shows removes
/// the entities its own region introduced and that it lacks, and leaves one that
/// another region introduced: a player of that region who stands in a chunk this
/// one holds. It brings the blocks that differ.
#[tokio::test]
async fn a_reconciled_snapshot_removes_what_its_region_introduced_and_leaves_the_rest() {
    // A chunk next to `COMMON` that the player sees as well.
    let beside = ChunkPos::new(3, -2);
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    // The west serves `COMMON`, with someone in it.
    let there = vec![stranger(EntityId(70), COMMON)];
    edge.tell(WEST, snapshot_of(COMMON, 1, empty_chunk(), there));
    // The east serves the chunk beside it, with someone who then walks over.
    edge.tell(WEST, elsewhere(beside, 1, EAST));
    edge.settle(WEST).await;
    assert_eq!(edge.asked(EAST).await, [as_guest(1, [beside])]);
    let there = vec![stranger(EntityId(80), beside)];
    edge.tell(EAST, snapshot_of(beside, 1, empty_chunk(), there));
    let walked_over = RegionEvent::EntityMoved {
        entity: EntityId(80),
        pose: Pose::at(within(COMMON)),
        previous_chunk: beside,
    };
    edge.tell(
        EAST,
        WorkerToEdge::TickDelta {
            tick: 2,
            events: vec![walked_over],
        },
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.entities, BTreeSet::from([70, 80]));

    // The west's next snapshot of its chunk has neither of them.
    let chunk = chunk_with(COMMON, STONE);
    edge.tell(WEST, snapshot_of(COMMON, 1, chunk, Vec::new()));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.entities, BTreeSet::from([80]));
    assert_eq!(client.state(COMMON), Some(STONE));
    edge.end().await;
}

/// Scenario 19, rule 32. A snapshot never removes the entity of one of the edge's
/// own players.
#[tokio::test]
async fn a_reconciled_snapshot_does_not_remove_the_edges_own_players() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let own = |number: u128, entity| EntityState {
        entity,
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(number),
            name: format!("Player{number}"),
        },
        pose: Pose::at(SPAWN),
    };
    let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
    edge.tell(WEST, snapshot_of(HOME, 1, empty_chunk(), there));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert!(first.entities.contains(&6), "{:?}", first.entities);
    assert!(second.entities.contains(&5), "{:?}", second.entities);

    edge.tell(WEST, snapshot(HOME, 1));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;
    assert!(first.entities.contains(&6), "{:?}", first.entities);
    assert!(second.entities.contains(&5), "{:?}", second.entities);
    edge.end().await;
}

/// Statement E, where section 2 as first written broke it. Two regions' players
/// see a chunk a third holds. The north names the east for it, as it heard at a
/// time when that was so; the east is asked for it already, for its own player,
/// and names the west. Then the east's player goes. Its subscription was told
/// elsewhere, so it is ended as one that carried nothing. But it was what the
/// north's subscription pointed at: left as it is, the north's would be told
/// elsewhere with a region where nothing is asked, and nothing would ever have
/// the north asked again. Statement E says that there is a subscription at the
/// east, or an asking due; section 2 now has the north asked again.
#[tokio::test]
async fn statement_e_holds_when_a_subscription_another_points_at_is_ended() {
    let mut edge = Harness::start().await;
    let first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
        .await;
    edge.quiet().await;
    // The west holds the chunk and is asked for it as a guest, as both players
    // saw it when the west let them go.
    assert_eq!(edge.at(WEST).subscriptions[&COMMON].0, Role::Guest);
    edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
    edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
    edge.settle(EAST).await;
    edge.tell(NORTH, elsewhere(COMMON, edge.ask(NORTH, COMMON), EAST));
    edge.quiet().await;
    assert_eq!(edge.at(EAST).subscriptions[&COMMON].0, Role::Viewer);
    assert!(edge.shows(&mut second, NORTH, COMMON).await);

    edge.leave(&first).await;
    edge.quiet().await;
    let at_the_east = edge.at(EAST).subscriptions.contains_key(&COMMON);
    let asked_again = edge.ask(NORTH, COMMON) > 1;
    assert!(
        at_the_east || asked_again,
        "the north named the east for the chunk, where nothing is asked now"
    );
    edge.end().await;
}

/// A player is let go to the east, which has no link, and leaves. When the east
/// has a link again it takes them in, as the arrival comes first, and lets them
/// go to the west, as what they did last took them there; only then it hears that
/// they left, which it passes over. The edge has the west discard the entity. The
/// west reports it removed to those watching the chunk, and it has to go from
/// their screens although it was the east that showed it last: ADR-0008 says of an
/// entity that departed that one "that never arrived anywhere is" taken off the
/// screens, and ADR-0006 has the region it was heading for report it removed.
#[tokio::test]
async fn a_discarded_entity_another_region_showed_last_is_taken_off_the_screens() {
    let near = ChunkPos::new(3, 0);
    let mut edge = Harness::start().await;
    // Someone who stands a chunk east of where players enter the world, and sees
    // a chunk of the west and one the east serves.
    let mut watcher = edge.joined(player(2), EntityId(6)).await;
    edge.tell(WEST, walked(EntityId(6), HOME, STEP_EAST));
    edge.quiet().await;
    edge.tell(WEST, snapshot(near, edge.ask(WEST, near)));
    edge.tell(WEST, elsewhere(EASTERN, edge.ask(WEST, EASTERN), EAST));
    edge.settle(WEST).await;
    edge.settle(EAST).await;
    edge.tell(EAST, snapshot(EASTERN, edge.ask(EAST, EASTERN)));
    edge.quiet().await;

    let leaver = edge.joined(player(1), EntityId(5)).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.leave(&leaver).await;
    edge.quiet().await;

    // The east takes the arrival in, and reports the entity as any that appears.
    let arrived = EntityState {
        entity: EntityId(5),
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(1),
            name: "Player".to_owned(),
        },
        pose: Pose::at(within(EASTERN)),
    };
    edge.tell(
        EAST,
        WorkerToEdge::TickDelta {
            tick: 2,
            events: vec![RegionEvent::EntitySpawned(arrived)],
        },
    );
    // They step west and are let go there.
    edge.tell(EAST, walked(EntityId(5), EASTERN, near));
    edge.say(EAST, departed(player(1), EntityId(5), WEST, near));
    edge.settle(EAST).await;
    let said = numbered(&edge.said(WEST).await);
    let discard = EdgeToWorker::Discard {
        player: player(1),
        entity: EntityId(5),
        hops: 0,
        chunk: near,
    };
    assert_eq!(
        said.last().map(|(_, body)| body),
        Some(&discard),
        "{said:?}"
    );

    let removed = RegionEvent::EntityRemoved {
        entity: EntityId(5),
        chunk: near,
    };
    edge.tell(
        WEST,
        WorkerToEdge::TickDelta {
            tick: 3,
            events: vec![removed],
        },
    );
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    assert!(
        !watcher.entities.contains(&5),
        "the entity of a player who left is still shown"
    );
    edge.end().await;
}

// Wire ids (`docs/adr/0028-entities.md`, section 1).

/// A key of a block far into the world's life, which no packet's `i32` can hold.
fn far_key(n: u64) -> EntityId {
    EntityId(3 * clustine_world::EntityIds::BLOCK_SIZE + n)
}

/// Someone who is not this edge's, as [`stranger`] has them, with a name a client
/// takes whatever the key.
fn far_stranger(entity: EntityId, chunk: ChunkPos) -> EntityState {
    EntityState {
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(u128::from(entity.0) + 1000),
            name: format!("Stranger{}", entity.0 % 1000),
        },
        ..stranger(entity, chunk)
    }
}

/// A region's word, in one tick, that `events` happened.
fn ticked(events: Vec<RegionEvent>) -> WorkerToEdge {
    WorkerToEdge::TickDelta { tick: 2, events }
}

fn gone(entity: EntityId, chunk: ChunkPos) -> RegionEvent {
    RegionEvent::EntityRemoved { entity, chunk }
}

/// Two players and a stranger in one chunk: each client is shown the others, the
/// stranger moves and is removed. Every packet about an entity names it by one id,
/// which is not its key, the same on both clients, and the one the login gave the
/// player who is that entity.
#[tokio::test]
async fn a_client_is_shown_entities_by_ids_that_are_not_their_keys_and_the_same_in_every_packet() {
    let (first_key, second_key, stranger_key) = (far_key(1000), far_key(1001), far_key(1002));
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), first_key).await;
    let mut second = edge.joined(player(2), second_key).await;
    edge.quiet().await;
    let there = vec![
        own(1, first_key.0, HOME),
        own(2, second_key.0, HOME),
        far_stranger(stranger_key, HOME),
    ];
    edge.tell(
        WEST,
        snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there),
    );
    let moved = RegionEvent::EntityMoved {
        entity: stranger_key,
        pose: Pose::at(Vec3::new(3.5, -60.0, 4.5)),
        previous_chunk: HOME,
    };
    edge.tell(WEST, ticked(vec![moved]));
    edge.tell(WEST, ticked(vec![gone(stranger_key, HOME)]));
    edge.settle(WEST).await;
    edge.sync(&mut first).await;
    edge.sync(&mut second).await;

    // One id for each entity, in every packet about it.
    let one_id = |client: &Client, key: EntityId| {
        let naming = client.naming(key.0);
        let id = naming.first().expect("the client was told of the entity").1;
        assert!(naming.iter().all(|(_, named)| *named == id), "{naming:?}");
        assert_ne!(id, key.game_id(), "the client was told the key's low half");
        id
    };
    let stranger_id = one_id(&first, stranger_key);
    assert_eq!(one_id(&second, stranger_key), stranger_id);
    use Naming::{HeadRotation, Login, Position, Removal, Spawn};
    assert_eq!(
        first.naming(stranger_key.0),
        [
            (Spawn, stranger_id),
            (HeadRotation, stranger_id),
            (Position, stranger_id),
            (HeadRotation, stranger_id),
            (Removal, stranger_id),
        ]
    );
    // A player is known to the others by the id their own login gave them, and is
    // never shown to themselves.
    assert_eq!(one_id(&second, first_key), first.own_id());
    assert_eq!(one_id(&first, second_key), second.own_id());
    assert_eq!(first.naming(first_key.0), [(Login, first.own_id())]);
    assert_eq!(second.naming(second_key.0), [(Login, second.own_id())]);
    let ids = BTreeSet::from([first.own_id(), second.own_id(), stranger_id]);
    assert_eq!(ids.len(), 3, "two entities were given one id");
    edge.end().await;
}

/// A player's entity is removed from the only client that showed it, as when the
/// region it was in reports it gone on the way elsewhere: nobody is shown it and the
/// edge does not keep it, but the player is still connected, and their client
/// knows itself by the id of their login. Shown again, they are shown by that id.
#[tokio::test]
async fn a_players_own_login_id_stays_theirs_while_connected() {
    let key = far_key(7);
    let mut edge = Harness::start().await;
    let mut player_one = edge.joined(player(1), key).await;
    let mut watcher = edge.joined(player(2), far_key(8)).await;
    edge.quiet().await;
    let there = vec![own(1, key.0, HOME), own(2, far_key(8).0, HOME)];
    edge.tell(
        WEST,
        snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there),
    );
    edge.settle(WEST).await;
    edge.sync(&mut player_one).await;
    edge.sync(&mut watcher).await;
    let login = player_one.own_id();
    assert_eq!(watcher.naming(key.0).first(), Some(&(Naming::Spawn, login)));

    edge.tell(WEST, ticked(vec![gone(key, HOME)]));
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    assert_eq!(
        watcher.naming(key.0).last(),
        Some(&(Naming::Removal, login))
    );
    assert!(!watcher.entities.contains(&key.0), "{:?}", watcher.entities);

    edge.tell(WEST, introduced(1, key.0, HOME));
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    edge.sync(&mut player_one).await;
    assert!(watcher.entities.contains(&key.0), "{:?}", watcher.entities);
    let since: Vec<_> = watcher.naming(key.0).into_iter().skip(3).collect();
    assert_eq!(
        since,
        [(Naming::Spawn, login), (Naming::HeadRotation, login)],
        "{:?}",
        watcher.naming(key.0)
    );
    assert!(player_one.connected());
    // Logged in once, by that id.
    assert_eq!(player_one.own_id(), login);
    edge.end().await;
}

/// A cow standing in `chunk`, its record as a mob's with nothing in it.
fn a_cow(entity: EntityId, chunk: ChunkPos) -> EntityState {
    EntityState {
        entity,
        kind: EntityKind::Other {
            kind: entity_types::COW as u16,
            uuid: Uuid::from_u128(u128::from(entity.0)),
            motion: [0.0, 0.0, 0.0],
            hops: 0,
            lived: 0,
            data: EntityData::Mob {
                body: Vec::new(),
                mind: Vec::new(),
                state: Default::default(),
            },
            riding: Default::default(),
        },
        pose: Pose::at(within(chunk)),
    }
}

/// A mob's hurt and attributes go to those who see it, with the edge's ids for it
/// and for who hurt it; a client that comes to see it later is sent its attributes
/// with its spawn (`docs/groundwork/n7-brief.md`, section 9.1).
#[tokio::test]
async fn a_mob_s_hurt_and_attributes_are_shown_and_its_attributes_paired() {
    let mut edge = Harness::start().await;
    let watcher_key = far_key(1);
    let mut watcher = edge.joined(player(1), watcher_key).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
    edge.settle(WEST).await;
    let key = far_key(100);
    let chunk = HOME;
    let speed = AttributeShown {
        attribute: 20,
        base: 0.2,
        modifiers: vec![("minecraft:random_spawn_bonus".to_owned(), 0.05, 1)],
    };
    edge.tell(
        WEST,
        ticked(vec![
            RegionEvent::EntitySpawned(a_cow(key, HOME)),
            RegionEvent::AttributesChanged {
                entity: key,
                chunk,
                attributes: vec![speed],
            },
            RegionEvent::DamageEvent {
                entity: key,
                chunk,
                damage_type: 7,
                cause: Some(watcher_key),
                direct: Some(watcher_key),
                position: None,
            },
        ]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&key.0), "{:?}", watcher.entities);
    let id = watcher.naming(key.0)[0].1;
    let own = watcher.own_id();
    let [
        ClientboundPlay::UpdateAttributes(attributes),
        ClientboundPlay::DamageEvent(hurt),
    ] = &watcher.living[..]
    else {
        panic!("{:?}", watcher.living);
    };
    assert_eq!(attributes.entity_id, id);
    assert_eq!(attributes.attributes[0].attribute, 20);
    assert_eq!(
        (hurt.entity_id, hurt.damage_type, hurt.cause, hurt.direct),
        (id, 7, Some(own), Some(own))
    );

    let mut newcomer = edge.joined(player(2), far_key(2)).await;
    edge.quiet().await;
    edge.sync(&mut newcomer).await;
    assert!(newcomer.entities.contains(&key.0));
    assert!(
        matches!(&newcomer.living[..], [ClientboundPlay::UpdateAttributes(paired)] if paired.attributes == attributes.attributes),
        "{:?}",
        newcomer.living
    );
    edge.end().await;
}

/// What another player holds and wears is shown as their region reports it, to
/// those who see them then and to those who come to see them later.
#[tokio::test]
async fn another_player_is_shown_holding_and_wearing_what_they_do() {
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(1), far_key(1)).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
    edge.settle(WEST).await;
    let other = far_key(50);
    let state = EntityState {
        entity: other,
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(9),
            name: "Player9".to_owned(),
        },
        pose: Pose::at(SPAWN),
    };
    let sword = clustine_world::ItemStack::new(1021, 1);
    let mut slots: Vec<(u8, Option<clustine_world::ItemStack>)> =
        (0..6).map(|slot| (slot, None)).collect();
    slots[0].1 = Some(sword);
    edge.tell(
        WEST,
        ticked(vec![
            RegionEvent::EntitySpawned(state),
            RegionEvent::EquipmentChanged {
                entity: other,
                chunk: HOME,
                slots,
            },
        ]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    let held = |living: &[ClientboundPlay]| {
        living.iter().any(|packet| {
            matches!(packet, ClientboundPlay::SetEquipment(worn)
                    if worn.slots.contains(&(0, Some(WireStack::new(1021, 1)))))
        })
    };
    assert!(
        watcher.entities.contains(&other.0),
        "{:?}",
        watcher.entities
    );
    assert!(held(&watcher.living), "{:?}", watcher.living);
    let mut newcomer = edge.joined(player(2), far_key(2)).await;
    edge.quiet().await;
    edge.sync(&mut newcomer).await;
    assert!(newcomer.entities.contains(&other.0));
    assert!(held(&newcomer.living), "{:?}", newcomer.living);
    edge.end().await;
}

/// What another player's data has that is not the defaults (the pose, the skin's
/// parts, the main hand) is shown with them to whoever comes to see them, as their
/// region reports it with them and as it changes since: a pose back at standing is
/// not, and a sleeper's pose goes with their bed, once.
#[tokio::test]
async fn another_player_s_pose_skin_and_hand_are_shown_to_whoever_comes_to_see_them() {
    use clustine_sim::shown::{DataShown, Serialiser};
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(1), far_key(1)).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
    edge.settle(WEST).await;
    let other = far_key(50);
    let state = EntityState {
        entity: other,
        kind: EntityKind::Player {
            game_mode: 1,
            player: player(9),
            name: "Player9".to_owned(),
            data: vec![(16, DataShown::Byte(0x7F))],
        },
        pose: Pose::at(SPAWN),
    };
    let left = EntityDataValue::Registered {
        serializer: 42,
        id: 0,
    };
    edge.tell(
        WEST,
        ticked(vec![
            RegionEvent::EntitySpawned(state),
            RegionEvent::PlayerData {
                entity: other,
                chunk: HOME,
                values: vec![
                    (6, DataShown::Pose(pose::CROUCHING)),
                    (15, DataShown::Serialised(Serialiser::HumanoidArm, 0)),
                ],
            },
        ]),
    );
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    let data_of = |client: &Client| -> Vec<Vec<(u8, EntityDataValue)>> {
        let ids: Vec<i32> = client
            .naming(other.0)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        client
            .entity_data
            .iter()
            .filter(|data| ids.contains(&data.entity_id))
            .map(|data| data.values.clone())
            .collect()
    };
    let skin = (16, EntityDataValue::Byte(0x7F));
    assert_eq!(
        data_of(&watcher),
        [
            vec![skin.clone()],
            vec![
                (6, EntityDataValue::Pose(pose::CROUCHING)),
                (15, left.clone())
            ],
        ]
    );
    let mut newcomer = edge.joined(player(2), far_key(2)).await;
    edge.quiet().await;
    edge.sync(&mut newcomer).await;
    assert_eq!(
        data_of(&newcomer),
        [vec![
            (6, EntityDataValue::Pose(pose::CROUCHING)),
            (15, left.clone()),
            skin.clone(),
        ]]
    );

    // Standing again: no pose for the next to come.
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::PlayerData {
            entity: other,
            chunk: HOME,
            values: vec![(6, DataShown::Pose(pose::STANDING))],
        }]),
    );
    edge.settle(WEST).await;
    let mut second = edge.joined(player(3), far_key(3)).await;
    edge.quiet().await;
    edge.sync(&mut second).await;
    assert_eq!(data_of(&second), [vec![(15, left.clone()), skin.clone()]]);

    // Asleep: the pose with the bed, and not again with the rest.
    let head = BlockPos::new(1, -60, 1);
    edge.tell(
        WEST,
        ticked(vec![RegionEvent::Sleeping {
            entity: other,
            chunk: HOME,
            head: Some(head),
        }]),
    );
    edge.settle(WEST).await;
    let mut third = edge.joined(player(4), far_key(4)).await;
    edge.quiet().await;
    edge.sync(&mut third).await;
    let bed = Position {
        x: head.x,
        y: head.y,
        z: head.z,
    };
    assert_eq!(
        data_of(&third),
        [
            vec![
                (6, EntityDataValue::Pose(pose::SLEEPING)),
                (14, EntityDataValue::OptionalBlockPos(Some(bed))),
            ],
            vec![(15, left), skin],
        ]
    );
    edge.end().await;
}

/// A mob made wearing something (a skeleton's bow), or met for the first time
/// wearing it (loaded after a restart), is shown wearing it with its spawn, to those
/// who see it then and to those who come to see it later (`sendPairingData`), though
/// no change of its equipment is ever reported.
#[tokio::test]
async fn a_mob_met_wearing_something_is_shown_wearing_it() {
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(1), far_key(1)).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
    edge.settle(WEST).await;
    let key = far_key(100);
    let bow = clustine_world::ItemStack::new(1021, 1);
    let mut skeleton = a_cow(key, HOME);
    if let EntityKind::Other { kind, data, .. } = &mut skeleton.kind {
        *kind = entity_types::SKELETON as u16;
        *data = clustine_sim::shown::mob_data_wearing(&[(0, bow)]);
    }
    edge.tell(WEST, ticked(vec![RegionEvent::EntitySpawned(skeleton)]));
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    let worn = |living: &[ClientboundPlay]| {
        living
            .iter()
            .filter_map(|packet| match packet {
                ClientboundPlay::SetEquipment(worn) => Some(worn.slots.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let expected = vec![vec![(0, Some(WireStack::new(1021, 1)))]];
    assert!(watcher.entities.contains(&key.0), "{:?}", watcher.entities);
    assert_eq!(worn(&watcher.living), expected, "{:?}", watcher.living);

    let mut newcomer = edge.joined(player(2), far_key(2)).await;
    edge.quiet().await;
    edge.sync(&mut newcomer).await;
    assert_eq!(worn(&newcomer.living), expected, "{:?}", newcomer.living);
    edge.end().await;
}

/// Entities appear one after another and are removed: none is given an id an
/// entity had before, nor the id of a player's login.
#[tokio::test]
async fn an_id_is_not_given_to_another_entity_after_its_entity_was_removed() {
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(1), far_key(1)).await;
    edge.quiet().await;
    edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
    edge.settle(WEST).await;
    let mut ids = vec![];
    for n in 0..4 {
        let key = far_key(100 + n);
        edge.tell(
            WEST,
            ticked(vec![RegionEvent::EntitySpawned(far_stranger(key, HOME))]),
        );
        edge.tell(WEST, ticked(vec![gone(key, HOME)]));
        edge.settle(WEST).await;
        edge.sync(&mut watcher).await;
        let naming = watcher.naming(key.0);
        assert_eq!(naming.first().map(|(how, _)| *how), Some(Naming::Spawn));
        assert_eq!(naming.last().map(|(how, _)| *how), Some(Naming::Removal));
        ids.push(naming[0].1);
    }
    ids.push(watcher.own_id());
    let distinct: BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), ids.len(), "an id was given twice: {ids:?}");
    assert!(watcher.entities.is_empty(), "{:?}", watcher.entities);
    edge.end().await;
}

/// An entity that its only viewer walks away from leaves the replica with its
/// chunk, and the edge lets its id go; when the viewer comes back, it is shown by a
/// new one, and was hidden by the old.
#[tokio::test]
async fn an_entity_that_left_every_view_is_known_by_a_new_id_when_it_comes_back() {
    let watcher_key = far_key(1);
    let stranger_key = far_key(2);
    let mut edge = Harness::start().await;
    let mut watcher = edge.joined(player(1), watcher_key).await;
    edge.quiet().await;
    let there = vec![far_stranger(stranger_key, FAR)];
    edge.tell(
        WEST,
        snapshot_of(FAR, edge.ask(WEST, FAR), empty_chunk(), there),
    );
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&stranger_key.0));
    let before = watcher.naming(stranger_key.0)[0].1;

    edge.tell(WEST, walked(watcher_key, HOME, STEP_EAST));
    edge.quiet().await;
    edge.sync(&mut watcher).await;
    assert!(!watcher.entities.contains(&stranger_key.0));
    assert_eq!(
        watcher.naming(stranger_key.0).last(),
        Some(&(Naming::Removal, before))
    );

    edge.tell(WEST, walked(watcher_key, STEP_EAST, HOME));
    edge.settle(WEST).await;
    let there = vec![far_stranger(stranger_key, FAR)];
    edge.tell(
        WEST,
        snapshot_of(FAR, edge.ask(WEST, FAR), empty_chunk(), there),
    );
    edge.settle(WEST).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&stranger_key.0));
    let after = watcher
        .naming(stranger_key.0)
        .last()
        .expect("it was shown")
        .1;
    let shown_again = watcher.naming(stranger_key.0)[3];
    assert_eq!(shown_again, (Naming::Spawn, after));
    assert_ne!(
        after, before,
        "the edge kept the id of an entity nobody sees"
    );
    edge.end().await;
}

// Scenario 20: generated runs.
//
// A run is a sequence of small steps, each chosen by a generator of numbers that
// starts from the run's seed: a player joins, walks a chunk or leaves; a region
// looks at some of what the edge has sent it and makes its answers; one message
// of a region is read by the edge; a link ends or is replaced; a region gives a
// chunk back, comes to believe something that is no longer so, or forgets the
// edge; time passes. The regions are played by a model of what a region may send
// by section 5 of ADR-0012, with a world store that says who holds a chunk: the
// west, the east and the north are pinned to their parts of the world, and the
// chunks between them are held by whoever was granted them first. What a region
// has made reaches the edge later, in the order of its link, so answers come for
// subscriptions the edge has changed since.
//
// Beside the edge runs what ADR-0013 says an edge holds: the players with their
// views, and a subscription per region and chunk, changed by `want` and `unwant`
// and by the answers as sections 2 to 6 have it. It takes only the numbers from
// what the edge sends. After every step:
//
// - the subscriptions the edge's messages make at each region it has a link to
//   are the ones this account has, of the same kind, and a hello names exactly
//   the account's subscriptions of a region without a link;
// - statements V, G and E hold for the account, so that a region is asked as a
//   viewer exactly for what its players see and for nothing nobody sees;
// - the subscription messages of a link ascend, and so do its numbered messages
//   and each player's inputs, without a gap;
// - a client was sent a chunk only if a snapshot of it was taken, has every chunk
//   it sees of which one was taken, and shows it as that snapshot and what the
//   region said since have it.
//
// Every now and then, and at the end, everything comes to rest. Then everybody is
// in the region the edge has them in, at the place their last step took them to,
// and is shown every chunk they see as its holder has it, and the others who
// stand there.
//
// Scenario 32 of ADR-0015 is the same with regions that merge and split
// (`Run::reshaping`). A region absorbs another, or the players standing in one of
// its chunks are split off with what is around them; both close the region's
// links. The edge's process hands the edge what the routing table says of
// absorbed regions at moments of the run's choosing. A region answers a hello as
// ADR-0014, section 3.7, has it, and beside the edge runs what ADR-0015 says it
// holds: which region stands for which, which entries came with a merge, whom a
// welcome brought. What the edge keeps for a region a run cannot see. Where a
// rule turns on it (a join or an arrival that is kept, a region of which the edge
// has nothing), the run goes by the numbered messages the edge was seen to send
// and by what the regions said they had applied.

/// A generator of numbers that gives the same sequence for the same seed.
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A number below `limit`.
    fn below(&mut self, limit: usize) -> usize {
        (self.roll() % limit as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.roll() % 100 < percent
    }
}

/// The region a chunk is pinned to in a generated run. The chunks between the
/// pinned areas are held by whoever was granted them first, and given back.
fn pinned(chunk: ChunkPos) -> Option<RegionId> {
    match chunk {
        ChunkPos { x, .. } if x <= 0 => Some(WEST),
        ChunkPos { x, z } if x >= 4 && z >= 0 => Some(EAST),
        ChunkPos { x, .. } if x >= 4 => Some(NORTH),
        _ => None,
    }
}

fn chunk_of(position: Vec3) -> ChunkPos {
    ChunkPos::containing(position.x, position.z)
}

/// What has become of a subscription: what the region has made of it, or what the
/// edge has read of that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Waiting,
    Served,
    Told(RegionId),
}

/// A subscription as a region has it for its link.
#[derive(Debug)]
struct Ticket {
    role: Role,
    ask: u64,
    answer: Answer,
}

/// A player in a region.
#[derive(Debug)]
struct Resident {
    entity: EntityId,
    chunk: ChunkPos,
    last_input: u64,
    /// The attempt of the stay's join, until the region applies an input of the
    /// stay (`docs/adr/0020-one-stay-per-player.md`, section 4.3).
    attempt: Option<u64>,
}

/// Something a region has made for its link and not sent yet.
#[derive(Debug)]
enum Made {
    Said(WorkerToEdge),
    /// An entry of its outbox, with its number.
    Entry(u64, Durable),
    /// The answer to a hello, which goes out in one piece: the welcome, the
    /// entries it announces and the presence answers.
    Welcome {
        resumed: bool,
        since: u64,
        entries: Vec<(u64, Durable)>,
        presences: Vec<WorkerToEdge>,
        /// The number of the edge's last numbered message the region had taken
        /// when it answered the hello.
        applied: u64,
    },
}

/// A region as a generated run plays it.
#[derive(Debug, Default)]
struct Played {
    /// What the edge has sent on the link that the region has not looked at.
    inbox: VecDeque<EdgeMessage>,
    /// What the region has made for the link and the edge has not been sent.
    out: VecDeque<Made>,
    tickets: BTreeMap<ChunkPos, Ticket>,
    /// The chunks its link's hello named that it has not answered yet. Until it
    /// has, it looks at nothing the edge has sent behind the hello: what the edge
    /// sends again is to act on chunks that are there, and whoever stands in a
    /// chunk is in its snapshot before anything else is said of them.
    held: BTreeSet<ChunkPos>,
    /// The number of the last subscription message it looked at.
    asked: u64,
    /// Who the store said holds chunks the region does not hold. It does not
    /// learn by itself that this has changed.
    believes: BTreeMap<ChunkPos, RegionId>,
    residents: BTreeMap<PlayerId, Resident>,
    /// The entries of its outbox that the edge has not confirmed, each with its
    /// number, and the number of the last entry it made.
    outbox: VecDeque<(u64, Durable)>,
    sent: u64,
    /// Whether the region is there: one of the three the world began with, or a
    /// part that was split off, and not absorbed since.
    alive: bool,
    /// Whether it knows the edge. One that has forgotten it makes a state for it
    /// when the edge says hello again.
    knows: bool,
    /// The number of the edge's last numbered message it took.
    received: u64,
    /// Its word for the numbering it shares with the edge.
    since: u64,
    entities: i32,
    tick: u64,
}

/// What one tick of a region makes for its link, in the order it is sent.
#[derive(Default)]
struct Ticked {
    events: Vec<RegionEvent>,
    spawned: Vec<WorkerToEdge>,
    entries: Vec<Durable>,
    departures: Vec<Durable>,
    inputs: Vec<(PlayerId, u64)>,
}

/// How long after a subscription was asked again the next asking comes at once.
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(1);

/// A subscription as the edge has to hold it by ADR-0013.
#[derive(Debug)]
struct Kept {
    role: Role,
    answer: Answer,
    ask: u64,
    begun: u64,
    /// How many viewers of this region's players see the chunk.
    viewers: u32,
    /// The region it was told elsewhere with has said `NotMine`, and it is to be
    /// asked again with the edge's next messages.
    at_once: bool,
    /// The same, when it was asked again less than a second before: the asking
    /// is due at one of the edge's checks.
    due: bool,
    asked_again: Option<Instant>,
}

impl Kept {
    fn new(role: Role, viewers: u32) -> Self {
        Self {
            role,
            answer: Answer::Waiting,
            ask: 0,
            begun: 0,
            viewers,
            at_once: false,
            due: false,
            asked_again: None,
        }
    }
}

/// A player as the edge has to have them.
struct Person {
    client: Client,
    region: RegionId,
    entity: Option<EntityId>,
    centre: ChunkPos,
    wanted: BTreeSet<ChunkPos>,
    /// Where the last step they made takes them, and how many steps they made.
    target: ChunkPos,
    inputs: u64,
    /// Where they are walking to.
    goal: ChunkPos,
    /// How many of the chunks their client was sent have been looked at.
    looked_at: usize,
    /// Whether their join is among what the edge keeps for the home region, and
    /// its number once the edge has been seen to send it.
    join: Option<Option<u64>>,
}

/// What the edge has to hold about its link to a region while it resumes on it
/// (ADR-0015, section 1).
#[derive(Debug, Default)]
struct Resume {
    /// How many of the entries and of the presence answers that the welcome
    /// announced are still to come; nothing before the welcome.
    entries: Option<u32>,
    presences: Option<u32>,
    /// The players this welcome made the region's, of whom no `Present` has come.
    brought: BTreeSet<PlayerId>,
    /// The regions whose `Absorbed` the region owed when the hello was said.
    owed: BTreeSet<RegionId>,
}

/// Every region a test can play, in ascending order.
fn ports() -> impl Iterator<Item = RegionId> {
    (0..PORTS).map(|index| RegionId(index as u32))
}

struct Run {
    seed: u64,
    dice: Dice,
    edge: Harness,
    log: Vec<String>,
    // The world as it is.
    granted: BTreeMap<ChunkPos, RegionId>,
    /// Chunks a region is giving back: the store names it for them until it has
    /// freed them.
    returning: BTreeSet<ChunkPos>,
    content: BTreeMap<ChunkPos, BlockState>,
    played: Vec<Played>,
    /// The entities that a region has let go and no region has taken in or
    /// discarded, each with the chunk it is to arrive in and the region it was
    /// let go to.
    on_the_way: BTreeMap<EntityId, (ChunkPos, RegionId)>,
    // What the edge has to hold.
    people: BTreeMap<PlayerId, Person>,
    kept: Vec<BTreeMap<ChunkPos, Kept>>,
    /// What the edge's messages on each current link make of the region's
    /// subscriptions: the kind, the number of the last message that named the
    /// chunk, and the number the subscription began with.
    heard: Vec<BTreeMap<ChunkPos, (Role, u64, u64)>>,
    /// Subscriptions the edge asked for again in the step that is being read.
    again: BTreeSet<(usize, ChunkPos)>,
    /// The chunks the replica has a snapshot of, as that snapshot and what the
    /// region said since leave them.
    replica: BTreeMap<ChunkPos, BlockState>,
    served_by: BTreeMap<ChunkPos, RegionId>,
    /// Chunks of the replica of which the records do not say what they are like
    /// until the next snapshot: a block changed in them when the edge did not
    /// hold their region to serve them.
    unsure: BTreeSet<ChunkPos>,
    /// The `since` of the last welcome the edge read from each region.
    since: [u64; PORTS],
    /// Whether regions merge and split in this run.
    reshaping: bool,
    /// The regions that were absorbed, each with the region it went into, as the
    /// routing table has them, and how many parts were split off.
    absorbed: Vec<(RegionId, RegionId)>,
    parts: usize,
    /// The last `since` a region has given out.
    sinces: u64,
    // What the edge has to hold through merges and splits (ADR-0015).
    /// The regions that are no more, as far as the edge has acted on it.
    stands_for: BTreeMap<RegionId, RegionId>,
    /// The entries of each region's outbox that came to it with a merge.
    came_with: Vec<BTreeMap<u64, (RegionId, u64)>>,
    /// The regions the table says went into each region, of which the edge has
    /// something and has handled no `Absorbed`.
    owes: Vec<BTreeSet<RegionId>>,
    /// The number of the last outbox entry the edge has seen of each region.
    seen: [u64; PORTS],
    /// How far each region has told the edge that it applied its messages, and
    /// the highest number the edge was seen to send each region.
    applied: [u64; PORTS],
    numbered: [u64; PORTS],
    /// The regions for which the edge has made a numbered message that it could
    /// not send yet. With `numbered` and `applied` this says whether anything is
    /// kept for a region, which the run cannot see for one without a link.
    unsent: [bool; PORTS],
    /// The arrivals among what the edge keeps: the region each is for, the player,
    /// and its number once the edge has been seen to send it.
    arrivals: Vec<(RegionId, PlayerId, Option<u64>)>,
    resumes: Vec<Option<Resume>>,
    /// The links the edge has to end by itself, over what the table says.
    to_end: BTreeSet<RegionId>,
    /// Clients of players the edge had to disconnect.
    dropped: Vec<Client>,
    joined: u128,
    /// How often a run came to the cases it is there for.
    tally: BTreeMap<&'static str, u64>,
}

impl Drop for Run {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // The whole account for whoever asks for it; it is long.
            let whole = std::env::var_os("CLUSTINE_WHOLE_RUN").is_some();
            let from = if whole {
                0
            } else {
                self.log.len().saturating_sub(60)
            };
            eprintln!(
                "seed {}, {} steps; the last of them:\n{}",
                self.seed,
                self.log.len(),
                self.log[from..].join("\n")
            );
        }
    }
}

/// A message of a region in few words, for the account of a run.
fn brief(message: &WorkerToEdge) -> String {
    match message {
        WorkerToEdge::ChunkSnapshot {
            position,
            ask,
            chunk,
            entities,
            ..
        } => {
            let state = chunk.get(3, -60, 5);
            let entities: Vec<_> = entities.iter().map(|state| state.entity.0).collect();
            format!(
                "ChunkSnapshot ({}, {}) ask {ask} {state:?} {entities:?}",
                position.x, position.z
            )
        }
        other => format!("{other:?}"),
    }
}

impl Run {
    async fn new(seed: u64, reshaping: bool) -> Self {
        // The witness lets a run wait for the edge between the entries of a
        // welcome, where `settle` cannot be used. The edge has read the regions'
        // first welcomes when the run begins.
        let edge = Harness::witnessed().await;
        let mut played = Vec::new();
        let mut since = [0; PORTS];
        let mut resumes = Vec::new();
        for (index, since) in since.iter_mut().enumerate() {
            let there = index < REGIONS.len();
            *since = u64::from(there);
            played.push(Played {
                since: *since,
                entities: 1000 * (index as i32 + 1),
                // What the witness's arrangement had the region say and take.
                sent: edge.outbox[index],
                received: edge.heard[index].numbered,
                alive: there,
                knows: there,
                ..Played::default()
            });
            resumes.push(there.then(|| Resume {
                entries: Some(0),
                presences: Some(0),
                ..Resume::default()
            }));
        }
        let seen = edge.outbox;
        let mut applied = [0; PORTS];
        for (index, applied) in applied.iter_mut().enumerate() {
            *applied = edge.heard[index].numbered;
        }
        Self {
            seed,
            dice: Dice(seed),
            edge,
            log: Vec::new(),
            granted: BTreeMap::new(),
            returning: BTreeSet::new(),
            content: BTreeMap::new(),
            played,
            on_the_way: BTreeMap::new(),
            people: BTreeMap::new(),
            kept: (0..PORTS).map(|_| BTreeMap::new()).collect(),
            heard: (0..PORTS).map(|_| BTreeMap::new()).collect(),
            again: BTreeSet::new(),
            replica: BTreeMap::new(),
            served_by: BTreeMap::new(),
            unsure: BTreeSet::new(),
            since,
            reshaping,
            absorbed: Vec::new(),
            parts: 0,
            sinces: 1,
            stands_for: BTreeMap::new(),
            came_with: (0..PORTS).map(|_| BTreeMap::new()).collect(),
            owes: (0..PORTS).map(|_| BTreeSet::new()).collect(),
            seen,
            applied,
            numbered: applied,
            unsent: [false; PORTS],
            arrivals: Vec::new(),
            resumes,
            to_end: BTreeSet::new(),
            dropped: Vec::new(),
            joined: 0,
            tally: BTreeMap::new(),
        }
    }

    fn count(&mut self, what: &'static str) {
        *self.tally.entry(what).or_default() += 1;
    }

    fn fail(&self, what: String) -> ! {
        panic!("seed {}, step {}: {what}", self.seed, self.log.len());
    }

    fn linked(&self, region: RegionId) -> bool {
        self.edge.links[region.0 as usize].is_some()
    }

    // The world.

    /// The regions that are there, in ascending order.
    fn living(&self) -> Vec<RegionId> {
        let alive = |region: &RegionId| self.played[region.0 as usize].alive;
        ports().filter(alive).collect()
    }

    /// The region that `region` is by now, if it was absorbed.
    fn now(&self, region: RegionId) -> RegionId {
        let mut region = region;
        while let Some((_, into)) = self.absorbed.iter().find(|(gone, _)| *gone == region) {
            region = *into;
        }
        region
    }

    /// What was granted comes first: a part holds chunks of the areas that the
    /// region it was split off is pinned to, until it gives them back.
    fn holder(&self, chunk: ChunkPos) -> Option<RegionId> {
        let granted = self.granted.get(&chunk).copied();
        granted.or_else(|| pinned(chunk).map(|region| self.now(region)))
    }

    fn state(&self, chunk: ChunkPos) -> BlockState {
        self.content.get(&chunk).copied().unwrap_or(AIR)
    }

    /// What `region` finds when it needs `chunk`: nothing if it holds the chunk,
    /// which it does from now on if nobody held it, or the region that does. With
    /// `trusting` it goes by what it believes, if it believes something, and
    /// asks the store only otherwise.
    fn needs(&mut self, region: RegionId, chunk: ChunkPos, trusting: bool) -> Option<RegionId> {
        let found = self.holder(chunk);
        let played = &mut self.played[region.0 as usize];
        if found == Some(region) {
            // A claim that comes before the chunk is free keeps it.
            self.returning.remove(&chunk);
            played.believes.remove(&chunk);
            return None;
        }
        if trusting && let Some(believed) = played.believes.get(&chunk).copied() {
            if found != Some(believed) {
                self.count("a region goes by a belief that is no longer so");
            }
            return Some(believed);
        }
        match found {
            Some(holder) => {
                played.believes.insert(chunk, holder);
                Some(holder)
            }
            None => {
                self.granted.insert(chunk, region);
                played.believes.remove(&chunk);
                None
            }
        }
    }

    /// One tick of a region: it looks at what the edge has sent it, at all of it
    /// or at the first of it, answers the subscriptions that wait, all or some,
    /// and what it makes waits to be sent in the order of ADR-0012, section 5.2.
    fn tick(&mut self, region: RegionId, thorough: bool) {
        let index = region.0 as usize;
        let take = if !self.played[index].held.is_empty() {
            0
        } else if thorough || self.dice.chance(70) {
            usize::MAX
        } else {
            self.dice.below(4)
        };
        let answering = if thorough {
            100
        } else {
            [0, 50, 100, 100][self.dice.below(4)]
        };
        self.played[index].tick += 1;
        let mut ticked = Ticked::default();
        let mut taken = 0;
        while taken < take {
            let Some(message) = self.played[index].inbox.pop_front() else {
                break;
            };
            taken += 1;
            self.look_at(region, message, &mut ticked);
        }

        // An entity has one new pose in a tick at most: where it is after the tick,
        // with the chunk it was in before.
        let mut events: Vec<RegionEvent> = Vec::new();
        for event in std::mem::take(&mut ticked.events) {
            if let RegionEvent::EntityMoved { entity, pose, .. } = &event {
                let earlier = events.iter_mut().find_map(|earlier| match earlier {
                    RegionEvent::EntityMoved {
                        entity: moved,
                        pose,
                        ..
                    } if moved == entity => Some(pose),
                    _ => None,
                });
                if let Some(earlier) = earlier {
                    *earlier = *pose;
                    continue;
                }
            }
            events.push(event);
        }
        ticked.events = events;

        // The events of the chunks the link is subscribed to, if the region has
        // not said that another holds them.
        let tickets = &self.played[index].tickets;
        ticked.events.retain(|event| {
            event.chunks().iter().any(|chunk| {
                tickets
                    .get(chunk)
                    .is_some_and(|ticket| !matches!(ticket.answer, Answer::Told(_)))
            })
        });
        let tick = self.played[index].tick;
        let mut made = Vec::new();
        if !ticked.events.is_empty() {
            let events = std::mem::take(&mut ticked.events);
            made.push(Made::Said(WorkerToEdge::TickDelta { tick, events }));
        }
        made.extend(ticked.spawned.into_iter().map(Made::Said));
        // An entry has its number from when it is made, and stays in the outbox
        // until the edge has confirmed it.
        let played = &mut self.played[index];
        for entry in ticked.entries.into_iter().chain(ticked.departures) {
            played.sent += 1;
            played.outbox.push_back((played.sent, entry.clone()));
            made.push(Made::Entry(played.sent, entry));
        }

        let waiting: Vec<_> = self.played[index]
            .tickets
            .iter()
            .filter(|(_, ticket)| ticket.answer == Answer::Waiting)
            .map(|(chunk, ticket)| (*chunk, ticket.role, ticket.ask))
            .collect();
        for (chunk, role, ask) in waiting {
            if !self.dice.chance(answering) {
                continue;
            }
            self.played[index].held.remove(&chunk);
            let another = match role {
                Role::Viewer => self.needs(region, chunk, true),
                // A guest makes a region claim nothing outside its pinned area.
                Role::Guest => {
                    let holds = self.holder(chunk) == Some(region);
                    (!holds || self.returning.contains(&chunk)).then_some(region)
                }
            };
            let answer = match (role, another) {
                (_, None) => {
                    let entities = self.played[index]
                        .residents
                        .iter()
                        .filter(|(_, resident)| resident.chunk == chunk)
                        .map(|(player, resident)| EntityState {
                            entity: resident.entity,
                            kind: EntityKind::Player {
                                game_mode: 1,
                                data: Default::default(),
                                player: *player,
                                name: "Player".to_owned(),
                            },
                            pose: Pose::at(within(chunk)),
                        })
                        .collect();
                    let state = self.state(chunk);
                    let ticket = self.played[index]
                        .tickets
                        .get_mut(&chunk)
                        .expect("the ticket was there");
                    ticket.answer = Answer::Served;
                    WorkerToEdge::ChunkSnapshot {
                        position: chunk,
                        ask,
                        tick,
                        chunk: chunk_with(chunk, state),
                        entities,
                    }
                }
                (Role::Viewer, Some(holder)) => {
                    let ticket = self.played[index]
                        .tickets
                        .get_mut(&chunk)
                        .expect("the ticket was there");
                    ticket.answer = Answer::Told(holder);
                    elsewhere(chunk, ask, holder)
                }
                (Role::Guest, Some(_)) => {
                    self.played[index].tickets.remove(&chunk);
                    if self.returning.contains(&chunk) {
                        self.count("a guest is told not mine for a chunk that is given back");
                    }
                    not_mine(chunk, ask)
                }
            };
            made.push(Made::Said(answer));
        }
        if taken > 0 {
            let progress = WorkerToEdge::Progress {
                applied: self.played[index].received,
                inputs: ticked.inputs,
            };
            made.push(Made::Said(progress));
        }
        self.log.push(format!(
            "{region:?} looks at {taken} messages and makes {}",
            made.len()
        ));
        self.played[index].out.extend(made);
    }

    /// A region looks at one message of the edge.
    fn look_at(&mut self, region: RegionId, message: EdgeMessage, ticked: &mut Ticked) {
        let index = region.0 as usize;
        if let Some(number) = message.number {
            let received = self.played[index].received;
            if number <= received {
                // Sent again on a new link.
                return;
            }
            if number != received + 1 {
                self.fail(format!(
                    "{region:?} was sent the numbered message {number} after {received}"
                ));
            }
            self.played[index].received = number;
        }
        match message.body {
            EdgeToWorker::Subscribe { ask, chunks } => {
                self.asked(region, ask);
                let played = &mut self.played[index];
                for chunk in chunks {
                    match played.tickets.get_mut(&chunk) {
                        Some(ticket) => {
                            ticket.role = Role::Viewer;
                            ticket.ask = ask;
                            // Asked again, the region asks the store again.
                            if matches!(ticket.answer, Answer::Told(_)) {
                                ticket.answer = Answer::Waiting;
                                played.believes.remove(&chunk);
                            }
                        }
                        None => {
                            let ticket = Ticket {
                                role: Role::Viewer,
                                ask,
                                answer: Answer::Waiting,
                            };
                            played.tickets.insert(chunk, ticket);
                        }
                    }
                }
            }
            EdgeToWorker::SubscribeAsGuest { ask, chunks } => {
                self.asked(region, ask);
                let played = &mut self.played[index];
                for chunk in chunks {
                    match played.tickets.get_mut(&chunk) {
                        Some(ticket) => {
                            ticket.role = Role::Guest;
                            ticket.ask = ask;
                            if matches!(ticket.answer, Answer::Told(_)) {
                                ticket.answer = Answer::Waiting;
                            }
                        }
                        None => {
                            let ticket = Ticket {
                                role: Role::Guest,
                                ask,
                                answer: Answer::Waiting,
                            };
                            played.tickets.insert(chunk, ticket);
                        }
                    }
                }
            }
            EdgeToWorker::Unsubscribe { ask, chunks } => {
                self.asked(region, ask);
                for chunk in chunks {
                    self.played[index].tickets.remove(&chunk);
                    // What it believed of the chunk it may keep for another need.
                    if self.dice.chance(20) {
                        self.played[index].believes.remove(&chunk);
                    }
                }
            }
            EdgeToWorker::PlayerJoin(join) => {
                let played = &mut self.played[index];
                // A join begins a new stay whatever the region has (ADR-0014,
                // section 2.1).
                if let Some(before) = played.residents.remove(&join.player) {
                    ticked.events.push(RegionEvent::EntityRemoved {
                        entity: before.entity,
                        chunk: before.chunk,
                    });
                }
                played.entities += 1;
                let entity = EntityId(played.entities as u64);
                let resident = Resident {
                    entity,
                    chunk: HOME,
                    last_input: 0,
                    attempt: Some(join.attempt),
                };
                played.residents.insert(join.player, resident);
                ticked
                    .spawned
                    .push(spawned(join.attempt, join.player, entity));
                ticked.events.push(RegionEvent::EntitySpawned(EntityState {
                    entity,
                    kind: EntityKind::Player {
                        game_mode: 1,
                        data: Default::default(),
                        player: join.player,
                        name: join.name,
                    },
                    pose: Pose::at(SPAWN),
                }));
            }
            EdgeToWorker::PlayerLeave {
                player,
                entity,
                attempt,
            } => {
                // A leave ends the stay it names, and no other: by its entity, or
                // by the attempt of its join for a stay the edge was not told the
                // entity of.
                let residents = &mut self.played[index].residents;
                let named = residents.get(&player).is_some_and(|resident| match entity {
                    Some(entity) => resident.entity == entity,
                    None => attempt.is_some() && resident.attempt == attempt,
                });
                if named && let Some(resident) = residents.remove(&player) {
                    ticked.events.push(RegionEvent::EntityRemoved {
                        entity: resident.entity,
                        chunk: resident.chunk,
                    });
                }
            }
            EdgeToWorker::PlayerArrive { player, transfer } => {
                let chunk = chunk_of(transfer.pose.position);
                self.on_the_way.remove(&transfer.entity_id);
                // One claim covers the chunk a player arrives in and their view:
                // the region has been asked for the chunk as a viewer, unless the
                // player has left or gone on since.
                let theirs = self.people.get(&player).is_some_and(|person| {
                    person.region == region && person.entity == Some(transfer.entity_id)
                });
                let asked = self.played[index]
                    .tickets
                    .get(&chunk)
                    .is_some_and(|ticket| ticket.role == Role::Viewer);
                if theirs && !asked {
                    self.fail(format!(
                        "{player:?} arrives in {region:?} in {chunk:?}, which it was not \
                             asked for as a viewer before"
                    ));
                }
                // Of two stays of a player the later one stays, which is the one
                // with the higher entity.
                let residents = &mut self.played[index].residents;
                let there = residents.get(&player);
                if let Some((entity, stood)) = there.map(|there| (there.entity, there.chunk)) {
                    if entity >= transfer.entity_id {
                        if entity != transfer.entity_id {
                            ticked.events.push(RegionEvent::EntityRemoved {
                                entity: transfer.entity_id,
                                chunk,
                            });
                        }
                        return;
                    }
                    ticked.events.push(RegionEvent::EntityRemoved {
                        entity,
                        chunk: stood,
                    });
                    residents.remove(&player);
                }
                match self.needs(region, chunk, false) {
                    None => {
                        let resident = Resident {
                            entity: transfer.entity_id,
                            chunk,
                            last_input: transfer.last_input,
                            attempt: transfer.attempt,
                        };
                        self.played[index].residents.insert(player, resident);
                        ticked.events.push(RegionEvent::EntitySpawned(EntityState {
                            entity: transfer.entity_id,
                            kind: EntityKind::Player {
                                game_mode: 1,
                                data: Default::default(),
                                player,
                                name: transfer.name,
                            },
                            pose: transfer.pose,
                        }));
                    }
                    Some(holder) => {
                        self.count("an arrival that is sent on");
                        self.on_the_way.insert(transfer.entity_id, (chunk, holder));
                        ticked.entries.push(Durable::NotMine {
                            what: Misdirected::Arrival { player, transfer },
                            holder,
                        });
                    }
                }
            }
            EdgeToWorker::Discard { entity, chunk, .. } => {
                self.on_the_way.remove(&entity);
                ticked
                    .events
                    .push(RegionEvent::EntityRemoved { entity, chunk });
            }
            EdgeToWorker::Input {
                player,
                entity,
                number,
                input,
            } => {
                let Some(resident) = self.played[index].residents.get_mut(&player) else {
                    // Not this region's: the edge sends it to the region that is
                    // theirs, or sends it again when they have arrived.
                    return;
                };
                // An input is of the stay it names, and passed over by a region
                // that has another.
                if resident.entity != entity {
                    return;
                }
                if number <= resident.last_input {
                    return;
                }
                if number != resident.last_input + 1 {
                    let last = resident.last_input;
                    self.fail(format!(
                        "{region:?} was sent the input {number} of {player:?} after {last}: \
                             what they did in between is lost"
                    ));
                }
                let PlayerInput::Move {
                    position: Some(position),
                    ..
                } = input
                else {
                    panic!("a generated run has players walk only");
                };
                let entity = resident.entity;
                let previous_chunk = resident.chunk;
                let target = chunk_of(position);
                resident.last_input = number;
                resident.attempt = None;
                resident.chunk = target;
                ticked.inputs.push((player, number));
                ticked.events.push(RegionEvent::EntityMoved {
                    entity,
                    pose: Pose::at(position),
                    previous_chunk,
                });
                // A player who steps into a chunk the region believes another's
                // is let go to that region.
                if let Some(to) = self.needs(region, target, true) {
                    self.played[index].residents.remove(&player);
                    self.on_the_way.insert(entity, (target, to));
                    let mut transfer = transfer(entity, number, target);
                    transfer.pose = Pose::at(position);
                    ticked.departures.push(Durable::Departed {
                        player,
                        transfer,
                        to,
                    });
                }
            }
            other => panic!("a generated run does not have the edge say {other:?}"),
        }
    }

    /// A region takes the subscription messages of a link in ascending order.
    fn asked(&mut self, region: RegionId, ask: u64) {
        let played = &mut self.played[region.0 as usize];
        if ask <= played.asked {
            let before = played.asked;
            self.fail(format!(
                "{region:?} was sent the subscription message {ask} after {before}"
            ));
        }
        played.asked = ask;
    }

    // What the edge has to hold.

    fn seen(&self, chunk: ChunkPos) -> bool {
        self.people
            .values()
            .any(|person| person.wanted.contains(&chunk))
    }

    /// ADR-0013, section 2.
    fn want(&mut self, region: RegionId, chunk: ChunkPos) {
        let subscriptions = &mut self.kept[region.0 as usize];
        match subscriptions.get_mut(&chunk) {
            Some(kept) => {
                // A guest's becomes a viewer's; what it waits for and the number
                // it began with stay.
                let was = (kept.role, kept.answer);
                kept.role = Role::Viewer;
                kept.viewers += 1;
                match was {
                    (Role::Guest, Answer::Waiting) => {
                        self.count("a guest's that waits becomes a viewer's");
                    }
                    (Role::Guest, _) => self.count("a served guest's becomes a viewer's"),
                    _ => {}
                }
            }
            None => {
                subscriptions.insert(chunk, Kept::new(Role::Viewer, 1));
            }
        }
    }

    /// ADR-0013, section 2. Whoever stops seeing the chunk altogether has been
    /// taken out of `wanted` before.
    fn unwant(&mut self, region: RegionId, chunk: ChunkPos) {
        let seen = self.seen(chunk);
        let subscriptions = &mut self.kept[region.0 as usize];
        let Some(kept) = subscriptions.get_mut(&chunk) else {
            self.fail(format!(
                "the account has no subscription at {region:?} for {chunk:?}"
            ));
        };
        kept.viewers -= 1;
        if kept.viewers > 0 {
            return;
        }
        let was = kept.answer;
        if matches!(kept.answer, Answer::Told(_)) {
            subscriptions.remove(&chunk);
            // Nothing is asked here any more, so whoever was told elsewhere with
            // this region asks again, as after its `NotMine`.
            if seen {
                self.those_told_ask_again(region, chunk);
            }
        } else {
            kept.role = Role::Guest;
        }
        match (was, seen) {
            (Answer::Told(_), true) => self.count("one told elsewhere ends, others see it"),
            (Answer::Waiting, true) => self.count("a viewer's that waits becomes a guest's"),
            (Answer::Served, true) => self.count("a served viewer's becomes a guest's"),
            (_, false) => {}
        }
        if !seen {
            for subscriptions in &mut self.kept {
                subscriptions.remove(&chunk);
            }
            self.replica.remove(&chunk);
            self.served_by.remove(&chunk);
            self.unsure.remove(&chunk);
        }
    }

    /// Nothing is asked of `holder` for `chunk` any more: the viewers' subscriptions
    /// that were told elsewhere with it are asked again, at once or, where that
    /// was done less than a second ago, at one of the edge's checks.
    fn those_told_ask_again(&mut self, holder: RegionId, chunk: ChunkPos) {
        let now = Instant::now();
        let mut asked_again = Vec::new();
        for subscriptions in &mut self.kept {
            let Some(kept) = subscriptions.get_mut(&chunk) else {
                continue;
            };
            if kept.role != Role::Viewer || kept.answer != Answer::Told(holder) {
                continue;
            }
            let lately = kept
                .asked_again
                .is_some_and(|at| now.duration_since(at) < ASK_AGAIN_AFTER);
            if lately {
                kept.due = true;
            } else {
                kept.at_once = true;
            }
            asked_again.push(lately);
        }
        for lately in asked_again {
            self.count(if lately {
                "an asking again that is due"
            } else {
                "an asking again at once"
            });
        }
    }

    /// A player's view is centred anew.
    fn recentre(&mut self, player: PlayerId, centre: ChunkPos) {
        let person = self.people.get_mut(&player).expect("the player is there");
        let region = person.region;
        let before = std::mem::replace(&mut person.wanted, view(centre));
        person.centre = centre;
        let now = view(centre);
        for chunk in before.difference(&now) {
            self.unwant(region, *chunk);
        }
        for chunk in now.difference(&before) {
            self.want(region, *chunk);
        }
    }

    /// A player leaves or is removed: they are unwanted everywhere.
    fn gone(&mut self, player: PlayerId) -> Option<Person> {
        let person = self.people.remove(&player)?;
        for chunk in &person.wanted {
            self.unwant(person.region, *chunk);
        }
        Some(person)
    }

    /// Whether the edge has `player` without an entity, on the connection whose
    /// join was the attempt named: a stay that carries that attempt is theirs.
    fn joined_by(&self, player: PlayerId, attempt: Option<u64>) -> bool {
        self.people.get(&player).is_some_and(|person| {
            person.entity.is_none() && attempt == Some(person.client.session.0)
        })
    }

    /// A region has placed a player the edge has not shown their entity yet.
    fn placed(&mut self, region: RegionId, player: PlayerId, entity: EntityId, at: ChunkPos) {
        let Some(person) = self.people.get_mut(&player) else {
            return;
        };
        if person.region != region || person.entity.is_some() {
            return;
        }
        person.entity = Some(entity);
        person.target = at;
        self.recentre(player, at);
    }

    /// The region a name of a region means to the edge: ADR-0015, section 1.
    fn stands(&self, region: RegionId) -> RegionId {
        self.stands_for.get(&region).copied().unwrap_or(region)
    }

    /// Whether the edge's link to `region` has had its welcome and the entries
    /// the welcome announced: what the edge makes for the region is sent then.
    fn through(&self, region: RegionId) -> bool {
        let resume = self.resumes[region.0 as usize].as_ref();
        self.linked(region) && resume.is_some_and(|resume| resume.entries == Some(0))
    }

    /// The edge makes a numbered message for `region`.
    fn made_for(&mut self, region: RegionId) {
        if !self.through(region) {
            self.unsent[region.0 as usize] = true;
        }
    }

    /// ADR-0013, section 4, and ADR-0015, section 4. `to` is the region the entry
    /// names as the edge reads it, and `back` whether the entry may send the
    /// player to the region it comes from: it came with a merge, or names a
    /// region that stands for the one it comes from.
    fn hand_over(
        &mut self,
        from: RegionId,
        player: PlayerId,
        transfer: &PlayerTransfer,
        to: RegionId,
        back: bool,
    ) {
        let entity = self.people.get(&player).map(|person| person.entity);
        if entity != Some(Some(transfer.entity_id)) {
            self.count(match entity {
                None => "a hand-over of a player who left",
                Some(_) => "a hand-over of who a player was before",
            });
            // The entity that is on its way is discarded where it was sent.
            self.made_for(to);
            return;
        }
        let person = &self.people[&player];
        let (theirs, wanted) = (person.region, person.wanted.clone());
        if theirs != from {
            self.count("a hand-over of an earlier stay in a region");
            return;
        }
        if to == from {
            if !back {
                self.fail(format!("{from:?} lets {player:?} go to itself"));
            }
            self.count("an arrival at the region the entry came from");
            self.arrivals.push((from, player, None));
            self.made_for(from);
            self.recentre(player, chunk_of(transfer.pose.position));
            return;
        }
        let linked = (self.linked(from), self.linked(to));
        self.count(match linked {
            (true, true) => "a hand-over",
            (_, false) => "a hand-over into a region without a link",
            (false, true) => "a hand-over out of a region without a link",
        });
        for chunk in wanted {
            self.unwant(from, chunk);
            self.want(to, chunk);
        }
        let person = self.people.get_mut(&player).expect("the player is there");
        person.region = to;
        self.arrivals.push((to, player, None));
        self.made_for(to);
        self.recentre(player, chunk_of(transfer.pose.position));
    }

    /// A player's view moves from one region to another as at a hand-over, with
    /// no arrival: by a presence answer, an `Absorbed` or a `SplitOff`.
    fn moved(&mut self, player: PlayerId, to: RegionId) {
        let person = self.people.get_mut(&player).expect("the player is there");
        let from = std::mem::replace(&mut person.region, to);
        let wanted = person.wanted.clone();
        for chunk in &wanted {
            self.unwant(from, *chunk);
        }
        for chunk in &wanted {
            self.want(to, *chunk);
        }
    }

    /// A region says how far it has applied the edge's messages, in a `Progress`
    /// or in a welcome: what is kept for it up to there is dropped.
    fn progress(&mut self, region: RegionId, applied: u64) {
        let index = region.0 as usize;
        self.applied[index] = self.applied[index].max(applied);
        let applied = self.applied[index];
        self.arrivals
            .retain(|(to, _, number)| *to != region || number.is_none_or(|n| n > applied));
        if region == WEST {
            for person in self.people.values_mut() {
                if let Some(Some(number)) = person.join
                    && number <= applied
                {
                    person.join = None;
                }
            }
        }
    }

    /// What is kept for a region is given up, and its numbering begins anew.
    fn given_up(&mut self, region: RegionId) {
        let index = region.0 as usize;
        self.arrivals.retain(|(to, ..)| *to != region);
        self.seen[index] = 0;
        self.applied[index] = 0;
        self.numbered[index] = 0;
        self.unsent[index] = false;
        self.came_with[index].clear();
    }

    /// Whether a join or an arrival of the player is among what is kept for the
    /// region.
    fn on_their_way(&self, region: RegionId, player: PlayerId) -> bool {
        let joining = region == WEST && self.people[&player].join.is_some();
        let arriving =
            |(to, who, _): &(RegionId, PlayerId, Option<u64>)| *to == region && *who == player;
        joining || self.arrivals.iter().any(arriving)
    }

    /// A player the edge has under `region` is not there by the region's word:
    /// ADR-0015, section 2.1, `Absent`. In a run nobody is disconnected for that
    /// whose stay a region has.
    fn judge(&mut self, region: RegionId, player: PlayerId) {
        if self.on_their_way(region, player) {
            self.count("a player who is absent is on their way");
            return;
        }
        let entity = self.people[&player].entity;
        let has = |played: &Played| {
            let resident = played.residents.get(&player);
            played.alive && resident.is_some_and(|resident| Some(resident.entity) == entity)
        };
        let on_the_way = entity.is_some_and(|entity| self.on_the_way.contains_key(&entity));
        if self.played.iter().any(has) || on_the_way {
            self.fail(format!(
                "the records have the edge disconnect {player:?} as absent from {region:?}, \
                     and their stay is not lost"
            ));
        }
        self.count("a player is judged absent");
        let person = self.gone(player).expect("the player is there");
        self.made_for(region);
        self.dropped.push(person.client);
    }

    /// ADR-0015, section 1: the one place a region comes to stand for another.
    fn retire(&mut self, gone: RegionId, into: RegionId) {
        self.stands_for.insert(gone, into);
        for stands in self.stands_for.values_mut() {
            if *stands == gone {
                *stands = into;
            }
        }
        self.owes[into.0 as usize].remove(&gone);
    }

    /// ADR-0015, section 3: `Absorbed`, the entry numbered `number` of `into`.
    fn absorbed_into(
        &mut self,
        into: RegionId,
        number: u64,
        gone: RegionId,
        since: u64,
        applied: u64,
        numbers: &[u64],
    ) {
        let (a, b) = (into.0 as usize, gone.0 as usize);
        self.count("an absorbed is handled");
        // 1. Whether the absorbed region shared a numbering with the edge.
        let shared = since != 0 && since == self.since[b];
        let mut applied = applied;
        if !shared {
            if self.seen[b] != 0 || self.applied[b] != 0 {
                self.count("an absorbed region had forgotten the edge");
                self.given_up(gone);
            }
            applied = 0;
        }
        // 2.
        self.retire(gone, into);
        // 3. Its players and their views.
        let theirs: Vec<_> = self
            .people
            .iter()
            .filter(|(_, person)| person.region == gone)
            .map(|(player, _)| *player)
            .collect();
        for player in theirs {
            self.count("a merge brings a player");
            self.moved(player, into);
            let resume = self.resumes[a].as_mut().expect("an entry comes on a link");
            resume.brought.insert(player);
        }
        // 4. What is left there are guest's subscriptions.
        let left = std::mem::take(&mut self.kept[b]);
        if !left.is_empty() {
            self.count("a merge brings guest's subscriptions");
        }
        for (chunk, _) in left {
            self.kept[a]
                .entry(chunk)
                .or_insert_with(|| Kept::new(Role::Guest, 0));
        }
        // 5. Whatever named it.
        for kept in self.kept.iter_mut().flat_map(|kept| kept.values_mut()) {
            if kept.answer == Answer::Told(gone) {
                kept.answer = Answer::Told(into);
            }
        }
        for by in self.served_by.values_mut() {
            if *by == gone {
                *by = into;
            }
        }
        // 6. What was kept for it above what it had applied.
        let kept = self.unsent[b] || self.numbered[b] > applied;
        self.arrivals
            .retain(|(to, _, n)| *to != gone || n.is_none_or(|n| n > applied));
        for arrival in &mut self.arrivals {
            if arrival.0 == gone {
                *arrival = (into, arrival.1, None);
            }
        }
        if kept {
            self.unsent[a] = true;
        }
        self.unsent[b] = false;
        self.numbered[b] = 0;
        // 7. The entries behind it.
        for (place, was) in numbers.iter().enumerate() {
            let origin = self.came_with[b].get(was).copied().unwrap_or((gone, *was));
            self.came_with[a].insert(number + 1 + place as u64, origin);
        }
        self.came_with[b].clear();
    }

    /// ADR-0015, section 6: `SplitOff` from `from`, which names `part` as the
    /// edge reads it.
    fn split_off(
        &mut self,
        from: RegionId,
        part: RegionId,
        stays: &[(PlayerId, EntityId, Option<u64>)],
    ) {
        for (player, entity, attempt) in stays {
            // The stay of a join the edge was not told the entity of: the view
            // is the part's, and nothing else (ADR-0020, section 4.3, path 2).
            if self.joined_by(*player, *attempt) && self.people[player].region == from {
                self.count("a split off takes a player who is entering");
                let person = self.people.get_mut(player).expect("the player is there");
                person.region = part;
                continue;
            }
            let theirs = self
                .people
                .get(player)
                .is_some_and(|person| person.region == from && person.entity == Some(*entity));
            let came_back =
                |(to, who, _): &(RegionId, PlayerId, Option<u64>)| *to == from && who == player;
            if !theirs {
                self.count("a split off for a stay the edge does not have there");
            } else if self.arrivals.iter().any(came_back) {
                self.count("a split off for a stay that came back");
            } else {
                self.count("a split off moves a stay");
                self.moved(*player, part);
                // Whatever the player did and the edge still keeps goes there.
                self.made_for(part);
            }
        }
    }

    /// An entry of a region's outbox that the edge has not seen and acts on.
    /// `came_with` says whether it came to the region with a merge.
    fn entry(&mut self, region: RegionId, number: u64, entry: &Durable, came_with: bool) {
        match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            }
            | Durable::NotMine {
                what: Misdirected::Arrival { player, transfer },
                holder: to,
            } => {
                let back = came_with || *to != region;
                self.hand_over(region, *player, transfer, self.stands(*to), back);
            }
            Durable::Absorbed {
                region: gone,
                since,
                applied,
                numbers,
            } => self.absorbed_into(region, number, *gone, *since, *applied, numbers),
            Durable::SplitOff {
                region: part,
                players,
            } => self.split_off(region, self.stands(*part), players),
            _ => {}
        }
    }

    /// The entries a welcome announced have been handled: ADR-0015, section 5.
    fn entries_through(&mut self, region: RegionId) {
        let index = region.0 as usize;
        let resume = self.resumes[index].as_ref().expect("the region has a link");
        let silent = resume.owed.intersection(&self.owes[index]).next().copied();
        let no_answers = resume.presences == Some(0);
        // A run's survivor never forgets the edge, so it always says `Absorbed`.
        if let Some(gone) = silent {
            self.fail(format!(
                "{region:?} has said no Absorbed for {gone:?}, which it owed the edge"
            ));
        }
        if !self.owes[index].is_empty() {
            self.count("a link is ended when its entries are through");
            self.to_end.insert(region);
            return;
        }
        // What was kept is sent.
        self.unsent[index] = false;
        if no_answers {
            self.presence_through(region);
        }
    }

    /// A player has had a `Present` of their own on the link to `region`.
    fn answered(&mut self, region: RegionId, player: PlayerId) {
        if let Some(resume) = &mut self.resumes[region.0 as usize] {
            resume.brought.remove(&player);
        }
    }

    /// The presence answers a welcome announced have come: ADR-0015, section 2.2.
    fn presence_through(&mut self, region: RegionId) {
        let resume = self.resumes[region.0 as usize].as_mut();
        let brought = std::mem::take(&mut resume.expect("the region has a link").brought);
        for player in brought {
            let theirs = |person: &Person| person.region == region;
            if self.people.get(&player).is_some_and(theirs) {
                self.judge(region, player);
            }
        }
    }

    /// ADR-0015, section 2.
    fn welcomed(&mut self, region: RegionId, welcome: Welcome) {
        let index = region.0 as usize;
        let (unknown, entries, presences, applied) = match welcome {
            Welcome::Resumed {
                entries,
                presences,
                applied,
            } => (None, entries, presences, applied),
            Welcome::Unknown {
                since,
                entries,
                presences,
                applied,
            } => (Some(since), entries, presences, applied),
            Welcome::Superseded => panic!("no region of a run says that"),
        };
        if let Some(since) = unknown {
            // The region has forgotten the edge, if the two had shared anything:
            // the players the edge believed to be there are removed.
            if self.seen[index] != 0 || self.applied[index] != 0 {
                let theirs: Vec<_> = self
                    .people
                    .iter()
                    .filter(|(_, person)| person.region == region)
                    .map(|(player, _)| *player)
                    .collect();
                self.count("a region has forgotten the edge");
                // What the edge kept for the region it gives up, also the arrival
                // of someone who has left since: nobody will hear of that entity.
                self.on_the_way.retain(|_, (_, to)| *to != region);
                for player in theirs {
                    self.count("a player of a region that has forgotten the edge");
                    let person = self.gone(player).expect("the player is there");
                    if let Some(entity) = person.entity {
                        self.on_the_way.remove(&entity);
                    }
                    self.dropped.push(person.client);
                }
                self.given_up(region);
            } else {
                self.count("a welcome of a region the edge had nothing from");
            }
            self.since[index] = since;
        }
        self.progress(region, applied);
        let resume = self.resumes[index].as_mut().expect("the region has a link");
        resume.entries = Some(entries);
        resume.presences = Some(presences);
        if entries == 0 {
            self.entries_through(region);
        }
    }

    /// An entry of a region's outbox, with its number: ADR-0015, section 3.
    fn outbox(&mut self, region: RegionId, number: u64, entry: &Durable) {
        let index = region.0 as usize;
        if number > self.seen[index] {
            self.seen[index] = number;
            match self.came_with[index].remove(&number) {
                Some((origin, was)) if was <= self.seen[origin.0 as usize] => {
                    self.count("an entry that came with a merge is passed over");
                }
                Some(_) => {
                    self.count("an entry that came with a merge is handled");
                    self.entry(region, number, entry, true);
                }
                None => self.entry(region, number, entry, false),
            }
        } else {
            self.count("an entry the edge had seen is passed over");
        }
        let resume = self.resumes[index].as_mut().expect("the region has a link");
        if let Some(left) = &mut resume.entries
            && *left > 0
        {
            *left -= 1;
            if *left == 0 {
                self.entries_through(region);
            }
        }
    }

    /// A presence answer: ADR-0015, section 2.1.
    fn presence(&mut self, region: RegionId, player: PlayerId, answer: &Presence) {
        let index = region.0 as usize;
        let resume = self.resumes[index].as_mut().expect("the region has a link");
        let mut last = false;
        if let Some(left) = &mut resume.presences
            && *left > 0
        {
            *left -= 1;
            last = *left == 0;
        }
        let has = self
            .people
            .get(&player)
            .map(|person| (person.region, person.entity));
        match (answer, has) {
            // The stay of the join of the player's connection, wherever a split
            // or a merge has taken it: ADR-0020, section 4.3, paths 1 and 1a.
            (
                Presence::Present {
                    entity,
                    pose,
                    attempt,
                    ..
                },
                Some((_, None)),
            ) if self.joined_by(player, *attempt) => {
                self.count("a present places a player");
                self.answered(region, player);
                let person = self.people.get_mut(&player).expect("the player is there");
                person.region = region;
                self.placed(region, player, *entity, chunk_of(pose.position));
            }
            (Presence::Present { entity, .. }, Some((theirs, Some(known))))
                if theirs == region && known == *entity =>
            {
                self.count("a present for a stay the edge has there");
                self.answered(region, player);
            }
            (Presence::Present { .. }, Some((theirs, None)))
                if theirs == region && self.people[&player].join.is_some() =>
            {
                self.answered(region, player);
                self.count("a present from before a player's join");
            }
            (Presence::Present { entity, .. }, Some((_, Some(known)))) if known == *entity => {
                self.count("a present moves a stay");
                self.answered(region, player);
                self.moved(player, region);
            }
            (Presence::Present { .. }, _) => {
                self.count("a present for a stay the edge does not have");
            }
            (Presence::Absent, Some((theirs, _))) if theirs == region => {
                self.judge(region, player);
            }
            (Presence::Absent, _) => self.count("an absent passed over"),
            // No region says it yet, and the edge does nothing by it.
            (Presence::Entering { .. }, _) => {}
        }
        if last {
            self.presence_through(region);
        }
    }

    /// Whether the edge has nothing of a region: ADR-0015, section 5.
    fn nothing_of(&self, region: RegionId) -> bool {
        let index = region.0 as usize;
        let mut subscriptions = self.kept.iter().flat_map(|kept| kept.values());
        !self.people.values().any(|person| person.region == region)
            && self.kept[index].is_empty()
            && !self.unsent[index]
            && self.numbered[index] <= self.applied[index]
            && !subscriptions.any(|kept| kept.answer == Answer::Told(region))
            && !self.served_by.values().any(|by| *by == region)
    }

    /// What the routing table says of regions that were absorbed: ADR-0015,
    /// section 5.
    fn table(&mut self, pairs: &[(RegionId, RegionId)]) {
        for (gone, into) in pairs {
            let mut survivor = *into;
            while let Some((_, further)) = pairs.iter().find(|(gone, _)| *gone == survivor) {
                survivor = *further;
            }
            let survivor = self.stands(survivor);
            if self.stands_for.contains_key(gone) {
                continue;
            }
            if self.nothing_of(*gone) {
                self.count("the table retires a region the edge has nothing of");
                self.retire(*gone, survivor);
                continue;
            }
            let index = survivor.0 as usize;
            self.owes[index].insert(*gone);
            let resume = self.resumes[index].as_ref();
            let lacks = resume.is_some_and(|resume| !resume.owed.contains(gone));
            if self.through(survivor) && lacks {
                self.count("the table ends a link to a survivor");
                self.to_end.insert(survivor);
            }
        }
    }

    /// What the edge has to make of one message of a region: ADR-0013, sections 3,
    /// 4 and 6, and ADR-0015.
    fn told(&mut self, region: RegionId, message: &WorkerToEdge) {
        let index = region.0 as usize;
        match message {
            WorkerToEdge::ChunkSnapshot {
                position,
                ask,
                chunk,
                ..
            } => {
                let Some(kept) = self.kept[index].get_mut(position) else {
                    self.count("a snapshot without a subscription");
                    return;
                };
                if *ask < kept.begun {
                    self.count("a snapshot below the number its subscription began with");
                    return;
                }
                let late = *ask < kept.ask;
                kept.answer = Answer::Served;
                self.count(if late {
                    "a snapshot taken under an earlier number"
                } else {
                    "a snapshot taken"
                });
                let state = chunk.get(3, -60, 5).expect("the block is in the chunk");
                self.replica.insert(*position, state);
                self.served_by.insert(*position, region);
                self.unsure.remove(position);
            }
            WorkerToEdge::Elsewhere {
                chunk,
                ask,
                region: holder,
            } => {
                // A name of a region that is no more means the region it went into.
                let holder = self.stands(*holder);
                let Some(kept) = self.kept[index].get_mut(chunk) else {
                    self.count("an elsewhere without a subscription");
                    return;
                };
                if kept.role != Role::Viewer || kept.ask != *ask || holder == region {
                    self.count("an elsewhere passed over");
                    return;
                }
                kept.answer = Answer::Told(holder);
                kept.at_once = false;
                kept.due = false;
                if self.served_by.get(chunk) == Some(&region) {
                    self.served_by.remove(chunk);
                }
                let linked = self.linked(holder);
                let there = &mut self.kept[holder.0 as usize];
                if let Some(named) = there.get_mut(chunk) {
                    // A region that is named and has itself said that another
                    // holds the chunk is asked again: the two may name each other.
                    if named.role == Role::Viewer && matches!(named.answer, Answer::Told(_)) {
                        let lately = named
                            .asked_again
                            .is_some_and(|at| Instant::now().duration_since(at) < ASK_AGAIN_AFTER);
                        if lately {
                            named.due = true;
                        } else {
                            named.at_once = true;
                        }
                    }
                    self.count("an elsewhere that names a region the edge is subscribed at");
                } else {
                    there.insert(*chunk, Kept::new(Role::Guest, 0));
                    self.count(if linked {
                        "an elsewhere that makes a guest's"
                    } else {
                        "an elsewhere that makes a guest's at a region without a link"
                    });
                }
            }
            WorkerToEdge::NotMine { chunk, ask } => {
                let taken = self.kept[index]
                    .get(chunk)
                    .is_some_and(|kept| kept.role == Role::Guest && kept.ask == *ask);
                if !taken {
                    self.count("a not mine passed over");
                    return;
                }
                self.count("a not mine taken");
                // The region has ended the subscription by saying so.
                self.kept[index].remove(chunk);
                self.heard[index].remove(chunk);
                if self.served_by.get(chunk) == Some(&region) {
                    self.served_by.remove(chunk);
                }
                self.those_told_ask_again(region, *chunk);
            }
            WorkerToEdge::TickDelta { events, .. } => {
                for event in events {
                    match event {
                        RegionEvent::EntityMoved { entity, pose, .. } => {
                            let moved = self
                                .people
                                .iter()
                                .find(|(_, person)| person.entity == Some(*entity))
                                .map(|(player, person)| (*player, person.region));
                            let Some((player, theirs)) = moved else {
                                continue;
                            };
                            assert_eq!(theirs, region, "a run has a player's region move them");
                            self.recentre(player, chunk_of(pose.position));
                        }
                        RegionEvent::BlockChanged {
                            position, state, ..
                        } => {
                            let chunk = position.chunk();
                            if !self.replica.contains_key(&chunk) {
                                continue;
                            }
                            let serving = self.served_by.get(&chunk) == Some(&region)
                                && self.kept[index]
                                    .get(&chunk)
                                    .is_some_and(|kept| kept.answer == Answer::Served);
                            if serving {
                                self.replica.insert(chunk, *state);
                                self.count("a block changes in a chunk that is served");
                            } else {
                                self.unsure.insert(chunk);
                                self.count("a block changes in a chunk that is not served");
                            }
                        }
                        _ => {}
                    }
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::Spawned {
                        attempt,
                        entity_id,
                        pose,
                        ..
                    },
            } => {
                // Only the connection whose join it answers enters by it.
                if self.joined_by(*player, Some(*attempt)) {
                    self.placed(region, *player, *entity_id, chunk_of(pose.position));
                }
            }
            WorkerToEdge::Progress { applied, .. } => self.progress(region, *applied),
            WorkerToEdge::Presence { player, answer } => self.presence(region, *player, answer),
            WorkerToEdge::Outbox { number, entry } => self.outbox(region, *number, entry),
            WorkerToEdge::Welcome(welcome) => self.welcomed(region, *welcome),
            _ => {}
        }
    }

    /// What the edge has to make of its link to `region` ending or being
    /// replaced: ADR-0013, section 6. And what the region is left with.
    fn unlinked(&mut self, region: RegionId) {
        let index = region.0 as usize;
        for kept in self.kept[index].values_mut() {
            kept.ask = 0;
            kept.begun = 0;
            kept.answer = Answer::Waiting;
            kept.at_once = false;
            kept.due = false;
        }
        self.heard[index].clear();
        self.resumes[index] = None;
        self.to_end.remove(&region);
        let played = &mut self.played[index];
        played.inbox.clear();
        played.tickets.clear();
        played.held.clear();
        played.asked = 0;
        // Only the entries of its outbox outlive the link, and those are in the
        // outbox until the edge has confirmed them.
        played.out.clear();
    }

    // The steps.

    async fn join(&mut self) {
        if self.people.len() >= 4 {
            return;
        }
        self.joined += 1;
        let player = player(100 + self.joined);
        self.log.push(format!("player {} joins", 100 + self.joined));
        let client = self.edge.join(player).await;
        let person = Person {
            client,
            region: WEST,
            entity: None,
            centre: HOME,
            wanted: BTreeSet::new(),
            target: HOME,
            inputs: 0,
            goal: HOME,
            looked_at: 0,
            join: Some(None),
        };
        self.people.insert(player, person);
        self.made_for(WEST);
        self.sync().await;
    }

    fn someone(&mut self, placed: bool) -> Option<PlayerId> {
        let those: Vec<_> = self
            .people
            .iter()
            .filter(|(_, person)| !placed || person.entity.is_some())
            .map(|(player, _)| *player)
            .collect();
        (!those.is_empty()).then(|| those[self.dice.below(those.len())])
    }

    async fn leave(&mut self) {
        let Some(player) = self.someone(false) else {
            return;
        };
        self.log.push(format!("{player:?} leaves"));
        let person = self.gone(player).expect("the player is there");
        self.made_for(person.region);
        self.edge.leave(&person.client).await;
        self.sync().await;
    }

    /// A player steps into a chunk next to the one their last step took them to.
    /// They wait with it while the edge has yet to hear of two steps, so that a
    /// region is asked for the chunks its players stand in, as it is when players
    /// walk at the speed they can. And they wait until their client has the chunk
    /// the edge has them in, as a client does: so a region's snapshot of the chunk
    /// someone enters the world in has them.
    async fn walk(&mut self) {
        let Some(player) = self.someone(true) else {
            return;
        };
        let person = self.people.get_mut(&player).expect("the player is there");
        let behind = (person.target.x - person.centre.x)
            .abs()
            .max((person.target.z - person.centre.z).abs());
        if behind > 1 || !person.client.chunks.contains_key(&person.centre) {
            return;
        }
        // They walk towards a place somewhere in the world, and then to another.
        if person.goal == person.target {
            let x = self.dice.below(7) as i32 - 1;
            let z = self.dice.below(5) as i32 - 2;
            person.goal = ChunkPos::new(x, z);
            if person.goal == person.target {
                return;
            }
        }
        let target = ChunkPos::new(
            person.target.x + (person.goal.x - person.target.x).signum(),
            person.target.z + (person.goal.z - person.target.z).signum(),
        );
        person.target = target;
        person.inputs += 1;
        *self.tally.entry("a step").or_default() += 1;
        self.log.push(format!(
            "{player:?} steps into {target:?}, input {}",
            person.inputs
        ));
        let region = person.region;
        self.edge.input(&person.client, step_into(target)).await;
        self.made_for(region);
        self.sync().await;
    }

    fn some_region(&mut self, linked: Option<bool>) -> Option<RegionId> {
        let those: Vec<_> = self
            .living()
            .into_iter()
            .filter(|region| linked.is_none_or(|linked| self.linked(*region) == linked))
            .collect();
        (!those.is_empty()).then(|| those[self.dice.below(those.len())])
    }

    /// The edge reads the next thing a region has made for it. A welcome is read
    /// by itself: the entries and the presence answers it announces follow as
    /// things of their own, so that what other regions say, what players do and
    /// what the table says can fall between any two of them.
    async fn deliver(&mut self, region: RegionId) {
        let index = region.0 as usize;
        if !self.linked(region) {
            return;
        }
        let Some(made) = self.played[index].out.pop_front() else {
            return;
        };
        let message = match made {
            Made::Said(message) => message,
            Made::Entry(number, entry) => WorkerToEdge::Outbox { number, entry },
            Made::Welcome {
                resumed,
                since,
                entries,
                presences,
                applied,
            } => {
                for (_, entry) in &entries {
                    self.count(match entry {
                        Durable::Departed { .. } | Durable::NotMine { .. } => {
                            "a hand-over among a welcome's entries"
                        }
                        Durable::Absorbed { .. } => "an absorbed among a welcome's entries",
                        Durable::SplitOff { .. } => "a split off among a welcome's entries",
                        _ => "another entry among a welcome's entries",
                    });
                }
                let (count, answers) = (entries.len() as u32, presences.len() as u32);
                let out = &mut self.played[index].out;
                for presence in presences.into_iter().rev() {
                    out.push_front(Made::Said(presence));
                }
                for (number, entry) in entries.into_iter().rev() {
                    out.push_front(Made::Entry(number, entry));
                }
                WorkerToEdge::Welcome(if resumed {
                    Welcome::Resumed {
                        entries: count,
                        presences: answers,
                        applied,
                    }
                } else {
                    Welcome::Unknown {
                        since,
                        entries: count,
                        presences: answers,
                        applied,
                    }
                })
            }
        };
        self.log
            .push(format!("{region:?} says {}", brief(&message)));
        self.told(region, &message);
        self.edge.tell(region, message);
        self.sync().await;
    }

    /// Whether the link of a region may end now. A step of a player that the
    /// region has taken and the edge has not heard of would be lost with the
    /// link, and the records do not say how the edge learns where the player is
    /// then.
    fn may_end(&self, region: RegionId) -> bool {
        let moved = |made: &Made| match made {
            Made::Said(WorkerToEdge::TickDelta { events, .. }) => events
                .iter()
                .any(|event| matches!(event, RegionEvent::EntityMoved { .. })),
            _ => false,
        };
        !self.played[region.0 as usize].out.iter().any(moved)
    }

    async fn lose(&mut self) {
        let Some(region) = self.some_region(Some(true)) else {
            return;
        };
        if !self.may_end(region) {
            return;
        }
        self.log.push(format!("the link to {region:?} ends"));
        self.count("a link ends");
        self.unlinked(region);
        self.edge.lose(region).await;
        self.sync().await;
    }

    /// The edge gets a link to a region, which takes its hello at once: the
    /// welcome, with the entries of its outbox and the presence answers, waits to
    /// be sent.
    async fn relink(&mut self, region: RegionId) {
        let index = region.0 as usize;
        if self.linked(region) {
            if !self.may_end(region) {
                return;
            }
            self.unlinked(region);
            self.count("a link is replaced while it stands");
        }
        self.log.push(format!("a new link to {region:?}"));
        let hello = self.edge.relink(region).await;
        if !hello.chunks.is_empty() {
            self.count("a hello with viewer's subscriptions");
        }
        if !hello.guests.is_empty() {
            self.count("a hello with guest's subscriptions");
        }

        // The hello names exactly what the link before had left.
        let players: BTreeSet<_> = self
            .people
            .iter()
            .filter(|(_, person)| person.region == region)
            .map(|(player, _)| *player)
            .collect();
        let of = |role| -> BTreeSet<ChunkPos> {
            self.kept[index]
                .iter()
                .filter(|(_, kept)| kept.role == role)
                .map(|(chunk, _)| *chunk)
                .collect()
        };
        let said: BTreeSet<_> = hello.players.iter().copied().collect();
        if said != players || said.len() != hello.players.len() {
            self.fail(format!(
                "the hello to {region:?} names the players {:?}, not {players:?}",
                hello.players
            ));
        }
        for (role, list) in [(Role::Viewer, &hello.chunks), (Role::Guest, &hello.guests)] {
            let expected = of(role);
            if set(list) != expected || list.len() != expected.len() {
                self.fail(format!(
                    "the hello to {region:?} names as {role:?}'s {:?} and lacks {:?}",
                    minus(&set(list), &expected),
                    minus(&expected, &set(list)),
                ));
            }
        }
        if (hello.since, hello.seen) != (self.since[index], self.seen[index]) {
            self.fail(format!(
                "the hello to {region:?} says since {} and seen {}, not {} and {}",
                hello.since, hello.seen, self.since[index], self.seen[index]
            ));
        }
        for chunk in &hello.guests {
            self.heard[index].insert(*chunk, (Role::Guest, 0, 0));
        }
        for chunk in &hello.chunks {
            self.heard[index].insert(*chunk, (Role::Viewer, 0, 0));
        }
        self.resumes[index] = Some(Resume {
            owed: self.owes[index].clone(),
            ..Resume::default()
        });

        let fresh = self.sinces + 1;
        let played = &mut self.played[index];
        for (chunk, (role, ..)) in &self.heard[index] {
            let ticket = Ticket {
                role: *role,
                ask: 0,
                answer: Answer::Waiting,
            };
            played.tickets.insert(*chunk, ticket);
            played.held.insert(*chunk);
        }
        // ADR-0012, section 4.5. A region that knows the edge with the `since` of
        // the hello resumes. One that does not know it, or knows it with another
        // `since` and has taken messages from it, makes a state for it anew, with
        // nobody in it. One whose state for the edge came with a merge or a split
        // has taken nothing: it says its `since`, and answers from that state.
        let resumed = played.knows && hello.since == played.since;
        if resumed {
            played.outbox.retain(|(number, _)| *number > hello.seen);
        } else if !played.knows || played.received > 0 {
            played.knows = true;
            played.since = fresh;
            played.residents.clear();
            played.outbox.clear();
            played.sent = 0;
            played.received = 0;
            self.sinces = fresh;
        }
        let entries: Vec<_> = played.outbox.iter().cloned().collect();
        let present = |player: &PlayerId, resident: &Resident| WorkerToEdge::Presence {
            player: *player,
            answer: Presence::Present {
                last_death: None,
                mode: Default::default(),
                riding: None,
                entity: resident.entity,
                pose: Pose::at(within(resident.chunk)),
                inventory: Inventory::default(),
                selected_slot: 0,
                last_input: resident.last_input,
                handled: None,
                flying: false,
                attempt: resident.attempt,
            },
        };
        // ADR-0014, section 3.7: an answer for each player the hello named, and
        // then `Present` for every other stay the region has for the edge.
        let mut presences = Vec::new();
        for player in &hello.players {
            presences.push(match played.residents.get(player) {
                Some(resident) => present(player, resident),
                None => WorkerToEdge::Presence {
                    player: *player,
                    answer: Presence::Absent,
                },
            });
        }
        for (player, resident) in &played.residents {
            if !hello.players.contains(player) {
                presences.push(present(player, resident));
            }
        }
        let welcome = Made::Welcome {
            resumed,
            since: played.since,
            entries,
            presences,
            applied: played.received,
        };
        played.out.push_back(welcome);
        self.sync().await;
    }

    /// A region gives back a chunk outside its pinned area that it has no use for:
    /// no link is subscribed to it, nobody stands in it or is on their way there,
    /// and nothing about it is still to be sent.
    fn give_back(&mut self) {
        let about = |made: &Made, chunk: ChunkPos| match made {
            Made::Said(WorkerToEdge::ChunkSnapshot { position, .. }) => *position == chunk,
            Made::Said(WorkerToEdge::TickDelta { events, .. }) => {
                events.iter().any(|event| event.chunks().contains(&chunk))
            }
            _ => false,
        };
        let free: Vec<_> = self
            .granted
            .iter()
            .filter(|(chunk, region)| {
                let played = &self.played[region.0 as usize];
                // A part keeps what it holds of the areas another region is
                // pinned to. Given back, such a chunk is the pinned region's again
                // and nobody tells it, so it goes on naming the part, which names
                // it. The edge then has the one that was named ask the store again;
                // the regions of these runs do not play a pinned region that is
                // behind in that way, and the case is tried by itself in
                // `two_regions_that_name_each_other_for_a_chunk_are_asked_again`.
                !self.returning.contains(chunk)
                    && pinned(**chunk).is_none()
                    && !played.tickets.contains_key(chunk)
                    && !played
                        .inbox
                        .iter()
                        .any(|message| names(&message.body, **chunk))
                    && !played
                        .residents
                        .values()
                        .any(|resident| resident.chunk == **chunk)
                    && !self.on_the_way.values().any(|(to, _)| to == *chunk)
                    && !played.out.iter().any(|made| about(made, **chunk))
            })
            .map(|(chunk, region)| (*chunk, *region))
            .collect();
        if free.is_empty() {
            return;
        }
        let (chunk, region) = free[self.dice.below(free.len())];
        self.log.push(format!("{region:?} gives back {chunk:?}"));
        self.count("a region gives a chunk back");
        self.returning.insert(chunk);
    }

    /// The store frees a chunk that was given back.
    fn free(&mut self, all: bool) {
        let returning: Vec<_> = self.returning.iter().copied().collect();
        if returning.is_empty() {
            return;
        }
        let chosen = returning[self.dice.below(returning.len())];
        for chunk in returning {
            if all || chunk == chosen {
                self.log.push(format!("the store frees {chunk:?}"));
                self.returning.remove(&chunk);
                self.granted.remove(&chunk);
            }
        }
    }

    /// A region comes to believe that another holds a chunk between the pinned
    /// areas which it does not hold itself, as it would have heard for another
    /// need at a time when that was so.
    fn mislead(&mut self) {
        let living = self.living();
        let region = living[self.dice.below(living.len())];
        let residents: Vec<_> = self.played[region.0 as usize]
            .residents
            .values()
            .map(|resident| resident.chunk)
            .collect();
        // Half the time about a chunk next to one of its players, who may step
        // into it and be let go to the wrong region.
        let chunk = if !residents.is_empty() && self.dice.chance(50) {
            let near = residents[self.dice.below(residents.len())];
            let dx = self.dice.below(3) as i32 - 1;
            let dz = self.dice.below(3) as i32 - 1;
            ChunkPos::new(near.x + dx, near.z + dz)
        } else {
            let x = 1 + self.dice.below(3) as i32;
            let z = self.dice.below(11) as i32 - 5;
            ChunkPos::new(x, z)
        };
        let holder = self.holder(chunk);
        if pinned(chunk).is_some() || holder == Some(region) {
            return;
        }
        let others: Vec<_> = living
            .into_iter()
            .filter(|other| *other != region && Some(*other) != holder)
            .collect();
        if others.is_empty() {
            return;
        }
        let other = others[self.dice.below(others.len())];
        self.log.push(format!(
            "{region:?} believes that {other:?} holds {chunk:?}"
        ));
        self.played[region.0 as usize].believes.insert(chunk, other);
    }

    /// The edge reads what a region has made up to an entry of its outbox that
    /// lets a player go, and then the region's link ends: the entry is read from
    /// the next welcome.
    async fn lose_before_a_hand_over(&mut self) {
        let lets_go = |made: &Made| {
            matches!(
                made,
                Made::Entry(_, Durable::Departed { .. } | Durable::NotMine { .. })
            )
        };
        let those: Vec<_> = self
            .living()
            .into_iter()
            .filter(|region| {
                self.linked(*region) && self.played[region.0 as usize].out.iter().any(lets_go)
            })
            .collect();
        if those.is_empty() {
            return;
        }
        let region = those[self.dice.below(those.len())];
        while self.linked(region)
            && !self.played[region.0 as usize]
                .out
                .front()
                .is_some_and(lets_go)
        {
            self.deliver(region).await;
        }
        if !self.linked(region) || !self.may_end(region) {
            return;
        }
        self.log.push(format!(
            "the link to {region:?} ends before a hand-over is read"
        ));
        self.count("a link ends before a hand-over is read");
        self.unlinked(region);
        self.edge.lose(region).await;
        self.sync().await;
    }

    /// A block changes in a chunk a region serves its link.
    fn change_a_block(&mut self) {
        let Some(region) = self.some_region(Some(true)) else {
            return;
        };
        let index = region.0 as usize;
        let served: Vec<_> = self.played[index]
            .tickets
            .iter()
            .filter(|(_, ticket)| ticket.answer == Answer::Served)
            .map(|(chunk, _)| *chunk)
            .collect();
        if served.is_empty() {
            return;
        }
        let chunk = served[self.dice.below(served.len())];
        let state = match self.state(chunk) {
            AIR => STONE,
            STONE => GRANITE,
            _ => AIR,
        };
        self.log
            .push(format!("{region:?} changes {chunk:?} to {state:?}"));
        self.content.insert(chunk, state);
        let played = &mut self.played[index];
        played.tick += 1;
        played.out.push_back(Made::Said(changed(chunk, state)));
    }

    /// A region that has no link forgets the edge, as after thirty seconds
    /// without one: its players of that edge are gone with what it had for the
    /// edge. Not in a run with merges and splits, in which nobody is to be
    /// disconnected.
    fn forget(&mut self) {
        if self.reshaping {
            return;
        }
        let Some(region) = self.some_region(Some(false)) else {
            return;
        };
        self.log.push(format!("{region:?} forgets the edge"));
        let played = &mut self.played[region.0 as usize];
        played.knows = false;
        played.residents.clear();
        played.received = 0;
        played.sent = 0;
        for (_, entry) in std::mem::take(&mut played.outbox) {
            let (Durable::Departed { transfer, .. }
            | Durable::NotMine {
                what: Misdirected::Arrival { transfer, .. },
                ..
            }) = entry
            else {
                continue;
            };
            self.on_the_way.remove(&transfer.entity_id);
        }
    }

    /// A region stops, with its link if it has one: for a merge or a split,
    /// which close every link of a region (ADR-0014, section 3.3).
    async fn stop(&mut self, region: RegionId) {
        let linked = self.linked(region);
        self.unlinked(region);
        if linked {
            self.edge.lose(region).await;
        }
    }

    /// A region absorbs another: ADR-0014, section 2.3. The players of the
    /// absorbed region are the survivor's, of two stays of one player the later;
    /// the survivor's outbox gets `Absorbed` and behind it what the absorbed
    /// region had in its own; its chunks and its areas are the survivor's; and
    /// the survivor forgets what it believed of other regions.
    async fn merge(&mut self) {
        let living = self.living();
        let absorbable: Vec<_> = living
            .iter()
            .copied()
            .filter(|gone| *gone != WEST)
            .collect();
        if absorbable.is_empty() {
            return;
        }
        let gone = absorbable[self.dice.below(absorbable.len())];
        let others: Vec<_> = living.into_iter().filter(|into| *into != gone).collect();
        let into = others[self.dice.below(others.len())];
        if !self.may_end(gone) || !self.may_end(into) {
            return;
        }
        self.log.push(format!("{into:?} absorbs {gone:?}"));
        self.count("a merge");
        self.stop(gone).await;
        self.stop(into).await;

        let absorbed = std::mem::take(&mut self.played[gone.0 as usize]);
        let fresh = self.sinces + 1;
        let survivor = &mut self.played[into.0 as usize];
        survivor.believes.clear();
        if absorbed.knows && !survivor.knows {
            // The survivor comes by a state for the edge through the merge.
            survivor.knows = true;
            survivor.since = fresh;
            survivor.received = 0;
            survivor.sent = 0;
            survivor.outbox.clear();
            self.sinces = fresh;
        }
        if survivor.knows {
            let numbers = absorbed.outbox.iter().map(|(number, _)| *number);
            let entry = Durable::Absorbed {
                region: gone,
                since: if absorbed.knows { absorbed.since } else { 0 },
                applied: if absorbed.knows { absorbed.received } else { 0 },
                numbers: numbers.collect(),
            };
            let behind = absorbed.outbox.into_iter().map(|(_, entry)| entry);
            for entry in std::iter::once(entry).chain(behind) {
                survivor.sent += 1;
                survivor.outbox.push_back((survivor.sent, entry));
            }
        }
        for (player, resident) in absorbed.residents {
            let later = |there: &Resident| there.entity >= resident.entity;
            if !survivor.residents.get(&player).is_some_and(later) {
                survivor.residents.insert(player, resident);
            }
        }
        for holder in self.granted.values_mut() {
            if *holder == gone {
                *holder = into;
            }
        }
        for (_, to) in self.on_the_way.values_mut() {
            if *to == gone {
                *to = into;
            }
        }
        self.absorbed.push((gone, into));
        self.sync().await;
    }

    /// A region is split: ADR-0014, section 2.4. The players standing in one chunk
    /// that the region holds go, with the chunks around it that are nearer to it
    /// than to anyone who stays, and are a new region's; the region's outbox gets
    /// `SplitOff`; and both begin as a restored region does, knowing nothing of
    /// who holds what.
    async fn split(&mut self) {
        // A region's id is never used again, and the last is the witness's.
        let part = RegionId((REGIONS.len() + self.parts) as u32);
        if part.0 as usize >= PORTS - 1 {
            return;
        }
        let mut seeds = Vec::new();
        for region in self.living() {
            if !self.may_end(region) {
                continue;
            }
            for resident in self.played[region.0 as usize].residents.values() {
                let chunk = resident.chunk;
                let held = self.holder(chunk) == Some(region) && !self.returning.contains(&chunk);
                if chunk != HOME && held {
                    seeds.push((region, chunk));
                }
            }
        }
        if seeds.is_empty() {
            return;
        }
        let (region, seed) = seeds[self.dice.below(seeds.len())];
        let index = region.0 as usize;
        let residents = &self.played[index].residents;
        let go: Vec<_> = residents
            .iter()
            .filter(|(_, resident)| resident.chunk == seed)
            .map(|(player, _)| *player)
            .collect();
        let mut stay: Vec<_> = residents
            .values()
            .filter(|resident| resident.chunk != seed)
            .map(|resident| resident.chunk)
            .collect();
        if self.holder(HOME) == Some(region) {
            stay.push(HOME);
        }
        // A region that would be left with nothing is not split. The three the
        // world began with are pinned to their areas, and always keep those.
        if stay.is_empty() && index >= REGIONS.len() {
            return;
        }
        let apart =
            |one: ChunkPos, other: ChunkPos| (one.x - other.x).abs().max((one.z - other.z).abs());
        let mut chunks = Vec::new();
        for x in seed.x - 2..=seed.x + 2 {
            for z in seed.z - 2..=seed.z + 2 {
                let chunk = ChunkPos::new(x, z);
                let held = self.holder(chunk) == Some(region) && !self.returning.contains(&chunk);
                let nearer = |other: &ChunkPos| apart(chunk, seed) < apart(chunk, *other);
                if held && stay.iter().all(nearer) {
                    chunks.push(chunk);
                }
            }
        }
        self.log.push(format!(
            "{region:?} is split: {part:?} has {go:?} and {} chunks around {seed:?}",
            chunks.len()
        ));
        self.count("a split");
        self.stop(region).await;

        self.parts += 1;
        self.sinces += 1;
        let played = &mut self.played[index];
        played.believes.clear();
        let mut residents = BTreeMap::new();
        let mut stays = Vec::new();
        for player in go {
            let resident = played.residents.remove(&player).expect("they stand there");
            stays.push((player, resident.entity, resident.attempt));
            residents.insert(player, resident);
        }
        played.sent += 1;
        let entry = Durable::SplitOff {
            region: part,
            players: stays,
        };
        played.outbox.push_back((played.sent, entry));
        let tick = played.tick;
        self.played[part.0 as usize] = Played {
            since: self.sinces,
            residents,
            tick,
            alive: true,
            knows: true,
            ..Played::default()
        };
        for chunk in chunks {
            self.granted.insert(chunk, part);
        }
        self.sync().await;
    }

    /// The edge's process hands the edge what the routing table says of regions
    /// that were absorbed: all of it, every time.
    async fn tell_the_table(&mut self) {
        if self.absorbed.is_empty() {
            return;
        }
        let pairs = self.absorbed.clone();
        self.log.push(format!("the table says {pairs:?}"));
        self.count("the table is told");
        self.table(&pairs);
        self.edge.pairs(pairs).await;
        self.sync().await;
    }

    /// For a moment everything goes fast: the regions that have a link look at
    /// everything and answer everything, and the edge reads it all, a few times
    /// over and with no time passing. A region that is asked again and names the
    /// same region once more has its answer read within the second.
    async fn hurry(&mut self) {
        self.log.push("everything hurries".to_owned());
        for _ in 0..3 {
            for region in self.living() {
                if !self.linked(region) {
                    continue;
                }
                self.tick(region, true);
                while self.linked(region) && !self.played[region.0 as usize].out.is_empty() {
                    self.deliver(region).await;
                }
            }
        }
    }

    async fn pass_time(&mut self) {
        self.log.push("time passes".to_owned());
        tokio::time::advance(Duration::from_millis(1100)).await;
        self.sync().await;
    }

    async fn step(&mut self) {
        // Now and then a region absorbs another or is split, or the edge is told
        // what the routing table says of that.
        if self.reshaping && self.dice.chance(4) {
            match self.dice.below(10) {
                0..=3 => self.merge().await,
                4..=7 => self.split().await,
                _ => self.tell_the_table().await,
            }
            self.check().await;
            return;
        }
        match self.dice.below(100) {
            0..=3 => self.join().await,
            4..=5 => self.leave().await,
            6..=33 => self.walk().await,
            34..=55 => {
                // A region that has something to look at or to answer, if any.
                let busy: Vec<_> = self
                    .living()
                    .into_iter()
                    .filter(|region| {
                        let played = &self.played[region.0 as usize];
                        let waits = |ticket: &Ticket| ticket.answer == Answer::Waiting;
                        let reads = played.held.is_empty() && !played.inbox.is_empty();
                        self.linked(*region) && (reads || played.tickets.values().any(waits))
                    })
                    .collect();
                if !busy.is_empty() {
                    let region = busy[self.dice.below(busy.len())];
                    self.tick(region, false);
                }
            }
            56..=85 => {
                let busy: Vec<_> = self
                    .living()
                    .into_iter()
                    .filter(|region| {
                        self.linked(*region) && !self.played[region.0 as usize].out.is_empty()
                    })
                    .collect();
                if !busy.is_empty() {
                    let region = busy[self.dice.below(busy.len())];
                    for _ in 0..=self.dice.below(8) {
                        self.deliver(region).await;
                    }
                }
            }
            86 => {
                if self.dice.chance(50) {
                    self.lose().await;
                } else {
                    self.lose_before_a_hand_over().await;
                }
            }
            87..=89 => {
                if let Some(region) = self.some_region(None) {
                    self.relink(region).await;
                }
            }
            90..=93 => self.give_back(),
            94..=95 => {
                if self.dice.chance(15) {
                    self.free(false);
                } else {
                    self.mislead();
                }
            }
            96 => self.change_a_block(),
            97 => self.pass_time().await,
            98 => self.hurry().await,
            // Seldom: it takes every player of the region with it.
            _ if self.dice.chance(30) => self.forget(),
            _ => self.walk().await,
        }
        self.check().await;
    }

    // Looking at what the edge did.

    /// Waits until the edge has handled everything said to it, reads everything
    /// it has sent the regions, and holds what that makes of the regions'
    /// subscriptions against what the edge has to hold. A link the edge has ended
    /// by itself has to be one that the table's word gave it reason to end.
    async fn sync(&mut self) {
        self.edge.drained().await;
        for region in self.edge.linked() {
            self.edge.handled(region).await;
        }
        let ended: Vec<_> = self.edge.gone.iter().copied().collect();
        for region in ended {
            if !self.to_end.contains(&region) {
                self.fail(format!(
                    "the edge ended its link to {region:?}, which the records give it no \
                         reason for"
                ));
            }
            self.log
                .push(format!("the edge ends its link to {region:?}"));
            self.count("the edge ends a link over the table's word");
            self.edge.ended_by_the_edge(region).await;
            self.edge.heard[region.0 as usize] = Heard::default();
            self.unlinked(region);
        }
        if let Some(region) = self.to_end.iter().find(|region| self.linked(**region)) {
            self.fail(format!(
                "the edge has not ended its link to {region:?}, which owes it an Absorbed"
            ));
        }
        // What the edge said to a region before the one whose word made it say so
        // was waited for is on the link by now.
        for region in self.edge.linked() {
            for message in self.edge.waiting(region) {
                self.read(region, message);
            }
        }
        self.compare();
        self.check().await;
    }

    /// Reads a message the edge sent a region.
    fn read(&mut self, region: RegionId, message: EdgeMessage) {
        let index = region.0 as usize;
        if let Some(number) = message.number {
            // What the edge is seen to send says what it keeps: the run cannot
            // see that otherwise.
            self.numbered[index] = self.numbered[index].max(number);
            match &message.body {
                EdgeToWorker::PlayerJoin(join) => {
                    if let Some(person) = self.people.get_mut(&join.player)
                        && let Some(known) = &mut person.join
                    {
                        *known = Some(known.unwrap_or(0).max(number));
                    }
                }
                EdgeToWorker::PlayerArrive { player, .. } => {
                    let arrivals = self.arrivals.iter_mut();
                    let mut theirs: Vec<_> = arrivals
                        .filter(|(to, who, _)| *to == region && who == player)
                        .collect();
                    // One that is sent again has its number already.
                    if !theirs.iter().any(|arrival| arrival.2 == Some(number))
                        && let Some(arrival) = theirs.iter_mut().find(|arrival| arrival.2.is_none())
                    {
                        arrival.2 = Some(number);
                    }
                }
                _ => {}
            }
        }
        let (ask, chunks, role) = match &message.body {
            EdgeToWorker::Subscribe { ask, chunks } => (*ask, chunks, Some(Role::Viewer)),
            EdgeToWorker::SubscribeAsGuest { ask, chunks } => (*ask, chunks, Some(Role::Guest)),
            EdgeToWorker::Unsubscribe { ask, chunks } => (*ask, chunks, None),
            EdgeToWorker::Confirm { number } => {
                // The region drops what the edge has handled, if it comes to read
                // that: in a run with merges a confirmation is lost now and then,
                // as with a link that ends. The entry is then in the outbox the
                // region's survivor takes over, and the edge has seen it.
                if !self.reshaping || self.dice.chance(70) {
                    let outbox = &mut self.played[index].outbox;
                    outbox.retain(|(entry, _)| entry > number);
                }
                return;
            }
            _ => {
                self.log.push(format!("  to {region:?}: {message:?}"));
                self.played[index].inbox.push_back(message);
                return;
            }
        };
        let listed: Vec<_> = chunks.iter().map(|chunk| (chunk.x, chunk.z)).collect();
        self.log
            .push(format!("  to {region:?}: {role:?} {ask} {listed:?}"));
        for chunk in chunks {
            let before = self.heard[index].get(chunk).copied();
            match (role, before) {
                (Some(role), None) => {
                    self.heard[index].insert(*chunk, (role, ask, ask));
                }
                // A viewer's subscription that is named again is asked again.
                (Some(Role::Viewer), Some((Role::Viewer, ..))) => {
                    self.heard[index].insert(*chunk, (Role::Viewer, ask, ask));
                    self.again.insert((index, *chunk));
                }
                (Some(Role::Guest), Some((Role::Guest, ..))) => self.fail(format!(
                    "{region:?} is asked as a guest for {chunk:?}, which it is asked for as \
                         a guest already"
                )),
                // A change of kind: the number it began with stays.
                (Some(role), Some((_, _, begun))) => {
                    self.heard[index].insert(*chunk, (role, ask, begun));
                }
                (None, Some(_)) => {
                    self.heard[index].remove(chunk);
                }
                (None, None) => self.fail(format!(
                    "{region:?} is sent Unsubscribe for {chunk:?}, which it is not asked for"
                )),
            }
        }
        self.played[index].inbox.push_back(message);
    }

    /// Holds the subscriptions the edge's messages make at each region it has a
    /// link to against the ones it has to hold, and takes their numbers.
    fn compare(&mut self) {
        let now = Instant::now();
        for region in ports() {
            if !self.linked(region) {
                continue;
            }
            let index = region.0 as usize;
            let expected: BTreeMap<_, _> = self.kept[index]
                .iter()
                .map(|(chunk, kept)| (*chunk, kept.role))
                .collect();
            let found: BTreeMap<_, _> = self.heard[index]
                .iter()
                .map(|(chunk, heard)| (*chunk, heard.0))
                .collect();
            if expected != found {
                let lacking: Vec<_> = expected
                    .iter()
                    .filter(|(chunk, role)| found.get(chunk) != Some(role))
                    .collect();
                let beyond: Vec<_> = found
                    .iter()
                    .filter(|(chunk, role)| expected.get(chunk) != Some(role))
                    .collect();
                self.fail(format!(
                    "{region:?} is not asked for {lacking:?}, which the records have it \
                         asked for, and is asked for {beyond:?}"
                ));
            }
            for (chunk, kept) in &mut self.kept[index] {
                let (_, ask, begun) = self.heard[index][chunk];
                kept.ask = ask;
                kept.begun = begun;
            }
        }
        for (index, chunk) in std::mem::take(&mut self.again) {
            let region = RegionId(index as u32);
            let Some(kept) = self.kept[index].get_mut(&chunk) else {
                continue;
            };
            if !kept.at_once && !kept.due {
                self.fail(format!(
                    "{region:?} is asked again for {chunk:?} though nothing called for it"
                ));
            }
            let was_due = kept.due;
            kept.answer = Answer::Waiting;
            kept.at_once = false;
            kept.due = false;
            kept.asked_again = Some(now);
            if was_due {
                self.count("an asking that was due is made");
            }
        }
        for region in ports() {
            let late = self.kept[region.0 as usize]
                .iter()
                .find(|(_, kept)| kept.at_once);
            if let Some((chunk, _)) = late {
                self.fail(format!(
                    "{region:?} is not asked again for {chunk:?} when the region it named \
                         has said NotMine"
                ));
            }
        }
    }

    /// What has to hold after every step.
    async fn check(&mut self) {
        // Statement V: a viewer's subscription exactly where a player of the
        // region sees the chunk.
        for region in ports() {
            let mut seen = BTreeSet::new();
            for person in self.people.values() {
                if person.region == region {
                    seen.extend(person.wanted.iter().copied());
                }
            }
            let viewers: BTreeSet<_> = self.kept[region.0 as usize]
                .iter()
                .filter(|(_, kept)| kept.role == Role::Viewer)
                .map(|(chunk, _)| *chunk)
                .collect();
            if viewers != seen {
                self.fail(format!(
                    "{region:?} has a viewer's subscription for {:?} without a viewer, and \
                         none for {:?}",
                    minus(&viewers, &seen),
                    minus(&seen, &viewers)
                ));
            }
        }
        // Statement G, and that everything is ended by the time nobody sees a
        // chunk. Statement E.
        // Statement S: a region that stands for another has no player, no
        // subscription and is nobody's word for who holds a chunk.
        for gone in self.stands_for.keys() {
            let named = |kept: &Kept| kept.answer == Answer::Told(*gone);
            let mut subscriptions = self.kept.iter().flat_map(|kept| kept.values());
            if self.people.values().any(|person| person.region == *gone)
                || !self.kept[gone.0 as usize].is_empty()
                || subscriptions.any(named)
                || self.served_by.values().any(|by| by == gone)
                || self.stands_for.contains_key(&self.stands_for[gone])
            {
                self.fail(format!(
                    "the records leave the edge with something of {gone:?}, which is no more"
                ));
            }
        }
        for region in ports() {
            for (chunk, kept) in &self.kept[region.0 as usize] {
                if !self.seen(*chunk) {
                    self.fail(format!(
                        "{region:?} is asked for {chunk:?}, which nobody sees"
                    ));
                }
                if let Answer::Told(holder) = kept.answer {
                    let there = self.kept[holder.0 as usize].contains_key(chunk);
                    if !there && !kept.due && !kept.at_once {
                        self.fail(format!(
                            "{region:?} named {holder:?} for {chunk:?}, which is not asked \
                                 for it"
                        ));
                    }
                }
            }
        }

        // What the clients were sent.
        let players: Vec<_> = self.people.keys().copied().collect();
        for player in players {
            let person = self.people.get_mut(&player).expect("the player is there");
            if person.entity.is_some() {
                self.edge.sync(&mut person.client).await;
            }
            if !person.client.connected() {
                let reason = person.client.disconnected.clone();
                self.fail(format!("{player:?} was disconnected: {reason:?}"));
            }
            let person = self.people.get_mut(&player).expect("the player is there");
            let sent = person.client.sent[person.looked_at..].to_vec();
            person.looked_at = person.client.sent.len();
            let person = &self.people[&player];
            for chunk in sent {
                if !self.replica.contains_key(&chunk) {
                    self.fail(format!(
                        "{player:?} was sent {chunk:?}, of which no snapshot was taken"
                    ));
                }
            }
            for chunk in &person.wanted {
                let Some(state) = self.replica.get(chunk) else {
                    continue;
                };
                if !person.client.chunks.contains_key(chunk) {
                    self.fail(format!(
                        "{player:?} sees {chunk:?}, of which a snapshot was taken, and was \
                             not sent it"
                    ));
                }
                let shown = person.client.state(*chunk);
                if !self.unsure.contains(chunk) && shown != Some(*state) {
                    self.fail(format!(
                        "{player:?} is shown {chunk:?} with {shown:?}, and the last \
                             snapshot and what followed it have {state:?}"
                    ));
                }
            }
        }
        for client in &mut self.dropped {
            if client.connected() {
                let who = client.player;
                panic!("{who:?} is still connected though their region forgot the edge");
            }
        }
        self.dropped.clear();
    }

    /// Lets everything come to rest: the edge is told what the table says, every
    /// region gets a link, looks at everything and answers everything, and the
    /// edge reads everything, until nothing is left to do. Then everybody has to
    /// be where the edge believes them to be, and has to be shown everything they
    /// see as it is.
    async fn rest(&mut self) {
        self.log.push("everything comes to rest".to_owned());
        for _ in 0..80 {
            let behind = |(gone, _): &(RegionId, RegionId)| !self.stands_for.contains_key(gone);
            if self.absorbed.iter().any(behind) {
                self.tell_the_table().await;
            }
            for region in self.living() {
                if !self.linked(region) {
                    self.relink(region).await;
                }
            }
            self.free(true);
            for region in self.living() {
                self.tick(region, true);
                while self.linked(region) && !self.played[region.0 as usize].out.is_empty() {
                    self.deliver(region).await;
                }
            }
            self.pass_time().await;
            self.check().await;
            let busy = self.played.iter().any(|played| {
                !played.inbox.is_empty()
                    || !played.out.is_empty()
                    || played
                        .tickets
                        .values()
                        .any(|ticket| ticket.answer == Answer::Waiting)
            });
            let due = self
                .kept
                .iter()
                .flat_map(|subscriptions| subscriptions.values())
                .any(|kept| kept.due || kept.at_once || kept.answer == Answer::Waiting);
            let unlinked = self.living().into_iter().any(|region| !self.linked(region));
            let behind = |(gone, _): &(RegionId, RegionId)| !self.stands_for.contains_key(gone);
            if !busy && !due && !unlinked && !self.absorbed.iter().any(behind) {
                self.rested();
                return;
            }
        }
        self.fail("the run does not come to rest".to_owned());
    }

    fn rested(&mut self) {
        if !self.on_the_way.is_empty() {
            self.fail(format!(
                "the entities {:?} were let go and neither arrived nor were discarded",
                self.on_the_way
            ));
        }
        // The edge has caught up with every merge.
        for (gone, _) in &self.absorbed {
            if self.stands_for.get(gone) != Some(&self.now(*gone)) {
                self.fail(format!(
                    "{gone:?} is {:?} by now, and the records have it stand for {:?}",
                    self.now(*gone),
                    self.stands_for.get(gone)
                ));
            }
        }
        // No stay is left in a region that the edge does not have there: every
        // region has answered a hello of the edge since.
        for region in self.living() {
            for (player, resident) in &self.played[region.0 as usize].residents {
                let believed = self.people.get(player).is_some_and(|person| {
                    person.region == region && person.entity == Some(resident.entity)
                });
                if !believed {
                    self.fail(format!(
                        "{region:?} has {player:?} as {resident:?}, and the edge does not"
                    ));
                }
            }
        }
        for (player, person) in &self.people {
            let resident = self.played[person.region.0 as usize].residents.get(player);
            let there = resident.is_some_and(|resident| {
                resident.chunk == person.centre && resident.chunk == person.target
            });
            if !there {
                self.fail(format!(
                    "{player:?} is {resident:?} in {:?}, and the edge has them in {:?}, \
                         where their last step was into {:?}",
                    person.region, person.centre, person.target
                ));
            }
            // They are shown the others who stand where they see, and none of the
            // others who do not. Someone who has left can still be shown; see
            // `a_discarded_entity_another_region_showed_last_is_taken_off_the_screens`.
            for (other, there) in &self.people {
                let Some(entity) = there.entity else {
                    continue;
                };
                let seen = other != player && person.wanted.contains(&there.centre);
                if person.client.entities.contains(&entity.0) != seen {
                    self.fail(format!(
                        "{player:?} is shown the entities {:?}; {other:?} is {entity:?} and \
                             stands in {:?}, which they see: {seen}",
                        person.client.entities, there.centre
                    ));
                }
            }
            for chunk in &person.wanted {
                let holder = self.holder(*chunk);
                let served = holder.is_some_and(|holder| {
                    self.kept[holder.0 as usize]
                        .get(chunk)
                        .is_some_and(|kept| kept.answer == Answer::Served)
                });
                let shown = person.client.state(*chunk);
                if !served || self.unsure.contains(chunk) || shown != Some(self.state(*chunk)) {
                    self.fail(format!(
                        "{player:?} sees {chunk:?}, which {holder:?} holds with {:?}: \
                             served {served}, shown {shown:?}",
                        self.state(*chunk)
                    ));
                }
            }
        }
    }

    async fn play(mut self, steps: usize) -> BTreeMap<&'static str, u64> {
        for step in 1..=steps {
            self.step().await;
            if step % 150 == 0 {
                self.rest().await;
            }
        }
        self.rest().await;
        // Statement S: the edge takes no link to a region that is no more.
        for (gone, _) in self.absorbed.clone() {
            self.edge.offers_in_vain(gone).await;
        }
        self.sync().await;
        std::mem::take(&mut self.tally)
    }
}

/// The seeds of the generated runs a test makes: five, or those asked for with the
/// environment variable named, as `<first seed>,<how many>`.
fn runs_asked_for(variable: &str) -> (Option<(u64, u64)>, std::ops::Range<u64>) {
    let asked = std::env::var(variable).ok().and_then(|asked| {
        let (first, runs) = asked.split_once(',')?;
        Some((first.parse::<u64>().ok()?, runs.parse::<u64>().ok()?))
    });
    let (first, runs) = asked.unwrap_or((0, 5));
    (asked, first..first + runs)
}

/// Scenario 20. The clock of this test stands still until a run moves it.
///
/// Other runs than the five it makes can be asked for with
/// `CLUSTINE_EDGE_RUNS=<first seed>,<how many>`; four hundred take some minutes.
/// A run that fails says its seed and its last steps, and all of its steps with
/// `CLUSTINE_WHOLE_RUN` set.
#[tokio::test(start_paused = true)]
async fn the_edge_holds_what_the_records_say_after_every_step_of_generated_runs() {
    let (asked, seeds) = runs_asked_for("CLUSTINE_EDGE_RUNS");
    let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
    for seed in seeds {
        for (what, times) in Run::new(seed, false).await.play(500).await {
            *tally.entry(what).or_default() += times;
        }
    }
    // Shown when the test fails, or to whoever asks for the test's output.
    println!("{tally:#?}");
    if asked.is_some() {
        return;
    }
    // The runs are worth something only if they come to the cases they are for.
    for what in [
        "a hand-over",
        "a hello with guest's subscriptions",
        "a link is replaced while it stands",
        "a not mine taken",
        "a served viewer's becomes a guest's",
        "a snapshot below the number its subscription began with",
        "a snapshot taken under an earlier number",
        "a viewer's that waits becomes a guest's",
        "an asking again at once",
        "an elsewhere passed over",
        "an elsewhere that makes a guest's",
        "one told elsewhere ends, others see it",
    ] {
        assert!(tally.contains_key(what), "no run came to {what}");
    }
}

/// Scenario 32 of ADR-0015: the generated runs of scenario 20 with regions that
/// merge and split while links are lost, played by a model of ADR-0014, sections
/// 2.1, 2.3, 2.4, 3.3 and 3.7. A merge and a split close the region's links; a
/// welcome has the entries of the outbox, `Absorbed` with the absorbed region's
/// entries behind it and `SplitOff` among them, says how far the region had
/// applied, and is followed by an answer for every stay the region has; an input
/// and a leave are for the stay they name. The edge is told what the routing
/// table says of absorbed regions at moments of the run's choosing.
///
/// Beside the edge runs what ADR-0015 says it holds, which is checked as in
/// scenario 20, with statement S. At rest everyone is in the region whose state
/// has their stay, no region has a stay the edge does not have there, and the
/// edge takes no link to a region that is no more. Nobody is disconnected: no
/// region of these runs forgets the edge, so nobody has done anything for it.
///
/// Other runs than the five it makes can be asked for with
/// `CLUSTINE_EDGE_RESHAPES=<first seed>,<how many>`.
#[tokio::test(start_paused = true)]
async fn the_edge_holds_what_the_records_say_while_regions_merge_and_split() {
    let (asked, seeds) = runs_asked_for("CLUSTINE_EDGE_RESHAPES");
    let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
    for seed in seeds {
        for (what, times) in Run::new(seed, true).await.play(600).await {
            *tally.entry(what).or_default() += times;
        }
    }
    println!("{tally:#?}");
    if asked.is_some() {
        return;
    }
    for what in [
        "a merge",
        "a split",
        "a merge brings a player",
        "a present moves a stay",
        "a split off moves a stay",
        "an absorbed is handled",
        "the table is told",
    ] {
        assert!(tally.contains_key(what), "no run came to {what}");
    }
}

// The edge through merges and splits: ADR-0015, section 9, and ADR-0014, section 10.
//
// These tests were written from those two records, ADR-0013 and section 5 of
// ADR-0012, without the edge's code. A merge and a split reach the edge only among
// the entries of a welcome, so the regions of these tests answer hellos: a `Reply`
// says what a region has for the edge, and the welcome, its entries and the
// presence answers follow from it and from the hello as ADR-0014, section 3.7, has
// it. Where two regions answer at the same time, a scenario is a script per link,
// and it is run with the scripts' messages in every order that keeps each script's
// own (`Orders`): the edge reads its links in no order between them.
//
// `settle` cannot be used between a welcome's entries, as it makes an entry of its
// own, which the welcome did not announce and which would stand where an entry
// that came with a merge is expected. So these tests have a witness: a player who
// is kept aside in a region without a link and takes no part. A region says to the
// witness that an action was handled, which the edge passes on whatever region
// says it, and the test waits for that: the edge has then read everything the
// region said before.

/// The player who is the witness of `Harness::witnessed`.
const WITNESS: u128 = 999;
/// Where a player stands whom these tests hand to the southern region.
const SOUTHERN: ChunkPos = ChunkPos::new(0, 4);
/// Two chunks that are seen from `HOME` and neither from `NORTHERN` nor from
/// `SOUTHERN`.
const LENT: ChunkPos = ChunkPos::new(-1, 0);
const SOUGHT: ChunkPos = ChunkPos::new(1, 0);

/// A region's word that the player is in it as `entity`, with their inputs up to
/// `last_input` applied.
fn present_with(player: PlayerId, entity: EntityId, last_input: u64) -> WorkerToEdge {
    WorkerToEdge::Presence {
        player,
        answer: Presence::Present {
            last_death: None,
            mode: Default::default(),
            riding: None,
            entity,
            pose: Pose::at(SPAWN),
            inventory: Inventory::default(),
            selected_slot: 0,
            last_input,
            handled: None,
            flying: false,
            attempt: None,
        },
    }
}

fn absent(player: PlayerId) -> WorkerToEdge {
    WorkerToEdge::Presence {
        player,
        answer: Presence::Absent,
    }
}

fn progress(applied: u64, inputs: Vec<(PlayerId, u64)>) -> WorkerToEdge {
    WorkerToEdge::Progress { applied, inputs }
}

/// The outbox entry of a region that has absorbed `region`.
fn absorbed(region: RegionId, since: u64, applied: u64, numbers: &[u64]) -> Durable {
    Durable::Absorbed {
        region,
        since,
        applied,
        numbers: numbers.to_vec(),
    }
}

/// The outbox entry of a region of which `region` was split off with `stays`.
fn split_off(region: RegionId, stays: &[(u128, u64)]) -> Durable {
    let stay = |(who, entity): &(u128, u64)| (player(*who), EntityId(*entity), None);
    Durable::SplitOff {
        region,
        players: stays.iter().map(stay).collect(),
    }
}

/// The outbox entry that says an action of the stay `entity` of `player` was
/// dealt with.
fn done(player: PlayerId, entity: EntityId, sequence: i32) -> Durable {
    Durable::RemoteDone {
        player,
        entity,
        sequence,
        shown: Vec::new(),
    }
}

/// An input as the edge passes it on.
fn passed_on(who: u128, entity: u64, number: u64, input: PlayerInput) -> EdgeToWorker {
    EdgeToWorker::Input {
        player: player(who),
        entity: EntityId(entity),
        number,
        input,
    }
}

/// The word that a player's stay has ended.
fn left(who: u128, entity: Option<u64>) -> EdgeToWorker {
    EdgeToWorker::PlayerLeave {
        player: player(who),
        entity: entity.map(EntityId),
        attempt: None,
    }
}

/// The chunks that subscription messages among `said` ask for as `role` before the
/// first numbered message.
fn asked_before_numbered(said: &[EdgeMessage], role: Role) -> BTreeSet<ChunkPos> {
    let mut asked = BTreeSet::new();
    for message in said {
        if message.number.is_some() {
            break;
        }
        if is_about_subscriptions(&message.body) {
            let (kind, chunks) = meaning(&message.body);
            if kind == Some(role) {
                asked.extend(chunks);
            }
        }
    }
    asked
}

/// The inputs of `who` among `said`: each with the number of its message, the
/// entity it names and its own number.
fn inputs_of(said: &[EdgeMessage], who: u128) -> Vec<(u64, EntityId, u64)> {
    let mut inputs = Vec::new();
    for message in said {
        if let EdgeToWorker::Input {
            player: actor,
            entity,
            number,
            ..
        } = &message.body
            && *actor == player(who)
        {
            let numbered = message.number.expect("an input is numbered");
            inputs.push((numbered, *entity, *number));
        }
    }
    inputs
}

/// The arrival of `who` among `said`, if there is one: its number and the player
/// as they arrive.
fn arrival_of(said: &[EdgeMessage], who: u128) -> Option<(u64, PlayerTransfer)> {
    said.iter().find_map(|message| match &message.body {
        EdgeToWorker::PlayerArrive {
            player: arriving,
            transfer,
        } if *arriving == player(who) => Some((message.number?, transfer.clone())),
        _ => None,
    })
}

/// What a region has for the edge when it answers a hello, from which the
/// welcome, its entries and the presence answers follow.
#[derive(Debug, Clone, Default)]
struct Reply {
    /// The region's word for its numbering, if it does not share the one the
    /// hello named: it then says `Unknown`, and its outbox is numbered from 1.
    unknown: Option<u64>,
    /// The number of the edge's last message the region had applied.
    applied: u64,
    /// The entries of its outbox the edge has not confirmed, in order.
    entries: Vec<Durable>,
    /// The stays it has for the edge, each with the number of the last input
    /// applied.
    stays: Vec<(PlayerId, EntityId, u64)>,
    /// The attempt of the join that the stay of a player still carries: no input
    /// of it has been applied.
    attempts: BTreeMap<PlayerId, u64>,
}

impl Reply {
    fn resumed() -> Self {
        Self::default()
    }

    fn unknown(since: u64) -> Self {
        Self {
            unknown: Some(since),
            ..Self::default()
        }
    }

    fn applied(mut self, applied: u64) -> Self {
        self.applied = applied;
        self
    }

    fn entry(mut self, entry: Durable) -> Self {
        self.entries.push(entry);
        self
    }

    fn stay(mut self, who: u128, entity: u64, last_input: u64) -> Self {
        self.stays.push((player(who), EntityId(entity), last_input));
        self
    }

    /// The stay of `who` is that of the join with `attempt`, and the region has
    /// applied no input of it.
    fn of_join(mut self, who: u128, attempt: u64) -> Self {
        self.attempts.insert(player(who), attempt);
        self
    }

    /// The presence answers, by ADR-0014, section 3.7: one for each player the
    /// hello named, in its order, and then `Present` for every other stay, in
    /// ascending order of the players.
    fn presences(&self, named: &[PlayerId]) -> Vec<WorkerToEdge> {
        let stay = |who: &PlayerId| self.stays.iter().find(|(player, ..)| player == who);
        let mut answers = Vec::new();
        for who in named {
            answers.push(match stay(who) {
                Some((_, entity, last_input)) => present_with(*who, *entity, *last_input),
                None => absent(*who),
            });
        }
        let mut others: Vec<_> = self.stays.iter().collect();
        others.retain(|(who, ..)| !named.contains(who));
        others.sort();
        for (who, entity, last_input) in others {
            answers.push(present_with(*who, *entity, *last_input));
        }
        for answer in &mut answers {
            if let WorkerToEdge::Presence {
                player,
                answer: Presence::Present { attempt, .. },
            } = answer
            {
                *attempt = self.attempts.get(player).copied();
            }
        }
        answers
    }
}

/// Something that happens to the edge in a scenario. A script is a sequence of
/// these for one link, or for the routing table.
#[derive(Debug, Clone)]
enum Step {
    /// The edge is handed a new link to the region and says hello on it.
    Link(RegionId),
    /// The region answers the hello with its welcome. The entries and the
    /// presence answers follow as steps of their own, so that what another script
    /// says can fall between any two of them.
    Welcome(RegionId, Reply),
    /// The region says the next entry of its outbox.
    Entry(RegionId, Durable),
    /// The region says something else, which the edge has read when the step is
    /// over.
    Says(RegionId, WorkerToEdge),
    /// The edge's process tells it which regions the routing table says were
    /// absorbed.
    Pairs(Vec<(RegionId, RegionId)>),
}

/// How many orders of a scenario's steps are run one after the other before the
/// rest is left to chance, and how many are then drawn.
const EVERY_ORDER_UP_TO: usize = 600;
const ORDERS_DRAWN: usize = 400;

/// The orders in which the steps of a scenario's scripts are taken: every order
/// that keeps each script's own, one per run, or, where there are too many, the
/// first few hundred and as many again drawn by a generator of numbers. A scenario
/// is run anew for each, as an edge cannot be put back to where it was. Scripts
/// can grow while they run (a welcome is followed by as many presence answers as
/// its hello named players), so the orders are found by walking: each run goes as
/// the one before up to the last choice that has another way left, and takes that.
struct Orders {
    /// The choices of the run that is under way, or was made last: which of how
    /// many scripts went next.
    path: Vec<(usize, usize)>,
    at: usize,
    runs: usize,
    begun: bool,
    /// Set once the orders are drawn.
    dice: Option<Dice>,
    /// The steps of the run that is under way, for whoever reads a failure.
    account: Vec<String>,
}

impl Orders {
    fn new() -> Self {
        Self {
            path: Vec::new(),
            at: 0,
            runs: 0,
            begun: false,
            dice: None,
            account: Vec::new(),
        }
    }

    /// Whether there is another order to run, which is then the one `choose`
    /// gives.
    fn another(&mut self) -> bool {
        self.account.clear();
        self.at = 0;
        if !self.begun {
            self.begun = true;
            return true;
        }
        self.runs += 1;
        if self.dice.is_some() {
            let more = self.runs < EVERY_ORDER_UP_TO + ORDERS_DRAWN;
            if !more {
                println!("{} orders were run, the later ones drawn", self.runs);
            }
            return more;
        }
        while let Some((chosen, of)) = self.path.pop() {
            if chosen + 1 < of {
                self.path.push((chosen + 1, of));
                if self.runs >= EVERY_ORDER_UP_TO {
                    self.path.clear();
                    self.dice = Some(Dice(self.runs as u64));
                }
                return true;
            }
        }
        // Shown to whoever asks for the test's output.
        println!("every order was run: {}", self.runs);
        false
    }

    /// Which of `of` scripts goes next.
    fn choose(&mut self, of: usize) -> usize {
        if of == 1 {
            return 0;
        }
        if let Some(dice) = &mut self.dice {
            return dice.below(of);
        }
        if self.at == self.path.len() {
            self.path.push((0, of));
        }
        let (chosen, was) = self.path[self.at];
        assert_eq!(was, of, "a scenario went another way under the same order");
        self.at += 1;
        chosen
    }
}

impl Drop for Orders {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "run {} of the scenario, with its steps in this order:\n{}",
                self.runs + 1,
                self.account.join("\n")
            );
        }
    }
}

impl Harness {
    /// An edge as `start` gives it, and a witness: a player whom the west has
    /// placed and let go to a region the edge is never given a link to, far from
    /// everything, where they see nothing that anyone else sees. The west has
    /// applied their join, so nothing is kept for it.
    async fn witnessed() -> Self {
        let mut edge = Self::start().await;
        let who = player(WITNESS);
        let mut witness = edge.joined(who, EntityId(900)).await;
        let far = ChunkPos::new(100, 100);
        edge.say(WEST, departed(who, EntityId(900), ASIDE, far));
        edge.caught_up(WEST);
        edge.quiet().await;
        edge.sync(&mut witness).await;
        for region in REGIONS {
            let left = &edge.at(region).subscriptions;
            assert!(left.is_empty(), "{region:?} is still asked for {left:?}");
        }
        edge.witness = Some(witness);
        edge
    }

    /// The region says that it has applied every numbered message it was sent on
    /// its link.
    fn caught_up(&mut self, region: RegionId) {
        let applied = self.at(region).numbered;
        self.tell(region, progress(applied, Vec::new()));
    }

    /// Waits until the edge has read everything `region` has said so far, or has
    /// closed its link to the region, which is then noted in `gone`. It makes no
    /// entry, so it can be used between the entries of a welcome. What the edge
    /// said to the region meanwhile is kept for the test to look at.
    async fn handled(&mut self, region: RegionId) {
        let index = region.0 as usize;
        let sequence = self.sequence();
        let Some(link) = self.links[index].as_ref() else {
            return;
        };
        let asked = WorkerToEdge::ToPlayer {
            player: player(WITNESS),
            event: PlayerEvent::Acknowledged { sequence },
        };
        if link.try_send(asked).is_err() {
            self.links[index] = None;
            self.gone.insert(region);
            return;
        }
        loop {
            let witness = self.witness.as_mut().expect("the test has a witness");
            if witness.acknowledged.contains(&sequence) {
                return;
            }
            let link = self.links[index].as_mut().expect("the region has a link");
            tokio::select! {
                biased;
                ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                packet = timeout(SOON, witness.packets.recv()) => {
                    let packet = packet
                        .unwrap_or_else(|_| panic!("the edge does not read what {region:?} says"))
                        .expect("the edge ended the witness's connection");
                    witness.take(packet);
                }
                message = link.recv() => match message {
                    Some(message) => {
                        self.heard[index].note(region, &message);
                        self.heard[index].said.push_back(message);
                    }
                    None => {
                        self.links[index] = None;
                        self.gone.insert(region);
                        return;
                    }
                },
            }
        }
    }

    /// Says something as `region`, if its link is there and the edge has not
    /// closed it, and waits until the edge has read it. A region whose link the
    /// edge has closed says it into nothing, as a real one would.
    async fn says(&mut self, region: RegionId, message: WorkerToEdge) {
        let index = region.0 as usize;
        // A region that says `NotMine` to a guest has ended that subscription.
        if let WorkerToEdge::NotMine { chunk, ask } = &message {
            let subscriptions = &mut self.heard[index].subscriptions;
            if subscriptions.get(chunk) == Some(&(Role::Guest, *ask)) {
                subscriptions.remove(chunk);
            }
        }
        let Some(link) = self.links[index].as_ref() else {
            return;
        };
        if link.try_send(message).is_ok() {
            self.handled(region).await;
        }
    }

    /// Everything the edge has said to `region` that no test has looked at, once
    /// it has read what the region has said so far. Unlike `said` it makes no
    /// entry.
    async fn sent(&mut self, region: RegionId) -> Vec<EdgeMessage> {
        self.handled(region).await;
        self.waiting(region)
    }

    /// Tells the edge which regions the routing table says were absorbed, and
    /// waits until it has taken that.
    async fn pairs(&mut self, pairs: Vec<(RegionId, RegionId)>) {
        assert!(self.relinks.absorbed(pairs).await, "the edge is gone");
        let sender = &self.relinks.sender;
        let taken = async {
            while sender.capacity() < sender.max_capacity() {
                tokio::task::yield_now().await;
            }
        };
        timeout(SOON, taken)
            .await
            .expect("the edge takes what the routing table says");
    }

    /// Waits until the edge has closed its link to `region` by itself and has
    /// said so to whoever gives it links. What it said on the link before is
    /// kept for the test to look at.
    async fn ended_by_the_edge(&mut self, region: RegionId) {
        let index = region.0 as usize;
        if !self.gone.remove(&region) {
            loop {
                let link = self.links[index].as_mut().expect("the region has a link");
                let message = tokio::select! {
                    biased;
                    ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                    message = timeout(SOON, link.recv()) => message,
                };
                let message = message
                    .unwrap_or_else(|_| panic!("the edge did not end its link to {region:?}"));
                let Some(message) = message else {
                    break;
                };
                self.heard[index].note(region, &message);
                self.heard[index].said.push_back(message);
            }
            self.links[index] = None;
        }
        let word = (region, self.epochs[index]);
        if let Some(at) = self.ended.iter().position(|ended| *ended == word) {
            self.ended.remove(at);
            return;
        }
        loop {
            let ended = timeout(SOON, self.relinks.ended())
                .await
                .unwrap_or_else(|_| {
                    panic!("the edge did not say that its link to {region:?} ended")
                })
                .expect("the edge is there");
            if ended == word {
                return;
            }
            self.ended.push(ended);
        }
    }

    /// Hands the edge a link to a region that is no more, and makes sure that the
    /// edge drops it without a hello.
    async fn offers_in_vain(&mut self, region: RegionId) {
        let index = region.0 as usize;
        self.epochs[index] += 1;
        let (link, mut worker) = link_to(region, self.epochs[index]);
        assert!(self.relinks.replace(link).await);
        let said = tokio::select! {
            biased;
            ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
            said = timeout(SOON, worker.recv()) => said,
        };
        let said = said
            .unwrap_or_else(|_| panic!("the edge keeps a link to {region:?}, which is no more"));
        assert!(
            said.is_none(),
            "the edge said {said:?} to {region:?}, which is no more"
        );
    }

    /// The edge is handed a new link to `region`; its hello is kept for `hello`.
    async fn link(&mut self, region: RegionId) {
        let hello = self.relink(region).await;
        self.hellos[region.0 as usize] = Some(hello);
    }

    /// The hello the edge said to `region` on the last link `link` made.
    fn hello(&self, region: RegionId) -> &Hello {
        let hello = self.hellos[region.0 as usize].as_ref();
        hello.expect("the edge was given a link to the region")
    }

    /// Takes one step of a script, and returns the steps that follow from it and
    /// come before the rest of its script.
    async fn take(&mut self, step: Step) -> Vec<Step> {
        match step {
            Step::Link(region) => {
                self.link(region).await;
                Vec::new()
            }
            Step::Welcome(region, reply) => {
                let index = region.0 as usize;
                let named = match &self.hellos[index] {
                    Some(hello) => hello.players.clone(),
                    None => Vec::new(),
                };
                let presences = reply.presences(&named);
                let (entries, answers) = (reply.entries.len() as u32, presences.len() as u32);
                let welcome = match reply.unknown {
                    Some(since) => {
                        // Its outbox begins anew for an edge it does not share a
                        // numbering with.
                        self.outbox[index] = 0;
                        Welcome::Unknown {
                            since,
                            entries,
                            presences: answers,
                            applied: reply.applied,
                        }
                    }
                    None => Welcome::Resumed {
                        entries,
                        presences: answers,
                        applied: reply.applied,
                    },
                };
                self.says(region, WorkerToEdge::Welcome(welcome)).await;
                let entries = reply.entries.into_iter();
                let mut follow: Vec<_> = entries.map(|entry| Step::Entry(region, entry)).collect();
                let answers = presences.into_iter();
                follow.extend(answers.map(|answer| Step::Says(region, answer)));
                follow
            }
            Step::Entry(region, entry) => {
                let number = &mut self.outbox[region.0 as usize];
                *number += 1;
                let number = *number;
                self.says(region, WorkerToEdge::Outbox { number, entry })
                    .await;
                Vec::new()
            }
            Step::Says(region, message) => {
                self.says(region, message).await;
                Vec::new()
            }
            Step::Pairs(pairs) => {
                self.pairs(pairs).await;
                Vec::new()
            }
        }
    }

    /// Runs the scripts with their steps in the order `orders` gives.
    async fn play(&mut self, scripts: Vec<Vec<Step>>, orders: &mut Orders) {
        let mut scripts: Vec<VecDeque<Step>> = scripts.into_iter().map(VecDeque::from).collect();
        loop {
            let ready: Vec<_> = (0..scripts.len())
                .filter(|script| !scripts[*script].is_empty())
                .collect();
            if ready.is_empty() {
                return;
            }
            let script = ready[orders.choose(ready.len())];
            let step = scripts[script].pop_front().expect("the script has a step");
            let mut told = format!("{step:?}");
            if told.len() > 400 {
                told.truncate(400);
            }
            orders.account.push(told);
            for follows in self.take(step).await.into_iter().rev() {
                scripts[script].push_front(follows);
            }
        }
    }

    /// The region answers the hello of its link: the welcome, its entries and
    /// the presence answers, one after the other.
    async fn answer(&mut self, region: RegionId, reply: Reply) {
        let script = vec![Step::Welcome(region, reply)];
        self.play(vec![script], &mut Orders::new()).await;
    }

    /// A player who has entered the world as `entity` and whom the west, unless
    /// it is to be their region, has let go to `region`, into `chunk`. The west
    /// has applied their join, and everything else is as the hand-over left it.
    async fn settler(
        &mut self,
        who: u128,
        entity: u64,
        region: RegionId,
        chunk: ChunkPos,
    ) -> Client {
        let client = self.joined(player(who), EntityId(entity)).await;
        self.caught_up(WEST);
        if region != WEST {
            self.say(WEST, departed(player(who), EntityId(entity), region, chunk));
        }
        self.quiet().await;
        client
    }

    /// The player makes a step. Returns the region the edge passes it on to, the
    /// entity it names and its number among the player's inputs. That region has
    /// to have a link.
    async fn acts(&mut self, client: &Client) -> (RegionId, EntityId, u64) {
        self.input(client, step_into(HOME)).await;
        self.drained().await;
        for region in self.linked() {
            self.handled(region).await;
        }
        // Without the links the edge has closed, which the waiting showed.
        let mut found = Vec::new();
        for region in self.linked() {
            for message in self.waiting(region) {
                if let EdgeToWorker::Input {
                    player,
                    entity,
                    number,
                    ..
                } = message.body
                    && player == client.player
                {
                    found.push((number, region, entity));
                }
            }
        }
        // What they did before can have been sent again; this is their last.
        found.sort();
        let (number, region, entity) = found
            .pop()
            .unwrap_or_else(|| panic!("no region was sent what {:?} did", client.player));
        (region, entity, number)
    }

    /// An action of `who` about `COMMON` that the west passes on to `to`, where it
    /// is under way when this returns.
    async fn under_way(&mut self, who: u128, to: RegionId) -> RemoteAction {
        let action = breaking(
            player(who),
            self.entity_of(player(who)),
            self.sequence(),
            COMMON,
        );
        self.say(WEST, remote(&action, Some(to)));
        self.settle(WEST).await;
        action
    }

    /// The player makes `steps` steps, which the edge has taken when this
    /// returns.
    async fn walks(&mut self, client: &Client, chunk: ChunkPos, steps: usize) {
        for _ in 0..steps {
            self.input(client, step_into(chunk)).await;
        }
        self.drained().await;
    }
}

/// The subscription messages among `said`.
fn subscriptions(said: Vec<EdgeMessage>) -> Vec<EdgeToWorker> {
    let bodies = said.into_iter().map(|message| message.body);
    bodies.filter(is_about_subscriptions).collect()
}

/// The highest number among the entries the edge has confirmed in `said`.
fn confirmed(said: &[EdgeMessage]) -> Option<u64> {
    let confirmations = said.iter().filter_map(|message| match message.body {
        EdgeToWorker::Confirm { number } => Some(number),
        _ => None,
    });
    confirmations.max()
}

/// Scenario 1. A region says whom it has, also those the hello did not name. A stay
/// the edge does not have is the region's to end, with the entity the region said.
#[tokio::test]
async fn a_stay_the_edge_does_not_have_is_ended_at_the_region_that_says_it_has_it() {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let reply = Reply::resumed().applied(applied);
    edge.answer(WEST, reply.stay(1, 5, 0).stay(2, 9, 0)).await;
    let said = edge.sent(WEST).await;
    assert_eq!(numbered(&said), [(applied + 1, left(2, Some(9)))]);
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "{asked:?}");
    for region in [EAST, NORTH] {
        let said = edge.sent(region).await;
        assert!(said.is_empty(), "{region:?} was sent {said:?}");
    }
    assert!(first.connected());
    assert_eq!(edge.acts(&first).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 1, and the rest of case 4 of section 2.1: the edge has the player, as
/// another entity under another region, or under another region without an entity
/// yet. The region's stay is ended, and the edge's own is untouched.
#[tokio::test]
async fn a_stay_of_a_player_the_edge_has_otherwise_is_ended_and_the_edges_own_is_untouched() {
    let mut edge = Harness::witnessed().await;
    let mut second = edge.settler(2, 6, EAST, EASTERN).await;
    // The third has joined, and the west has not placed them yet.
    let mut third = edge.join(player(3)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.quiet().await;

    edge.link(NORTH).await;
    assert!(edge.hello(NORTH).players.is_empty());
    let reply = Reply::resumed().stay(2, 9, 0).stay(3, 4, 0);
    edge.answer(NORTH, reply).await;
    let said = edge.sent(NORTH).await;
    let expected = [(1, left(2, Some(9))), (2, left(3, Some(4)))];
    assert_eq!(numbered(&said), expected);
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "{asked:?}");

    assert!(second.connected());
    assert_eq!(edge.acts(&second).await, (EAST, EntityId(6), 1));
    edge.says(WEST, edge.spawned(player(3), EntityId(7))).await;
    assert!(third.connected());
    assert_eq!(edge.acts(&third).await, (WEST, EntityId(7), 1));
    edge.end().await;
}

/// Scenario 2. A region that has a player as another entity than the edge has
/// them under that very region has a stay the edge does not have: that stay is
/// ended, and the player is not disconnected for it.
#[tokio::test]
async fn a_player_a_region_has_as_another_entity_is_not_disconnected() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let reply = Reply::resumed().applied(applied).stay(1, 77, 0);
    edge.answer(WEST, reply).await;
    let said = edge.sent(WEST).await;
    assert_eq!(numbered(&said), [(applied + 1, left(1, Some(77)))]);
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 2. A player the hello named and the region does not have is
/// disconnected, with a leave that names their entity.
#[tokio::test]
async fn a_player_a_region_says_it_does_not_have_is_disconnected() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    edge.link(WEST).await;
    edge.answer(WEST, Reply::resumed().applied(applied)).await;
    client.disconnected().await;
    let said = edge.sent(WEST).await;
    assert_eq!(numbered(&said), [(applied + 1, left(1, Some(5)))]);
    edge.end().await;
}

/// Scenarios 2 and 6. A player who is entering the world is not disconnected by
/// `Absent` while their join is kept: the region has yet to apply it. Whether it
/// is kept, the welcome says before anything else is done: a join at or below its
/// `applied` is dropped, and `Absent` then finds none.
#[tokio::test]
async fn a_player_who_is_absent_stays_only_while_their_join_is_kept() {
    for applied_the_join in [false, true] {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.join(player(1)).await;
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        // The join names the connection it is of.
        let EdgeToWorker::PlayerJoin(joined) = &join else {
            unreachable!("asserted above");
        };
        assert_eq!(joined.attempt, client.session.0);
        edge.lose(WEST).await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let applied = if applied_the_join { number } else { number - 1 };
        edge.answer(WEST, Reply::resumed().applied(applied)).await;
        if applied_the_join {
            // They quit, as far as any region can tell: no entity was told. So the
            // leave names the attempt of the join instead (ADR-0020, section 4.4).
            client.disconnected().await;
            let said = edge.sent(WEST).await;
            let leave = EdgeToWorker::PlayerLeave {
                player: player(1),
                entity: None,
                attempt: Some(joined.attempt),
            };
            assert_eq!(numbered(&said), [(number + 1, leave)]);
        } else {
            let sent = numbered(&edge.sent(WEST).await);
            assert!(
                matches!(&sent[..], [(again, EdgeToWorker::PlayerJoin(_))] if *again == number),
                "{sent:?}"
            );
            assert!(client.connected());
            edge.says(WEST, edge.spawned(player(1), EntityId(5))).await;
            assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
        }
        edge.end().await;
    }
}

/// Scenario 2. A player who is on their way to a region is not disconnected by its
/// `Absent`: their arrival is among what is kept for it.
#[tokio::test]
async fn a_player_who_is_absent_stays_while_their_arrival_is_kept() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, EAST, EASTERN).await;
    edge.link(EAST).await;
    assert_eq!(edge.hello(EAST).players, [player(1)]);
    edge.answer(EAST, Reply::resumed()).await;
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, EASTERN),
    };
    assert_eq!(numbered(&edge.sent(EAST).await), [(1, arrival)]);
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// What scenario 3 begins with: the west's player has made three steps, of which
/// the west has said nothing, and the west has named the east for `COMMON`. Then
/// the east answers a hello that named nobody with a `Present` for that stay,
/// with the first of the three steps applied. Returns what the east was sent.
async fn a_stay_another_region_has() -> (Harness, Client, Vec<EdgeMessage>) {
    let mut edge = Harness::witnessed().await;
    let client = edge.settler(1, 5, WEST, HOME).await;
    edge.tell(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), EAST));
    edge.walks(&client, HOME, 3).await;
    edge.quiet().await;
    assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[COMMON]));

    edge.link(EAST).await;
    let hello = edge.hello(EAST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(hello.guests, [COMMON]);
    edge.answer(EAST, Reply::resumed().stay(1, 5, 1)).await;
    let said = edge.sent(EAST).await;
    (edge, client, said)
}

/// Scenario 3, as far as it is about what the stay's new region is sent that is
/// numbered: the inputs above the answer's `last_input`, and no others.
#[tokio::test]
async fn a_stay_another_region_says_it_has_is_sent_only_the_inputs_above_the_answers_last() {
    // The sequence: a player of the west makes three steps, kept as inputs 1 to
    // 3; the east says `Present { last_input: 1 }` for that stay in answer to a
    // hello that named nobody.
    //
    // The records: ADR-0015, section 2.1, case 3 ("the kept inputs up to
    // `last_input` are dropped; ... then every input still kept is sent to
    // `R`"), its scenario 3 ("the inputs above `last_input` go to `R`"), and
    // ADR-0014, rule 38 ("every input of it the edge keeps above the answer's
    // `last_input` is sent here").
    //
    // What happened: the east is sent the inputs 1, 2 and 3; with `last_input`
    // 3 it is sent all three as well. The region passes over what is not above
    // the stay's last (ADR-0014, section 2.1), so nothing is applied twice.
    let (edge, _client, said) = a_stay_another_region_has().await;
    let step = step_into(HOME);
    let expected = [
        (1, passed_on(1, 5, 2, step.clone())),
        (2, passed_on(1, 5, 3, step)),
    ];
    assert_eq!(numbered(&said), expected);
    edge.end().await;
}

/// Scenario 3. A region says `Present` for a stay the edge has under another: the
/// stay is that region's, by a merge or a split the edge has not caught up with.
/// Nobody arrives; the view is asked of the region before anything the player did
/// is sent there; and the region they were under keeps, as a guest's, what it was
/// asked for and had not said another holds.
#[tokio::test]
async fn a_stay_another_region_says_it_has_is_that_regions_without_an_arrival() {
    let (mut edge, mut client, said) = a_stay_another_region_has().await;
    assert!(arrival_of(&said, 1).is_none(), "{said:?}");
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(HOME));
    // Everything numbered is of that stay, in order, and has what the east had
    // not applied.
    let sent = numbered(&said);
    let steps = inputs_of(&said, 1);
    assert_eq!(steps.len(), sent.len(), "{sent:?}");
    assert!(steps.iter().all(|(_, entity, _)| *entity == EntityId(5)));
    let last: Vec<_> = steps
        .iter()
        .rev()
        .take(2)
        .map(|(_, _, step)| *step)
        .collect();
    assert_eq!(last, [3, 2], "{sent:?}");
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
    assert!(edge.at(EAST).chunks(Role::Guest).is_empty());

    // ADR-0013, section 2: what was told elsewhere carried nothing and is ended;
    // the rest the player still sees, so it is a guest's.
    let asked = subscriptions(edge.sent(WEST).await);
    let rest = minus(&view(HOME), &set(&[COMMON]));
    let expected = BTreeSet::from([(None, set(&[COMMON])), (Some(Role::Guest), rest.clone())]);
    assert_eq!(meanings(&asked), expected);
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    assert_eq!(edge.at(WEST).chunks(Role::Guest), rest);

    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 4));
    let said = edge.sent(EAST).await;
    assert!(said.is_empty(), "{said:?}");
    edge.end().await;
}

/// Scenario 4. The region's word that it placed a player was lost with a link.
/// The welcome of the next says that the region had applied their join, so the
/// `Present` is the stay of that join.
#[tokio::test]
async fn a_player_whose_join_the_region_had_applied_is_placed_by_its_presence_answer() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.join(player(1)).await;
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.lose(WEST).await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let reply = Reply::resumed()
        .applied(number)
        .stay(1, 5, 0)
        .of_join(1, client.session.0);
    edge.answer(WEST, reply).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 4. The welcome says that the region had not applied the join when it
/// made its answers: the `Present` is of a stay from before the join, which the
/// join will end. Nothing is done with it; the join is sent again, and the region
/// then places the player anew. What the client sends before that is dropped
/// (section 7).
#[tokio::test]
async fn a_presence_answer_from_before_a_players_join_does_not_place_them() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.join(player(1)).await;
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.lose(WEST).await;
    edge.link(WEST).await;
    let reply = Reply::resumed().applied(number - 1).stay(1, 4, 0);
    edge.answer(WEST, reply).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(
        matches!(&sent[..], [(again, EdgeToWorker::PlayerJoin(_))] if *again == number),
        "{sent:?}"
    );
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "{asked:?}");

    edge.walks(&client, HOME, 1).await;
    let said = edge.sent(WEST).await;
    assert!(said.is_empty(), "{said:?}");

    edge.says(WEST, edge.spawned(player(1), EntityId(6))).await;
    edge.sent(WEST).await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    let (region, entity, _) = edge.acts(&client).await;
    assert_eq!((region, entity), (WEST, EntityId(6)));
    assert!(client.connected());
    edge.end().await;
}

/// Scenario 5, the sequence of the review's defect 2. The west is split with the
/// player in the part. The player leaves before the edge has read that, so the
/// leave goes to the west, which no longer has the stay and counts it as applied.
/// The west then absorbs the part, which brings the stay back, and the player
/// joins again. The west's next welcome has a `Present` for the old stay and no
/// leave is kept: only the welcome's `applied`, which is below the join, tells the
/// edge that this is not the stay of the join.
#[tokio::test]
async fn a_player_who_joins_again_is_not_placed_as_the_stay_a_merge_brought_back() {
    let mut edge = Harness::witnessed().await;
    let before = edge.settler(1, 5, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    edge.link(WEST).await;
    edge.leave(&before).await;
    let split = Reply::resumed().applied(applied);
    edge.answer(WEST, split.entry(split_off(PART, &[(1, 5)])))
        .await;
    let said = edge.sent(WEST).await;
    assert_eq!(numbered(&said), [(applied + 1, left(1, Some(5)))]);

    // The merge closes the west's links. The player comes back meanwhile.
    edge.lose(WEST).await;
    let mut again = edge.join(player(1)).await;
    edge.drained().await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let merged = Reply::resumed().applied(applied + 1);
    let merged = merged.entry(absorbed(PART, 40, 0, &[])).stay(1, 5, 0);
    edge.answer(WEST, merged).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(
        matches!(&sent[..], [(join, EdgeToWorker::PlayerJoin(_))] if *join == applied + 2),
        "{sent:?}"
    );
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "placed as their old self: {asked:?}");

    edge.says(WEST, edge.spawned(player(1), EntityId(8))).await;
    assert_eq!(edge.acts(&again).await, (WEST, EntityId(8), 1));
    assert!(again.connected());
    edge.end().await;
}

/// Scenario 6. What a welcome says was applied is dropped before what was kept is
/// sent: without it, the join would be sent again.
#[tokio::test]
async fn what_a_welcome_says_was_applied_is_not_sent_again() {
    for applied_the_join in [true, false] {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        let join = edge.at(WEST).numbered;
        edge.walks(&client, HOME, 1).await;
        edge.quiet().await;
        edge.link(WEST).await;
        let applied = if applied_the_join { join } else { join - 1 };
        edge.answer(WEST, Reply::resumed().applied(applied).stay(1, 5, 0))
            .await;
        let sent = numbered(&edge.sent(WEST).await);
        let step = (join + 1, passed_on(1, 5, 1, step_into(HOME)));
        if applied_the_join {
            assert_eq!(sent, [step]);
        } else {
            assert_eq!(sent.len(), 2, "{sent:?}");
            assert!(matches!(&sent[0], (again, EdgeToWorker::PlayerJoin(_)) if *again == join));
            assert_eq!(sent[1], step);
        }
        assert!(client.connected());
        edge.end().await;
    }
}

/// What the tests of a merge begin with.
struct Merging {
    edge: Harness,
    /// The first player, who is the north's.
    north: Client,
    /// The second player, who is the west's.
    west: Client,
    /// An action of the first player that is kept for the east.
    action: RemoteAction,
}

/// The east is about to absorb the north. The first player is the north's, at
/// `NORTHERN`; the north has applied their arrival, and they have made four steps
/// since, which are the north's messages 2 to 5. The second player is the west's,
/// at `HOME`, and the west has named the north for `LENT`, so the edge is a guest
/// there. The east has one message kept, an action of the first player about
/// `SOUGHT`, which the second sees, so that it made the edge a guest there.
async fn before_a_merge() -> Merging {
    let mut edge = Harness::witnessed().await;
    let north = edge.settler(1, 5, NORTH, NORTHERN).await;
    edge.caught_up(NORTH);
    let west = edge.settler(2, 6, WEST, HOME).await;
    edge.walks(&north, NORTHERN, 4).await;
    edge.tell(WEST, elsewhere(LENT, edge.ask(WEST, LENT), NORTH));
    let action = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        SOUGHT,
    );
    edge.say(NORTH, remote(&action, Some(EAST)));
    edge.quiet().await;
    assert_eq!(edge.at(NORTH).chunks(Role::Viewer), view(NORTHERN));
    assert_eq!(edge.at(NORTH).chunks(Role::Guest), set(&[LENT]));
    assert_eq!(edge.at(NORTH).numbered, 5);
    assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[SOUGHT]));
    assert_eq!(edge.at(EAST).numbered, 1);
    Merging {
        edge,
        north,
        west,
        action,
    }
}

/// The east's welcome after it absorbed the north of `before_a_merge`, which had
/// applied the edge's messages up to 3: the arrival and two steps.
fn merged() -> Reply {
    Reply::resumed().entry(absorbed(NORTH, 1, 3, &[]))
}

/// What the east of `before_a_merge` is sent that is numbered when the entries of
/// `merged` are through: what was kept for it, and behind that the north's
/// messages 4 and 5 under the east's next numbers.
fn kept_after_the_merge(action: &RemoteAction) -> Vec<(u64, EdgeToWorker)> {
    let step = step_into(NORTHERN);
    vec![
        (1, EdgeToWorker::Remote(action.clone())),
        (2, passed_on(1, 5, 3, step.clone())),
        (3, passed_on(1, 5, 4, step)),
    ]
}

/// Looks at what the east of `before_a_merge` was sent after `merged`, with a
/// `Present` for the first player: scenario 7.
async fn as_after_the_merge(edge: &mut Harness, north: &mut Client, action: &RemoteAction) {
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
    assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[LENT]));
    assert_eq!(numbered(&said), kept_after_the_merge(action));
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
    assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[LENT, SOUGHT]));
    assert!(north.connected());
    assert_eq!(edge.acts(north).await, (EAST, EntityId(5), 5));
}

/// Scenario 7.
#[tokio::test]
async fn what_the_edge_had_at_an_absorbed_region_it_has_at_the_survivor_from_the_absorbed_on() {
    let Merging {
        mut edge,
        mut north,
        mut west,
        action,
    } = before_a_merge().await;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    // Rule 43: the hello names nothing of the absorbed region.
    let hello = edge.hello(EAST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(hello.guests, [SOUGHT]);
    edge.answer(EAST, merged().stay(1, 5, 2)).await;
    as_after_the_merge(&mut edge, &mut north, &action).await;

    // Statement S: the north has no link, and is given none.
    edge.offers_in_vain(NORTH).await;
    // What the north was to show the second player, the east shows them.
    edge.tell(EAST, snapshot(LENT, edge.ask(EAST, LENT)));
    assert!(edge.shows(&mut west, EAST, LENT).await);
    // And the subscription that was told elsewhere with the north is told
    // elsewhere with the east: its `NotMine` has the west asked again.
    edge.tell(EAST, not_mine(LENT, edge.ask(EAST, LENT)));
    edge.settle(EAST).await;
    let asked = edge.asked(WEST).await;
    assert!(
        matches!(&asked[..], [EdgeToWorker::Subscribe { chunks, .. }] if chunks == &[LENT]),
        "{asked:?}"
    );
    edge.end().await;
}

/// Scenario 8. A player the absorbed region's `Absorbed` made the survivor's, and
/// of whom the welcome's presence has no `Present`, is absent: here the welcome
/// announces no answers at all.
#[tokio::test]
async fn a_player_a_merge_brought_and_the_survivor_does_not_have_is_disconnected() {
    let Merging {
        mut edge,
        mut north,
        west: _west,
        action,
    } = before_a_merge().await;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    edge.answer(EAST, merged()).await;
    north.disconnected().await;
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
    let mut expected = kept_after_the_merge(&action);
    expected.push((4, left(1, Some(5))));
    assert_eq!(numbered(&said), expected);
    // What the second player sees of it stays, as a guest's.
    assert!(edge.at(EAST).chunks(Role::Viewer).is_empty());
    let mut still_seen = both(&view(HOME), &view(NORTHERN));
    still_seen.extend([LENT, SOUGHT]);
    assert_eq!(edge.at(EAST).chunks(Role::Guest), still_seen);
    edge.end().await;
}

/// Scenario 8, where the welcome announces an answer, and it is for someone else.
#[tokio::test]
async fn a_player_a_merge_brought_is_judged_when_the_answers_announced_have_come() {
    let Merging {
        mut edge,
        mut north,
        west: _west,
        action,
    } = before_a_merge().await;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    edge.answer(EAST, merged().stay(8, 80, 0)).await;
    north.disconnected().await;
    let mut expected = kept_after_the_merge(&action);
    expected.push((4, left(8, Some(80))));
    expected.push((5, left(1, Some(5))));
    assert_eq!(numbered(&edge.sent(EAST).await), expected);
    edge.end().await;
}

/// Scenario 8. A player a merge brought whose arrival is among what was kept for
/// the absorbed region and goes to the survivor is on their way there, and is not
/// disconnected for want of a `Present`.
#[tokio::test]
async fn a_player_a_merge_brought_stays_while_their_arrival_is_kept_for_the_survivor() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, NORTH, NORTHERN).await;
    assert_eq!(edge.at(NORTH).numbered, 1);
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    // The north had applied nothing.
    let reply = Reply::resumed().entry(absorbed(NORTH, 1, 0, &[]));
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, NORTHERN),
    };
    assert_eq!(numbered(&said), [(1, arrival)]);
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 9. The `Absorbed` says another `since` than the edge holds for the
/// absorbed region, of which the edge has seen entries: that region had forgotten
/// the edge. Nothing kept for it goes to the survivor, and an action among it is
/// told to its player as handled. Its players become the survivor's all the same
/// and are judged by the presence, which has none of them.
#[tokio::test]
async fn nothing_kept_for_an_absorbed_region_that_had_forgotten_the_edge_goes_to_the_survivor() {
    let Merging {
        mut edge,
        mut north,
        west: _west,
        action,
    } = before_a_merge().await;
    // An action of a third player is under way to the north.
    let mut third = edge.settler(3, 7, WEST, HOME).await;
    let given_up = breaking(
        player(3),
        edge.entity_of(player(3)),
        edge.sequence(),
        NORTHERN,
    );
    edge.say(WEST, remote(&given_up, Some(NORTH)));
    edge.quiet().await;
    assert_eq!(edge.at(NORTH).numbered, 6);
    assert!(edge.outbox[NORTH.0 as usize] > 0);

    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    let reply = Reply::resumed().entry(absorbed(NORTH, 9, 3, &[]));
    edge.answer(EAST, reply).await;
    north.disconnected().await;
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[LENT]));
    let expected = [
        (1, EdgeToWorker::Remote(action.clone())),
        (2, left(1, Some(5))),
    ];
    assert_eq!(numbered(&said), expected);
    edge.acknowledged(&mut third, given_up.sequence).await;
    edge.end().await;
}

/// Scenario 10. The edge never had anything from the absorbed region: no entry
/// seen, nothing reported applied. Then nothing it kept for it was applied,
/// whatever the entry says, and all of it goes to the survivor.
#[tokio::test]
async fn all_that_was_kept_for_an_absorbed_region_the_edge_never_heard_from_goes_to_the_survivor() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, SOUTH, SOUTHERN).await;
    edge.walks(&client, SOUTHERN, 2).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    let hello = edge.hello(EAST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty() && hello.guests.is_empty());
    let reply = Reply::resumed().entry(absorbed(SOUTH, 4, 2, &[]));
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(SOUTHERN));
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, SOUTHERN),
    };
    let step = step_into(SOUTHERN);
    let expected = [
        (1, arrival),
        (2, passed_on(1, 5, 1, step.clone())),
        (3, passed_on(1, 5, 2, step)),
    ];
    assert_eq!(numbered(&said), expected);
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 3));
    edge.end().await;
}

/// What scenarios 11 and 14 begin with: three actions of the west's player are
/// under way to the north, which has dealt with the first two and said so in its
/// last two entries, and has dealt with the third without the edge reading of
/// it. Then the north is gone. Returns how many entries of the north the edge has
/// seen, and the actions' sequence numbers.
async fn seen_of_the_north() -> (Harness, Client, u64, [i32; 3]) {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(2, 6, WEST, HOME).await;
    let mut sequences = [0; 3];
    for sequence in &mut sequences {
        *sequence = edge.under_way(2, NORTH).await.sequence;
    }
    edge.quiet().await;
    assert_eq!(edge.at(NORTH).numbered, 3);
    edge.say(
        NORTH,
        done(player(2), edge.entity_of(player(2)), sequences[0]),
    );
    edge.say(
        NORTH,
        done(player(2), edge.entity_of(player(2)), sequences[1]),
    );
    edge.acknowledged(&mut client, sequences[1]).await;
    let seen = edge.outbox[NORTH.0 as usize];
    edge.lose(NORTH).await;
    (edge, client, seen, sequences)
}

/// The north's last three entries, as they stand behind an `Absorbed`. The player
/// is the one `seen_of_the_north` settled, as entity 6.
fn three_entries(sequences: [i32; 3]) -> Vec<Durable> {
    sequences
        .map(|sequence| done(player(2), EntityId(6), sequence))
        .to_vec()
}

/// Each of the three actions was acknowledged to the player once: the first two
/// when the north said so itself, and the third when its entry came with the
/// merge.
async fn each_acknowledged_once(edge: &mut Harness, client: &mut Client, sequences: [i32; 3]) {
    edge.acknowledged(client, sequences[2]).await;
    client.drain();
    assert_eq!(client.acknowledged, sequences);
}

/// Scenario 11. Of the entries behind an `Absorbed`, those whose number at the
/// absorbed region the edge had seen there are confirmed and not acted on.
#[tokio::test]
async fn entries_that_came_with_a_merge_are_passed_over_where_the_edge_had_seen_them() {
    let (mut edge, mut client, seen, sequences) = seen_of_the_north().await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    let first = edge.outbox[EAST.0 as usize] + 1;
    assert_eq!(edge.hello(EAST).seen, first - 1);
    let numbers = [seen - 1, seen, seen + 1];
    let mut reply = Reply::resumed().entry(absorbed(NORTH, 1, 3, &numbers));
    reply.entries.extend(three_entries(sequences));
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    assert_eq!(confirmed(&said), Some(first + 3));
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    each_acknowledged_once(&mut edge, &mut client, sequences).await;
    edge.end().await;
}

/// Scenario 12. An entry that was the absorbed region's and lets a player go to
/// the survivor names the region it now comes from. That is no fault: the player
/// arrives there, with an arrival and what they did since.
#[tokio::test]
async fn a_departure_to_the_survivor_that_came_with_the_merge_is_an_arrival_there() {
    let (mut edge, mut north, seen) = let_go_to_the_survivor().await;
    let reply = Reply::resumed().entry(absorbed(NORTH, 1, 3, &[seen + 1]));
    edge.answer(EAST, reply.entry(gone_to_the_survivor())).await;
    arrived_at_the_survivor(&mut edge, &mut north).await;
    edge.end().await;
}

/// The north has the first player, has applied their arrival and the first two of
/// their three steps, and has said so. It then let them go to the east, which
/// absorbed it before the edge read that. Returns how many entries of the north
/// the edge has seen.
async fn let_go_to_the_survivor() -> (Harness, Client, u64) {
    let mut edge = Harness::witnessed().await;
    let north = edge.settler(1, 5, NORTH, NORTHERN).await;
    edge.walks(&north, NORTHERN, 3).await;
    edge.tell(NORTH, progress(3, vec![(player(1), 2)]));
    edge.quiet().await;
    let seen = edge.outbox[NORTH.0 as usize];
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    (edge, north, seen)
}

fn gone_to_the_survivor() -> Durable {
    Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 2, NORTHERN),
        to: EAST,
    }
}

async fn arrived_at_the_survivor(edge: &mut Harness, north: &mut Client) {
    let said = edge.sent(EAST).await;
    let (number, arrived) = arrival_of(&said, 1).expect("the player arrives at the survivor");
    assert_eq!(arrived, transfer(EntityId(5), 2, NORTHERN));
    let asked = asked_before_numbered(&said, Role::Viewer);
    assert!(asked.is_superset(&view(NORTHERN)), "{asked:?}");
    // What they did and the north had not applied follows the arrival.
    let mut after = inputs_of(&said, 1);
    after.retain(|(numbered, ..)| *numbered > number);
    let after: Vec<_> = after
        .iter()
        .map(|(_, entity, step)| (*entity, *step))
        .collect();
    assert_eq!(after, [(EntityId(5), 3)]);
    let arrivals = said
        .iter()
        .filter(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }));
    assert_eq!(arrivals.count(), 1);
    assert!(north.connected());
    assert_eq!(edge.acts(north).await, (EAST, EntityId(5), 4));
}

/// Scenario 12. An entry of the survivor's own that names the survivor is the
/// fault it was, also behind entries that came with a merge.
#[tokio::test]
async fn a_departure_of_the_survivors_own_that_names_the_survivor_disconnects_the_player() {
    let mut edge = Harness::witnessed().await;
    let mut east = edge.settler(3, 7, EAST, EASTERN).await;
    let mut north = edge.settler(1, 5, NORTH, NORTHERN).await;
    let seen = edge.outbox[NORTH.0 as usize];
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    assert_eq!(edge.hello(EAST).players, [player(3)]);
    let came_with = Durable::Departed {
        player: player(1),
        transfer: transfer(EntityId(5), 0, NORTHERN),
        to: EAST,
    };
    let reply = Reply::resumed()
        .entry(absorbed(NORTH, 1, 1, &[seen + 1]))
        .entry(came_with)
        .entry(departed(player(3), EntityId(7), EAST, EASTERN));
    edge.answer(EAST, reply).await;
    east.disconnected().await;
    assert!(north.connected());
    assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 13. The survivor let a player go to the region it then absorbed. Read
/// in order, the first entry puts the player under the absorbed region, where
/// their arrival is kept as for any region without a link, and the `Absorbed`
/// brings all of it back: the arrival reaches the survivor under its numbers.
#[tokio::test]
async fn a_player_let_go_to_a_region_the_survivor_then_absorbed_arrives_at_the_survivor() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, EAST, EASTERN).await;
    edge.caught_up(EAST);
    edge.quiet().await;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    assert_eq!(edge.hello(EAST).players, [player(1)]);
    let reply = Reply::resumed()
        .applied(1)
        .entry(departed(player(1), EntityId(5), NORTH, EASTERN))
        .entry(absorbed(NORTH, 1, 0, &[]));
    // The hello named the player, whom the east does not have: `Absent`.
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, EASTERN),
    };
    assert_eq!(numbered(&said), [(2, arrival)]);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 13, with a link that ends between the two entries: after the first
/// the player is under the absorbed region, so the next hello to the survivor
/// names neither them nor their view as a viewer's.
#[tokio::test]
async fn a_player_let_go_to_a_region_that_is_absorbed_is_under_it_until_the_absorbed_is_read() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, EAST, EASTERN).await;
    edge.caught_up(EAST);
    edge.quiet().await;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    let let_go = Reply::resumed().applied(1);
    let let_go = let_go.entry(departed(player(1), EntityId(5), NORTH, EASTERN));
    edge.answer(EAST, let_go).await;
    let sent = numbered(&edge.sent(EAST).await);
    assert!(sent.is_empty(), "{sent:?}");
    assert!(client.connected());

    edge.lose(EAST).await;
    edge.link(EAST).await;
    let hello = edge.hello(EAST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(set(&hello.guests), view(EASTERN));
    let merged = Reply::resumed().applied(1);
    edge.answer(EAST, merged.entry(absorbed(NORTH, 1, 0, &[])))
        .await;
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(EASTERN));
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(5), 0, EASTERN),
    };
    assert_eq!(numbered(&said), [(2, arrival)]);
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// Scenario 14, and A7 of ADR-0014. The link ends when the edge has read the
/// `Absorbed` and none of the entries behind it. The next welcome brings those
/// without the `Absorbed` in front, and they are still told by their numbers.
#[tokio::test]
async fn entries_that_came_with_a_merge_are_told_by_their_numbers_on_a_later_link() {
    let (mut edge, mut client, seen, sequences) = seen_of_the_north().await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    let welcome = Welcome::Resumed {
        entries: 4,
        presences: 0,
        applied: 0,
    };
    edge.says(EAST, WorkerToEdge::Welcome(welcome)).await;
    let numbers = [seen - 1, seen, seen + 1];
    let entry = absorbed(NORTH, 1, 3, &numbers);
    edge.take(Step::Entry(EAST, entry)).await;
    let first = edge.outbox[EAST.0 as usize];
    edge.lose(EAST).await;

    edge.link(EAST).await;
    assert_eq!(edge.hello(EAST).seen, first);
    let mut reply = Reply::resumed();
    reply.entries.extend(three_entries(sequences));
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    assert_eq!(confirmed(&said), Some(first + 3));
    each_acknowledged_once(&mut edge, &mut client, sequences).await;
    edge.end().await;
}

/// Scenario 15. The survivor forgets the edge while entries that came with a merge
/// are still to come. Its outbox is numbered anew, and an entry under a number
/// that one of those had is the survivor's own: it is acted on, though the edge
/// had seen, at the absorbed region, the entry that number stood for.
#[tokio::test]
async fn an_entry_of_a_survivor_that_forgot_the_edge_is_not_taken_for_one_that_came_with_a_merge() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(2, 6, WEST, HOME).await;
    let seen = edge.outbox[NORTH.0 as usize];
    assert!(seen >= 3, "{seen}");
    edge.lose(NORTH).await;
    // The south is new to the edge, so its outbox begins at 1.
    edge.link(SOUTH).await;
    let welcome = Welcome::Unknown {
        since: 1,
        entries: 4,
        presences: 0,
        applied: 0,
    };
    edge.says(SOUTH, WorkerToEdge::Welcome(welcome)).await;
    // The entries behind it would be the south's 2, 3 and 4, all of which the
    // edge had seen at the north.
    let numbers = [seen - 2, seen - 1, seen];
    let entry = absorbed(NORTH, 1, 0, &numbers);
    edge.take(Step::Entry(SOUTH, entry)).await;
    edge.lose(SOUTH).await;

    // The south forgets the edge and is told of it anew. Three actions of the
    // player are then passed on to it, and it deals with them: its entries 1, 2
    // and 3.
    edge.link(SOUTH).await;
    let hello = edge.hello(SOUTH);
    assert_eq!((hello.since, hello.seen), (1, 1));
    edge.answer(SOUTH, Reply::unknown(2)).await;
    let mut sequences = [0; 3];
    for sequence in &mut sequences {
        *sequence = edge.under_way(2, SOUTH).await.sequence;
    }
    assert_eq!(edge.outbox[SOUTH.0 as usize], 0);
    for sequence in sequences {
        edge.take(Step::Entry(
            SOUTH,
            done(player(2), edge.entity_of(player(2)), sequence),
        ))
        .await;
    }
    edge.acknowledged(&mut client, sequences[2]).await;
    assert_eq!(client.acknowledged, sequences);
    // The north is no more all the same.
    edge.offers_in_vain(NORTH).await;
    edge.end().await;
}

/// Scenario 16, and A2 of ADR-0014. The north absorbed the south and the east then
/// absorbed the north, with no link meanwhile. The east's welcome has `Absorbed`
/// for the north and, among the entries behind it, `Absorbed` for the south with
/// the south's entries behind that. Everything the edge had at either is at the
/// east afterwards, and an entry of the south's that the edge had seen at the
/// south is passed over.
#[tokio::test]
async fn two_merges_in_a_row_bring_what_the_edge_had_at_both_regions_to_the_survivor() {
    let mut edge = Harness::witnessed().await;
    edge.link(SOUTH).await;
    edge.answer(SOUTH, Reply::unknown(1)).await;
    // Two actions of a player of the west are under way to the south.
    let mut third = edge.settler(3, 7, WEST, HOME).await;
    let seen_before = edge.under_way(3, SOUTH).await.sequence;
    let new = edge.under_way(3, SOUTH).await.sequence;
    let mut first = edge.settler(1, 5, NORTH, NORTHERN).await;
    let mut second = edge.settler(2, 6, SOUTH, SOUTHERN).await;
    edge.walks(&first, NORTHERN, 1).await;
    edge.walks(&second, SOUTHERN, 1).await;
    edge.quiet().await;
    // The two actions, the arrival and the step.
    assert_eq!(edge.at(SOUTH).numbered, 4);
    assert_eq!(edge.at(NORTH).numbered, 2);
    edge.say(
        SOUTH,
        done(player(3), edge.entity_of(player(3)), seen_before),
    );
    edge.acknowledged(&mut third, seen_before).await;
    let north = edge.outbox[NORTH.0 as usize];
    let south = edge.outbox[SOUTH.0 as usize];
    for region in [SOUTH, NORTH, EAST] {
        edge.lose(region).await;
    }
    edge.link(EAST).await;
    let hello = edge.hello(EAST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty());

    // Each had applied everything but the step of its player.
    let reply = Reply::resumed()
        .entry(absorbed(NORTH, 1, 1, &[north + 1, north + 2, north + 3]))
        .entry(absorbed(SOUTH, 1, 3, &[south, south + 1]))
        .entry(done(player(3), edge.entity_of(player(3)), seen_before))
        .entry(done(player(3), edge.entity_of(player(3)), new))
        .stay(1, 5, 0)
        .stay(2, 6, 0);
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    let mut views = view(NORTHERN);
    views.extend(view(SOUTHERN));
    assert_eq!(asked_before_numbered(&said, Role::Viewer), views);
    let expected = [
        (1, passed_on(1, 5, 1, step_into(NORTHERN))),
        (2, passed_on(2, 6, 1, step_into(SOUTHERN))),
    ];
    assert_eq!(numbered(&said), expected);
    edge.acknowledged(&mut third, new).await;
    third.drain();
    assert_eq!(third.acknowledged, [seen_before, new]);
    assert!(first.connected() && second.connected());
    assert_eq!(edge.acts(&first).await, (EAST, EntityId(5), 2));
    assert_eq!(edge.acts(&second).await, (EAST, EntityId(6), 2));
    edge.offers_in_vain(NORTH).await;
    edge.offers_in_vain(SOUTH).await;
    edge.end().await;
}

/// Section 3, step 7. The edge has read the north's `Absorbed` for the south and
/// not the south's entries behind it when the east absorbs the north. Behind the
/// east's `Absorbed` for the north those entries keep the origin they had: they
/// are the south's, and are told by what the edge had seen of the south. A name
/// of the south then means the east.
#[tokio::test]
async fn an_entry_that_came_with_two_merges_is_told_by_the_number_it_had_first() {
    let mut edge = Harness::witnessed().await;
    edge.link(SOUTH).await;
    edge.answer(SOUTH, Reply::unknown(1)).await;
    let mut third = edge.settler(3, 7, WEST, HOME).await;
    let seen_before = edge.under_way(3, SOUTH).await.sequence;
    let new = edge.under_way(3, SOUTH).await.sequence;
    edge.say(
        SOUTH,
        done(player(3), edge.entity_of(player(3)), seen_before),
    );
    edge.acknowledged(&mut third, seen_before).await;
    let south = edge.outbox[SOUTH.0 as usize];
    edge.lose(SOUTH).await;
    edge.lose(NORTH).await;
    edge.link(NORTH).await;
    let welcome = Welcome::Resumed {
        entries: 3,
        presences: 0,
        applied: 0,
    };
    edge.says(NORTH, WorkerToEdge::Welcome(welcome)).await;
    // The south had applied both actions.
    let entry = absorbed(SOUTH, 1, 2, &[south, south + 1]);
    edge.take(Step::Entry(NORTH, entry)).await;
    let north = edge.outbox[NORTH.0 as usize];
    edge.lose(NORTH).await;

    edge.lose(EAST).await;
    edge.link(EAST).await;
    let reply = Reply::resumed()
        .entry(absorbed(NORTH, 1, 0, &[north + 1, north + 2]))
        .entry(done(player(3), edge.entity_of(player(3)), seen_before))
        .entry(done(player(3), edge.entity_of(player(3)), new));
    edge.answer(EAST, reply).await;
    edge.acknowledged(&mut third, new).await;
    third.drain();
    assert_eq!(third.acknowledged, [seen_before, new]);

    edge.say(WEST, departed(player(3), EntityId(7), SOUTH, SOUTHERN));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    let arrival = arrival_of(&said, 3);
    assert!(arrival.is_some(), "{said:?}");
    let (region, entity, _) = edge.acts(&third).await;
    assert_eq!((region, entity), (EAST, EntityId(7)));
    edge.end().await;
}

/// Scenario 17. The edge reads the `Absorbed` while its link to the absorbed
/// region still stands, with entries unread on it that are also behind the
/// `Absorbed`. Whichever link it reads from first, each entry is acted on once:
/// one read from the absorbed region's link is passed over behind the `Absorbed`
/// by its number, and once the `Absorbed` is read that link is no longer the
/// region's.
#[tokio::test]
async fn an_entry_on_the_absorbed_regions_link_and_behind_the_absorbed_is_acted_on_once() {
    let mut orders = Orders::new();
    while orders.another() {
        let mut edge = Harness::witnessed().await;
        let mut first = edge.settler(1, 5, NORTH, NORTHERN).await;
        let mut second = edge.settler(2, 6, WEST, HOME).await;
        let sequence = edge.under_way(2, NORTH).await.sequence;
        edge.quiet().await;
        // The north has applied the arrival and the action.
        assert_eq!(edge.at(NORTH).numbered, 2);
        edge.caught_up(NORTH);
        edge.quiet().await;
        let next = edge.outbox[NORTH.0 as usize] + 1;
        // The north's last two entries: an action of the second player was dealt
        // with, and the first player was let go to the west.
        let gone = departed(player(1), EntityId(5), WEST, HOME);
        edge.lose(EAST).await;
        let merged = Reply::resumed()
            .entry(absorbed(NORTH, 1, 2, &[next, next + 1]))
            .entry(done(player(2), edge.entity_of(player(2)), sequence))
            .entry(gone.clone());
        let scripts = vec![
            vec![Step::Link(EAST), Step::Welcome(EAST, merged)],
            vec![
                Step::Entry(NORTH, done(player(2), edge.entity_of(player(2)), sequence)),
                Step::Entry(NORTH, gone),
            ],
        ];
        edge.play(scripts, &mut orders).await;

        edge.acknowledged(&mut second, sequence).await;
        let said = edge.sent(WEST).await;
        let arrivals = said.iter().filter(|message| {
                matches!(&message.body, EdgeToWorker::PlayerArrive { player: who, .. } if *who == player(1))
            });
        assert_eq!(arrivals.count(), 1, "{said:?}");
        let said = edge.sent(EAST).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        assert!(first.connected());
        assert_eq!(edge.acts(&first).await, (WEST, EntityId(5), 1));
        second.drain();
        assert_eq!(second.acknowledged, [sequence]);
        edge.end().await;
    }
}

/// What scenario 18 begins with: the west's player sees `COMMON`, for which the
/// west has named the north twice; the north said `NotMine` in between, so the
/// west was asked again just now. Then the east absorbs the north.
async fn told_elsewhere_with_the_absorbed() -> (Harness, Client) {
    let mut edge = Harness::witnessed().await;
    let client = edge.settler(1, 5, WEST, HOME).await;
    edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), NORTH))
        .await;
    let asked = subscriptions(edge.sent(NORTH).await);
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Guest), set(&[COMMON]))])
    );
    edge.says(NORTH, not_mine(COMMON, edge.ask(NORTH, COMMON)))
        .await;
    let asked = subscriptions(edge.sent(WEST).await);
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Viewer), set(&[COMMON]))])
    );
    edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), NORTH))
        .await;
    let asked = subscriptions(edge.sent(NORTH).await);
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Guest), set(&[COMMON]))])
    );
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    assert!(edge.hello(EAST).guests.is_empty());
    (edge, client)
}

/// Scenario 18. From the `Absorbed` on, a subscription that was told elsewhere
/// with the absorbed region is told elsewhere with the survivor, where the edge
/// is a guest for the chunk from then on. Nothing is asked again for it, also
/// when it was asked again within the last second. `NotMine` from the survivor
/// has the viewer's region asked again, here when the second is over; and a later
/// `Elsewhere` that names the absorbed region names the survivor.
#[tokio::test(start_paused = true)]
async fn a_subscription_told_elsewhere_with_an_absorbed_region_is_told_so_with_the_survivor() {
    let (mut edge, mut client) = told_elsewhere_with_the_absorbed().await;
    edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
        .await;
    let asked = subscriptions(edge.sent(EAST).await);
    assert_eq!(asked, [as_guest(1, [COMMON])]);
    let asked = subscriptions(edge.sent(WEST).await);
    assert!(asked.is_empty(), "{asked:?}");

    edge.says(EAST, not_mine(COMMON, 1)).await;
    let asked = subscriptions(edge.sent(WEST).await);
    assert!(asked.is_empty(), "asked again within a second: {asked:?}");
    tokio::time::advance(Duration::from_millis(1100)).await;
    let again = edge.next_asked(WEST).await;
    assert_eq!(meaning(&again), (Some(Role::Viewer), set(&[COMMON])));

    edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), NORTH))
        .await;
    let asked = subscriptions(edge.sent(EAST).await);
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Guest), set(&[COMMON]))])
    );
    edge.says(EAST, snapshot(COMMON, edge.ask(EAST, COMMON)))
        .await;
    edge.sync(&mut client).await;
    assert!(client.chunks.contains_key(&COMMON));
    edge.end().await;
}

/// Scenario 18, when more than a second has passed since the west was asked
/// again: the survivor's `NotMine` has it asked again at once.
#[tokio::test(start_paused = true)]
async fn not_mine_from_the_survivor_has_the_region_asked_again_that_named_the_absorbed() {
    let (mut edge, _client) = told_elsewhere_with_the_absorbed().await;
    tokio::time::advance(Duration::from_millis(2500)).await;
    edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
        .await;
    let asked = subscriptions(edge.sent(EAST).await);
    assert_eq!(asked, [as_guest(1, [COMMON])]);
    let asked = subscriptions(edge.sent(WEST).await);
    assert!(asked.is_empty(), "{asked:?}");
    edge.says(EAST, not_mine(COMMON, 1)).await;
    let asked = subscriptions(edge.sent(WEST).await);
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Viewer), set(&[COMMON]))])
    );
    edge.end().await;
}

/// Scenario 19: the routing table says that the east absorbed the north while the
/// edge has a player and messages under the north and a link to the east whose
/// entries are through. The edge ends that link, so that a hello is said which
/// the east answers from after the merge. That welcome has no `Absorbed` for the
/// north: the north had forgotten the edge, or the east has since.
async fn a_survivor_that_says_no_absorbed(present: bool) {
    let Merging {
        mut edge,
        mut north,
        west: _west,
        action,
    } = before_a_merge().await;
    let mut third = edge.settler(3, 7, WEST, HOME).await;
    let given_up = breaking(
        player(3),
        edge.entity_of(player(3)),
        edge.sequence(),
        NORTHERN,
    );
    edge.say(WEST, remote(&given_up, Some(NORTH)));
    edge.quiet().await;
    edge.lose(NORTH).await;

    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(EAST).await;
    edge.link(EAST).await;
    // Rule 43: the hello names nothing of the absorbed region.
    let hello = edge.hello(EAST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(hello.guests, [SOUGHT]);
    let mut reply = Reply::resumed();
    if present {
        reply = reply.stay(1, 5, 4);
    }
    edge.answer(EAST, reply).await;
    if !present {
        north.disconnected().await;
    }
    // What was kept for the north is given up, and none of it is sent on.
    edge.acknowledged(&mut third, given_up.sequence).await;
    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
    assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[LENT]));
    let mut expected = vec![(1, EdgeToWorker::Remote(action))];
    if present {
        assert_eq!(numbered(&said), expected);
        assert!(north.connected());
        assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 5));
    } else {
        expected.push((2, left(1, Some(5))));
        assert_eq!(numbered(&said), expected);
    }

    // The table says all of its pairs every time. Nothing is owed any more.
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.offers_in_vain(NORTH).await;
    edge.end().await;
}

/// Scenario 19, with a welcome that announces no presence answers.
#[tokio::test]
async fn a_player_of_a_region_the_table_says_was_absorbed_is_judged_by_a_welcome_without_answers() {
    a_survivor_that_says_no_absorbed(false).await;
}

/// Scenario 19, with a `Present` for the player.
#[tokio::test]
async fn a_player_of_a_region_the_table_says_was_absorbed_stays_if_the_survivor_has_them() {
    a_survivor_that_says_no_absorbed(true).await;
}

/// Scenario 20. The table's word comes before the survivor's welcome, or while it
/// is being read, or after: the `Absorbed` is handled as without the table, and
/// no link is ended, as the edge had none to the survivor whose entries were
/// through and that could be from before the merge.
#[tokio::test]
async fn the_tables_word_around_a_welcome_that_has_the_absorbed_ends_no_link() {
    let mut orders = Orders::new();
    while orders.another() {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        let scripts = vec![
            vec![Step::Pairs(vec![(NORTH, EAST)])],
            vec![
                Step::Link(EAST),
                Step::Welcome(EAST, merged().stay(1, 5, 2)),
            ],
        ];
        edge.play(scripts, &mut orders).await;
        assert!(edge.gone.is_empty(), "the edge ended {:?}", edge.gone);
        as_after_the_merge(&mut edge, &mut north, &action).await;
        edge.end().await;
    }
}

/// Scenario 20, with a link to the survivor from before the merge that the edge
/// has not found ended: the table's word ends it, and the next welcome brings the
/// `Absorbed`.
#[tokio::test]
async fn the_tables_word_ends_a_link_from_before_the_merge_and_the_next_brings_the_absorbed() {
    let Merging {
        mut edge,
        mut north,
        west: _west,
        action,
    } = before_a_merge().await;
    edge.lose(NORTH).await;
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(EAST).await;
    edge.link(EAST).await;
    edge.answer(EAST, merged().stay(1, 5, 2)).await;
    as_after_the_merge(&mut edge, &mut north, &action).await;
    edge.end().await;
}

/// Scenario 21. The table's word comes while a welcome of the survivor is being
/// read that has no `Absorbed` for the north. The hello of that link was said
/// before the edge knew that such a word was owed, so nothing is concluded from
/// its silence: the link is ended when its entries are through, and nothing that
/// was kept is sent on it.
#[tokio::test]
async fn the_tables_word_during_a_welcome_without_the_absorbed_ends_the_link_after_its_entries() {
    let Merging {
        mut edge,
        mut north,
        west: _west,
        action,
    } = before_a_merge().await;
    edge.lose(NORTH).await;
    edge.link(EAST).await;
    let welcome = Welcome::Resumed {
        entries: 2,
        presences: 0,
        applied: 0,
    };
    edge.says(EAST, WorkerToEdge::Welcome(welcome)).await;
    let nobody = done(player(u128::MAX), EntityId(0), 0);
    edge.take(Step::Entry(EAST, nobody.clone())).await;
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.handled(EAST).await;
    assert!(
        edge.gone.is_empty(),
        "ended before its entries were through"
    );
    edge.take(Step::Entry(EAST, nobody)).await;
    edge.ended_by_the_edge(EAST).await;
    let said: Vec<_> = edge.heard[EAST.0 as usize].said.drain(..).collect();
    let sent = numbered(&said);
    assert!(sent.is_empty(), "sent on a link that was to end: {sent:?}");
    assert!(north.connected());

    edge.link(EAST).await;
    edge.answer(EAST, merged().stay(1, 5, 2)).await;
    as_after_the_merge(&mut edge, &mut north, &action).await;
    edge.end().await;
}

/// Scenario 22. Of the north the edge has nothing but a link. On the table's word
/// it stands for the east at once: its link is taken, nothing is sent to anyone,
/// and a player a third region lets go to the north arrives at the east.
#[tokio::test]
async fn a_region_the_edge_has_nothing_of_stands_for_its_survivor_on_the_tables_word() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(NORTH).await;
    for region in [WEST, EAST] {
        let said = edge.sent(region).await;
        assert!(said.is_empty(), "{region:?} was sent {said:?}");
    }

    edge.say(WEST, departed(player(1), EntityId(5), NORTH, NORTHERN));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    let arrival = arrival_of(&said, 1);
    assert_eq!(arrival, Some((1, transfer(EntityId(5), 0, NORTHERN))));
    // What the player saw is asked for before they arrive, as at any hand-over.
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(HOME));
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.offers_in_vain(NORTH).await;
    edge.end().await;
}

/// Section 1: every region id a region says is read through the stand-ins. The
/// north stands for the east. An action that is sent on to the north, by `Remote`
/// or by `NotMine`, goes to the east; a chunk for which the north is named is
/// asked of the east; and what the east itself sends to the north comes back to
/// the east (section 4): an action, and a player, who is not disconnected for it.
#[tokio::test]
async fn every_name_of_an_absorbed_region_means_its_survivor() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(NORTH).await;

    let first = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&first, Some(NORTH)));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(1, EdgeToWorker::Remote(first))]);
    assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[COMMON]));

    let second = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    let sent_on = Durable::NotMine {
        what: Misdirected::Remote(second.clone()),
        holder: NORTH,
    };
    edge.say(WEST, sent_on);
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(2, EdgeToWorker::Remote(second))]);

    edge.tell(WEST, elsewhere(FAR, edge.ask(WEST, FAR), NORTH));
    edge.settle(WEST).await;
    let asked = subscriptions(edge.sent(EAST).await);
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Guest), set(&[FAR]))])
    );

    // The east names the region it absorbed, as it believed before the merge.
    let third = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(EAST, remote(&third, Some(NORTH)));
    edge.settle(EAST).await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(3, EdgeToWorker::Remote(third.clone()))]);
    client.drain();
    assert!(!client.acknowledged.contains(&third.sequence));

    edge.say(WEST, departed(player(1), EntityId(5), NORTH, EASTERN));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    assert!(arrival_of(&said, 1).is_some(), "{said:?}");
    edge.say(EAST, departed(player(1), EntityId(5), NORTH, EASTERN));
    edge.settle(EAST).await;
    let said = edge.sent(EAST).await;
    assert!(arrival_of(&said, 1).is_some(), "{said:?}");
    assert!(client.connected());
    let (region, entity, _) = edge.acts(&client).await;
    assert_eq!((region, entity), (EAST, EntityId(5)));
    edge.end().await;
}

/// Scenario 23. The north absorbed the south, which the edge has handled; then
/// the table says that the east absorbed the north, of which the edge has
/// nothing. The stand-ins never make a chain: a name of the south means the east.
#[tokio::test]
async fn a_region_that_stood_for_one_that_is_absorbed_stands_for_the_last_survivor() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    edge.lose(NORTH).await;
    edge.link(NORTH).await;
    let merged = Reply::resumed().entry(absorbed(SOUTH, 0, 0, &[]));
    edge.answer(NORTH, merged).await;
    edge.pairs(vec![(SOUTH, NORTH), (NORTH, EAST)]).await;
    edge.ended_by_the_edge(NORTH).await;

    edge.say(WEST, departed(player(1), EntityId(5), SOUTH, SOUTHERN));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    let arrival = arrival_of(&said, 1);
    assert_eq!(arrival, Some((1, transfer(EntityId(5), 0, SOUTHERN))));
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.offers_in_vain(SOUTH).await;
    edge.offers_in_vain(NORTH).await;
    edge.end().await;
}

/// Scenario 24. The east absorbed the north and answered a hello of the edge, and
/// was then absorbed by the south. The edge has a player under the north and the
/// east's welcome unread on a link. The table's word, that welcome and the south's
/// come in every order; afterwards the player is the south's, and was never
/// disconnected.
#[tokio::test]
async fn two_merges_told_by_the_table_and_two_welcomes_in_any_order_end_at_the_last_survivor() {
    let mut orders = Orders::new();
    while orders.another() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.caught_up(NORTH);
        edge.walks(&client, NORTHERN, 1).await;
        edge.quiet().await;
        edge.lose(NORTH).await;
        edge.link(EAST).await;
        assert!(edge.hello(EAST).players.is_empty());
        let at_east = edge.outbox[EAST.0 as usize] + 1;
        let east = Reply::resumed()
            .entry(absorbed(NORTH, 1, 1, &[]))
            .stay(1, 5, 0);
        // The south came by its state for the edge through the merge.
        let south = Reply::unknown(7)
            .entry(absorbed(EAST, 1, 0, &[at_east]))
            .entry(absorbed(NORTH, 1, 1, &[]))
            .stay(1, 5, 0);
        let scripts = vec![
            vec![Step::Pairs(vec![(NORTH, EAST), (EAST, SOUTH)])],
            vec![Step::Welcome(EAST, east)],
            vec![Step::Link(SOUTH), Step::Welcome(SOUTH, south)],
        ];
        edge.play(scripts, &mut orders).await;
        assert!(
            !edge.gone.contains(&SOUTH),
            "the edge ended its link to the south"
        );
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (SOUTH, EntityId(5), 2));
        edge.end().await;
    }
}

/// Scenario 25. The edge has handled the south's `Absorbed` for the east when a
/// table arrives that is behind it: it says that the east absorbed the north and
/// nothing of the south. The pair's target is read through the stand-ins: the
/// word is owed by the south, whose link is ended for it.
#[tokio::test]
async fn a_pair_whose_target_the_edge_has_retired_names_the_living_region() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, NORTH, NORTHERN).await;
    edge.lose(NORTH).await;
    edge.link(SOUTH).await;
    let merged = Reply::unknown(7).entry(absorbed(EAST, 1, 0, &[]));
    edge.answer(SOUTH, merged).await;
    // The east's link stood: it is taken with the `Absorbed`.
    edge.ended_by_the_edge(EAST).await;

    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(SOUTH).await;
    edge.link(SOUTH).await;
    assert!(edge.hello(SOUTH).players.is_empty());
    let merged = Reply::resumed()
        .entry(absorbed(NORTH, 1, 0, &[]))
        .stay(1, 5, 0);
    edge.answer(SOUTH, merged).await;
    let said = edge.sent(SOUTH).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (SOUTH, EntityId(5), 1));
    edge.end().await;
}

/// What the scenarios of a split begin with: a player of the west, at `HOME`, who
/// has made two steps that the west has not said it applied. Returns the number
/// of the edge's last message that the west has applied.
async fn before_a_split() -> (Harness, Client, u64) {
    let mut edge = Harness::witnessed().await;
    let client = edge.settler(1, 5, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    edge.walks(&client, HOME, 2).await;
    edge.quiet().await;
    edge.lose(WEST).await;
    (edge, client, applied)
}

/// Scenario 26. A stay that a `SplitOff` names and the edge has under the region
/// that says it is the new region's: its view is asked of the new region in the
/// hello of its first link, and what the player did is sent there after its
/// welcome, numbered from 1, without an arrival. Of a stay the edge does not have
/// nothing is said until the new region's presence shows it.
#[tokio::test]
async fn a_stay_that_was_split_off_is_the_new_regions_without_an_arrival() {
    let (mut edge, mut client, applied) = before_a_split().await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let split = Reply::resumed().applied(applied);
    // The hello named the player, who is no longer the west's: `Absent`.
    edge.answer(WEST, split.entry(split_off(PART, &[(1, 5), (2, 6)])))
        .await;
    let said = edge.sent(WEST).await;
    let step = step_into(HOME);
    // What the west had not applied it is sent again, and answers itself.
    let expected = [
        (applied + 1, passed_on(1, 5, 1, step.clone())),
        (applied + 2, passed_on(1, 5, 2, step.clone())),
    ];
    assert_eq!(numbered(&said), expected);
    let asked = subscriptions(said);
    let guests = BTreeSet::from([(Some(Role::Guest), view(HOME))]);
    assert_eq!(meanings(&asked), guests);
    assert!(client.connected());
    for region in [EAST, NORTH] {
        let said = edge.sent(region).await;
        assert!(said.is_empty(), "{region:?} was sent {said:?}");
    }

    edge.link(PART).await;
    let hello = edge.hello(PART);
    assert_eq!((hello.since, hello.seen), (0, 0));
    assert_eq!(hello.players, [player(1)]);
    assert_eq!(set(&hello.chunks), view(HOME));
    assert!(hello.guests.is_empty(), "{hello:?}");
    let part = Reply::unknown(40).stay(1, 5, 1).stay(2, 6, 0);
    edge.answer(PART, part).await;
    let said = edge.sent(PART).await;
    let expected = [
        (1, passed_on(1, 5, 1, step.clone())),
        (2, passed_on(1, 5, 2, step)),
        (3, left(2, Some(6))),
    ];
    assert_eq!(numbered(&said), expected);
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 3));
    edge.end().await;
}

/// Scenario 27. The link to the new region comes before the `SplitOff` is read:
/// its hello names nobody, its `Present` moves the stay as in scenario 3, and the
/// `SplitOff` then names a stay the edge has under the new region already.
#[tokio::test]
async fn a_new_region_linked_before_the_split_off_is_read_says_whom_it_has() {
    let (mut edge, mut client, applied) = before_a_split().await;
    edge.link(PART).await;
    let hello = edge.hello(PART);
    assert!(hello.players.is_empty() && hello.chunks.is_empty() && hello.guests.is_empty());
    edge.answer(PART, Reply::unknown(40).stay(1, 5, 1)).await;
    let said = edge.sent(PART).await;
    assert!(arrival_of(&said, 1).is_none(), "{said:?}");
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(HOME));
    // What the part had not applied is sent to it, with the stay's entity. (That
    // nothing else is sent is scenario 3's to say.)
    let sent = numbered(&said);
    let steps = inputs_of(&said, 1);
    assert_eq!(steps.len(), sent.len(), "{sent:?}");
    let last = steps.last().map(|(_, entity, step)| (*entity, *step));
    assert_eq!(last, Some((EntityId(5), 2)), "{sent:?}");

    edge.link(WEST).await;
    let hello = edge.hello(WEST);
    assert!(hello.players.is_empty() && hello.chunks.is_empty());
    assert_eq!(set(&hello.guests), view(HOME));
    let split = Reply::resumed().applied(applied);
    edge.answer(WEST, split.entry(split_off(PART, &[(1, 5)])))
        .await;
    let said = edge.sent(PART).await;
    assert!(said.is_empty(), "the SplitOff changed something: {said:?}");
    let said = edge.sent(WEST).await;
    assert!(arrival_of(&said, 1).is_none(), "{said:?}");
    assert!(subscriptions(said).is_empty());
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 3));
    edge.end().await;
}

/// Scenarios 26 and 27 with the two links' messages in every order: the stay ends
/// at the new region, nobody arrives anywhere, and what the new region had not
/// applied reaches it with the stay's entity.
#[tokio::test]
async fn a_split_off_and_the_new_regions_presence_in_any_order_leave_the_stay_at_the_new_region() {
    let mut orders = Orders::new();
    while orders.another() {
        let (mut edge, mut client, applied) = before_a_split().await;
        let split = Reply::resumed().applied(applied);
        let split = split.entry(split_off(PART, &[(1, 5)]));
        let part = Reply::unknown(40).stay(1, 5, 1);
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, split)],
            vec![Step::Link(PART), Step::Welcome(PART, part)],
        ];
        edge.play(scripts, &mut orders).await;
        assert!(client.connected());
        let said = edge.sent(PART).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        let steps = inputs_of(&said, 1);
        let second =
            |(_, entity, step): &(u64, EntityId, u64)| (*entity, *step) == (EntityId(5), 2);
        assert!(steps.iter().any(second), "{steps:?}");
        assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
        let said = edge.sent(WEST).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 3));
        edge.end().await;
    }
}

/// Scenario 28, the sequence of the review's defect 1, in every order of the two
/// links' messages. The new region has the player and lets them go back to the
/// region that was split. Read after that, the `SplitOff` finds the stay under
/// the region that says it, with its arrival kept for that region, and leaves it.
#[tokio::test]
async fn a_stay_that_walked_back_from_the_new_region_is_not_taken_there_again_by_the_split_off() {
    let mut orders = Orders::new();
    while orders.another() {
        let (mut edge, mut client, applied) = before_a_split().await;
        let split = Reply::resumed().applied(applied);
        let split = split.entry(split_off(PART, &[(1, 5)]));
        let part = Reply::unknown(40).stay(1, 5, 1);
        let back = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 2, HOME),
            to: WEST,
        };
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, split)],
            vec![
                Step::Link(PART),
                Step::Welcome(PART, part),
                Step::Entry(PART, back),
            ],
        ];
        edge.play(scripts, &mut orders).await;
        assert!(client.connected());
        let said = edge.sent(WEST).await;
        let arrivals: Vec<_> = said
            .iter()
            .filter(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
            .collect();
        assert_eq!(arrivals.len(), 1, "{said:?}");
        let arrived = arrival_of(&said, 1).map(|(_, transfer)| transfer);
        assert_eq!(arrived, Some(transfer(EntityId(5), 2, HOME)));
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 3));
        edge.end().await;
    }
}

/// Scenario 29, the review's defect 3. The west split both players into the part,
/// and the part split the first into a second part; the edge has read neither.
/// With the links to the west and to the part read in every order nobody is
/// disconnected: nobody is judged absent on another region's entry. After the
/// second part's presence the first player is there.
#[tokio::test]
async fn two_splits_read_in_any_order_disconnect_nobody() {
    let mut orders = Orders::new();
    while orders.another() {
        let mut edge = Harness::witnessed().await;
        let mut first = edge.settler(1, 5, WEST, HOME).await;
        let mut second = edge.settler(2, 6, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        edge.lose(WEST).await;
        let west = Reply::resumed().applied(applied);
        let west = west.entry(split_off(PART, &[(1, 5), (2, 6)]));
        let part = Reply::unknown(40)
            .entry(split_off(SECOND_PART, &[(1, 5)]))
            .stay(2, 6, 0);
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, west)],
            vec![Step::Link(PART), Step::Welcome(PART, part)],
        ];
        edge.play(scripts, &mut orders).await;
        assert!(first.connected() && second.connected());

        edge.link(SECOND_PART).await;
        edge.answer(SECOND_PART, Reply::unknown(41).stay(1, 5, 0))
            .await;
        assert!(first.connected() && second.connected());
        assert_eq!(edge.acts(&first).await, (SECOND_PART, EntityId(5), 1));
        assert_eq!(edge.acts(&second).await, (PART, EntityId(6), 1));
        edge.end().await;
    }
}

/// Scenario 30, and A3 of ADR-0014, the review's defect 6. The west split the
/// player into the part, which the east then absorbed; the edge had no link
/// meanwhile. Whichever of the two welcomes is read first, and however their
/// messages fall between each other, the stay ends at the east: a `SplitOff`
/// that names the part, read when the part stands for the east, puts it there.
/// What the player did since the west last applied something reaches the east.
#[tokio::test]
async fn a_stay_split_off_into_a_part_that_was_absorbed_ends_at_the_parts_survivor() {
    let mut orders = Orders::new();
    while orders.another() {
        let (mut edge, mut client, applied) = before_a_split().await;
        edge.lose(EAST).await;
        let split = Reply::resumed().applied(applied);
        let split = split.entry(split_off(PART, &[(1, 5)]));
        let merged = Reply::resumed()
            .entry(absorbed(PART, 40, 0, &[]))
            .stay(1, 5, 1);
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, split)],
            vec![Step::Link(EAST), Step::Welcome(EAST, merged)],
        ];
        edge.play(scripts, &mut orders).await;
        assert!(client.connected());
        let said = edge.sent(EAST).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        let steps = inputs_of(&said, 1);
        let second =
            |(_, entity, step): &(u64, EntityId, u64)| (*entity, *step) == (EntityId(5), 2);
        assert!(steps.iter().any(second), "{steps:?}");
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 3));
        edge.offers_in_vain(PART).await;
        edge.end().await;
    }
}

/// Scenario 31. An action is never sent to a region the edge is not asking for
/// its chunk: where it has no subscription there and someone sees the chunk, it
/// becomes a guest first, on that link, so that the region holds the action until
/// it has the chunk loaded (rule 34).
#[tokio::test]
async fn an_action_sent_on_to_a_region_is_asked_for_there_first() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let first = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    edge.say(WEST, remote(&first, Some(EAST)));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    let expected = [
        EdgeMessage::unnumbered(as_guest(1, [COMMON])),
        EdgeMessage {
            number: Some(1),
            body: EdgeToWorker::Remote(first),
        },
    ];
    assert_eq!(said, expected);

    // The edge is asking for the chunk there by now.
    let second = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        COMMON,
    );
    let sent_on = Durable::NotMine {
        what: Misdirected::Remote(second.clone()),
        holder: EAST,
    };
    edge.say(NORTH, sent_on);
    edge.settle(NORTH).await;
    let said = edge.sent(EAST).await;
    let expected = [EdgeMessage {
        number: Some(2),
        body: EdgeToWorker::Remote(second),
    }];
    assert_eq!(said, expected);

    // Nobody sees this chunk, so there is nothing to ask for.
    let unseen = ChunkPos::new(40, 40);
    let third = breaking(
        player(1),
        edge.entity_of(player(1)),
        edge.sequence(),
        unseen,
    );
    edge.say(WEST, remote(&third, Some(EAST)));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    let expected = [EdgeMessage {
        number: Some(3),
        body: EdgeToWorker::Remote(third),
    }];
    assert_eq!(said, expected);

    // A region without a link is asked in its hello, which holds what follows.
    let fourth = breaking(player(1), edge.entity_of(player(1)), edge.sequence(), FAR);
    edge.say(WEST, remote(&fourth, Some(SOUTH)));
    edge.settle(WEST).await;
    edge.link(SOUTH).await;
    assert_eq!(edge.hello(SOUTH).guests, [FAR]);
    edge.answer(SOUTH, Reply::unknown(1)).await;
    let said = edge.sent(SOUTH).await;
    assert_eq!(numbered(&said), [(1, EdgeToWorker::Remote(fourth))]);
    assert!(client.connected());
    edge.end().await;
}

/// A1 of ADR-0014. The hello to the survivor names only what the edge had there.
/// The `Absorbed` and the absorbed region's unconfirmed entry are among the
/// welcome's entries, and the presence answers have the absorbed region's stay,
/// unnamed, behind the one the hello named.
#[tokio::test]
async fn a_hello_to_a_survivor_names_what_the_edge_had_there_and_the_answers_bring_the_rest() {
    let mut edge = Harness::witnessed().await;
    let mut north = edge.settler(1, 5, NORTH, NORTHERN).await;
    let mut east = edge.settler(3, 7, EAST, EASTERN).await;
    // The north deals with an action of the east's player, and the edge does not
    // read of that before the north is absorbed.
    let sequence = edge.under_way(3, NORTH).await.sequence;
    edge.walks(&north, NORTHERN, 1).await;
    edge.quiet().await;
    assert_eq!(edge.at(NORTH).numbered, 3);
    assert_eq!(edge.at(EAST).numbered, 1);
    let next = edge.outbox[NORTH.0 as usize] + 1;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    let hello = edge.hello(EAST);
    assert_eq!(hello.players, [player(3)]);
    assert_eq!(set(&hello.chunks), view(EASTERN));
    // Of what its player saw on the way in, the first player still sees some.
    let on_the_way = minus(&view(HOME), &view(EASTERN));
    assert_eq!(set(&hello.guests), both(&on_the_way, &view(NORTHERN)));

    // The north had applied the arrival and the action, and not the step.
    let reply = Reply::resumed()
        .applied(1)
        .entry(absorbed(NORTH, 1, 2, &[next]))
        .entry(done(player(3), edge.entity_of(player(3)), sequence))
        .stay(3, 7, 0)
        .stay(1, 5, 0);
    edge.answer(EAST, reply).await;
    edge.acknowledged(&mut east, sequence).await;
    let said = edge.sent(EAST).await;
    // A `Subscribe` for each chunk the east was not asked for as a viewer already.
    let new = minus(&view(NORTHERN), &view(EASTERN));
    assert_eq!(asked_before_numbered(&said, Role::Viewer), new);
    assert_eq!(
        numbered(&said),
        [(2, passed_on(1, 5, 1, step_into(NORTHERN)))]
    );
    let mut views = view(EASTERN);
    views.extend(view(NORTHERN));
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), views);
    assert!(north.connected() && east.connected());
    assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 2));
    assert_eq!(edge.acts(&east).await, (EAST, EntityId(7), 1));
    edge.end().await;
}

/// A4 of ADR-0014. A player leaves while their region has no link, and the region
/// is absorbed. The survivor has the stay and says so, unnamed. The edge does not
/// have it: every leave the survivor is sent names the stay's entity, be it the
/// one that was kept for the absorbed region or the one the answer calls for.
#[tokio::test]
async fn a_stay_of_a_player_who_left_is_ended_at_the_survivor_of_their_region() {
    let mut edge = Harness::witnessed().await;
    let client = edge.settler(1, 5, NORTH, NORTHERN).await;
    edge.caught_up(NORTH);
    edge.quiet().await;
    edge.lose(NORTH).await;
    edge.leave(&client).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    assert!(edge.hello(EAST).players.is_empty());
    let reply = Reply::resumed()
        .entry(absorbed(NORTH, 1, 1, &[]))
        .stay(1, 5, 0);
    edge.answer(EAST, reply).await;
    let said = edge.sent(EAST).await;
    let sent = numbered(&said);
    assert!(!sent.is_empty(), "the stay is left at the survivor");
    let names_the_stay = |(_, body): &(u64, EdgeToWorker)| *body == left(1, Some(5));
    assert!(sent.iter().all(names_the_stay), "{sent:?}");
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "{asked:?}");
    edge.end().await;
}

/// A5 of ADR-0014. A player of the north walks and leaves while the north has no
/// link, and joins again; the west, where they join, absorbs the north meanwhile.
/// The join is sent first and begins a new stay; what was kept for the north
/// follows under the west's numbers and names the old entity, so it moves nobody
/// and ends nothing; and the new stay's first input is numbered 1.
#[tokio::test]
async fn what_an_earlier_stay_did_is_sent_to_the_survivor_under_the_entity_it_had() {
    let mut edge = Harness::witnessed().await;
    let before = edge.settler(1, 5, NORTH, NORTHERN).await;
    edge.caught_up(NORTH);
    edge.quiet().await;
    let applied = edge.at(WEST).numbered;
    edge.lose(NORTH).await;
    edge.walks(&before, NORTHERN, 2).await;
    edge.leave(&before).await;
    edge.lose(WEST).await;
    let mut again = edge.join(player(1)).await;
    edge.drained().await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let reply = Reply::resumed()
        .applied(applied)
        .entry(absorbed(NORTH, 1, 1, &[]))
        .stay(1, 5, 0);
    edge.answer(WEST, reply).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert!(
        matches!(&sent[0], (join, EdgeToWorker::PlayerJoin(_)) if *join == applied + 1),
        "{sent:?}"
    );
    let step = step_into(NORTHERN);
    let expected = [
        (applied + 2, passed_on(1, 5, 1, step.clone())),
        (applied + 3, passed_on(1, 5, 2, step)),
        (applied + 4, left(1, Some(5))),
    ];
    assert_eq!(sent[1..], expected);
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "placed as their old self: {asked:?}");

    edge.says(WEST, edge.spawned(player(1), EntityId(8))).await;
    assert_eq!(edge.acts(&again).await, (WEST, EntityId(8), 1));
    assert!(again.connected());
    edge.end().await;
}

/// A6 of ADR-0014. The east is split with the player in the part. The player
/// leaves, joins again and walks into the part before the edge has said hello to
/// it. The part's presence shows the old stay; the edge has the player there with
/// another entity, so that stay is the part's to end, behind the arrival that
/// takes its place, and the edge's own is untouched.
#[tokio::test]
async fn an_earlier_stay_in_a_part_is_ended_behind_the_arrival_of_the_later_one() {
    let mut edge = Harness::witnessed().await;
    let before = edge.settler(1, 5, EAST, EASTERN).await;
    edge.caught_up(EAST);
    edge.quiet().await;
    edge.lose(EAST).await;
    edge.leave(&before).await;
    let mut again = edge.settler(1, 8, PART, EASTERN).await;

    edge.link(PART).await;
    let hello = edge.hello(PART);
    assert_eq!(hello.players, [player(1)]);
    assert_eq!(set(&hello.chunks), view(EASTERN));
    edge.answer(PART, Reply::unknown(40).stay(1, 5, 0)).await;
    let said = edge.sent(PART).await;
    let arrival = EdgeToWorker::PlayerArrive {
        player: player(1),
        transfer: transfer(EntityId(8), 0, EASTERN),
    };
    assert_eq!(numbered(&said), [(1, arrival), (2, left(1, Some(5)))]);
    assert!(again.connected());

    // The east's entry names the old stay, which the edge does not have.
    edge.link(EAST).await;
    assert!(edge.hello(EAST).players.is_empty());
    let split = Reply::resumed().applied(1);
    edge.answer(EAST, split.entry(split_off(PART, &[(1, 5)])))
        .await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(2, left(1, Some(5)))]);
    let said = edge.sent(PART).await;
    assert!(said.is_empty(), "{said:?}");
    assert!(again.connected());
    assert_eq!(edge.acts(&again).await, (PART, EntityId(8), 1));
    edge.end().await;
}

/// The history of A8, which the edge has seen nothing of: the east absorbed the
/// north, the west was split (the part has the first player), the east was split
/// (the second part has the second player, who had been the north's), the south
/// absorbed the part, and the west absorbed the second part. Three regions live;
/// the scripts are their answers to a hello. `whole` gives the order in which
/// the three are answered one after the other, in place of `orders`.
async fn after_three_merges_and_two_splits(orders: &mut Orders, whole: Option<[usize; 3]>) {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let mut second = edge.settler(2, 6, NORTH, NORTHERN).await;
    let mut third = edge.settler(3, 7, EAST, EASTERN).await;
    let mut fourth = edge.settler(4, 8, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    for region in [WEST, EAST, NORTH] {
        edge.lose(region).await;
    }
    let west = Reply::resumed()
        .applied(applied)
        .entry(split_off(PART, &[(1, 5)]))
        .entry(absorbed(SECOND_PART, 50, 0, &[]))
        .stay(2, 6, 0)
        .stay(4, 8, 0);
    let east = Reply::resumed()
        .applied(1)
        .entry(absorbed(NORTH, 1, 1, &[]))
        .entry(split_off(SECOND_PART, &[(2, 6)]))
        .stay(3, 7, 0);
    // The south came by its state for the edge through its merge.
    let south = Reply::unknown(60)
        .entry(absorbed(PART, 40, 0, &[]))
        .stay(1, 5, 0);
    let scripts = vec![
        vec![Step::Link(WEST), Step::Welcome(WEST, west)],
        vec![Step::Link(EAST), Step::Welcome(EAST, east)],
        vec![Step::Link(SOUTH), Step::Welcome(SOUTH, south)],
    ];
    match whole {
        Some(order) => {
            for script in order {
                edge.play(vec![scripts[script].clone()], orders).await;
            }
        }
        None => edge.play(scripts, orders).await,
    }
    for client in [&mut first, &mut second, &mut third, &mut fourth] {
        assert!(client.connected(), "{:?} was disconnected", client.player);
    }
    // Every stay is where the one region that says `Present` for it has it.
    let (region, entity, _) = edge.acts(&first).await;
    assert_eq!((region, entity), (SOUTH, EntityId(5)));
    let (region, entity, _) = edge.acts(&second).await;
    assert_eq!((region, entity), (WEST, EntityId(6)));
    let (region, entity, _) = edge.acts(&third).await;
    assert_eq!((region, entity), (EAST, EntityId(7)));
    let (region, entity, _) = edge.acts(&fourth).await;
    assert_eq!((region, entity), (WEST, EntityId(8)));
    for region in [NORTH, PART, SECOND_PART] {
        edge.offers_in_vain(region).await;
    }
    edge.end().await;
}

/// A8 of ADR-0014, with a hello to each living region in each order.
#[tokio::test]
async fn after_three_merges_and_two_splits_every_stay_is_where_its_region_says() {
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in orders {
        let mut told = Orders::new();
        told.account
            .push(format!("the three welcomes in the order {order:?}"));
        after_three_merges_and_two_splits(&mut told, Some(order)).await;
    }
}

/// A8 of ADR-0014, with the messages of the three links falling between each
/// other (rule 50).
#[tokio::test]
async fn after_three_merges_and_two_splits_the_order_of_the_links_messages_does_not_matter() {
    let mut orders = Orders::new();
    while orders.another() {
        after_three_merges_and_two_splits(&mut orders, None).await;
    }
}

/// A9 of ADR-0014. The east is split with the player in the part, and then
/// forgets the edge. The part has had a link of the edge that named nobody, of
/// which the edge read nothing. The east's welcome is `Unknown` with no entries:
/// the player is gone as far as the edge can tell. The part still has the stay,
/// and the leave that names its entity ends it there.
#[tokio::test]
async fn a_stay_in_a_part_is_ended_there_when_the_region_it_was_split_off_forgot_the_edge() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, EAST, EASTERN).await;
    edge.caught_up(EAST);
    edge.quiet().await;
    edge.lose(EAST).await;
    edge.link(PART).await;
    assert!(edge.hello(PART).players.is_empty());
    edge.lose(PART).await;

    edge.link(EAST).await;
    assert_eq!(edge.hello(EAST).players, [player(1)]);
    edge.answer(EAST, Reply::unknown(2)).await;
    client.disconnected().await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(1, left(1, Some(5)))]);

    edge.link(PART).await;
    assert!(edge.hello(PART).players.is_empty());
    edge.answer(PART, Reply::unknown(40).stay(1, 5, 0)).await;
    let said = edge.sent(PART).await;
    assert_eq!(numbered(&said), [(1, left(1, Some(5)))]);
    edge.end().await;
}

/// A split and the merge that undoes it, both unread: the west's welcome has the
/// `SplitOff` and then the `Absorbed` of the part. The stay goes to the part and
/// comes back with what the player did, and the `Present` finds it where it is.
#[tokio::test]
async fn a_split_and_the_merge_that_undoes_it_leave_the_stay_where_it_was() {
    let (mut edge, mut client, applied) = before_a_split().await;
    edge.link(WEST).await;
    let reply = Reply::resumed()
        .applied(applied)
        .entry(split_off(PART, &[(1, 5)]))
        .entry(absorbed(PART, 40, 0, &[]))
        .stay(1, 5, 1);
    edge.answer(WEST, reply).await;
    let said = edge.sent(WEST).await;
    assert!(arrival_of(&said, 1).is_none(), "{said:?}");
    let steps = inputs_of(&said, 1);
    let second = |(_, entity, step): &(u64, EntityId, u64)| (*entity, *step) == (EntityId(5), 2);
    assert!(steps.iter().any(second), "{steps:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    assert!(edge.at(WEST).chunks(Role::Guest).is_empty());
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 3));
    edge.offers_in_vain(PART).await;
    edge.end().await;
}

/// Section 8, third item. A `Present` of the west from before the split is read
/// after the part's presence has moved the stay to the part, and moves it back.
/// The `SplitOff` on the west's next link puts that right.
#[tokio::test]
async fn a_split_off_puts_right_a_stay_that_an_answer_from_before_the_split_moved_back() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let applied = edge.at(WEST).numbered;
    // The west answers a hello, and is split before the edge has read all of it.
    edge.link(WEST).await;
    let welcome = Welcome::Resumed {
        entries: 0,
        presences: 1,
        applied,
    };
    edge.says(WEST, WorkerToEdge::Welcome(welcome)).await;
    edge.link(PART).await;
    assert!(edge.hello(PART).players.is_empty());
    edge.answer(PART, Reply::unknown(40).stay(1, 5, 0)).await;
    edge.sent(PART).await;
    assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
    edge.says(WEST, present(player(1), EntityId(5))).await;
    edge.sent(WEST).await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    assert!(client.connected());

    edge.lose(WEST).await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let split = Reply::resumed().applied(applied);
    edge.answer(WEST, split.entry(split_off(PART, &[(1, 5)])))
        .await;
    assert!(client.connected());
    assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 1));
    edge.sent(PART).await;
    assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
    edge.end().await;
}

/// A link to the survivor that ends at each point of its welcome after a merge.
/// The next welcome has the entries the edge has not seen and, as every welcome,
/// the presence answers; what was kept is sent when a welcome's entries are
/// through, under the same numbers on whichever link that is.
#[tokio::test]
async fn a_link_that_ends_at_any_point_of_a_survivors_welcome_loses_nothing() {
    for read in 1..=3 {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let seen = edge.hello(EAST).seen;
        // The welcome, the `Absorbed` and the `Present`, of which the edge reads
        // the first `read` before the link ends.
        let welcome = Welcome::Resumed {
            entries: 1,
            presences: 1,
            applied: 0,
        };
        edge.says(EAST, WorkerToEdge::Welcome(welcome)).await;
        if read >= 2 {
            edge.take(Step::Entry(EAST, absorbed(NORTH, 1, 3, &[])))
                .await;
        }
        if read >= 3 {
            edge.says(EAST, present_with(player(1), EntityId(5), 2))
                .await;
        }
        edge.lose(EAST).await;

        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        let mut again = Reply::resumed().stay(1, 5, 2);
        if read >= 2 {
            // The merge is behind the edge: the hello names what was the north's.
            assert_eq!(hello.seen, seen + 1);
            assert_eq!(hello.players, [player(1)]);
            assert_eq!(set(&hello.chunks), view(NORTHERN));
            assert_eq!(set(&hello.guests), set(&[LENT, SOUGHT]));
        } else {
            assert_eq!(hello.seen, seen);
            assert!(hello.players.is_empty() && hello.chunks.is_empty());
            edge.outbox[EAST.0 as usize] = seen;
            again = again.entry(absorbed(NORTH, 1, 3, &[]));
        }
        edge.answer(EAST, again).await;
        let said = edge.sent(EAST).await;
        assert_eq!(
            numbered(&said),
            kept_after_the_merge(&action),
            "read {read}"
        );
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
        assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[LENT, SOUGHT]));
        assert!(north.connected());
        assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 5));
        edge.end().await;
    }
}

/// Section 3, step 5. What the absorbed region showed counts as shown by the
/// survivor, whose snapshot puts it right, and what it served the survivor
/// serves: an action for which no region is named goes there, behind the
/// subscription the `Absorbed` made.
#[tokio::test]
async fn what_an_absorbed_region_showed_and_served_is_the_survivors_to_put_right() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let _second = edge.settler(2, 6, WEST, HOME).await;
    edge.says(WEST, elsewhere(LENT, edge.ask(WEST, LENT), NORTH))
        .await;
    edge.sent(NORTH).await;
    let there = vec![stranger(EntityId(70), LENT)];
    let chunk = chunk_with(LENT, STONE);
    edge.says(
        NORTH,
        snapshot_of(LENT, edge.ask(NORTH, LENT), chunk, there),
    )
    .await;
    edge.sync(&mut client).await;
    assert!(client.entities.contains(&70), "{:?}", client.entities);
    assert_eq!(client.state(LENT), Some(STONE));

    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
        .await;
    let asked = subscriptions(edge.sent(EAST).await);
    assert_eq!(asked, [as_guest(1, [LENT])]);
    // Nothing leaves a screen because a region was absorbed.
    edge.sync(&mut client).await;
    assert!(client.entities.contains(&70), "{:?}", client.entities);
    assert_eq!(client.state(LENT), Some(STONE));

    let action = breaking(player(2), edge.entity_of(player(2)), edge.sequence(), LENT);
    edge.say(WEST, remote(&action, None));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(1, EdgeToWorker::Remote(action))]);

    let chunk = chunk_with(LENT, GRANITE);
    edge.says(EAST, snapshot_of(LENT, 1, chunk, Vec::new()))
        .await;
    edge.sync(&mut client).await;
    assert!(!client.entities.contains(&70), "{:?}", client.entities);
    assert_eq!(client.state(LENT), Some(GRANITE));
    edge.end().await;
}

/// Rule 48, first item, and rule 34. An action that was kept for the absorbed
/// region goes to the survivor behind the subscription for its chunk, on the
/// same link, so that the survivor holds it until it has the chunk loaded.
#[tokio::test]
async fn an_action_kept_for_an_absorbed_region_follows_the_subscription_for_its_chunk() {
    let mut edge = Harness::witnessed().await;
    let _client = edge.settler(2, 6, WEST, HOME).await;
    let action = edge.under_way(2, NORTH).await;
    edge.quiet().await;
    assert_eq!(edge.at(NORTH).chunks(Role::Guest), set(&[COMMON]));
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    assert!(edge.hello(EAST).guests.is_empty());
    edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
        .await;
    let mut said = edge.sent(EAST).await;
    said.retain(|message| !matches!(message.body, EdgeToWorker::Confirm { .. }));
    let expected = [
        EdgeMessage::unnumbered(as_guest(1, [COMMON])),
        EdgeMessage {
            number: Some(1),
            body: EdgeToWorker::Remote(action),
        },
    ];
    assert_eq!(said, expected);
    edge.end().await;
}

/// Rule 41, and section 8.9 of ADR-0014. The survivor had forgotten the edge and
/// came by a state for it through the merge: its welcome is `Unknown`, with the
/// `Absorbed` among its entries and a `Present` for the stay that came. The edge
/// gives up what it had at the survivor, its player there among it, and then has
/// at the survivor what it had at the absorbed region, numbered from 1.
#[tokio::test]
async fn a_survivor_that_had_forgotten_the_edge_says_unknown_with_the_absorbed_behind_it() {
    let mut edge = Harness::witnessed().await;
    let mut north = edge.settler(1, 5, NORTH, NORTHERN).await;
    edge.caught_up(NORTH);
    let mut east = edge.settler(3, 7, EAST, EASTERN).await;
    edge.walks(&north, NORTHERN, 1).await;
    edge.quiet().await;
    edge.lose(NORTH).await;
    edge.lose(EAST).await;
    edge.link(EAST).await;
    assert_eq!(edge.hello(EAST).players, [player(3)]);
    let reply = Reply::unknown(9)
        .entry(absorbed(NORTH, 1, 1, &[]))
        .stay(1, 5, 0);
    edge.answer(EAST, reply).await;
    east.disconnected().await;
    let said = edge.sent(EAST).await;
    let expected = [
        (1, left(3, Some(7))),
        (2, passed_on(1, 5, 1, step_into(NORTHERN))),
    ];
    assert_eq!(numbered(&said), expected);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
    assert!(north.connected());
    assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 2));

    edge.link(EAST).await;
    let hello = edge.hello(EAST);
    assert_eq!((hello.since, hello.seen), (9, 1));
    assert_eq!(hello.players, [player(1)]);
    edge.answer(EAST, Reply::resumed().applied(3).stay(1, 5, 2))
        .await;
    assert!(north.connected());
    edge.end().await;
}

/// Section 5. A region is owed its `Absorbed` also when all the edge has of it is
/// a guest's subscription there and a subscription elsewhere that names it. The
/// survivor's next welcome has none, so the edge concludes that none comes: what
/// it was asking the absorbed region for it asks the survivor for, and the
/// subscription that named the one names the other.
#[tokio::test(start_paused = true)]
async fn a_region_the_edge_only_asks_for_a_chunk_is_owed_a_word_by_its_survivor() {
    let (mut edge, mut client) = told_elsewhere_with_the_absorbed().await;
    edge.answer(EAST, Reply::resumed()).await;
    tokio::time::advance(Duration::from_millis(2500)).await;
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(EAST).await;
    edge.link(EAST).await;
    assert!(edge.hello(EAST).guests.is_empty());
    edge.answer(EAST, Reply::resumed()).await;
    let asked = subscriptions(edge.sent(EAST).await);
    assert_eq!(asked, [as_guest(1, [COMMON])]);
    let asked = subscriptions(edge.sent(WEST).await);
    assert!(asked.is_empty(), "{asked:?}");

    edge.says(EAST, not_mine(COMMON, 1)).await;
    let asked = subscriptions(edge.sent(WEST).await);
    let again = BTreeSet::from([(Some(Role::Viewer), set(&[COMMON]))]);
    assert_eq!(meanings(&asked), again);
    edge.says(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)))
        .await;
    edge.sync(&mut client).await;
    assert!(client.chunks.contains_key(&COMMON));
    edge.offers_in_vain(NORTH).await;
    edge.end().await;
}

/// Section 4, last paragraph: the entity of a player who left, on its way to a
/// region that stands for another, is discarded at the living one.
#[tokio::test]
async fn an_entity_on_its_way_to_an_absorbed_region_is_discarded_at_the_survivor() {
    let mut edge = Harness::witnessed().await;
    let client = edge.settler(1, 5, WEST, HOME).await;
    edge.pairs(vec![(NORTH, EAST)]).await;
    edge.ended_by_the_edge(NORTH).await;
    edge.leave(&client).await;
    edge.say(WEST, departed(player(1), EntityId(5), NORTH, NORTHERN));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    let discard = EdgeToWorker::Discard {
        player: player(1),
        entity: EntityId(5),
        hops: 0,
        chunk: NORTHERN,
    };
    assert_eq!(numbered(&said), [(1, discard)]);
    edge.end().await;
}

/// A player's own entity as a region has it in a snapshot.
fn own(who: u128, entity: u64, chunk: ChunkPos) -> EntityState {
    EntityState {
        entity: EntityId(entity),
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(who),
            name: "Player".to_owned(),
        },
        pose: Pose::at(within(chunk)),
    }
}

/// What the two tests below begin with: the west has shown the first player's
/// entity, in its snapshot of the chunk they stand in, which a second player
/// sees. The stay is then the east's, by a split and a merge the edge has not
/// caught up with: the east says `Present` for it in answer to a hello that named
/// nobody, and the edge asks the east for the view and sends it the player's
/// step, as case 3 of ADR-0015, section 2.1, has it.
async fn a_shown_stay_another_region_has() -> (Harness, Client, Client) {
    let mut edge = Harness::witnessed().await;
    let client = edge.settler(1, 5, WEST, HOME).await;
    let mut second = edge.settler(2, 6, WEST, HOME).await;
    let there = vec![own(1, 5, HOME)];
    edge.says(
        WEST,
        snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there),
    )
    .await;
    edge.sync(&mut second).await;
    assert!(second.entities.contains(&5), "{:?}", second.entities);
    edge.walks(&client, HOME, 1).await;
    edge.quiet().await;

    edge.link(EAST).await;
    edge.answer(EAST, Reply::resumed().stay(1, 5, 0)).await;
    edge.sent(EAST).await;
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
    (edge, client, second)
}

/// Found by the generated runs of scenario 32. A stay comes to a region without
/// an arrival, and the region reports the player's next step before it has shown
/// the edge the player.
#[tokio::test]
async fn a_move_that_a_stays_new_region_reports_before_its_snapshot_is_taken() {
    // The sequence: as `a_shown_stay_another_region_has`. The east then applies
    // the step in the tick that takes it, as a move is held by nothing (ADR-0014,
    // section 3.6), and reports it among that tick's events, which come before
    // the tick's snapshots (ADR-0012, section 5.2, and rule 31).
    //
    // The records: the stay is the east's (ADR-0015, section 2.1, case 3;
    // ADR-0014, rule 38), "its view's subscriptions move here as at a
    // hand-over". They are silent on whose word on the player's entity counts
    // from then on. ADR-0013, section 7, takes it only from the region that last
    // introduced the entity; section 3 of ADR-0015 makes the survivor that region
    // at a merge, and its section 8 names the split only for several edges.
    // After a hand-over the new region introduces the entity when it takes the
    // arrival in; here nobody arrives. Read as: the region a stay is in is the
    // one whose word on the player's own entity counts.
    //
    // What happened: the east's `EntityMoved` is passed over. The view stays
    // centred where the west last had the player until the east's snapshot has
    // shown the entity and the player steps again. Where no region had shown the
    // entity before, the step is taken.
    let (mut edge, _first, _second) = a_shown_stay_another_region_has().await;
    edge.says(EAST, walked(EntityId(5), HOME, STEP_EAST)).await;
    edge.sent(EAST).await;
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(STEP_EAST));
    edge.end().await;
}

/// The same step, reported when the region has shown the entity: it is taken.
/// This is what the regions of the generated runs do.
#[tokio::test]
async fn a_move_that_a_stays_new_region_reports_after_its_snapshot_is_taken() {
    let (mut edge, _first, _second) = a_shown_stay_another_region_has().await;
    let there = vec![own(1, 5, HOME)];
    edge.says(
        EAST,
        snapshot_of(HOME, edge.ask(EAST, HOME), empty_chunk(), there),
    )
    .await;
    edge.says(EAST, walked(EntityId(5), HOME, STEP_EAST)).await;
    edge.sent(EAST).await;
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(STEP_EAST));
    edge.end().await;
}

/// Found by the generated runs of scenario 32 (seed 33, before the regions of
/// those runs were kept from it). Two regions each have a viewer of a chunk and
/// each name the other for it.
#[tokio::test(start_paused = true)]
async fn two_regions_that_name_each_other_for_a_chunk_are_asked_again() {
    // The sequence, as the run came to it: the west is pinned to where the chunk
    // lies and is split, and the chunk goes to the part. A player of the west
    // sees it: the west asks the store and says `Elsewhere` with the part. The
    // part gives the chunk back, which makes it the west's again by the store's
    // table, "and nobody tells the pinned region" (ADR-0014, section 2.2). A
    // player of the part sees the chunk: the part asks the store and says
    // `Elsewhere` with the west. Here the two regions are the west and the east.
    //
    // The records: ADR-0013, section 3, has the edge become a guest at the
    // region an `Elsewhere` names only if it has no subscription there, and ask
    // a viewer's region again only on a `NotMine` from a guest's region or when
    // the subscription it pointed at ends. Statement E holds: each subscription
    // names a region where the edge has one. ADR-0014, rule 49, ends such a ring
    // of beliefs for players and actions (a pinned region takes in whoever is
    // sent to it and asks again) and says nothing of subscriptions; by rule 13 of
    // ADR-0012 the west does not learn by itself. Read as: a chunk somebody sees
    // is served in the end, so one of the two is asked again.
    //
    // What happened: nothing is asked of either region again. The chunk is
    // served to nobody for as long as both players see it.
    let mut edge = Harness::witnessed().await;
    let _first = edge.settler(1, 5, WEST, HOME).await;
    let _second = edge.settler(2, 6, EAST, EASTERN).await;
    edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), EAST))
        .await;
    edge.sent(EAST).await;
    edge.says(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST))
        .await;
    let mut asked = Vec::new();
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(1100)).await;
        for region in [WEST, EAST] {
            asked.extend(subscriptions(edge.sent(region).await));
        }
    }
    let again = asked.iter().any(|body| names(body, COMMON));
    assert!(
        again,
        "neither region is asked again for the chunk: {asked:?}"
    );
    edge.end().await;
}

// ---------------------------------------------------------------------------
// ADR-0020, step R1.4, written from the record by someone who did not write the
// edge's code.
//
// What each of these tests expects is taken from
// `docs/adr/0020-one-stay-per-player.md`: sections 4 (step 1), 4.1 to 4.4 and 6
// (rules 5 and 6, "The player list"), and the row "Edge" of its table of tests.
// Where the edge does otherwise the test is kept as it is and marked as a finding.
//
// Most of them begin with a player who has connected twice: the first connection
// joined and went before the home region answered, so that there is an attempt
// of an earlier connection of the same player for a region to name, which is what
// "stale" means below.
// ---------------------------------------------------------------------------

/// The reason of the disconnect packet a connection is sent that a later login
/// of its player has put out, as `Client::disconnected` has it: the compound
/// `{ translate: "multiplayer.disconnect.duplicate_login" }` of section 6, and
/// not a string.
fn duplicate_login() -> String {
    let key = Nbt::String("multiplayer.disconnect.duplicate_login".to_owned());
    format!("{:?}", Nbt::Compound(vec![("translate".to_owned(), key)]))
}

/// A stay as the home region lets it go without having placed it, or having
/// placed it without an input applied since: one hand-over behind it, and the
/// attempt of its join if it still carries one (section 4, step 7; section 4.3).
fn let_go(entity: u64, chunk: ChunkPos, attempt: Option<u64>) -> PlayerTransfer {
    PlayerTransfer {
        hops: 1,
        attempt,
        ..transfer(EntityId(entity), 0, chunk)
    }
}

/// The outbox entry with which a region lets `stay` of `who` go to `to`.
fn gone_to(who: u128, stay: &PlayerTransfer, to: RegionId) -> Durable {
    Durable::Departed {
        player: player(who),
        transfer: stay.clone(),
        to,
    }
}

/// The outbox entry of a region of which `region` was split off with one stay,
/// which carries `attempt`.
fn split_with(region: RegionId, who: u128, entity: u64, attempt: Option<u64>) -> Durable {
    Durable::SplitOff {
        region,
        players: vec![(player(who), EntityId(entity), attempt)],
    }
}

/// The outbox entry that says the server has ended a stay (section 6, rule 5).
fn ended(who: u128, entity: u64, attempt: Option<u64>) -> Durable {
    Durable::Ended {
        player: player(who),
        entity: EntityId(entity),
        attempt,
    }
}

/// The leave of a view that was never told its entity: it names the attempt of
/// its join instead (section 4.4).
fn left_unentered(who: u128, attempt: u64) -> EdgeToWorker {
    EdgeToWorker::PlayerLeave {
        player: player(who),
        entity: None,
        attempt: Some(attempt),
    }
}

/// A region's word that an entity of `who` has come into existence in `chunk`.
fn introduced(who: u128, entity: u64, chunk: ChunkPos) -> WorkerToEdge {
    WorkerToEdge::TickDelta {
        tick: 2,
        events: vec![RegionEvent::EntitySpawned(own(who, entity, chunk))],
    }
}

/// The arrival of `stay` of `who`, as the edge sends it.
fn arrives(who: u128, stay: &PlayerTransfer) -> EdgeToWorker {
    EdgeToWorker::PlayerArrive {
        player: player(who),
        transfer: stay.clone(),
    }
}

/// How often the client was put into the world, by everything the edge has sent
/// it so far.
fn times_entered(client: &mut Client) -> usize {
    client.drain();
    let entered = |told: &&Told| **told == Told::Entered;
    client.told.iter().filter(entered).count()
}

/// Makes sure that the connection goes on waiting: it is open, and its client
/// has not been put into the world.
fn waits(client: &mut Client) {
    assert_eq!(times_entered(client), 0, "the player was entered");
    assert!(!client.closed, "the connection was ended");
    assert_eq!(client.disconnected, None);
}

/// Makes sure that the client was put into the world, once, and is still served.
fn is_in_the_world(client: &mut Client) {
    assert_eq!(times_entered(client), 1, "{:?}", client.told);
    assert!(!client.closed, "the connection was ended");
    assert_eq!(client.disconnected, None);
}

/// Whether the client has `who` in its player list, by what it was told last.
fn lists(client: &Client, who: u128) -> bool {
    let uuid = player(who).0;
    let last = client.told.iter().rev().find_map(|told| match told {
        Told::Listed(listed) if *listed == uuid => Some(true),
        Told::Unlisted(unlisted) if *unlisted == uuid => Some(false),
        _ => None,
    });
    last.unwrap_or(false)
}

/// Goes through everything the client was told of entities and makes sure that
/// it never showed `older` and `newer` at once and was never shown `older` after
/// `newer` (section 6, rule 6).
fn never_both_and_never_the_older_again(client: &Client, older: u64, newer: u64) {
    let mut shown = BTreeSet::new();
    let mut had_the_newer = false;
    for told in &client.told {
        match told {
            Told::Shown(entity) => {
                assert!(
                    !(*entity == older && had_the_newer),
                    "the older entity came back: {:?}",
                    client.told
                );
                had_the_newer |= *entity == newer;
                shown.insert(*entity);
            }
            Told::Hidden(entity) => {
                shown.remove(entity);
            }
            _ => {}
        }
        assert!(
            !(shown.contains(&older) && shown.contains(&newer)),
            "both entities were shown at once: {:?}",
            client.told
        );
    }
}

impl Harness {
    /// A player whose first connection joined and went before the west said
    /// anything of it, and who has connected again. The west has been sent the
    /// first join, the leave of the first connection and the second join, in
    /// that order. Returns the client of the second connection, the number of
    /// its join (the leave has the number before it and the first join the one
    /// before that) and the attempt of the first connection.
    async fn come_again(&mut self, who: u128) -> (Client, u64, u64) {
        let first = self.join(player(who)).await;
        let (_, join) = self.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        self.leave(&first).await;
        let again = self.join(player(who)).await;
        let (_, leave) = self.next_numbered(WEST).await;
        assert!(
            matches!(leave, EdgeToWorker::PlayerLeave { .. }),
            "{leave:?}"
        );
        let (number, join) = self.next_numbered(WEST).await;
        assert!(
            matches!(&join, EdgeToWorker::PlayerJoin(join) if join.attempt == again.session.0),
            "{join:?}"
        );
        (again, number, first.session.0)
    }

    /// A player connects with a client that takes only `room` packets before
    /// its queue is full.
    async fn join_with_room(&mut self, player: PlayerId, room: usize) -> Client {
        self.sessions += 1;
        self.attempts.insert(player, self.sessions);
        let session = SessionId(self.sessions);
        let (outbound, packets) = mpsc::channel(room);
        let awaiting = Arc::new(AtomicI32::new(NO_TELEPORT));
        let join = Command::Join {
            appearance: Default::default(),
            session,
            profile: Profile {
                uuid: player.0,
                name: format!("Player{}", self.sessions),
            },
            requested_view_distance: None,
            outbound,
            awaiting_teleport: Arc::clone(&awaiting),
        };
        self.commands.send(join).await.unwrap();
        Client::new(player, session, packets, Arc::clone(&self.ledger), awaiting)
    }

    /// Makes sure that no region the test plays is asked for anything.
    fn nobody_is_asked_for_anything(&self) {
        for region in self.linked() {
            let left = &self.at(region).subscriptions;
            assert!(left.is_empty(), "{region:?} is asked for {left:?}");
        }
    }
}

/// Section 4.1, the table's first row: `Spawned` with the attempt of the view's
/// join enters the player, with the view's region as it is.
#[tokio::test]
async fn spawned_with_the_attempt_of_the_views_join_enters_the_player() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    let placed = spawned(client.session.0, player(1), EntityId(5));
    edge.says(WEST, placed).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    assert_eq!(edge.at(WEST).numbered, join + 1);

    // Told again, or told by another path for the same stay while it still
    // carries its attempt, the view has its entity: it is not entered twice.
    let again = spawned(client.session.0, player(1), EntityId(5));
    edge.says(WEST, again).await;
    edge.link(WEST).await;
    let reply = Reply::resumed().applied(join).stay(1, 5, 0);
    edge.answer(WEST, reply.of_join(1, client.session.0)).await;
    edge.sent(WEST).await;
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 2));
    edge.end().await;
}

/// Section 4.3, the departure from the first draft: one path serves "a stay
/// placed and let go before its edge heard of the placing" and one whose edge
/// did hear. Here the edge has read `Spawned`, and the `Departed` still carries
/// the attempt, as no input was applied. The view has its entity, so this is a
/// hand-over like any other: one arrival, and the client is not entered again.
#[tokio::test]
async fn a_departed_that_still_carries_the_attempt_of_a_view_that_was_entered_is_a_hand_over() {
    let mut edge = Harness::witnessed().await;
    let (mut client, _, _) = edge.come_again(1).await;
    let attempt = client.session.0;
    edge.says(WEST, spawned(attempt, player(1), EntityId(5)))
        .await;
    edge.quiet().await;
    let stay = let_go(5, EASTERN, Some(attempt));
    edge.say(WEST, gone_to(1, &stay, EAST));
    edge.settle(WEST).await;
    let said = edge.sent(EAST).await;
    assert_eq!(numbered(&said), [(1, arrives(1, &stay))]);
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// Section 4.1, after its table: "a view that was entered and whose arrival
/// waits for a region without a link is in the world with no chunks". The stay
/// is let go unplaced to a region the edge has no link to. The arrival is kept
/// and is the first thing numbered on the region's next link; a client that was
/// gone leaves its leave behind it there.
#[tokio::test]
async fn a_stay_let_go_unplaced_to_a_region_without_a_link_is_entered_and_its_arrival_kept() {
    for gone in [false, true] {
        let mut edge = Harness::witnessed().await;
        let (mut client, _, _) = edge.come_again(1).await;
        let stay = let_go(5, EASTERN, Some(client.session.0));
        edge.lose(EAST).await;
        if gone {
            client.packets.close();
        }
        edge.say(WEST, gone_to(1, &stay, EAST));
        edge.settle(WEST).await;
        if !gone {
            is_in_the_world(&mut client);
        }

        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        if gone {
            assert!(
                hello.players.is_empty() && hello.chunks.is_empty(),
                "{hello:?}"
            );
        } else {
            assert_eq!(hello.players, [player(1)]);
            assert_eq!(set(&hello.chunks), view(EASTERN));
        }
        // The east has yet to hear of the player: `Absent` for whom the hello
        // named, which a kept arrival answers.
        edge.answer(EAST, Reply::resumed()).await;
        let sent = numbered(&edge.sent(EAST).await);
        if gone {
            let expected = [(1, arrives(1, &stay)), (2, left(1, Some(5)))];
            assert_eq!(sent, expected);
        } else {
            assert_eq!(sent, [(1, arrives(1, &stay))]);
            is_in_the_world(&mut client);
            assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        }
        edge.end().await;
    }
}

/// Section 4.1, "Why the join names its attempt": the answer to the join of an
/// earlier connection finds the later connection's view without an entity. It is
/// passed over, the connection goes on waiting, and the answer to its own join
/// then enters it. Taken, it would give the view an entity no region has.
#[tokio::test]
async fn spawned_with_the_attempt_of_an_earlier_connection_is_passed_over() {
    let mut edge = Harness::witnessed().await;
    let (mut client, _, stale) = edge.come_again(1).await;
    edge.says(WEST, spawned(stale, player(1), EntityId(5)))
        .await;
    let said = edge.sent(WEST).await;
    assert!(said.is_empty(), "{said:?}");
    waits(&mut client);
    edge.nobody_is_asked_for_anything();

    let placed = spawned(client.session.0, player(1), EntityId(6));
    edge.says(WEST, placed).await;
    edge.sent(WEST).await;
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
    edge.end().await;
}

/// Section 4.3, path 1. The link to the home region is lost before `Spawned` is
/// read. The region's `Present` carries the attempt of the view's join, so the
/// stay is the view's: the player is entered and stays when the presence is
/// through.
#[tokio::test]
async fn a_present_from_the_views_region_with_the_views_attempt_enters_the_player() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    edge.lose(WEST).await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let reply = Reply::resumed().applied(join).stay(1, 5, 0);
    edge.answer(WEST, reply.of_join(1, client.session.0)).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Section 4.3, the second row of the new table: a `Present` with another attempt
/// or none for a view without an entity, from the region the view is under, while
/// the join is kept for that region. Nothing is done with it: it is a stay from
/// before the join, which the join will end. The join is sent again and its own
/// answer enters the player.
#[tokio::test]
async fn a_present_from_the_views_region_with_another_attempt_does_nothing_while_the_join_is_kept()
{
    for carries_the_earlier_attempt in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, stale) = edge.come_again(1).await;
        edge.lose(WEST).await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        // The west had applied the first connection's join and nothing after it.
        let mut reply = Reply::resumed().applied(join - 2).stay(1, 4, 0);
        if carries_the_earlier_attempt {
            reply = reply.of_join(1, stale);
        }
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], (join - 1, left_unentered(1, stale)));
        assert!(
            matches!(&sent[1], (again, EdgeToWorker::PlayerJoin(_)) if *again == join),
            "{sent:?}"
        );
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "placed as an earlier stay: {asked:?}");
        waits(&mut client);

        let placed = spawned(client.session.0, player(1), EntityId(6));
        edge.says(WEST, placed).await;
        edge.sent(WEST).await;
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
        edge.end().await;
    }
}

/// Section 4.3, the same row, "otherwise": no join is kept for the region, and
/// its `Present` has another attempt or none. That stay is one the edge does not
/// have, so it is ended with a leave that names its entity, and the view is not
/// entered as it.
#[tokio::test]
async fn a_present_with_another_attempt_and_no_join_kept_is_a_stay_the_edge_does_not_have() {
    for carries_the_earlier_attempt in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, stale) = edge.come_again(1).await;
        edge.lose(WEST).await;
        edge.link(WEST).await;
        let mut reply = Reply::resumed().applied(join).stay(1, 4, 0);
        if carries_the_earlier_attempt {
            reply = reply.of_join(1, stale);
        }
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        assert_eq!(numbered(&said), [(join + 1, left(1, Some(4)))]);
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "placed as an earlier stay: {asked:?}");
        assert_eq!(times_entered(&mut client), 0);
        edge.end().await;
    }
}

/// Section 4.3, path 1a, before the `SplitOff` is read: the link to the part
/// comes first. Its `Present` has the view's attempt, so the stay is the view's
/// whatever region the view is under: the view is the part's and the player is
/// entered. The `SplitOff` then finds the view with an entity under the part and
/// does nothing, and the split region's `Absent` is passed over.
#[tokio::test]
async fn a_present_from_a_part_before_the_split_off_is_read_enters_the_player_there() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    let attempt = client.session.0;
    edge.lose(WEST).await;

    edge.link(PART).await;
    assert!(edge.hello(PART).players.is_empty());
    let part = Reply::unknown(40).stay(1, 5, 0).of_join(1, attempt);
    edge.answer(PART, part).await;
    let said = edge.sent(PART).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
    is_in_the_world(&mut client);

    edge.link(WEST).await;
    let hello = edge.hello(WEST);
    assert!(hello.players.is_empty(), "{hello:?}");
    let split = Reply::resumed().applied(join);
    edge.answer(WEST, split.entry(split_with(PART, 1, 5, Some(attempt))))
        .await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    let said = edge.sent(PART).await;
    assert!(said.is_empty(), "the SplitOff changed something: {said:?}");
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 1));
    edge.end().await;
}

/// Section 4.3, the second row for a `Present` from another region: with another
/// attempt or none it is a stay the edge does not have, ended with a leave that
/// names its entity. The view is untouched: it waits under the home region, whose
/// answer to its own join enters it.
#[tokio::test]
async fn a_present_from_a_part_with_another_attempt_is_ended_there_and_the_view_waits() {
    for carries_the_earlier_attempt in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, stale) = edge.come_again(1).await;
        edge.lose(WEST).await;

        edge.link(PART).await;
        let mut part = Reply::unknown(40).stay(1, 4, 0);
        if carries_the_earlier_attempt {
            part = part.of_join(1, stale);
        }
        edge.answer(PART, part).await;
        let said = edge.sent(PART).await;
        assert_eq!(numbered(&said), [(1, left(1, Some(4)))]);
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "placed as an earlier stay: {asked:?}");
        waits(&mut client);

        // The view is still the home region's: its hello names the player.
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        edge.answer(WEST, Reply::resumed().applied(join - 2)).await;
        let sent = numbered(&edge.sent(WEST).await);
        assert_eq!(sent.len(), 2, "{sent:?}");
        waits(&mut client);
        let placed = spawned(client.session.0, player(1), EntityId(6));
        edge.says(WEST, placed).await;
        edge.sent(WEST).await;
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
        edge.end().await;
    }
}

/// What the tests of path 2 begin with: the link to the home region was lost
/// before `Spawned` was read, and its next welcome has a `SplitOff` that names
/// the stay with `named`, followed by `Absent` for the player the hello named.
/// The west had applied everything up to `applied`. Returns what the west was
/// sent.
async fn split_before_spawned_was_read(
    edge: &mut Harness,
    applied: u64,
    named: Option<u64>,
) -> Vec<EdgeMessage> {
    edge.lose(WEST).await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let split = Reply::resumed().applied(applied);
    edge.answer(WEST, split.entry(split_with(PART, 1, 5, named)))
        .await;
    edge.sent(WEST).await
}

/// Section 4.3, path 2: the `SplitOff` alone, followed by the split region's
/// `Absent`. The entry has the view's attempt, so the view is under the part
/// before the `Absent` is read, and that is passed over: the connection goes on
/// waiting, nothing is asked of anyone and nothing is sent, and the part's first
/// hello names the player.
#[tokio::test]
async fn a_view_that_a_split_off_names_by_its_attempt_survives_the_split_regions_absent() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    let attempt = client.session.0;
    let said = split_before_spawned_was_read(&mut edge, join, Some(attempt)).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    let asked = subscriptions(said);
    assert!(asked.is_empty(), "{asked:?}");
    waits(&mut client);

    edge.link(PART).await;
    let hello = edge.hello(PART);
    assert_eq!(hello.players, [player(1)]);
    assert!(
        hello.chunks.is_empty() && hello.guests.is_empty(),
        "{hello:?}"
    );
    waits(&mut client);
    // The part has the stay and says so, and the player is entered there: the
    // test after this one looks at that.
    let part = Reply::unknown(40).stay(1, 5, 0).of_join(1, attempt);
    edge.answer(PART, part).await;
    is_in_the_world(&mut client);
    edge.end().await;
}

/// Section 4.3, paths 2 and 1: after the `SplitOff` the part's hello names the
/// player, and the part's `Present` enters them there.
#[tokio::test]
async fn a_present_from_a_part_after_the_split_off_was_read_enters_the_player_there() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    let attempt = client.session.0;
    split_before_spawned_was_read(&mut edge, join, Some(attempt)).await;

    edge.link(PART).await;
    assert_eq!(edge.hello(PART).players, [player(1)]);
    let part = Reply::unknown(40).stay(1, 5, 0).of_join(1, attempt);
    edge.answer(PART, part).await;
    let said = edge.sent(PART).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
    assert!(edge.at(WEST).subscriptions.is_empty());
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 1));
    edge.end().await;
}

/// Section 4.3, "What makes each safe": the split region's welcome and the
/// part's, with their messages in every order. Whichever of `SplitOff` and the
/// part's `Present` is read first, the player is entered once, under the part,
/// and nobody is sent an arrival, a leave or a join.
#[tokio::test]
async fn a_split_off_and_the_parts_present_in_any_order_enter_the_player_once_at_the_part() {
    let mut orders = Orders::new();
    while orders.another() {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, _) = edge.come_again(1).await;
        let attempt = client.session.0;
        edge.lose(WEST).await;
        let split = Reply::resumed().applied(join);
        let split = split.entry(split_with(PART, 1, 5, Some(attempt)));
        let part = Reply::unknown(40).stay(1, 5, 0).of_join(1, attempt);
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, split)],
            vec![Step::Link(PART), Step::Welcome(PART, part)],
        ];
        edge.play(scripts, &mut orders).await;
        for region in [WEST, PART] {
            let sent = numbered(&edge.sent(region).await);
            assert!(sent.is_empty(), "{region:?} was sent {sent:?}");
        }
        assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 1));
        edge.end().await;
    }
}

/// Section 4.3, path 2, with another attempt or none in the entry: the view is
/// not the one the entry means, and stays under the home region. Here the home
/// region had applied only the first connection's join, so the view is carried
/// by its kept join; the part's hello does not name the player, and the part's
/// `Present` for that stay is of a stay the edge does not have.
#[tokio::test]
async fn a_split_off_with_another_attempt_leaves_the_view_under_the_home_region() {
    for carries_the_earlier_attempt in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, stale) = edge.come_again(1).await;
        let named = carries_the_earlier_attempt.then_some(stale);
        let said = split_before_spawned_was_read(&mut edge, join - 2, named).await;
        let sent = numbered(&said);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], (join - 1, left_unentered(1, stale)));
        assert!(
            matches!(&sent[1], (again, EdgeToWorker::PlayerJoin(_)) if *again == join),
            "{sent:?}"
        );
        waits(&mut client);

        edge.link(PART).await;
        let hello = edge.hello(PART);
        assert!(hello.players.is_empty(), "{hello:?}");
        let mut part = Reply::unknown(40).stay(1, 5, 0);
        if carries_the_earlier_attempt {
            part = part.of_join(1, stale);
        }
        edge.answer(PART, part).await;
        let said = edge.sent(PART).await;
        assert_eq!(numbered(&said), [(1, left(1, Some(5)))]);
        waits(&mut client);

        let placed = spawned(client.session.0, player(1), EntityId(6));
        edge.says(WEST, placed).await;
        edge.sent(WEST).await;
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
        edge.end().await;
    }
}

/// Section 4.3, path 2 as it was before this record, which a stale entry still
/// meets: the entry does not move the view, the home region had applied the
/// join and answers `Absent`, nothing is kept, and the player is put out. Their
/// leave names the attempt of their join (section 4.4).
#[tokio::test]
async fn a_view_that_a_split_off_does_not_mean_is_put_out_by_the_absent_when_nothing_is_kept() {
    for carries_the_earlier_attempt in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, stale) = edge.come_again(1).await;
        let attempt = client.session.0;
        let named = carries_the_earlier_attempt.then_some(stale);
        edge.lose(WEST).await;
        edge.link(WEST).await;
        let split = Reply::resumed().applied(join);
        edge.answer(WEST, split.entry(split_with(PART, 1, 5, named)))
            .await;
        client.disconnected().await;
        let said = edge.sent(WEST).await;
        assert_eq!(numbered(&said), [(join + 1, left_unentered(1, attempt))]);
        assert_eq!(times_entered(&mut client), 0);
        edge.end().await;
    }
}

/// Section 4.3, path 3, and row 22 of section 10. The home region placed the
/// player far from home and let them go, or let the stay go without placing it;
/// the link was lost before anything of it was read. The `Departed` has the
/// view's attempt. In order: the region the stay goes to is asked for the view
/// around the transfer's chunk, then sent the arrival with the transfer as it
/// came, and the client is put into the world. The split region's `Absent`,
/// which follows the entries, is passed over.
#[tokio::test]
async fn a_departed_with_the_views_attempt_asks_for_the_view_then_sends_the_arrival_and_enters() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    let stay = let_go(5, EASTERN, Some(client.session.0));
    edge.lose(WEST).await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let reply = Reply::resumed().applied(join);
    edge.answer(WEST, reply.entry(gone_to(1, &stay, EAST)))
        .await;

    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(EASTERN));
    assert_eq!(numbered(&said), [(1, arrives(1, &stay))]);
    assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    // The view saw nothing under the home region, so nothing is left there.
    assert!(edge.at(WEST).subscriptions.is_empty());
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
    edge.end().await;
}

/// Section 4.3, the last row of path 3, and row 16 of section 10: a `Departed`
/// with another attempt or none, for a view without an entity. It is the stay
/// of an earlier connection: `Discard` where it was sent, and nothing else. The
/// view waits for the answer to its own join.
#[tokio::test]
async fn a_departed_with_another_attempt_is_discarded_and_the_view_waits() {
    for carries_the_earlier_attempt in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, stale) = edge.come_again(1).await;
        let attempt = carries_the_earlier_attempt.then_some(stale);
        let stay = let_go(5, EASTERN, attempt);
        edge.lose(WEST).await;
        edge.link(WEST).await;
        // The west had applied the first connection's join and nothing after it.
        let reply = Reply::resumed().applied(join - 2);
        edge.answer(WEST, reply.entry(gone_to(1, &stay, EAST)))
            .await;

        let said = edge.sent(EAST).await;
        let discard = EdgeToWorker::Discard {
            player: player(1),
            entity: EntityId(5),
            hops: 1,
            chunk: EASTERN,
        };
        assert_eq!(numbered(&said), [(1, discard)]);
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "{asked:?}");
        let sent = numbered(&edge.sent(WEST).await);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], (join - 1, left_unentered(1, stale)));
        assert!(
            matches!(&sent[1], (again, EdgeToWorker::PlayerJoin(_)) if *again == join),
            "{sent:?}"
        );
        waits(&mut client);

        let placed = spawned(client.session.0, player(1), EntityId(6));
        edge.says(WEST, placed).await;
        edge.sent(WEST).await;
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
        edge.end().await;
    }
}

/// Section 4.3, by the name the record gives it. The client's queue is closed
/// when the `Departed` is read. The arrival is sent before the client is told
/// anything, so the leave that the first packet's failure brings is numbered
/// behind it: the region takes the stay in and then ends it. The other way round
/// it would apply the leave to nobody and keep an entity nobody is behind.
#[tokio::test]
async fn a_client_that_is_gone_when_its_stay_is_let_go_to_it_unplaced_leaves_an_arrival_followed_by_a_leave_in_that_order()
 {
    let mut edge = Harness::witnessed().await;
    let (client, _, _) = edge.come_again(1).await;
    let stay = let_go(5, EASTERN, Some(client.session.0));
    // The connection is gone, and the edge has not been told so.
    drop(client);
    edge.say(WEST, gone_to(1, &stay, EAST));
    edge.settle(WEST).await;

    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(EASTERN));
    let expected = [(1, arrives(1, &stay)), (2, left(1, Some(5)))];
    assert_eq!(numbered(&said), expected);
    let sent = numbered(&edge.sent(WEST).await);
    assert!(sent.is_empty(), "{sent:?}");
    edge.quiet().await;
    edge.nobody_is_asked_for_anything();
    edge.end().await;
}

/// The same with a client whose queue is full: it takes one packet. It is
/// removed by the first packet that does not fit, which is after the arrival.
#[tokio::test]
async fn a_client_that_is_full_when_its_stay_is_let_go_to_it_unplaced_leaves_an_arrival_followed_by_a_leave()
 {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.join_with_room(player(1), 1).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    let stay = let_go(5, EASTERN, Some(client.session.0));
    edge.say(WEST, gone_to(1, &stay, EAST));
    edge.settle(WEST).await;

    let said = edge.sent(EAST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(EASTERN));
    let expected = [(1, arrives(1, &stay)), (2, left(1, Some(5)))];
    assert_eq!(numbered(&said), expected);
    client.disconnected().await;
    edge.quiet().await;
    edge.nobody_is_asked_for_anything();
    edge.end().await;
}

/// Section 4.3, path 3: "`to` ... may be `R` itself under the same condition as
/// at any hand-over: the region the stay was let go to has gone into `R` since".
/// The west let the stay go to the north and then absorbed it, which the edge
/// knows from the routing table when it reads the `Departed`. That is no fault
/// of the west's: the player arrives at the west and is entered.
#[tokio::test]
async fn a_stay_let_go_unplaced_to_a_region_that_has_gone_into_the_one_that_says_so_arrives_there()
{
    let mut edge = Harness::witnessed().await;
    edge.pairs(vec![(NORTH, WEST)]).await;
    edge.ended_by_the_edge(NORTH).await;
    let (mut client, join, _) = edge.come_again(1).await;
    let stay = let_go(5, NORTHERN, Some(client.session.0));
    edge.say(WEST, gone_to(1, &stay, NORTH));
    edge.settle(WEST).await;

    let said = edge.sent(WEST).await;
    assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
    assert_eq!(numbered(&said), [(join + 1, arrives(1, &stay))]);
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(NORTHERN));
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Section 6, "The edge, on `Ended`": it applies to the view whose entity it
/// names, whether or not the stay still carried its attempt. The client is sent
/// the disconnect with the game's sentence as a translatable component, the view
/// is removed, with its leave to the view's region as ever, and the entry is
/// confirmed.
#[tokio::test]
async fn ended_for_the_views_entity_puts_the_player_out_with_the_sentence_of_a_second_login() {
    for (region, chunk) in [(WEST, HOME), (EAST, EASTERN)] {
        for with_the_attempt in [false, true] {
            let mut edge = Harness::witnessed().await;
            let mut client = edge.settler(1, 5, region, chunk).await;
            let before = edge.at(region).numbered;
            let attempt = with_the_attempt.then_some(client.session.0);
            let number = edge.say(region, ended(1, 5, attempt));
            client.disconnected().await;
            assert_eq!(client.disconnected, Some(duplicate_login()));
            let said = edge.sent(region).await;
            assert_eq!(numbered(&said), [(before + 1, left(1, Some(5)))]);
            assert_eq!(confirmed(&said), Some(number));
            edge.quiet().await;
            edge.nobody_is_asked_for_anything();
            edge.end().await;
        }
    }
}

/// Section 6: an `Ended` that names another entity than the view has is about a
/// connection the edge has ended itself, and is passed over, also when it
/// carries the session of the view, which counts only for a view without an
/// entity. The connection stays. The entry is confirmed all the same.
#[tokio::test]
async fn ended_for_another_entity_than_the_views_is_passed_over_and_confirmed() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let session = client.session.0;
    for (entity, attempt) in [(4, None), (4, Some(session)), (9, None)] {
        let number = edge.say(WEST, ended(1, entity, attempt));
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(confirmed(&said), Some(number));
        is_in_the_world(&mut client);
    }
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Section 6, and rows 13 and 14 of section 10: an `Ended` with the attempt of a
/// view that has no entity yet ends that view. Its leave names the attempt
/// (section 4.4).
#[tokio::test]
async fn ended_with_the_attempt_of_a_view_that_is_entering_puts_the_player_out() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, _) = edge.come_again(1).await;
    let attempt = client.session.0;
    let number = edge.say(WEST, ended(1, 5, Some(attempt)));
    client.disconnected().await;
    assert_eq!(client.disconnected, Some(duplicate_login()));
    assert_eq!(times_entered(&mut client), 0);
    let said = edge.sent(WEST).await;
    assert_eq!(numbered(&said), [(join + 1, left_unentered(1, attempt))]);
    assert_eq!(confirmed(&said), Some(number));
    edge.end().await;
}

/// Section 6: an `Ended` with another attempt, or with none, is not about a view
/// that is entering. It is passed over and confirmed, and the view goes on
/// waiting for the answer to its join.
#[tokio::test]
async fn ended_with_another_attempt_is_passed_over_by_a_view_that_is_entering() {
    let mut edge = Harness::witnessed().await;
    let (mut client, _, stale) = edge.come_again(1).await;
    for attempt in [Some(stale), None] {
        let number = edge.say(WEST, ended(1, 5, attempt));
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(confirmed(&said), Some(number));
        waits(&mut client);
    }
    let placed = spawned(client.session.0, player(1), EntityId(6));
    edge.says(WEST, placed).await;
    edge.sent(WEST).await;
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
    edge.end().await;
}

/// Section 6, rule 6, and row 1 of section 10 for an edge that watches. The west
/// has shown the older entity of a player; the east then introduces a newer one.
/// The older is taken off the screen before the newer is shown. The older then
/// walks on for a tick, as the row has it, and is even introduced anew: neither
/// brings it back, and the newer stays.
#[tokio::test]
async fn the_older_entity_of_a_player_that_moves_after_the_newer_was_shown_does_not_come_back() {
    let mut edge = Harness::witnessed().await;
    let mut watcher = edge.settler(2, 6, WEST, HOME).await;
    edge.tell(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), EAST));
    edge.quiet().await;
    let older = vec![own(1, 5, HOME)];
    let home = snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), older);
    edge.says(WEST, home).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&5), "{:?}", watcher.entities);

    let newer = vec![own(1, 8, COMMON)];
    let common = snapshot_of(COMMON, edge.ask(EAST, COMMON), empty_chunk(), newer);
    edge.says(EAST, common).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&8), "{:?}", watcher.entities);
    assert!(!watcher.entities.contains(&5), "{:?}", watcher.entities);

    edge.says(WEST, walked(EntityId(5), HOME, STEP_EAST)).await;
    edge.sync(&mut watcher).await;
    assert!(!watcher.entities.contains(&5), "the older entity is back");
    assert!(watcher.entities.contains(&8), "the newer entity is hidden");
    edge.says(WEST, walked(EntityId(5), STEP_EAST, HOME)).await;
    edge.says(WEST, introduced(1, 5, HOME)).await;
    edge.sync(&mut watcher).await;
    assert!(!watcher.entities.contains(&5), "the older entity is back");
    assert!(watcher.entities.contains(&8), "the newer entity is hidden");
    // The older stay's region reads its `Dead` and reports the entity removed.
    let removed = RegionEvent::EntityRemoved {
        entity: EntityId(5),
        chunk: HOME,
    };
    let delta = WorkerToEdge::TickDelta {
        tick: 3,
        events: vec![removed],
    };
    edge.says(WEST, delta).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&8), "the newer entity is hidden");
    never_both_and_never_the_older_again(&watcher, 5, 8);
    edge.end().await;
}

/// Rule 6 with both entities in one region's word, as a home region has them
/// when a join removes the older stay (row 3 of section 10) and the edge reads
/// of the newer first.
#[tokio::test]
async fn the_older_entity_of_a_player_is_hidden_before_the_newer_is_shown() {
    let mut edge = Harness::witnessed().await;
    let mut watcher = edge.settler(2, 6, WEST, HOME).await;
    let older = vec![own(1, 5, HOME)];
    let home = snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), older);
    edge.says(WEST, home).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&5), "{:?}", watcher.entities);

    edge.says(WEST, introduced(1, 8, HOME)).await;
    edge.says(WEST, walked(EntityId(5), HOME, HOME)).await;
    edge.sync(&mut watcher).await;
    assert!(watcher.entities.contains(&8), "{:?}", watcher.entities);
    assert!(!watcher.entities.contains(&5), "{:?}", watcher.entities);
    never_both_and_never_the_older_again(&watcher, 5, 8);
    edge.end().await;
}

/// Rule 6: the highest entity the edge has been told of is also one it was told
/// by entering one of its own players. An introduction of an older entity of
/// that player, which another edge's connection left somewhere, is passed over.
#[tokio::test]
async fn an_older_entity_of_a_player_the_edge_has_entered_is_never_shown() {
    let mut edge = Harness::witnessed().await;
    let mut watcher = edge.settler(2, 6, WEST, HOME).await;
    let _own = edge.settler(1, 8, WEST, HOME).await;
    older_is_introduced_and_not_shown(&mut edge, &mut watcher).await;
    edge.end().await;
}

/// The same for a player entered by a `Present` (section 4.3, path 1) and for
/// one entered by a `Departed` (path 3, whose first step notes "rule 6's map").
#[tokio::test]
async fn an_older_entity_of_a_player_entered_by_another_path_is_never_shown() {
    for by_departure in [false, true] {
        let mut edge = Harness::witnessed().await;
        let mut watcher = edge.settler(2, 6, WEST, HOME).await;
        let (mut client, join, _) = edge.come_again(1).await;
        let attempt = client.session.0;
        if by_departure {
            let stay = let_go(8, EASTERN, Some(attempt));
            edge.say(WEST, gone_to(1, &stay, EAST));
            edge.settle(WEST).await;
        } else {
            edge.link(WEST).await;
            // The hello names both; the answers come in its order.
            let reply = Reply::resumed().applied(join).stay(2, 6, 0);
            let reply = reply.stay(1, 8, 0).of_join(1, attempt);
            edge.answer(WEST, reply).await;
        }
        edge.quiet().await;
        is_in_the_world(&mut client);
        older_is_introduced_and_not_shown(&mut edge, &mut watcher).await;
        edge.end().await;
    }
}

/// The west shows an entity 5 of the first player in the chunk `HOME`, in a
/// snapshot, as newly come and as moving. The watcher, who sees that chunk, is
/// never shown it.
async fn older_is_introduced_and_not_shown(edge: &mut Harness, watcher: &mut Client) {
    let older = vec![own(1, 5, HOME)];
    let home = snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), older);
    edge.says(WEST, home).await;
    edge.says(WEST, introduced(1, 5, HOME)).await;
    edge.says(WEST, walked(EntityId(5), HOME, HOME)).await;
    edge.sync(watcher).await;
    assert!(!watcher.entities.contains(&5), "{:?}", watcher.told);
    assert!(
        !watcher.told.contains(&Told::Shown(5)),
        "{:?}",
        watcher.told
    );
}

/// Section 6, "The player list", by the name the record gives it. The player
/// logged in a second time through another edge. This edge is shown the winner's
/// entity first and reads `Ended` for its own view after that. A friend who
/// shows the higher entity keeps the player in the list: nothing would list
/// them again until the entity left the friend's view and came back.
#[tokio::test]
async fn a_friend_keeps_the_player_in_the_list_when_the_first_connection_is_ended_after_the_second_was_shown()
 {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let mut friend = edge.settler(2, 6, WEST, HOME).await;
    let there = vec![own(1, 5, HOME)];
    let home = snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there);
    edge.says(WEST, home).await;
    edge.sync(&mut friend).await;
    assert!(friend.entities.contains(&5), "{:?}", friend.entities);
    assert!(lists(&friend, 1), "{:?}", friend.told);

    edge.says(WEST, introduced(1, 8, HOME)).await;
    edge.sync(&mut friend).await;
    assert!(friend.entities.contains(&8), "{:?}", friend.entities);
    edge.say(WEST, ended(1, 5, None));
    first.disconnected().await;
    assert_eq!(first.disconnected, Some(duplicate_login()));
    edge.quiet().await;
    edge.sync(&mut friend).await;
    assert!(lists(&friend, 1), "{:?}", friend.told);
    assert!(friend.entities.contains(&8), "{:?}", friend.entities);
    assert!(!friend.entities.contains(&5), "{:?}", friend.entities);
    never_both_and_never_the_older_again(&friend, 5, 8);
    edge.end().await;
}

/// The other side of the same paragraph, which is as before: a friend who shows
/// no higher entity of the player loses them from the list when their view is
/// removed on `Ended`.
#[tokio::test]
async fn a_friend_who_shows_no_later_stay_loses_the_player_from_the_list_when_the_connection_is_ended()
 {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let mut friend = edge.settler(2, 6, WEST, HOME).await;
    edge.sync(&mut friend).await;
    assert!(lists(&friend, 1), "{:?}", friend.told);
    edge.say(WEST, ended(1, 5, None));
    first.disconnected().await;
    edge.quiet().await;
    edge.sync(&mut friend).await;
    assert!(!lists(&friend, 1), "{:?}", friend.told);
    edge.end().await;
}

/// Section 6, the end of "The player list": "on one edge nothing changes: step 1
/// of section 4 removes the old view before the join is sent". A friend loses
/// the player from the list with the old view and is given the entry again when
/// the entity of the second login comes into view; the old entity, which its
/// region may not have reported removed yet, is hidden for it by rule 6.
#[tokio::test]
async fn a_friend_lists_the_player_again_when_the_stay_of_a_second_login_on_the_edge_comes_into_view()
 {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let mut friend = edge.settler(2, 6, WEST, HOME).await;
    let there = vec![own(1, 5, HOME)];
    let home = snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there);
    edge.says(WEST, home).await;
    edge.sync(&mut friend).await;
    assert!(friend.entities.contains(&5), "{:?}", friend.entities);

    let mut second = edge.join(player(1)).await;
    first.disconnected().await;
    edge.drained().await;
    let placed = spawned(second.session.0, player(1), EntityId(8));
    edge.says(WEST, placed).await;
    edge.says(WEST, introduced(1, 8, HOME)).await;
    edge.sync(&mut friend).await;
    assert!(lists(&friend, 1), "{:?}", friend.told);
    assert!(friend.entities.contains(&8), "{:?}", friend.entities);
    assert!(!friend.entities.contains(&5), "{:?}", friend.entities);
    never_both_and_never_the_older_again(&friend, 5, 8);
    is_in_the_world(&mut second);
    edge.end().await;
}

/// Found by the ledger's bots under load. A player logs in again just after being
/// handed over: the edge hides the old entity at once, and the region it was
/// handed to takes the arrival in only afterwards and reports the old entity come,
/// then removed, before the edge has heard of the new stay. A friend who watched
/// is never shown the old entity again, neither as come nor as gone.
#[tokio::test]
async fn a_stay_the_edge_ended_is_not_shown_again_by_a_late_report_of_its_arrival() {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let mut friend = edge.settler(2, 6, WEST, HOME).await;
    let there = vec![own(1, 5, HOME)];
    let home = snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there);
    edge.says(WEST, home).await;
    edge.sync(&mut friend).await;
    assert!(friend.entities.contains(&5), "{:?}", friend.entities);

    let mut second = edge.join(player(1)).await;
    first.disconnected().await;
    edge.drained().await;
    edge.sync(&mut friend).await;
    assert!(!friend.entities.contains(&5), "{:?}", friend.entities);
    let told = friend.told.len();

    edge.says(WEST, introduced(1, 5, HOME)).await;
    edge.says(WEST, walked(EntityId(5), HOME, HOME)).await;
    let removed = RegionEvent::EntityRemoved {
        entity: EntityId(5),
        chunk: HOME,
    };
    let delta = WorkerToEdge::TickDelta {
        tick: 3,
        events: vec![removed],
    };
    edge.says(WEST, delta).await;
    edge.sync(&mut friend).await;
    let since: Vec<_> = friend.told[told..]
        .iter()
        .filter(|told| matches!(told, Told::Shown(_) | Told::Hidden(_)))
        .collect();
    assert!(
        since.is_empty(),
        "the old entity was told of again: {since:?}"
    );

    let placed = spawned(second.session.0, player(1), EntityId(8));
    edge.says(WEST, placed).await;
    edge.says(WEST, introduced(1, 8, HOME)).await;
    edge.sync(&mut friend).await;
    assert!(friend.entities.contains(&8), "{:?}", friend.entities);
    never_both_and_never_the_older_again(&friend, 5, 8);
    is_in_the_world(&mut second);
    edge.end().await;
}

/// Section 4.2: a `Refused` names the attempt of the join it answers, and puts
/// out only a view without an entity that has that session. One of an earlier
/// connection leaves the view, and is confirmed; the refusal of the view's own
/// join puts it out.
#[tokio::test]
async fn a_refusal_of_an_earlier_attempt_leaves_the_view_and_one_of_its_own_puts_it_out() {
    let mut edge = Harness::witnessed().await;
    let (mut client, _, stale) = edge.come_again(1).await;
    let earlier = Durable::Refused {
        player: player(1),
        attempt: stale,
    };
    let number = edge.say(WEST, earlier);
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(confirmed(&said), Some(number));
    waits(&mut client);

    let its_own = Durable::Refused {
        player: player(1),
        attempt: client.session.0,
    };
    edge.say(WEST, its_own);
    client.disconnected().await;
    assert_eq!(times_entered(&mut client), 0);
    edge.end().await;
}

/// Section 4.2: a view that has its entity is not put out by a `Refused`, also
/// not by one that names its session.
#[tokio::test]
async fn a_refusal_does_not_put_out_a_view_that_has_its_entity() {
    let mut edge = Harness::witnessed().await;
    let mut client = edge.settler(1, 5, WEST, HOME).await;
    let refused = Durable::Refused {
        player: player(1),
        attempt: client.session.0,
    };
    let number = edge.say(WEST, refused);
    let said = edge.sent(WEST).await;
    assert_eq!(confirmed(&said), Some(number));
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
    edge.end().await;
}

/// Section 4.3, the last of "What makes each safe", and the row of paths 1 and
/// 1a ("it is an arm of the match and falls through"), by the name the record
/// gives it. A `SplitOff` put the view under a part, which another region
/// absorbed before the edge had a link to it. The survivor's `Absorbed` brings
/// the view, its `Present` with the view's attempt enters the player, and the
/// player is not judged absent when the presence is through.
#[tokio::test]
async fn a_view_without_an_entity_that_a_merge_brought_is_entered_by_the_survivors_answer_and_stays()
 {
    for survivor in [EAST, WEST] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, _) = edge.come_again(1).await;
        let attempt = client.session.0;
        split_before_spawned_was_read(&mut edge, join, Some(attempt)).await;
        waits(&mut client);

        edge.lose(survivor).await;
        edge.link(survivor).await;
        // The view is the part's: the hello does not name the player.
        assert!(edge.hello(survivor).players.is_empty());
        let mut merged = Reply::resumed();
        if survivor == WEST {
            merged = merged.applied(join);
        }
        let merged = merged.entry(absorbed(PART, 40, 0, &[])).stay(1, 5, 0);
        edge.answer(survivor, merged.of_join(1, attempt)).await;
        let said = edge.sent(survivor).await;
        let sent = numbered(&said);
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(edge.at(survivor).chunks(Role::Viewer), view(HOME));
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (survivor, EntityId(5), 1));
        is_in_the_world(&mut client);
        edge.offers_in_vain(PART).await;
        edge.end().await;
    }
}

/// The same with the split region's welcome and the survivor's in every order
/// of their messages: the player is entered once, at the survivor, and stays.
#[tokio::test]
async fn a_view_split_off_into_a_part_that_was_absorbed_is_entered_at_the_survivor_in_any_order() {
    let mut orders = Orders::new();
    while orders.another() {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, _) = edge.come_again(1).await;
        let attempt = client.session.0;
        edge.lose(WEST).await;
        edge.lose(EAST).await;
        let split = Reply::resumed().applied(join);
        let split = split.entry(split_with(PART, 1, 5, Some(attempt)));
        let merged = Reply::resumed()
            .entry(absorbed(PART, 40, 0, &[]))
            .stay(1, 5, 0)
            .of_join(1, attempt);
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, split)],
            vec![Step::Link(EAST), Step::Welcome(EAST, merged)],
        ];
        edge.play(scripts, &mut orders).await;
        for region in [WEST, EAST] {
            let sent = numbered(&edge.sent(region).await);
            assert!(sent.is_empty(), "{region:?} was sent {sent:?}");
        }
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.offers_in_vain(PART).await;
        edge.end().await;
    }
}

/// Section 4, step 1, and R1.4's "first step that changes what a client is
/// sent": a second connection of a player on one edge puts the first out with
/// the game's sentence, and the first stay's leave, which names its entity, is
/// sent to its region before the new join. What the first connection's task
/// says when it ends does not touch the second.
#[tokio::test]
async fn a_second_login_on_the_edge_puts_the_first_out_and_sends_its_leave_before_the_join() {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, WEST, HOME).await;
    let before = edge.at(WEST).numbered;
    let mut second = edge.join(player(1)).await;
    first.disconnected().await;
    assert_eq!(first.disconnected, Some(duplicate_login()));
    edge.drained().await;
    let sent = numbered(&edge.sent(WEST).await);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0], (before + 1, left(1, Some(5))));
    let session = second.session.0;
    assert!(
        matches!(
            &sent[1],
            (number, EdgeToWorker::PlayerJoin(join))
                if *number == before + 2 && join.player == player(1) && join.attempt == session
        ),
        "{sent:?}"
    );
    assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
    waits(&mut second);

    // The first connection's task ends and says so.
    edge.leave(&first).await;
    let sent = numbered(&edge.sent(WEST).await);
    assert!(sent.is_empty(), "{sent:?}");
    waits(&mut second);
    edge.says(WEST, spawned(session, player(1), EntityId(8)))
        .await;
    edge.sent(WEST).await;
    is_in_the_world(&mut second);
    assert_eq!(edge.acts(&second).await, (WEST, EntityId(8), 1));
    edge.end().await;
}

/// Step 1 with a first connection whose stay is another region's: the leave
/// goes there, the join to the home region.
#[tokio::test]
async fn a_second_login_ends_the_first_stay_at_the_region_it_is_in() {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.settler(1, 5, EAST, EASTERN).await;
    let (at_the_east, at_the_west) = (edge.at(EAST).numbered, edge.at(WEST).numbered);
    let mut second = edge.join(player(1)).await;
    first.disconnected().await;
    assert_eq!(first.disconnected, Some(duplicate_login()));
    edge.drained().await;
    let sent = numbered(&edge.sent(EAST).await);
    assert_eq!(sent, [(at_the_east + 1, left(1, Some(5)))]);
    let sent = numbered(&edge.sent(WEST).await);
    let session = second.session.0;
    assert!(
        matches!(
            &sent[..],
            [(number, EdgeToWorker::PlayerJoin(join))]
                if *number == at_the_west + 1 && join.attempt == session
        ),
        "{sent:?}"
    );
    waits(&mut second);
    edge.end().await;
}

/// Step 1 with a first connection that was not entered yet: its leave names its
/// attempt, and is sent before the new join. The answer to the first join is
/// then stale and the answer to the second enters the second connection.
#[tokio::test]
async fn a_second_login_puts_out_a_first_that_was_entering_with_a_leave_that_names_its_attempt() {
    let mut edge = Harness::witnessed().await;
    let mut first = edge.join(player(1)).await;
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(
        matches!(&join, EdgeToWorker::PlayerJoin(join) if join.attempt == first.session.0),
        "{join:?}"
    );
    let mut second = edge.join(player(1)).await;
    first.disconnected().await;
    assert_eq!(first.disconnected, Some(duplicate_login()));
    assert_eq!(times_entered(&mut first), 0);
    edge.drained().await;
    let sent = numbered(&edge.sent(WEST).await);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0], (number + 1, left_unentered(1, first.session.0)));
    let session = second.session.0;
    assert!(
        matches!(
            &sent[1],
            (again, EdgeToWorker::PlayerJoin(join))
                if *again == number + 2 && join.attempt == session
        ),
        "{sent:?}"
    );

    edge.says(WEST, spawned(first.session.0, player(1), EntityId(5)))
        .await;
    edge.sent(WEST).await;
    waits(&mut second);
    edge.says(WEST, spawned(session, player(1), EntityId(6)))
        .await;
    edge.sent(WEST).await;
    is_in_the_world(&mut second);
    assert_eq!(edge.acts(&second).await, (WEST, EntityId(6), 1));
    edge.end().await;
}

/// The home region answers a hello that named one player, having applied
/// everything up to `applied`, with `answer` for that player.
async fn answers_for_the_one_named(edge: &mut Harness, applied: u64, answer: Presence) {
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let welcome = Welcome::Resumed {
        entries: 0,
        presences: 1,
        applied,
    };
    edge.says(WEST, WorkerToEdge::Welcome(welcome)).await;
    let presence = WorkerToEdge::Presence {
        player: player(1),
        answer,
    };
    edge.says(WEST, presence).await;
}

/// Section 4.2, and row 18 of section 10. The link to the home region is lost
/// between a player's join and their entering, and the region had applied the
/// join: nothing is kept. `Entering` with the view's attempt has the connection
/// go on waiting, where `Absent` puts it out with a leave that names the
/// attempt. The region's `Spawned` then enters the player.
#[tokio::test]
async fn a_connection_whose_join_the_region_holds_as_entering_goes_on_waiting_where_absent_puts_it_out()
 {
    for entering in [true, false] {
        let mut edge = Harness::witnessed().await;
        let (mut client, join, _) = edge.come_again(1).await;
        let attempt = client.session.0;
        edge.lose(WEST).await;
        edge.link(WEST).await;
        if !entering {
            answers_for_the_one_named(&mut edge, join, Presence::Absent).await;
            client.disconnected().await;
            let sent = numbered(&edge.sent(WEST).await);
            assert_eq!(sent, [(join + 1, left_unentered(1, attempt))]);
            edge.end().await;
            continue;
        }
        answers_for_the_one_named(&mut edge, join, Presence::Entering { attempt }).await;
        let said = edge.sent(WEST).await;
        assert!(said.is_empty(), "{said:?}");
        waits(&mut client);
        edge.nobody_is_asked_for_anything();

        edge.says(WEST, spawned(attempt, player(1), EntityId(5)))
            .await;
        edge.sent(WEST).await;
        is_in_the_world(&mut client);
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
        edge.end().await;
    }
}

/// Section 4.2: "in every other case it does nothing". The region holds the
/// stay of an earlier connection's join as entering: it had applied that join
/// and neither the leave nor the join after it. Nothing is concluded from the
/// answer; the leave and the join are sent again, and the view waits.
#[tokio::test]
async fn an_entering_with_the_attempt_of_an_earlier_connection_changes_nothing() {
    let mut edge = Harness::witnessed().await;
    let (mut client, join, stale) = edge.come_again(1).await;
    edge.lose(WEST).await;
    edge.link(WEST).await;
    let answer = Presence::Entering { attempt: stale };
    answers_for_the_one_named(&mut edge, join - 2, answer).await;
    let said = edge.sent(WEST).await;
    let sent = numbered(&said);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0], (join - 1, left_unentered(1, stale)));
    assert!(
        matches!(&sent[1], (again, EdgeToWorker::PlayerJoin(_)) if *again == join),
        "{sent:?}"
    );
    waits(&mut client);

    let placed = spawned(client.session.0, player(1), EntityId(6));
    edge.says(WEST, placed).await;
    edge.sent(WEST).await;
    is_in_the_world(&mut client);
    assert_eq!(edge.acts(&client).await, (WEST, EntityId(6), 1));
    edge.end().await;
}

/// Section 4.4: the leave of a view without an entity names the attempt of its
/// join and no entity; the leave of a view that has its entity names the entity
/// and no attempt.
#[tokio::test]
async fn a_leave_names_the_attempt_for_a_view_without_an_entity_and_the_entity_otherwise() {
    let mut edge = Harness::witnessed().await;
    let unentered = edge.join(player(1)).await;
    let (number, join) = edge.next_numbered(WEST).await;
    assert!(
        matches!(&join, EdgeToWorker::PlayerJoin(join) if join.attempt == unentered.session.0),
        "{join:?}"
    );
    edge.leave(&unentered).await;
    let sent = numbered(&edge.sent(WEST).await);
    let expected = EdgeToWorker::PlayerLeave {
        player: player(1),
        entity: None,
        attempt: Some(unentered.session.0),
    };
    assert_eq!(sent, [(number + 1, expected)]);

    let entered = edge.settler(2, 6, WEST, HOME).await;
    let before = edge.at(WEST).numbered;
    edge.leave(&entered).await;
    let sent = numbered(&edge.sent(WEST).await);
    let expected = EdgeToWorker::PlayerLeave {
        player: player(2),
        entity: Some(EntityId(6)),
        attempt: None,
    };
    assert_eq!(sent, [(before + 1, expected)]);
    edge.end().await;
}

/// Section 4.4: since path 2 a view without an entity can be under another
/// region than the home region, and its leave goes there, with its attempt.
#[tokio::test]
async fn the_leave_of_a_view_that_a_split_off_put_under_a_part_goes_to_the_part_with_its_attempt() {
    let mut edge = Harness::witnessed().await;
    let (client, join, _) = edge.come_again(1).await;
    let attempt = client.session.0;
    split_before_spawned_was_read(&mut edge, join, Some(attempt)).await;
    edge.leave(&client).await;
    let sent = numbered(&edge.sent(WEST).await);
    assert!(sent.is_empty(), "{sent:?}");

    edge.link(PART).await;
    assert!(edge.hello(PART).players.is_empty());
    edge.answer(PART, Reply::unknown(40)).await;
    let sent = numbered(&edge.sent(PART).await);
    assert_eq!(sent, [(1, left_unentered(1, attempt))]);
    edge.end().await;
}

/// Section 4.4, its order of events on one edge. A first connection is placed
/// far from home, the link is lost, the home region is split and the view goes
/// under the part, which has no link; the client goes, and its leave is kept
/// for the part; the player joins again; the home region absorbs the part, and
/// what was kept for the part is put behind what is kept for the survivor. The
/// leave that reaches the home region behind the join names the first
/// connection's attempt, so that it ends nothing but its own stay, and the
/// second connection is entered by the stay of its own join.
#[tokio::test]
async fn a_leave_kept_for_a_part_that_is_absorbed_names_the_attempt_of_the_connection_that_went() {
    let mut edge = Harness::witnessed().await;
    let first = edge.join(player(1)).await;
    let (join, body) = edge.next_numbered(WEST).await;
    assert!(matches!(body, EdgeToWorker::PlayerJoin(_)), "{body:?}");
    let gone = first.session.0;
    split_before_spawned_was_read(&mut edge, join, Some(gone)).await;
    edge.leave(&first).await;

    let mut second = edge.join(player(1)).await;
    edge.drained().await;
    let sent = numbered(&edge.sent(WEST).await);
    let session = second.session.0;
    assert!(
        matches!(
            &sent[..],
            [(number, EdgeToWorker::PlayerJoin(again))]
                if *number == join + 1 && again.attempt == session
        ),
        "{sent:?}"
    );

    // The west absorbs the part. It has applied the second join and placed the
    // player; the older stay, which came back with the part, went by rule 7.
    edge.lose(WEST).await;
    edge.link(WEST).await;
    assert_eq!(edge.hello(WEST).players, [player(1)]);
    let merged = Reply::resumed()
        .applied(join + 1)
        .entry(absorbed(PART, 40, 0, &[]))
        .stay(1, 8, 0)
        .of_join(1, session);
    edge.answer(WEST, merged).await;
    let sent = numbered(&edge.sent(WEST).await);
    assert_eq!(sent, [(join + 2, left_unentered(1, gone))]);
    is_in_the_world(&mut second);
    assert_eq!(edge.acts(&second).await, (WEST, EntityId(8), 1));
    edge.end().await;
}

// A chunk that is sent again is unloaded first: ADR-0023, section 6. Written by
// whoever built it; the orders of events around light are tested from the record
// by someone else (its step L6).

/// A client that has a chunk and is to be sent it anew, because a snapshot shows
/// other blocks than the edge had, is sent an unload immediately before the
/// batch that carries the new one. What it holds of a chunk is then always what
/// one packet and the updates after it said.
#[tokio::test]
async fn a_chunk_that_is_sent_again_is_unloaded_immediately_before_it() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    client.of_chunks.clear();

    // The same blocks again: nothing is sent.
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    assert_eq!(client.of_chunks, []);

    let other = snapshot_of(HOME, ask, chunk_with(HOME, STONE), Vec::new());
    edge.tell(WEST, other);
    assert!(edge.shows(&mut client, WEST, HOME).await);
    assert_eq!(
        client.of_chunks,
        [OfChunk::Unloaded(HOME), OfChunk::Sent(HOME)]
    );
    assert_eq!(client.state(HOME), Some(STONE));
    edge.end().await;
}

/// A tick in which a player places a block and digs it again ends with the block
/// gone, and the region shows the player the block it read after the placing only
/// after the changes of that tick. The player is shown the block as the tick left
/// it, not as it was between the two.
#[tokio::test]
async fn a_block_shown_after_a_use_is_shown_as_the_tick_left_it() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);

    edge.tell(WEST, changed(HOME, AIR));
    let shown = PlayerEvent::BlockShown {
        position: block_of(HOME),
        state: STONE,
    };
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: shown,
        },
    );
    edge.settle(WEST).await;
    client.drain();
    assert_eq!(client.state(HOME), Some(AIR));
    edge.end().await;
}

/// While the new chunk cannot go out yet, the client goes on showing the one it
/// has and is told of blocks that change in it; a second snapshot before then
/// leaves one unload to be sent, not two; and the chunk that goes out in the end
/// is the last one.
#[tokio::test]
async fn a_chunk_that_waits_to_be_sent_again_is_shown_as_it_was_with_its_changes_meanwhile() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);

    // A batch the client does not confirm keeps everything else waiting.
    let beside = edge.ask(WEST, STEP_EAST);
    edge.tell(WEST, snapshot(STEP_EAST, beside));
    edge.settle(WEST).await;
    client.drain();
    assert_eq!(client.batches, 1);
    client.of_chunks.clear();

    edge.tell(
        WEST,
        snapshot_of(HOME, ask, chunk_with(HOME, STONE), Vec::new()),
    );
    edge.tell(WEST, changed(HOME, GRANITE));
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, chunk_with(HOME, STONE), Vec::new()),
    );
    edge.tell(WEST, changed(HOME, AIR));
    edge.tell(WEST, changed(HOME, GRANITE));
    edge.settle(WEST).await;
    client.drain();
    // The chunk of before is still there, with the three changes said of it. (The
    // chunk beside it, which the client has and of which it is at the rim, has
    // its light sent as well.)
    client
        .of_chunks
        .retain(|told| !matches!(told, OfChunk::Light(_)));
    assert_eq!(client.of_chunks, [OfChunk::Block(HOME); 3]);
    assert!(client.chunks.contains_key(&HOME));

    edge.sync(&mut client).await;
    assert_eq!(
        client.of_chunks[3..],
        [OfChunk::Unloaded(HOME), OfChunk::Sent(HOME)]
    );
    assert_eq!(client.state(HOME), Some(GRANITE));
    edge.end().await;
}

/// A view that moves away from a chunk that waits to be sent again unloads it, as
/// it unloads a chunk the client was sent: the client has it. Nothing of the
/// chunk follows.
#[tokio::test]
async fn a_view_that_moves_away_unloads_a_chunk_that_waited_to_be_sent_again() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    let beside = edge.ask(WEST, STEP_EAST);
    edge.tell(WEST, snapshot(STEP_EAST, beside));
    edge.settle(WEST).await;
    client.drain();
    assert_eq!(client.batches, 1);
    edge.tell(
        WEST,
        snapshot_of(HOME, ask, chunk_with(HOME, STONE), Vec::new()),
    );
    edge.settle(WEST).await;
    client.of_chunks.clear();

    // The region says the player has gone far enough that neither chunk is in
    // view any more.
    edge.tell(WEST, walked(EntityId(5), HOME, ChunkPos::new(40, 0)));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let of_home = |told: &&OfChunk| {
        matches!(
            told,
            OfChunk::Unloaded(HOME) | OfChunk::Sent(HOME) | OfChunk::Block(HOME)
        )
    };
    let of_home: Vec<_> = client.of_chunks.iter().filter(of_home).collect();
    assert_eq!(of_home, [&OfChunk::Unloaded(HOME)]);
    assert!(!client.chunks.contains_key(&HOME));
    edge.end().await;
}

// Light at the edge: ADR-0023, the part of its step L4 that makes light right.
// Written by whoever built it; the orders of events are tested from the record by
// someone else (its step L6).

/// The whole of `light` as a client reads it from a packet: a mask that names
/// nothing is read as one without words.
fn as_read(light: &Light) -> LightData {
    let mut data = crate::encode::light_data(&light.sky, &light.block, crate::encode::Named::ALL);
    let masks = [
        &mut data.sky_mask,
        &mut data.block_mask,
        &mut data.empty_sky_mask,
        &mut data.empty_block_mask,
    ];
    for mask in masks {
        while mask.last() == Some(&0) {
            mask.pop();
        }
    }
    data
}

/// What a client is sent as the light of `chunk` when it stands alone in the
/// replica and the edge's own rules lit it.
fn computed_alone(chunk: &Chunk) -> LightData {
    as_read(&clustine_light::light(&clustine_light::Around {
        centre: chunk,
        neighbours: [None; 8],
        sky: true,
    }))
}

/// What a client is sent as the light a chunk carries.
fn carried(chunk: &Chunk) -> LightData {
    as_read(chunk.light().expect("the tests' chunks carry light"))
}

/// A chunk that comes with light made beside chunks as its neighbours here are is
/// sent with that light, at once, and no light is made for it.
#[tokio::test]
async fn a_chunk_as_generated_is_sent_with_the_light_it_came_with() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    for chunk in [HOME, STEP_EAST] {
        let ask = edge.ask(WEST, chunk);
        edge.tell(WEST, snapshot(chunk, ask));
        assert!(edge.shows(&mut client, WEST, chunk).await);
        assert_eq!(client.light[&chunk], carried(&empty_chunk()));
    }
    edge.end().await;
}

/// A block that light sees changes in a chunk: whoever is sent the chunk from
/// then on is sent it with light made for the blocks as they are, and not with
/// the light the chunk came with.
#[tokio::test]
async fn a_chunk_that_was_changed_is_sent_with_light_made_for_its_blocks() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    edge.tell(WEST, changed(HOME, STONE));
    edge.settle(WEST).await;

    let mut second = edge.joined(player(2), EntityId(6)).await;
    assert!(edge.shows(&mut second, WEST, HOME).await);
    assert_eq!(second.state(HOME), Some(STONE));
    let with_stone = chunk_with(HOME, STONE);
    assert_eq!(second.light[&HOME], computed_alone(&with_stone));
    // The stone casts a shadow, which the light the chunk came with has not.
    assert_ne!(second.light[&HOME], carried(&empty_chunk()));
    edge.end().await;
}

/// A chunk that comes beside one a block has changed in: the light it came with
/// was made beside that neighbour as generated, which it no longer is, so the
/// edge makes its light from the two as they are.
#[tokio::test]
async fn a_chunk_beside_a_changed_one_is_sent_with_light_made_beside_it() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    edge.tell(WEST, changed(HOME, STONE));
    edge.settle(WEST).await;
    let beside = edge.ask(WEST, STEP_EAST);
    edge.tell(WEST, snapshot(STEP_EAST, beside));
    assert!(edge.shows(&mut client, WEST, STEP_EAST).await);

    let with_stone = chunk_with(HOME, STONE);
    let light = clustine_light::light(&clustine_light::Around {
        centre: &empty_chunk(),
        // West of it is the chunk with the stone.
        neighbours: [None, None, None, Some(&with_stone), None, None, None, None],
        sky: true,
    });
    assert_eq!(client.light[&STEP_EAST], as_read(&light));
    edge.end().await;
}

/// A chunk that comes without light, as one does that was changed and stored, is
/// sent with light the edge made.
#[tokio::test]
async fn a_chunk_that_comes_without_light_is_sent_with_light_the_edge_made() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    let mut bare = chunk_with(HOME, STONE);
    bare.take_light();
    edge.tell(WEST, snapshot_of(HOME, ask, bare.clone(), Vec::new()));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    assert_eq!(client.light[&HOME], computed_alone(&bare));
    edge.end().await;
}

// Light updates: ADR-0023, step L5, as decided after the comparison with the
// official server. Written by whoever built it.

/// The light sections a packet names: for the sky and for blocks, by the number
/// of the section, its levels, or nothing for one that is dark.
type Named = BTreeMap<(bool, usize), Option<Vec<u8>>>;

fn named(light: &LightData) -> Named {
    let mut named = Named::new();
    let lists = [
        (true, &light.sky_mask, &light.empty_sky_mask, &light.sky),
        (
            false,
            &light.block_mask,
            &light.empty_block_mask,
            &light.block,
        ),
    ];
    for (sky, mask, empty, arrays) in lists {
        let bit =
            |mask: &Vec<u64>, index: usize| mask.first().is_some_and(|word| word >> index & 1 == 1);
        let mut arrays = arrays.iter();
        for index in 0..SECTIONS + 2 {
            if bit(mask, index) {
                named.insert((sky, index), arrays.next().cloned());
            } else if bit(empty, index) {
                named.insert((sky, index), None);
            }
        }
    }
    named
}

/// The chunks beside `chunk`, sent to the client as empty ones.
async fn sent_around(edge: &mut Harness, client: &mut Client, chunk: ChunkPos) {
    let around = Fanout::around(LevelChunk::new(Dimension::Overworld, chunk));
    for beside in around.map(|at| at.position) {
        let ask = edge.ask(WEST, beside);
        edge.tell(WEST, snapshot(beside, ask));
        assert!(edge.shows(client, WEST, beside).await);
    }
}

/// A player at the rim of whose view a chunk is, who lacks a chunk beside it, is
/// sent the light a block change makes, behind the block update: the sections that
/// differ, which laid over what they had give the light of the blocks as they are.
#[tokio::test]
async fn a_block_change_sends_its_light_to_a_player_who_lacks_a_chunk_beside_it() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    client.of_chunks.clear();

    edge.tell(WEST, changed(HOME, STONE));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(
        client.of_chunks,
        [OfChunk::Block(HOME), OfChunk::Light(HOME)]
    );
    let (_, update) = &client.light_updates[0];
    let before = named(&client.light[&HOME]);
    let update = named(update);
    // Only what differs is named.
    assert!(
        update
            .iter()
            .all(|(section, levels)| before.get(section) != Some(levels))
    );
    let mut now = before;
    now.extend(update);
    assert_eq!(now, named(&computed_alone(&chunk_with(HOME, STONE))));
    edge.end().await;
}

/// A player who has every chunk beside a change is sent the block update and no
/// light for that chunk: their client lights the change by itself, as the official
/// server relies on for every player but those at the rim. The chunks around it
/// are at the rim of this player's view, and the change, which puts the first
/// block into a section, reaches them all: of those the player is sent the light.
#[tokio::test]
async fn a_block_change_sends_no_light_to_a_player_who_has_the_chunks_around_it() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    sent_around(&mut edge, &mut client, HOME).await;
    client.of_chunks.clear();

    edge.tell(WEST, changed(HOME, STONE));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.of_chunks[0], OfChunk::Block(HOME));
    let lit: BTreeSet<ChunkPos> = client.of_chunks[1..]
        .iter()
        .map(|told| match told {
            OfChunk::Light(chunk) => *chunk,
            other => panic!("{other:?}"),
        })
        .collect();
    let around = Fanout::around(LevelChunk::new(Dimension::Overworld, HOME));
    assert_eq!(lit, BTreeSet::from(around.map(|at| at.position)));
    edge.end().await;
}

/// A chunk beside one a player has is replaced by one that gives light across the
/// border. No client knows of that by itself, so the player is sent the light of
/// the chunk they have, although they have every chunk around it.
#[tokio::test]
async fn a_chunk_replaced_beside_one_a_player_has_sends_them_its_new_light() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot(HOME, ask));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    sent_around(&mut edge, &mut client, HOME).await;
    client.of_chunks.clear();

    // The chunk to the east, as a stored chunk comes: changed, and without light.
    // A lamp stands at its western edge.
    let mut lamp = empty_chunk();
    lamp.take_light();
    lamp.set(0, -60, 5, clustine_data::blocks::GLOWSTONE);
    let east = edge.ask(WEST, STEP_EAST);
    edge.tell(WEST, snapshot_of(STEP_EAST, east, lamp, Vec::new()));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let of = |chunk: ChunkPos| {
        let about = |told: &&OfChunk| matches!(told, OfChunk::Sent(at) | OfChunk::Unloaded(at) | OfChunk::Light(at) if *at == chunk);
        client
            .of_chunks
            .iter()
            .filter(about)
            .copied()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        of(STEP_EAST),
        [OfChunk::Unloaded(STEP_EAST), OfChunk::Sent(STEP_EAST)]
    );
    assert_eq!(of(HOME), [OfChunk::Light(HOME)]);
    // The lamp's light, in the section of the block beside it.
    let of_home = |(chunk, _): &&(ChunkPos, LightData)| *chunk == HOME;
    let (_, update) = client
        .light_updates
        .iter()
        .find(of_home)
        .expect("just counted");
    let block_light = named(update);
    let lit = block_light.get(&(false, 1)).cloned().flatten();
    let levels = lit.expect("block light with levels in the lowest section");
    let at = |x: usize, y: usize, z: usize| {
        let index = y << 8 | z << 4 | x;
        levels[index / 2] >> (index % 2 * 4) & 15
    };
    // One step west of the lamp, across the border, and then one less a step.
    assert_eq!((at(15, 4, 5), at(14, 4, 5), at(15, 5, 5)), (14, 13, 13));
    edge.end().await;
}

/// The server's limit of the view distance in the tests of a distance that
/// changes in play, and a shorter one a player asks for.
const LIMIT: i32 = 6;
const SHORT: i32 = 3;

/// What the client was told of chunks being sent or unloaded since `from`, in
/// order.
fn loads_since(client: &Client, from: usize) -> Vec<OfChunk> {
    let loads = client.of_chunks[from..].iter().copied();
    loads
        .filter(|told| matches!(told, OfChunk::Sent(_) | OfChunk::Unloaded(_)))
        .collect()
}

/// The player's view grows around the chunk they stand in: what came into it is
/// asked of their region and shown once served, with whoever is in it, and
/// nothing they had is taken away.
#[tokio::test]
async fn a_player_who_asks_for_a_longer_view_has_the_chunks_that_came_into_it_asked_for_and_sent() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let mut client = edge
        .joined_asking(player(1), EntityId(5), Some(SHORT))
        .await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, SHORT));
    let near = ChunkPos::new(1, 0);
    edge.tell(WEST, snapshot(near, edge.ask(WEST, near)));
    assert!(edge.shows(&mut client, WEST, near).await);
    edge.quiet().await;
    let (centred, told) = (client.centres.len(), client.of_chunks.len());

    edge.ask_view_distance(&client, client.session, 5).await;
    let asked = edge.asked(WEST).await;
    let grown = minus(&view_area(HOME, 5), &view_area(HOME, SHORT));
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Viewer), grown)])
    );
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, 5));
    for region in [EAST, NORTH] {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.sync(&mut client).await;
    assert_eq!(client.centres[centred..], [HOME]);
    assert_eq!(loads_since(&client, told), []);
    assert!(client.chunks.contains_key(&near));

    // A chunk that came into the view is sent once its region serves it, and
    // the stranger in it is shown.
    let new = ChunkPos::new(5, 0);
    assert!(!view_area(HOME, SHORT).contains(&new));
    let there = vec![stranger(EntityId(70), new)];
    let ask = edge.ask(WEST, new);
    edge.tell(WEST, snapshot_of(new, ask, empty_chunk(), there));
    assert!(edge.shows(&mut client, WEST, new).await);
    assert!(client.entities.contains(&70), "{:?}", client.entities);
    edge.end().await;
}

/// The player's view shrinks around the chunk they stand in: what left it is
/// unloaded at the client, with whoever was in it, and is no longer asked of the
/// region.
#[tokio::test]
async fn a_player_who_asks_for_a_shorter_view_has_the_chunks_outside_it_unloaded_and_given_up() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, LIMIT));
    let (near, far) = (ChunkPos::new(1, 0), ChunkPos::new(5, 0));
    assert!(!view_area(HOME, SHORT).contains(&far));
    for (chunk, entity) in [(near, 71), (far, 70)] {
        let there = vec![stranger(EntityId(entity), chunk)];
        let ask = edge.ask(WEST, chunk);
        edge.tell(WEST, snapshot_of(chunk, ask, empty_chunk(), there));
    }
    assert!(edge.shows(&mut client, WEST, far).await);
    assert!(client.chunks.contains_key(&near));
    assert_eq!(client.entities, BTreeSet::from([70, 71]));
    edge.quiet().await;
    let (centred, told) = (client.centres.len(), client.of_chunks.len());

    edge.ask_view_distance(&client, client.session, SHORT).await;
    let asked = edge.asked(WEST).await;
    let shrunk = minus(&view_area(HOME, LIMIT), &view_area(HOME, SHORT));
    assert_eq!(meanings(&asked), BTreeSet::from([(None, shrunk)]));
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, SHORT));
    edge.sync(&mut client).await;
    assert_eq!(client.centres[centred..], [HOME]);
    assert_eq!(loads_since(&client, told), [OfChunk::Unloaded(far)]);
    assert_eq!(client.chunks.keys().copied().collect::<Vec<_>>(), [near]);
    assert_eq!(client.entities, BTreeSet::from([71]));
    edge.end().await;
}

/// A chunk that leaves one player's view as they shrink it stays subscribed
/// where another player of the edge still sees it.
#[tokio::test]
async fn a_chunk_that_leaves_a_shrunk_view_stays_subscribed_while_another_player_sees_it() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let first = edge.joined(player(1), EntityId(5)).await;
    let _second = edge.joined(player(2), EntityId(6)).await;
    // The second walks a few chunks east, and sees further east than the first.
    let east = ChunkPos::new(4, 0);
    edge.tell(WEST, walked(EntityId(6), HOME, east));
    edge.settle(WEST).await;
    edge.quiet().await;
    let seen: BTreeSet<_> = view_area(HOME, LIMIT)
        .union(&view_area(east, LIMIT))
        .copied()
        .collect();
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), seen);

    edge.ask_view_distance(&first, first.session, SHORT).await;
    let asked = edge.asked(WEST).await;
    let left = minus(&view_area(HOME, LIMIT), &view_area(HOME, SHORT));
    let unseen = minus(&left, &view_area(east, LIMIT));
    assert!(!unseen.is_empty() && unseen.len() < left.len());
    assert_eq!(meanings(&asked), BTreeSet::from([(None, unseen)]));
    let kept = ChunkPos::new(5, 0);
    assert!(left.contains(&kept));
    assert!(!asked.iter().any(|body| names(body, kept)), "{asked:?}");
    let still: BTreeSet<_> = view_area(HOME, SHORT)
        .union(&view_area(east, LIMIT))
        .copied()
        .collect();
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), still);
    edge.end().await;
}

/// A distance above the server's limit is granted as the limit, as at the join.
#[tokio::test]
async fn a_view_distance_asked_for_above_the_servers_limit_is_granted_as_the_limit() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let client = edge
        .joined_asking(player(1), EntityId(5), Some(SHORT))
        .await;
    edge.quiet().await;
    edge.ask_view_distance(&client, client.session, 32).await;
    let asked = edge.asked(WEST).await;
    let grown = minus(&view_area(HOME, LIMIT), &view_area(HOME, SHORT));
    assert_eq!(
        meanings(&asked),
        BTreeSet::from([(Some(Role::Viewer), grown)])
    );
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, LIMIT));
    edge.end().await;
}

/// A distance below 2, which a client can send as any byte, is granted as 2, and
/// asking for less than 2 again then changes nothing.
#[tokio::test]
async fn a_view_distance_asked_for_below_two_is_granted_as_two() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.ask_view_distance(&client, client.session, 1).await;
    let asked = edge.asked(WEST).await;
    let shrunk = minus(&view_area(HOME, LIMIT), &view_area(HOME, 2));
    assert_eq!(meanings(&asked), BTreeSet::from([(None, shrunk)]));
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, 2));
    edge.sync(&mut client).await;
    let centred = client.centres.len();

    for requested in [0, -3] {
        edge.ask_view_distance(&client, client.session, requested)
            .await;
        let asked = edge.asked(WEST).await;
        assert!(asked.is_empty(), "{requested}: {asked:?}");
    }
    edge.sync(&mut client).await;
    assert_eq!(client.centres.len(), centred);
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, 2));
    edge.end().await;
}

/// Asking for the distance one has, or for one that is granted as it, is not a
/// change: nothing is sent to the client and nothing asked of any region.
#[tokio::test]
async fn asking_for_the_view_distance_one_has_changes_nothing() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let mut short = edge
        .joined_asking(player(1), EntityId(5), Some(SHORT))
        .await;
    let mut long = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    edge.sync(&mut short).await;
    edge.sync(&mut long).await;
    let (short_taken, long_taken) = (short.taken, long.taken);

    edge.ask_view_distance(&short, short.session, SHORT).await;
    edge.ask_view_distance(&long, long.session, LIMIT).await;
    edge.ask_view_distance(&long, long.session, LIMIT + 10)
        .await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    // The one packet each client takes is the acknowledgement `sync` asks for.
    edge.sync(&mut short).await;
    edge.sync(&mut long).await;
    assert_eq!(short.taken, short_taken + 1);
    assert_eq!(long.taken, long_taken + 1);
    edge.end().await;
}

/// A player who changes the distance while they wait to be placed has their
/// view laid out at the new distance when they are, and nothing before.
#[tokio::test]
async fn a_view_distance_asked_for_before_the_player_is_placed_is_the_one_their_view_has() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let mut client = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");

    edge.ask_view_distance(&client, client.session, SHORT).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    client.drain();
    assert!(client.centres.is_empty(), "{:?}", client.centres);
    assert!(client.of_chunks.is_empty(), "{:?}", client.of_chunks);

    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    assert_eq!(
        edge.asked(WEST).await,
        [subscribe(1, view_area(HOME, SHORT))]
    );
    edge.end().await;
}

/// A command from a connection that is not the player's current one, which can
/// still be on its way when the player has connected again, is passed over.
#[tokio::test]
async fn a_view_distance_asked_for_on_an_earlier_connection_changes_nothing() {
    let mut edge = Harness::start_with_limit(LIMIT).await;
    let earlier = edge.joined(player(1), EntityId(5)).await;
    let stale = earlier.session;
    edge.leave(&earlier).await;
    edge.quiet().await;
    let mut client = edge.joined(player(1), EntityId(6)).await;
    assert_ne!(client.session, stale);
    edge.quiet().await;
    edge.sync(&mut client).await;
    let (centred, told) = (client.centres.len(), client.of_chunks.len());

    edge.ask_view_distance(&client, stale, SHORT).await;
    for region in REGIONS {
        let asked = edge.asked(region).await;
        assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
    }
    edge.sync(&mut client).await;
    assert_eq!(client.centres.len(), centred);
    assert_eq!(loads_since(&client, told), []);
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, LIMIT));

    // The same on the current connection is taken.
    edge.ask_view_distance(&client, client.session, SHORT).await;
    let asked = edge.asked(WEST).await;
    assert!(!asked.is_empty());
    assert_eq!(edge.at(WEST).chunks(Role::Viewer), view_area(HOME, SHORT));
    edge.end().await;
}

/// An armour stand wearing a helmet, posed in the head, at rest on the ground.
fn dressed_stand() -> EntityData {
    let mut pose = [[clustine_world::Float(0.0); 3]; 6];
    pose[0] = [
        clustine_world::Float(10.0),
        clustine_world::Float(0.0),
        clustine_world::Float(0.0),
    ];
    for (part, default) in [
        [-10.0, 0.0, -10.0],
        [-15.0, 0.0, 10.0],
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
    ]
    .into_iter()
    .enumerate()
    {
        pose[part + 2] = default.map(clustine_world::Float);
    }
    EntityData::ArmorStand {
        flags: 4,
        disabled_slots: 0,
        pose: Box::new(pose),
        equipment: vec![(5, clustine_world::ItemStack::new(1100, 1))],
        health: clustine_world::Float(20.0),
        last_hit: None,
        air: 300,
        frozen: 0,
    }
}

#[test]
fn a_stand_is_spawned_facing_its_way_with_its_data_and_then_what_it_wears() {
    let data = dressed_stand();
    let state = EntityState {
        entity: EntityId(9),
        kind: EntityKind::Other {
            kind: entity_types::ARMOR_STAND as u16,
            uuid: Uuid::from_u128(9),
            motion: [0.0, -0.0784, 0.0],
            hops: 0,
            lived: 0,
            data: data.clone(),
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(3.5, -60.0, 3.5),
            yaw: -180.0,
            pitch: 0.0,
            on_ground: true,
        },
    };
    let seen = Seen {
        kind: entity_types::ARMOR_STAND as u16,
        position: [3.5, -60.0, 3.5],
        motion: [0.0, -0.0784, 0.0],
        yaw: -180.0,
        pitch: 0.0,
        on_ground: true,
    };
    let pairing = Tracker::new(seen, &data, 0, &mut |key| i32::try_from(key.0).unwrap()).pairing();
    let packets: Vec<ClientboundPlay> = spawn_packets(&state, 4, Some(&pairing), 0, &mut |key| {
        i32::try_from(key.0).unwrap()
    })
    .iter()
    .map(|packet| ClientboundPlay::decode(packet).expect("the edge sends packets"))
    .collect();
    let [
        ClientboundPlay::SpawnEntity(spawn),
        ClientboundPlay::SetEntityData(values),
        ClientboundPlay::SetEquipment(worn),
    ] = &packets[..]
    else {
        panic!("{packets:?}");
    };
    // Its head faces as it does: -180 degrees is half a turn, 128 of 256.
    assert_eq!((spawn.yaw, spawn.head_yaw), (128, 128));
    // The health its constructor sets, its arms, and the head's pose: what is not a
    // default, in the order of their indices.
    assert_eq!(
        values.values,
        [
            (9, EntityDataValue::Float(20.0)),
            (15, EntityDataValue::Byte(4)),
            (16, EntityDataValue::Rotations([10.0, 0.0, 0.0])),
        ]
    );
    assert_eq!(worn.entity_id, 4);
    assert_eq!(worn.slots, [(5, Some(protocol::ItemStack::new(1100, 1)))]);
}

/// A snowball is spawned with its motion and its owner's id as its data
/// (`Projectile.getAddEntityPacket`), then the stack it shows at index 8, which its
/// constructor set and the official server always sends.
#[test]
fn a_snowball_is_spawned_with_its_owners_id_and_then_its_stack() {
    let data = EntityData::Thrown {
        stack: clustine_world::ItemStack::new(clustine_data::items::SNOWBALL, 1),
        owner: Some(EntityId(3)),
        left_owner: false,
        has_been_shot: false,
    };
    let motion = [0.25, 0.5, 1.25];
    let state = EntityState {
        entity: EntityId(9),
        kind: EntityKind::Other {
            kind: entity_types::SNOWBALL as u16,
            uuid: Uuid::from_u128(9),
            motion,
            hops: 0,
            lived: 0,
            data: data.clone(),
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(3.5, -58.5, 3.5),
            yaw: 11.0,
            pitch: 22.0,
            on_ground: false,
        },
    };
    let seen = Seen {
        kind: entity_types::SNOWBALL as u16,
        position: [3.5, -58.5, 3.5],
        motion,
        yaw: 11.0,
        pitch: 22.0,
        on_ground: false,
    };
    let pairing = Tracker::new(seen, &data, 0, &mut |key| i32::try_from(key.0).unwrap()).pairing();
    let packets: Vec<ClientboundPlay> = spawn_packets(&state, 4, Some(&pairing), 7, &mut |key| {
        i32::try_from(key.0).unwrap()
    })
    .iter()
    .map(|packet| ClientboundPlay::decode(packet).expect("the edge sends packets"))
    .collect();
    let [
        ClientboundPlay::SpawnEntity(spawn),
        ClientboundPlay::SetEntityData(values),
    ] = &packets[..]
    else {
        panic!("{packets:?}");
    };
    assert_eq!(spawn.kind, entity_types::SNOWBALL);
    assert_eq!(spawn.data, 7);
    // The motion as the wire's packed vector carries it, to its precision.
    for (sent, made) in spawn.velocity.iter().zip(motion) {
        assert!((sent - made).abs() < 1.0e-3, "{:?}", spawn.velocity);
    }
    assert_eq!(
        values.values,
        [(
            8,
            EntityDataValue::ItemStack(Some(protocol::ItemStack::new(
                clustine_data::items::SNOWBALL,
                1
            )))
        )]
    );
    // The owner whose id the edge looks up is the projectile's.
    assert_eq!(owner_of(&state), Some(EntityId(3)));
}

/// A thrown eye of ender is no projectile: its spawn carries its motion and no
/// data (`Entity.getAddEntityPacket`), whatever owner id is offered, and then the eye
/// it shows at index 8, which `setItem` set as a copy and the official server always
/// sends; a snapshot without pairing shows it alike.
#[test]
fn an_eye_of_ender_is_spawned_with_no_data_and_then_its_stack() {
    let data = EntityData::EyeOfEnder(Box::new(clustine_world::EyeOfEnder {
        item: clustine_world::ItemStack::new(clustine_data::items::ENDER_EYE, 1),
        target: [clustine_world::Double(12.0); 3],
        survives: true,
    }));
    let motion = [0.0125, 0.015, -0.025];
    let kind = entity_types::EYE_OF_ENDER as u16;
    let state = EntityState {
        entity: EntityId(9),
        kind: EntityKind::Other {
            kind,
            uuid: Uuid::from_u128(9),
            motion,
            hops: 0,
            lived: 1,
            data: data.clone(),
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(3.5, -59.1, 3.5),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        },
    };
    let seen = Seen {
        kind,
        position: [3.5, -59.1, 3.5],
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
    };
    let pairing = Tracker::new(seen, &data, 0, &mut |key| i32::try_from(key.0).unwrap()).pairing();
    let shown = protocol::ItemStack::new(clustine_data::items::ENDER_EYE, 1);
    for pairing in [Some(&pairing), None] {
        let packets: Vec<ClientboundPlay> = spawn_packets(&state, 4, pairing, 7, &mut |key| {
            i32::try_from(key.0).unwrap()
        })
        .iter()
        .map(|packet| ClientboundPlay::decode(packet).expect("the edge sends packets"))
        .collect();
        let [
            ClientboundPlay::SpawnEntity(spawn),
            ClientboundPlay::SetEntityData(values),
        ] = &packets[..]
        else {
            panic!("{packets:?}");
        };
        assert_eq!(spawn.kind, entity_types::EYE_OF_ENDER);
        assert_eq!(spawn.data, 0);
        for (sent, made) in spawn.velocity.iter().zip(motion) {
            assert!((sent - made).abs() < 1.0e-3, "{:?}", spawn.velocity);
        }
        assert_eq!(
            values.values,
            [(8, EntityDataValue::ItemStack(Some(shown.clone())))]
        );
    }
    assert_eq!(owner_of(&state), None);
}

/// A shulker's bullet is a projectile: its spawn carries its owner's wire id
/// (`Projectile.getAddEntityPacket`) and its motion, and it has no data of its own
/// (`ShulkerBullet.defineSynchedData` defines none), so nothing follows.
#[test]
fn a_shulker_s_bullet_is_spawned_with_its_owner_and_nothing_after() {
    let data = EntityData::ShulkerBullet(Box::new(clustine_world::ShulkerBullet {
        owner: Some(clustine_world::projectiles::ProjectileOwner {
            key: EntityId(3),
            kind: entity_types::SHULKER as u16,
            player: None,
        }),
        target: Some(EntityId(5)),
        steps: 20,
        direction: Some(3),
        target_delta: [
            clustine_world::Double(0.0),
            clustine_world::Double(0.0),
            clustine_world::Double(0.15),
        ],
    }));
    let motion = [0.0, 0.0, 0.03];
    let kind = entity_types::SHULKER_BULLET as u16;
    let state = EntityState {
        entity: EntityId(9),
        kind: EntityKind::Other {
            kind,
            uuid: Uuid::from_u128(9),
            motion,
            hops: 0,
            lived: 1,
            data: data.clone(),
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(0.5, -59.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        },
    };
    let seen = Seen {
        kind,
        position: [0.5, -59.0, 0.5],
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
    };
    let pairing = Tracker::new(seen, &data, 0, &mut |key| i32::try_from(key.0).unwrap()).pairing();
    let packets: Vec<ClientboundPlay> = spawn_packets(&state, 4, Some(&pairing), 7, &mut |key| {
        i32::try_from(key.0).unwrap()
    })
    .iter()
    .map(|packet| ClientboundPlay::decode(packet).expect("the edge sends packets"))
    .collect();
    let [ClientboundPlay::SpawnEntity(spawn)] = &packets[..] else {
        panic!("{packets:?}");
    };
    assert_eq!(spawn.kind, entity_types::SHULKER_BULLET);
    assert_eq!(spawn.data, 7);
    for (sent, made) in spawn.velocity.iter().zip(motion) {
        assert!((sent - made).abs() < 1.0e-3, "{:?}", spawn.velocity);
    }
    assert_eq!(owner_of(&state), Some(EntityId(3)));
}

/// A shulker shows the face it clings to as a direction (`DATA_ATTACH_FACE_ID`, 16),
/// its peek (17) and its colour (18) as bytes, each paired only where it is not the
/// default (down, 0 and 16).
#[test]
fn a_shulker_shows_its_face_as_a_direction_and_its_peek_and_colour_as_bytes() {
    let mut ids = |key: EntityId| i32::try_from(key.0).unwrap();
    let kind = entity_types::SHULKER as u16;
    let values = crate::tracking::living_values(
        kind,
        &EntityData::Mob {
            body: Vec::new(),
            mind: Vec::new(),
            state: Default::default(),
        },
        &mut ids,
    );
    let own: Vec<_> = values
        .into_iter()
        .filter(|(index, _, _)| (16..=18).contains(index))
        .collect();
    assert_eq!(
        own,
        [
            (16, EntityDataValue::Direction(0), true),
            (17, EntityDataValue::Byte(0), true),
            (18, EntityDataValue::Byte(16), true),
        ]
    );
}
