//! A fan-out task whose regions, clients and light jobs a test plays: the scaffolding
//! for test 4 of `docs/adr/0023-light-at-the-edge.md` ("the record" below).
//!
//! **A script.** [`Script::start`] starts one edge's fan-out task and plays everything
//! around it. A test sees what the task says to regions, the packets it sends clients
//! and the jobs it dispatches, and nothing inside it. [`Setup`] has the edge's settings
//! (`config`, the fan-out's own `FanoutConfig`: the view distance and whatever else it
//! has), the end of lighting ([`End::Here`]: a job is done in the turn that dispatches
//! it; [`End::Held`]: the test answers) and the regions linked at the start.
//!
//! **Regions.** [`WEST`], where players enter the world at [`HOME`], [`EAST`],
//! [`NORTH`] and [`SOUTH`] are linked at the start; [`PART`] is for a split. Which of
//! them holds a chunk is whatever the test has them say. The edge asks a region for
//! chunks in numbered messages and an answer carries the number; the script keeps the
//! numbers as it reads what the edge says, so **settle a region before it answers for
//! a chunk the edge has only just asked it for**. [`Script::asked`] is what a region
//! is asked for now.
//!
//! - [`Script::snapshot`]: the region sends a chunk (E1, E2 or E3 of the record's
//!   section 3, by what the edge has). [`Script::change`]: one tick's message with one
//!   or several block changes (E4, E5). [`Script::elsewhere`], [`Script::not_mine`]:
//!   another region holds the chunk; the edge asks that one, or asks again.
//! - [`Script::walk`]: a player's region says that they moved into another chunk,
//!   which moves their view (E6 for what it leaves). [`Script::hand`]: it lets them go
//!   to another region, which is then asked for their view.
//! - [`Script::lose`] ends a region's link; [`Script::link`] gives the edge a new one
//!   and returns its [`Hello`]; [`Script::resume`] answers that as a region that
//!   knows the edge and [`Script::begin`] as one that does not. The region then owes
//!   a `snapshot` for each chunk the hello named.
//! - [`Script::merge`] and [`Script::split`]: afterwards the survivor, or the part, is
//!   asked for chunks and answers like any region. That, a hand-over and `not_mine`
//!   are how the same chunk comes from another region than the one that served it.
//! - [`Script::tell`] and [`Script::say`] say anything else, past the model.
//!
//! **Clients.** [`Script::join`] connects a player, has the west place them at
//! [`HOME`] and returns their [`Client`]. A client takes packets only when the test
//! has it: [`Script::sync`], or [`Client::drain`] for what is in its queue now. It
//! keeps what a real client would hold: for each chunk it has, the blocks
//! ([`Client::block`]: the chunk's packet and the block updates since) and the light
//! its packets add up to ([`Client::light`], a `ChunkLight`: the chunk's packet, then
//! each light update laid over it); and in [`Client::told`] everything it was told of
//! chunks, in order ([`Told`]: sent, unloaded, a block changed, a light update with
//! the sections it named, a batch begun and ended). It confirms batches when it is
//! synced unless [`Client::confirm`] says not to. Its queue takes 4,096 packets, or
//! what [`Script::join_with_room`] says, and [`Client::fill`] makes it full **now**
//! ([`Client::fill_leaving`]: all but full), so that the next packet the edge has for
//! it finds no room. The edge removes such a player: [`Script::removed`] waits for
//! that. [`Script::leave`] ends a connection from the client's side.
//!
//! **The model.** [`Script::model`] is the replica as the test fed it: for every chunk
//! some player's view wants and a region has sent, the chunk as it came
//! ([`Model::came`], with its light and versions) and its blocks now
//! ([`Model::chunk`]). It follows the record's section 3 and nothing else: a snapshot
//! that shows the same as the chunk that is there changes nothing, a chunk no view
//! wants is gone, a change for a chunk that has not come is passed over.
//! [`Model::light`] is what `clustine_light::light` gives for a chunk beside the
//! neighbours the model has **now**; [`Model::nine`] is what a job dispatched now
//! would be handed, and [`Model::same_nine`] compares that with a job's. [`as_held`]
//! turns a `Light` into what a client holds that was sent all of it, to compare with
//! [`Client::light`]. The model changes when the script says something and the edge
//! when it has handled it: compare after a wait.
//!
//! **Held jobs.** With [`End::Held`], [`Script::held`] is the test's handle ([`Held`],
//! in `light.rs`). `next().await` waits until a job has been dispatched and returns it
//! (`position`, `basis`, `nine`, `old`, `whole`); `fresh()` and `out()` wait for
//! nothing. A job is answered by its basis, in any order: `answer` (what `relight`
//! gives), `answer_with` (a given light), `fail`, or `never`. `started` says `Started`
//! for a job; nothing else does. `set_in_flight` is how many jobs may be out at a time
//! (four at first; with none, nothing is dispatched until the test allows one and the
//! edge has a turn); `shut(true)` is an edge with no thread left. An answer is only
//! put where the edge will read it: [`Script::turn`] waits until it has.
//!
//! **The waits.** None is by time. Each panics after ten seconds of the tokio clock if
//! what it waits for does not come, and at once if the edge's task has ended (it holds
//! its own account to the record at the end of every turn, and panics where it is
//! wrong).
//!
//! - [`Script::settle`]`(region)`: the edge has handled everything that region has
//!   said so far, and the script has read all the edge said to it up to then.
//! - [`Script::turn`]: the edge has ended a turn since the call, so it has taken every
//!   answer given to a job before and has dispatched what that made room for.
//! - [`Script::quiet`]: the edge has taken every command and handled everything every
//!   linked region has said, and what that had it say to other regions. **With jobs
//!   held, quiet is not lit**: a job that is out stays out, its chunk keeps the light
//!   it had (or has none, and is not sent), and nothing more comes of it until the
//!   test answers. [`Script::answer_all`] answers every job with the real result until
//!   the quiet edge has none out; only then has the edge sent all it will, and the
//!   record's L1 and L2 can be checked.
//! - [`Script::sync`]`(client)`: the client has taken everything the edge has sent it
//!   and will send it without anything else happening, each batch confirmed. One that
//!   does not confirm has what came up to its first batch.
//!
//! **The clock.** All of it works under `#[tokio::test(start_paused = true)]`, and no
//! wait moves a paused clock while what it waits for comes. A wait for what does not
//! come lets the clock run, through every timer of the edge, so wait only for what has
//! to come. The times of light are settings (`light` in [`Setup::config`]: `latest`,
//! `rest`, `overdue`, `job_patience`), and a script starts with nothing held, nothing
//! resting and a patience without end. A test of them sets them, moves the clock with
//! `tokio::time::advance` and then settles a region, more than once, so that the edge
//! has had its turn at the new time (`light_policy.rs` does so). A held job that a
//! thread was said to have started lets the clock run to `job_patience` at the first
//! wait that has nothing to wait for: answer before waiting, or leave the patience
//! longer than anything waited for.
//!
//! **Chunks.** [`ground`] is a chunk as a generator makes it: stone below [`FLOOR`],
//! marked as generated by [`GENERATOR`], with the light of the rules beside eight like
//! it and `beside` saying so. [`unlit`] takes a chunk's light away. [`with`] sets
//! blocks (a change that light sees takes the light, and any change the mark, as in a
//! region); [`lamp`] sets a block that shines and [`roof`] stone over some columns.
//! [`lit_among`] gives a chunk the light of the rules beside given neighbours, as a
//! chunk that was stored with light comes. [`at`] is a block of a chunk.
//!
//! # Three examples
//!
//! A chunk, and then its neighbour with a lamp, lit in the turn:
//!
//! ```ignore
//! let mut script = Script::start(End::Here).await;
//! let mut client = script.join(1).await;
//! script.snapshot(WEST, HOME, ground());
//! script.snapshot(WEST, EAST_OF_HOME, lamp(ground(), 0, FLOOR, 8));
//! script.quiet().await;
//! script.sync(&mut client).await;
//! let due = script.model().light(HOME).unwrap();
//! assert_eq!(client.light(HOME), Some(&as_held(&due)));
//! ```
//!
//! A block change under a held job:
//!
//! ```ignore
//! let mut script = Script::start(End::Held).await;
//! let mut client = script.join(1).await;
//! script.snapshot(WEST, HOME, unlit(ground()));
//! let job = script.held().next().await;
//! script.change(WEST, &[(at(HOME, 3, FLOOR, 3), blocks::TORCH)]);
//! script.settle(WEST).await;
//! script.held().answer(job.basis);
//! script.turn().await;
//! script.sync(&mut client).await; // the chunk, with the torch in it
//! let next = script.held().next().await; // the job the change is owed
//! ```
//!
//! A client whose queue is full when a result is fanned out, and one behind it:
//!
//! ```ignore
//! let mut script = Script::start(End::Held).await;
//! let (mut first, mut second) = (script.join(1).await, script.join(2).await);
//! script.snapshot(WEST, HOME, unlit(ground()));
//! let job = script.held().next().await;
//! script.sync(&mut first).await;
//! first.fill();
//! script.held().answer(job.basis);
//! script.turn().await;
//! script.removed(&mut first).await;
//! script.sync(&mut second).await;
//! assert!(second.has(HOME));
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::AtomicI32;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use clustine_botswarm::{ChunkLight, Named, SectionLight as Said};
use clustine_data::{BlockState, blocks};
use clustine_light::Around;
use clustine_protocol::chunk::{SectionData, decode_sections};
use clustine_protocol::packets::play::ClientboundPlay;
use clustine_region::RegionId;
use clustine_rpc::link::{self, WorkerEnd};
use clustine_rpc::{EdgeMessage, EdgeToWorker, Presence, Welcome, WorkerToEdge};
use clustine_sim::api::{Durable, Inventory, PlayerEvent, PlayerTransfer, Pose, RegionEvent};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, EdgeId, EntityId, LIGHT_ARRAY_LENGTH, Light, PlayerId,
    SectionLight, Vec3,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

use crate::fanout::{
    Command, Fanout, FanoutConfig, LightSettings, NO_TELEPORT, SessionId, WireLedger,
};
use crate::light::{Held, LightJob, Lighting};
use crate::login::Profile;
use crate::{EdgeIdentity, LoginWorld, RegionLink, Relinks, Routing, Stopped};

/// How long a wait lasts before it gives up on something that has to happen.
const SOON: Duration = Duration::from_secs(10);

/// The regions a script plays. Players enter the world in the western one.
pub(crate) const WEST: RegionId = RegionId(0);
pub(crate) const EAST: RegionId = RegionId(1);
pub(crate) const NORTH: RegionId = RegionId(2);
pub(crate) const SOUTH: RegionId = RegionId(3);
/// A region that is split off: no link at the start.
pub(crate) const PART: RegionId = RegionId(4);
/// The regions that are there from the start of the world.
pub(crate) const REGIONS: [RegionId; 4] = [WEST, EAST, NORTH, SOUTH];
const PORTS: usize = 5;

/// The chunk players enter the world in, and the one east of it.
pub(crate) const HOME: ChunkPos = ChunkPos::new(0, 0);
pub(crate) const EAST_OF_HOME: ChunkPos = ChunkPos::new(1, 0);

/// The view distance of [`Setup::new`].
pub(crate) const VIEW: i32 = 2;

/// The lowest air of [`ground`]: stone is below it.
pub(crate) const FLOOR: i32 = -60;
/// The lowest block of a chunk, and how many sections and light sections it has.
pub(crate) const MIN_Y: i32 = -64;
pub(crate) const SECTIONS: usize = 24;
pub(crate) const LIGHT_SECTIONS: usize = SECTIONS + 2;
/// The version of the generator that [`ground`] says made it.
pub(crate) const GENERATOR: u16 = 1;

const IDENTITY: EdgeIdentity = EdgeIdentity {
    edge: EdgeId(7),
    start: 100,
};

/// The player numbered `number`.
pub(crate) fn player(number: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(number))
}

/// A place to stand in `chunk`.
fn within(chunk: ChunkPos) -> Vec3 {
    Vec3::new(
        f64::from(chunk.x) * 16.0 + 0.5,
        f64::from(FLOOR),
        f64::from(chunk.z) * 16.0 + 0.5,
    )
}

/// The block at `x`, `z` (each in `0..16`) and height `y` of `chunk`.
pub(crate) fn at(chunk: ChunkPos, x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(chunk.x * 16 + x, y, chunk.z * 16 + z)
}

/// The chunks someone standing in `centre` is shown at a view distance: what the
/// edge asks regions for on their behalf. A test of this file holds it to what the
/// edge asks.
pub(crate) fn view(centre: ChunkPos, view_distance: i32) -> BTreeSet<ChunkPos> {
    // A client keeps two chunks more than it draws, squarely around it.
    const BUFFER: i64 = 2;
    let reach = view_distance + BUFFER as i32;
    let limit = i64::from(view_distance) * i64::from(view_distance);
    let mut area = BTreeSet::new();
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let x = (i64::from(dx).abs() - BUFFER).max(0);
            let z = (i64::from(dz).abs() - BUFFER).max(0);
            if x * x + z * z < limit {
                area.insert(ChunkPos::new(centre.x + dx, centre.z + dz));
            }
        }
    }
    area
}

/// The eight places around `position` and itself, north-west first, row by row: the
/// order of a job's nine chunks.
pub(crate) fn nine_around(position: ChunkPos) -> [ChunkPos; 9] {
    let mut nine = [position; 9];
    for (slot, place) in nine.iter_mut().enumerate() {
        let (dx, dz) = (slot as i32 % 3 - 1, slot as i32 / 3 - 1);
        *place = ChunkPos::new(position.x + dx, position.z + dz);
    }
    nine
}

// ---------------------------------------------------------------------------------
// Chunks
// ---------------------------------------------------------------------------------

/// What the light rules give for `centre` among `nine` (its own slot is not read).
fn rules(centre: &Chunk, nine: [Option<&Chunk>; 9]) -> Light {
    let [nw, n, ne, w, _, e, sw, s, se] = nine;
    clustine_light::light(&Around {
        centre,
        neighbours: [nw, n, ne, w, e, sw, s, se],
        sky: true,
    })
}

/// Stone below [`FLOOR`] and air above, without light and marked by nobody.
fn bare_ground() -> Chunk {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the overworld is a dimension");
    let mut chunk = Chunk::empty(overworld, Biome(0));
    for y in MIN_Y..FLOOR {
        for x in 0..16 {
            for z in 0..16 {
                chunk.set(x, y, z, blocks::STONE);
            }
        }
    }
    chunk
}

/// A chunk as a generator makes it: stone below [`FLOOR`], marked as generated by
/// [`GENERATOR`], and with the light the rules give it beside eight chunks like it,
/// whose `beside` says so. The same chunk at every position.
pub(crate) fn ground() -> Chunk {
    // The light is the same every time, and a test makes dozens of these.
    static GROUND: OnceLock<Chunk> = OnceLock::new();
    GROUND
        .get_or_init(|| {
            let mut chunk = bare_ground();
            chunk.mark_generated(GENERATOR, false);
            let beside = chunk.clone();
            lit_among(chunk, [Some(&beside); 9])
        })
        .clone()
}

/// `chunk` without light. What it says of its generator stays.
pub(crate) fn unlit(mut chunk: Chunk) -> Chunk {
    chunk.take_light();
    chunk
}

/// `chunk` with the light the rules give it among `nine` (north-west first, row by
/// row; its own slot is not read; none is a wall), made by the edge's own version of
/// the rules and beside chunks of [`GENERATOR`]: as a chunk that was changed and
/// stored with light comes. Said after the last change, which would take it away.
pub(crate) fn lit_among(mut chunk: Chunk, nine: [Option<&Chunk>; 9]) -> Chunk {
    let mut light = rules(&chunk, nine);
    light.made_by = clustine_light::VERSION;
    light.beside = GENERATOR;
    chunk.set_light(light);
    chunk
}

/// `chunk` with each of `blocks` set: `(x, y, z, state)`, `x` and `z` in `0..16`. A
/// change that light sees takes the chunk's light with it, and any change takes its
/// mark of being as generated, as wherever a chunk is changed.
pub(crate) fn with(mut chunk: Chunk, blocks: &[(usize, i32, usize, BlockState)]) -> Chunk {
    for (x, y, z, state) in blocks {
        chunk
            .set(*x, *y, *z, *state)
            .expect("the block is within the chunk");
    }
    chunk
}

/// `chunk` with a block that shines at level 15 at `x`, `y`, `z`.
pub(crate) fn lamp(chunk: Chunk, x: usize, y: i32, z: usize) -> Chunk {
    with(chunk, &[(x, y, z, blocks::GLOWSTONE)])
}

/// `chunk` with stone at height `y` over the columns from `(x0, z0)` to `(x1, z1)`,
/// both within.
pub(crate) fn roof(mut chunk: Chunk, y: i32, from: (usize, usize), to: (usize, usize)) -> Chunk {
    for x in from.0..=to.0 {
        for z in from.1..=to.1 {
            chunk
                .set(x, y, z, blocks::STONE)
                .expect("the roof is within the chunk");
        }
    }
    chunk
}

/// Light as a client holds it that was sent all of `light`: what [`Client::light`]
/// is to be compared with.
pub(crate) fn as_held(light: &Light) -> ChunkLight {
    let held = |sections: &[SectionLight]| {
        sections
            .iter()
            .map(|section| match section {
                SectionLight::Absent => Said::Unsaid,
                SectionLight::Dark => Said::Dark,
                SectionLight::Full => Said::Levels(vec![0xff; LIGHT_ARRAY_LENGTH]),
                SectionLight::Levels(levels) => Said::Levels(levels.to_vec()),
            })
            .collect()
    };
    ChunkLight {
        sky: held(&light.sky),
        block: held(&light.block),
    }
}

// ---------------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------------

/// A chunk of the [`Model`].
#[derive(Debug, Clone)]
pub(crate) struct Fed {
    /// As the region sent it, with the light it came with.
    pub(crate) came: Chunk,
    /// Its blocks now. Without light.
    pub(crate) now: Chunk,
}

/// The replica as the test fed it; see the page at the head of this file.
#[derive(Debug)]
pub(crate) struct Model {
    fed: BTreeMap<ChunkPos, Fed>,
    /// The chunk each player's view is centred on.
    centres: BTreeMap<PlayerId, ChunkPos>,
    view_distance: i32,
}

impl Model {
    /// The chunks `player` is shown; nothing for someone who is not there.
    pub(crate) fn view(&self, player: PlayerId) -> BTreeSet<ChunkPos> {
        match self.centres.get(&player) {
            Some(centre) => view(*centre, self.view_distance),
            None => BTreeSet::new(),
        }
    }

    /// The chunks some player is shown.
    pub(crate) fn wanted(&self) -> BTreeSet<ChunkPos> {
        let views = self.centres.keys().map(|player| self.view(*player));
        views.flatten().collect()
    }

    /// Where the model has a chunk.
    pub(crate) fn positions(&self) -> BTreeSet<ChunkPos> {
        self.fed.keys().copied().collect()
    }

    pub(crate) fn has(&self, position: ChunkPos) -> bool {
        self.fed.contains_key(&position)
    }

    /// The chunk at `position` as its blocks are now.
    pub(crate) fn chunk(&self, position: ChunkPos) -> Option<&Chunk> {
        self.fed.get(&position).map(|fed| &fed.now)
    }

    /// The chunk at `position` as the region sent it.
    pub(crate) fn came(&self, position: ChunkPos) -> Option<&Chunk> {
        self.fed.get(&position).map(|fed| &fed.came)
    }

    /// The chunk at `position` and the eight around it as they are now, north-west
    /// first, row by row.
    pub(crate) fn nine(&self, position: ChunkPos) -> [Option<&Chunk>; 9] {
        nine_around(position).map(|place| self.chunk(place))
    }

    /// What the light rules give for the chunk at `position` beside the neighbours
    /// the model has now; none if it has no chunk there.
    pub(crate) fn light(&self, position: ChunkPos) -> Option<Light> {
        let centre = self.chunk(position)?;
        Some(rules(centre, self.nine(position)))
    }

    /// Whether the nine chunks of `job` are, block for block, the model's now.
    pub(crate) fn same_nine(&self, job: &LightJob) -> bool {
        let mine = self.nine(job.position);
        let same = |(mine, theirs): (&Option<&Chunk>, &Option<Arc<Chunk>>)| match (mine, theirs) {
            (Some(mine), Some(theirs)) => mine.shows_the_same(theirs),
            (None, None) => true,
            _ => false,
        };
        mine.iter().zip(&job.nine).all(same)
    }

    /// A region sent `chunk` for `position`.
    fn snapshot(&mut self, position: ChunkPos, chunk: &Chunk) {
        if !self.wanted().contains(&position) {
            return;
        }
        // Section 3, E2: the entry keeps the chunk it has.
        if (self.fed.get(&position)).is_some_and(|fed| fed.now.shows_the_same(chunk)) {
            return;
        }
        let fed = Fed {
            came: chunk.clone(),
            now: unlit(chunk.clone()),
        };
        self.fed.insert(position, fed);
    }

    /// A region said that a block has changed.
    fn change(&mut self, block: BlockPos, state: BlockState) {
        // Section 3: a change for a chunk that has not come is passed over.
        if let Some(fed) = self.fed.get_mut(&block.chunk()) {
            let (x, z) = (block.x.rem_euclid(16), block.z.rem_euclid(16));
            fed.now.set(x as usize, block.y, z as usize, state);
        }
    }

    /// Drops what no view wants any more.
    fn forget_the_unwanted(&mut self) {
        let wanted = self.wanted();
        self.fed.retain(|position, _| wanted.contains(position));
    }
}

// ---------------------------------------------------------------------------------
// Clients
// ---------------------------------------------------------------------------------

/// Something a client was told of chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Told {
    /// A batch of chunks begins.
    BatchStarted,
    /// A batch has ended, which the edge says had this many chunks.
    BatchFinished(i32),
    Sent(ChunkPos),
    Unloaded(ChunkPos),
    /// A block has become this.
    Block(BlockPos, BlockState),
    /// A light update for the chunk, and the sections it named.
    Light(ChunkPos, Named),
}

impl Told {
    /// The chunk this is about; none for the marks of a batch.
    pub(crate) fn chunk(&self) -> Option<ChunkPos> {
        match self {
            Told::BatchStarted | Told::BatchFinished(_) => None,
            Told::Sent(chunk) | Told::Unloaded(chunk) | Told::Light(chunk, _) => Some(*chunk),
            Told::Block(block, _) => Some(block.chunk()),
        }
    }
}

/// A chunk as a client holds it.
struct Shown {
    /// The sections of its packet, bottom first.
    sections: Vec<SectionData>,
    /// The blocks it was told have changed since.
    changed: BTreeMap<BlockPos, BlockState>,
    light: ChunkLight,
}

/// A player's client: what it was sent, and what it holds by that.
pub(crate) struct Client {
    pub(crate) player: PlayerId,
    /// The entity the west placed the player as.
    pub(crate) entity: EntityId,
    session: SessionId,
    packets: mpsc::Receiver<Bytes>,
    /// The edge's end of the queue, for filling it. Weak, so that the queue still
    /// ends when the edge drops the player.
    queue: mpsc::WeakSender<Bytes>,
    chunks: BTreeMap<ChunkPos, Shown>,
    told: Vec<Told>,
    acknowledged: Vec<i32>,
    /// Batches it has been sent and has not confirmed.
    batches: u32,
    confirms: bool,
    disconnected: Option<String>,
    closed: bool,
}

impl Client {
    /// A change of a block the client was sent, alone or with others of its section.
    fn take_block(&mut self, block: BlockPos, state: BlockState) {
        self.told.push(Told::Block(block, state));
        // A client has nowhere to put a block of a chunk it does not have.
        if let Some(shown) = self.chunks.get_mut(&block.chunk()) {
            shown.changed.insert(block, state);
        }
    }

    /// Takes a packet as a client would.
    fn take(&mut self, packet: Bytes) {
        // What `fill` put there to take up room.
        if packet.is_empty() {
            return;
        }
        let decoded = ClientboundPlay::decode(&packet).expect("the edge sends packets");
        match decoded {
            ClientboundPlay::LevelChunkWithLight(chunk) => {
                let position = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
                let sections = decode_sections(
                    &chunk.sections,
                    SECTIONS,
                    crate::encode::BLOCK_PALETTE,
                    crate::encode::biome_palette(),
                )
                .expect("the edge sends sections a client can read");
                let light = ChunkLight::of(&chunk.light, LIGHT_SECTIONS)
                    .expect("the edge sends light a client can read");
                let shown = Shown {
                    sections,
                    changed: BTreeMap::new(),
                    light,
                };
                self.chunks.insert(position, shown);
                self.told.push(Told::Sent(position));
            }
            ClientboundPlay::UnloadChunk(chunk) => {
                let position = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
                self.chunks.remove(&position);
                self.told.push(Told::Unloaded(position));
            }
            ClientboundPlay::BlockUpdate(update) => {
                let at = update.position;
                let block = BlockPos::new(at.x, at.y, at.z);
                self.take_block(block, BlockState(update.state as u16));
            }
            ClientboundPlay::SectionBlocksUpdate(update) => {
                let (sx, sy, sz) = update.section;
                for ((x, y, z), state) in update.blocks {
                    let block = BlockPos::new(
                        sx * 16 + i32::from(x),
                        sy * 16 + i32::from(y),
                        sz * 16 + i32::from(z),
                    );
                    self.take_block(block, BlockState(state as u16));
                }
            }
            ClientboundPlay::LightUpdate(update) => {
                let position = ChunkPos::new(update.chunk_x, update.chunk_z);
                let named = match self.chunks.get_mut(&position) {
                    Some(shown) => shown.light.lay_over(&update.light),
                    // Nowhere to lay it; the record still says what it named.
                    None => Named::of(&update.light, LIGHT_SECTIONS),
                };
                let named = named.expect("the edge sends light a client can read");
                self.told.push(Told::Light(position, named));
            }
            ClientboundPlay::ChunkBatchStart(_) => self.told.push(Told::BatchStarted),
            ClientboundPlay::ChunkBatchFinished(finished) => {
                self.batches += 1;
                self.told.push(Told::BatchFinished(finished.batch_size));
            }
            ClientboundPlay::AcknowledgeBlockChange(acknowledged) => {
                self.acknowledged.push(acknowledged.sequence);
            }
            ClientboundPlay::Disconnect(disconnect) => {
                self.disconnected = Some(format!("{:?}", disconnect.reason));
            }
            _ => {}
        }
    }

    /// Takes everything that is in the queue now. Waits for nothing.
    pub(crate) fn drain(&mut self) {
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

    /// Whether the edge still serves the player, as far as the queue tells now.
    pub(crate) fn connected(&mut self) -> bool {
        self.drain();
        !self.closed
    }

    /// Why the edge said it disconnects the player, if it did.
    pub(crate) fn disconnected(&self) -> Option<&str> {
        self.disconnected.as_deref()
    }

    /// Whether the client confirms the batches it was sent when it is synced. It
    /// does until it is told not to.
    pub(crate) fn confirm(&mut self, confirms: bool) {
        self.confirms = confirms;
    }

    /// Makes the queue full: the next packet the edge has for the client finds no
    /// room. Returns how many places were filled. [`Client::drain`] empties it.
    pub(crate) fn fill(&mut self) -> usize {
        self.fill_leaving(0)
    }

    /// Fills the queue but for `room` places. Returns how many were filled.
    pub(crate) fn fill_leaving(&mut self, room: usize) -> usize {
        let Some(queue) = self.queue.upgrade() else {
            return 0;
        };
        let mut filled = 0;
        // An empty packet, which `take` passes over: the edge sends none.
        while queue.capacity() > room && queue.try_send(Bytes::new()).is_ok() {
            filled += 1;
        }
        filled
    }

    /// How many more packets the queue takes now; none once the edge has dropped it.
    pub(crate) fn room(&self) -> usize {
        self.queue.upgrade().map_or(0, |queue| queue.capacity())
    }

    /// Everything the client was told of chunks, in order.
    pub(crate) fn told(&self) -> &[Told] {
        &self.told
    }

    /// What it was told of `chunk`, in order.
    pub(crate) fn told_of(&self, chunk: ChunkPos) -> Vec<Told> {
        let of_it = |told: &&Told| told.chunk() == Some(chunk);
        self.told.iter().filter(of_it).copied().collect()
    }

    /// How often it was sent `chunk`.
    pub(crate) fn times_sent(&self, chunk: ChunkPos) -> usize {
        let sent = |told: &&Told| **told == Told::Sent(chunk);
        self.told.iter().filter(sent).count()
    }

    /// Whether it has `chunk`: was sent it, and not told to unload it since.
    pub(crate) fn has(&self, chunk: ChunkPos) -> bool {
        self.chunks.contains_key(&chunk)
    }

    /// The chunks it has.
    pub(crate) fn chunks(&self) -> BTreeSet<ChunkPos> {
        self.chunks.keys().copied().collect()
    }

    /// The block it shows at `block`: as the chunk's packet had it, or as it was
    /// told since. None if it does not have the chunk, or the block is outside it.
    pub(crate) fn block(&self, block: BlockPos) -> Option<BlockState> {
        let shown = self.chunks.get(&block.chunk())?;
        if let Some(state) = shown.changed.get(&block) {
            return Some(*state);
        }
        let above = usize::try_from(block.y - MIN_Y).ok()?;
        let section = shown.sections.get(above / 16)?;
        let (x, z) = (
            block.x.rem_euclid(16) as usize,
            block.z.rem_euclid(16) as usize,
        );
        let state = section.blocks.get((above % 16) << 8 | z << 4 | x);
        Some(BlockState(state as u16))
    }

    /// The light its packets add up to for `chunk`: the chunk's packet, and every
    /// light update since laid over it. None if it does not have the chunk.
    pub(crate) fn light(&self, chunk: ChunkPos) -> Option<&ChunkLight> {
        self.chunks.get(&chunk).map(|shown| &shown.light)
    }
}

// ---------------------------------------------------------------------------------
// The script
// ---------------------------------------------------------------------------------

/// Which end of `Lighting` the edge is given (the record's section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// A job is done in the turn that dispatches it.
    Here,
    /// The test is handed each job and answers it: [`Script::held`].
    Held,
}

/// What a script starts with.
pub(crate) struct Setup {
    /// The fan-out's settings, all of them the test's to change.
    pub(crate) config: FanoutConfig,
    pub(crate) end: End,
    /// The regions the edge has a link to from the start, each of which has
    /// answered its hello as a region that does not know the edge.
    pub(crate) linked: Vec<RegionId>,
}

impl Setup {
    /// A view distance of [`VIEW`], twenty players, regions that are waited for
    /// without end, and links to the four [`REGIONS`].
    pub(crate) fn new(end: End) -> Self {
        Self {
            config: FanoutConfig {
                max_players: 20,
                view_distance: VIEW,
                online: Arc::default(),
                region_patience: Duration::from_secs(1_000_000),
                login: LoginWorld::FLAT,
                light: LightSettings::AT_ONCE,
                wire_ledger: WireLedger::default(),
            },
            end,
            linked: REGIONS.to_vec(),
        }
    }
}

/// What a region takes a subscription for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Viewer,
    Guest,
}

/// What a hello of the edge says.
#[derive(Debug, Clone)]
pub(crate) struct Hello {
    pub(crate) since: u64,
    pub(crate) seen: u64,
    pub(crate) players: Vec<PlayerId>,
    pub(crate) chunks: Vec<ChunkPos>,
    pub(crate) guests: Vec<ChunkPos>,
}

/// A region as the script plays it.
#[derive(Default)]
struct Port {
    /// The worker's end of the link; none where there is no link now.
    link: Option<WorkerEnd>,
    /// The epoch of the owner the last link went to.
    epoch: u64,
    /// The region's word for the numbering it shares with the edge.
    since: u64,
    /// The number of the last outbox entry the region has made.
    outbox: u64,
    /// The entries the edge has not confirmed, in order.
    unconfirmed: VecDeque<(u64, Durable)>,
    /// The number of the last subscription message on the link.
    asked: u64,
    /// What the region is subscribed to on the link, each with the number of the
    /// last message that named it.
    subscriptions: BTreeMap<ChunkPos, (Role, u64)>,
    /// What the edge said on the link that no test has looked at yet.
    said: VecDeque<EdgeMessage>,
    /// The highest number of a numbered message the region has read, on any link.
    numbered: u64,
    /// The hello of the link, if a test gave the link.
    hello: Option<Hello>,
}

impl Port {
    /// Takes in a message of the edge. A region ends a link whose subscription
    /// messages do not ascend, so that is held to for everything read.
    fn note(&mut self, region: RegionId, message: &EdgeMessage) {
        if let Some(number) = message.number {
            self.numbered = self.numbered.max(number);
        }
        let (ask, chunks, role) = match &message.body {
            EdgeToWorker::Confirm { number } => {
                self.unconfirmed.retain(|(entry, _)| entry > number);
                return;
            }
            EdgeToWorker::Subscribe { ask, chunks } => (*ask, chunks, Some(Role::Viewer)),
            EdgeToWorker::SubscribeAsGuest { ask, chunks } => (*ask, chunks, Some(Role::Guest)),
            EdgeToWorker::Unsubscribe { ask, chunks } => (*ask, chunks, None),
            _ => return,
        };
        assert!(
            ask > self.asked,
            "{region:?} was sent the subscription message {ask} after {}",
            self.asked
        );
        self.asked = ask;
        for chunk in chunks {
            match role {
                Some(role) => self.subscriptions.insert(*chunk, (role, ask)),
                None => self.subscriptions.remove(chunk),
            };
        }
    }

    /// The link is no more, and with it what was said on it.
    fn unlinked(&mut self) {
        self.link = None;
        self.asked = 0;
        self.subscriptions.clear();
        self.said.clear();
        self.hello = None;
    }
}

/// Where the script has a player.
#[derive(Debug, Clone, Copy)]
struct Person {
    entity: EntityId,
    region: RegionId,
    chunk: ChunkPos,
}

/// A fan-out task whose regions, clients and jobs the test plays; see the page at
/// the head of this file.
pub(crate) struct Script {
    commands: mpsc::Sender<Command>,
    relinks: Relinks,
    task: JoinHandle<Stopped>,
    ports: Vec<Port>,
    held: Option<Held>,
    model: Model,
    people: BTreeMap<PlayerId, Person>,
    sessions: u64,
    /// The last sequence number used to find out that the edge has got to a
    /// client's commands.
    sequences: i32,
    /// The tick of the last message a region said.
    tick: u64,
    /// The links the edge has said have ended, of which nothing has asked.
    ended: Vec<(RegionId, u64)>,
}

/// A link to `region` whose owner has `epoch`, and the worker's end of it.
fn link_to(region: RegionId, epoch: u64) -> (RegionLink, WorkerEnd) {
    let (end, worker) = link::in_process(1024);
    (RegionLink { region, epoch, end }, worker)
}

impl Script {
    /// An edge as [`Setup::new`] has it, with the given end of lighting.
    pub(crate) async fn start(end: End) -> Self {
        Self::start_with(Setup::new(end)).await
    }

    pub(crate) async fn start_with(setup: Setup) -> Self {
        let mut links = Vec::new();
        let mut ports: Vec<Port> = (0..PORTS).map(|_| Port::default()).collect();
        for region in &setup.linked {
            let (link, end) = link_to(*region, 1);
            links.push(link);
            let port = &mut ports[region.0 as usize];
            port.link = Some(end);
            port.epoch = 1;
        }
        let (routing, relinks) = Routing::new(WEST, BlockPos::new(0, -60, 0), IDENTITY, links);
        let (commands, receiver) = mpsc::channel(64);
        let view_distance = setup.config.view_distance;
        let (lighting, held) = match setup.end {
            End::Here => (Lighting::here(), None),
            End::Held => {
                let (lighting, held, results) = Lighting::held();
                ((lighting, results), Some(held))
            }
        };
        let task = tokio::spawn(Fanout::new(setup.config, routing, receiver, lighting).run());
        let mut script = Self {
            commands,
            relinks,
            task,
            ports,
            held,
            model: Model {
                fed: BTreeMap::new(),
                centres: BTreeMap::new(),
                view_distance,
            },
            people: BTreeMap::new(),
            sessions: 0,
            sequences: 0,
            tick: 0,
            ended: Vec::new(),
        };
        for region in setup.linked {
            let hello = script.read(region).await;
            assert!(
                matches!(hello.body, EdgeToWorker::Hello { .. }),
                "{hello:?}"
            );
            script.begin(region);
        }
        script
    }

    // ---- What the edge says to regions ----

    /// Reads what the edge sends `region` next.
    async fn read(&mut self, region: RegionId) -> EdgeMessage {
        let port = &mut self.ports[region.0 as usize];
        let link = port.link.as_mut().expect("the region has a link");
        let message = tokio::select! {
            biased;
            ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
            message = timeout(SOON, link.recv()) => message,
        };
        let message = message.unwrap_or_else(|_| panic!("the edge sent {region:?} nothing"));
        let Some(message) = message else {
            panic!("the edge closed its link to {region:?}");
        };
        port.note(region, &message);
        message
    }

    /// Waits until the edge has handled everything `region` has said so far, by an
    /// outbox entry about nobody, which the edge confirms like any other. What the
    /// edge said to the region meanwhile is read, so the numbers of what it asks for
    /// are known, and kept for [`Script::said`].
    pub(crate) async fn settle(&mut self, region: RegionId) {
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
            self.ports[region.0 as usize].said.push_back(message);
        }
    }

    /// Everything the edge has said to `region` on its link that no test has looked
    /// at, once it has handled what the region has said so far.
    pub(crate) async fn said(&mut self, region: RegionId) -> Vec<EdgeMessage> {
        self.settle(region).await;
        self.ports[region.0 as usize].said.drain(..).collect()
    }

    /// The chunks `region` is asked for now, as far as the script has read: as a
    /// viewer's region or as a guest's.
    pub(crate) fn asked(&self, region: RegionId) -> BTreeSet<ChunkPos> {
        let subscriptions = &self.ports[region.0 as usize].subscriptions;
        subscriptions.keys().copied().collect()
    }

    /// What `region` is asked for `chunk` as, if it is.
    pub(crate) fn role(&self, region: RegionId, chunk: ChunkPos) -> Option<Role> {
        let subscriptions = &self.ports[region.0 as usize].subscriptions;
        subscriptions.get(&chunk).map(|(role, _)| *role)
    }

    /// The number an answer of `region` for `chunk` has to carry.
    fn ask(&self, region: RegionId, chunk: ChunkPos) -> u64 {
        let subscription = self.ports[region.0 as usize].subscriptions.get(&chunk);
        let Some((_, ask)) = subscription else {
            panic!(
                "{region:?} is not asked for {chunk:?}, as far as the script has read: \
                 settle the region first, or answer from the region that is asked"
            );
        };
        *ask
    }

    /// The regions with a link, in ascending order.
    pub(crate) fn linked(&self) -> Vec<RegionId> {
        let linked = |index: &usize| self.ports[*index].link.is_some();
        let regions = (0..PORTS).filter(linked);
        regions.map(|index| RegionId(index as u32)).collect()
    }

    // ---- What regions say ----

    /// Says anything as `region`. The model hears nothing of it.
    pub(crate) fn tell(&mut self, region: RegionId, message: WorkerToEdge) {
        let port = &mut self.ports[region.0 as usize];
        // A region that says `NotMine` to a guest has ended that subscription.
        if let WorkerToEdge::NotMine { chunk, ask } = &message
            && port.subscriptions.get(chunk) == Some(&(Role::Guest, *ask))
        {
            port.subscriptions.remove(chunk);
        }
        let link = port.link.as_ref().expect("the region has a link");
        if let Err(error) = link.try_send(message) {
            let ended = self.task.is_finished();
            panic!("{region:?} cannot say anything: {error:?}; the edge's task ended: {ended}");
        }
    }

    /// Says `entry` as the next entry of the outbox of `region`. Returns its number.
    /// The model hears nothing of it.
    pub(crate) fn say(&mut self, region: RegionId, entry: Durable) -> u64 {
        let port = &mut self.ports[region.0 as usize];
        port.outbox += 1;
        let number = port.outbox;
        port.unconfirmed.push_back((number, entry.clone()));
        self.tell(region, WorkerToEdge::Outbox { number, entry });
        number
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// `region` sends `chunk` for `position`, in answer to what it is asked. Panics
    /// if it is not asked for the chunk as far as the script has read.
    pub(crate) fn snapshot(&mut self, region: RegionId, position: ChunkPos, chunk: Chunk) {
        let ask = self.ask(region, position);
        self.model.snapshot(position, &chunk);
        let snapshot = WorkerToEdge::ChunkSnapshot {
            position,
            ask,
            tick: self.next_tick(),
            chunk,
            entities: Vec::new(),
        };
        self.tell(region, snapshot);
    }

    /// `region` says that blocks have changed, all in one tick's message.
    pub(crate) fn change(&mut self, region: RegionId, changes: &[(BlockPos, BlockState)]) {
        let mut events = Vec::new();
        for (position, state) in changes {
            self.model.change(*position, *state);
            events.push(RegionEvent::BlockChanged {
                position: *position,
                state: *state,
                seen: true,
            });
        }
        let tick = self.next_tick();
        self.tell(region, WorkerToEdge::TickDelta { tick, events });
    }

    /// `region` answers that `holder` holds `chunk`.
    pub(crate) fn elsewhere(&mut self, region: RegionId, chunk: ChunkPos, holder: RegionId) {
        let ask = self.ask(region, chunk);
        let region_named = holder;
        self.tell(
            region,
            WorkerToEdge::Elsewhere {
                chunk,
                ask,
                region: region_named,
            },
        );
    }

    /// `region` answers that it does not hold `chunk`.
    pub(crate) fn not_mine(&mut self, region: RegionId, chunk: ChunkPos) {
        let ask = self.ask(region, chunk);
        self.tell(region, WorkerToEdge::NotMine { chunk, ask });
    }

    /// The region `who` is in says that they have moved into the chunk `to`, which
    /// moves their view.
    pub(crate) fn walk(&mut self, who: PlayerId, to: ChunkPos) {
        let person = self.people.get_mut(&who).expect("the player has joined");
        let (entity, region, from) = (person.entity, person.region, person.chunk);
        person.chunk = to;
        self.model.centres.insert(who, to);
        self.model.forget_the_unwanted();
        let moved = RegionEvent::EntityMoved {
            entity,
            pose: Pose::at(within(to)),
            previous_chunk: from,
        };
        let tick = self.next_tick();
        self.tell(
            region,
            WorkerToEdge::TickDelta {
                tick,
                events: vec![moved],
            },
        );
    }

    /// The region `who` is in lets them go to the region `to`, into `chunk`. The
    /// edge has handled that when this returns, and has told `to` of the arrival
    /// and asked it for the view, which the script has read if `to` has a link.
    pub(crate) async fn hand(&mut self, who: PlayerId, to: RegionId, chunk: ChunkPos) {
        let person = self.people.get_mut(&who).expect("the player has joined");
        let from = person.region;
        let transfer = PlayerTransfer {
            sleeping: None,
            respawn: None,
            experience: Default::default(),
            attack_ticks: None,
            using: None,
            cooldowns: Vec::new(),
            effects: Vec::new(),
            time_since_rest: 0,
            entity_id: person.entity,
            name: "Player".to_owned(),
            pose: Pose::at(within(chunk)),
            inventory: Inventory::default(),
            selected_slot: 0,
            ender: Default::default(),
            last_input: 0,
            hops: 0,
            flying: false,
            attempt: None,
            riding: None,
            keys: Default::default(),
            kept: Vec::new(),
            raid_omen_position: None,
        };
        person.region = to;
        person.chunk = chunk;
        self.model.centres.insert(who, chunk);
        self.model.forget_the_unwanted();
        let departed = Durable::Departed {
            player: who,
            transfer,
            to,
        };
        self.say(from, departed);
        self.settle(from).await;
        if self.ports[to.0 as usize].link.is_some() {
            self.settle(to).await;
        }
    }

    // ---- Links ----

    /// The link of `region` ends, and the edge has found that out when this
    /// returns.
    pub(crate) async fn lose(&mut self, region: RegionId) {
        let port = &mut self.ports[region.0 as usize];
        let mut link = port.link.take().expect("the region has a link");
        // What the edge said and nobody read: the region has it all the same.
        while let Ok(Some(message)) = link.try_recv() {
            port.note(region, &message);
        }
        drop(link);
        port.unlinked();
        let word = (region, port.epoch);
        if let Some(at) = self.ended.iter().position(|ended| *ended == word) {
            self.ended.remove(at);
            return;
        }
        loop {
            let ended = timeout(SOON, self.relinks.ended())
                .await
                .expect("the edge notices that a link ended")
                .expect("the edge is there");
            if ended == word {
                return;
            }
            self.ended.push(ended);
        }
    }

    /// Gives the edge a new link to `region`, in place of the one it has if it has
    /// one, and returns the hello the edge says on it. The region is asked for what
    /// the hello names from then on; it has yet to answer the hello
    /// ([`Script::resume`], [`Script::begin`]).
    pub(crate) async fn link(&mut self, region: RegionId) -> Hello {
        let port = &mut self.ports[region.0 as usize];
        port.unlinked();
        port.epoch += 1;
        let (link, worker) = link_to(region, port.epoch);
        port.link = Some(worker);
        assert!(self.relinks.replace(link).await, "the edge is gone");
        let hello = self.read(region).await;
        let EdgeToWorker::Hello {
            since,
            seen,
            players,
            chunks,
            guests,
            ..
        } = hello.body
        else {
            panic!("expected a hello, got {hello:?}");
        };
        let port = &mut self.ports[region.0 as usize];
        for chunk in &guests {
            port.subscriptions.insert(*chunk, (Role::Guest, 0));
        }
        // A chunk in both lists is a viewer's.
        for chunk in &chunks {
            port.subscriptions.insert(*chunk, (Role::Viewer, 0));
        }
        let hello = Hello {
            since,
            seen,
            players,
            chunks,
            guests,
        };
        port.hello = Some(hello.clone());
        hello
    }

    /// The presence answers a welcome of `region` announces: one for each player
    /// its hello named, in that order, and then one for every other player the
    /// region has, in ascending order.
    fn presences(&self, region: RegionId) -> Vec<WorkerToEdge> {
        let port = &self.ports[region.0 as usize];
        let named = port.hello.as_ref().map_or(&[][..], |hello| &hello.players);
        let here = |who: &PlayerId| (self.people.get(who)).filter(|person| person.region == region);
        let others = self.people.keys().filter(|who| !named.contains(who));
        let mut answers = Vec::new();
        for who in named.iter().chain(others) {
            let answer = match here(who) {
                Some(person) => Presence::Present {
                    riding: None,
                    entity: person.entity,
                    pose: Pose::at(within(person.chunk)),
                    inventory: Inventory::default(),
                    selected_slot: 0,
                    last_input: 0,
                    handled: None,
                    flying: false,
                    attempt: None,
                },
                None if named.contains(who) => Presence::Absent,
                None => continue,
            };
            answers.push(WorkerToEdge::Presence {
                player: *who,
                answer,
            });
        }
        answers
    }

    /// The region answers the hello of its link as one that knows the edge: its
    /// welcome, the outbox entries the edge has not seen and then `more`, and the
    /// presence answers.
    fn welcome(&mut self, region: RegionId, more: Vec<Durable>) {
        let presences = self.presences(region);
        let port = &mut self.ports[region.0 as usize];
        let seen = port.hello.as_ref().map_or(0, |hello| hello.seen);
        port.unconfirmed.retain(|(number, _)| *number > seen);
        for entry in more {
            port.outbox += 1;
            port.unconfirmed.push_back((port.outbox, entry));
        }
        let entries: Vec<_> = port.unconfirmed.iter().cloned().collect();
        let welcome = Welcome::Resumed {
            entries: entries.len() as u32,
            presences: presences.len() as u32,
            applied: port.numbered,
        };
        self.tell(region, WorkerToEdge::Welcome(welcome));
        for (number, entry) in entries {
            self.tell(region, WorkerToEdge::Outbox { number, entry });
        }
        for presence in presences {
            self.tell(region, presence);
        }
    }

    /// `region` answers the hello of its link as a region that knows the edge and
    /// has applied everything the edge sent it: with the outbox entries the edge has
    /// not seen, and with its word on each player. It then owes an answer for every
    /// chunk the hello named ([`Script::asked`]).
    pub(crate) fn resume(&mut self, region: RegionId) {
        self.welcome(region, Vec::new());
    }

    /// `region` answers the hello of its link as a region that does not know the
    /// edge: its outbox begins anew.
    pub(crate) fn begin(&mut self, region: RegionId) {
        let presences = self.presences(region);
        let port = &mut self.ports[region.0 as usize];
        port.since += 1;
        port.outbox = 0;
        port.unconfirmed.clear();
        port.numbered = 0;
        let welcome = Welcome::Unknown {
            since: port.since,
            entries: 0,
            presences: presences.len() as u32,
            applied: 0,
        };
        self.tell(region, WorkerToEdge::Welcome(welcome));
        for presence in presences {
            self.tell(region, presence);
        }
    }

    /// The region `into` absorbs `gone`, both of which have a link: both links end,
    /// the edge is given a new one to `into`, and `into` answers its hello with the
    /// word that it has absorbed `gone`, whose players are now its own. When this
    /// returns the edge has handled that, and `into` is asked for what `gone` was
    /// ([`Script::asked`]). `gone` is never linked again.
    pub(crate) async fn merge(&mut self, gone: RegionId, into: RegionId) {
        self.lose(gone).await;
        self.lose(into).await;
        let absorbed = std::mem::take(&mut self.ports[gone.0 as usize]);
        let mut entries = vec![Durable::Absorbed {
            region: gone,
            since: absorbed.since,
            applied: absorbed.numbered,
            numbers: absorbed
                .unconfirmed
                .iter()
                .map(|(number, _)| *number)
                .collect(),
        }];
        entries.extend(absorbed.unconfirmed.into_iter().map(|(_, entry)| entry));
        for person in self.people.values_mut() {
            if person.region == gone {
                person.region = into;
            }
        }
        self.link(into).await;
        self.welcome(into, entries);
        self.settle(into).await;
    }

    /// `part` is split off `from`, which has a link, with the players `movers`, who
    /// are in `from`: the link of `from` ends and comes again, `from` answers the
    /// hello with the word of the split, and the edge is given its first link to
    /// `part`, which does not know the edge. When this returns the edge has handled
    /// all of that. Which chunks went with the part is what the two then say:
    /// `from` owes an answer for every chunk its hello named, and answers
    /// `not_mine` or `elsewhere` for those that went.
    pub(crate) async fn split(&mut self, from: RegionId, part: RegionId, movers: &[PlayerId]) {
        self.lose(from).await;
        let mut stays = Vec::new();
        for who in movers {
            let person = self.people.get_mut(who).expect("the player has joined");
            assert_eq!(person.region, from, "{who:?} is not in {from:?}");
            person.region = part;
            stays.push((*who, person.entity, None));
        }
        self.link(from).await;
        let split = Durable::SplitOff {
            region: part,
            players: stays,
        };
        self.welcome(from, vec![split]);
        self.settle(from).await;
        self.link(part).await;
        self.begin(part);
        self.settle(part).await;
    }

    // ---- Clients ----

    /// A sequence number for an action of a player, above every one used before.
    fn sequence(&mut self) -> i32 {
        self.sequences += 1;
        self.sequences
    }

    async fn command(&mut self, command: Command) {
        let sent = self.commands.send(command).await;
        assert!(sent.is_ok(), "the edge's task has ended");
    }

    /// Waits until the edge has taken every command sent to it. It handles one
    /// thing at a time, so what a region is told after this is handled after them.
    async fn drained(&mut self) {
        let commands = &self.commands;
        let waited = async {
            while commands.capacity() < commands.max_capacity() {
                tokio::task::yield_now().await;
            }
        };
        timeout(SOON, waited)
            .await
            .expect("the edge takes its commands");
    }

    /// The player numbered `number` connects, and the west places them at
    /// [`HOME`]. The edge has asked the west for their view when this returns, and
    /// the script has read that. Returns their client, whose queue takes 4,096
    /// packets.
    pub(crate) async fn join(&mut self, number: u128) -> Client {
        self.join_with_room(number, 4096).await
    }

    /// As [`Script::join`], with a client whose queue takes `room` packets. Entering
    /// the world alone is some dozen packets: a client with less room than that is
    /// removed as it joins.
    pub(crate) async fn join_with_room(&mut self, number: u128, room: usize) -> Client {
        let who = player(number);
        self.sessions += 1;
        let session = SessionId(self.sessions);
        // Entities are numbered in the order stays begin.
        let entity = EntityId(100 + self.sessions);
        let (outbound, packets) = mpsc::channel(room);
        let queue = outbound.downgrade();
        let join = Command::Join {
            session,
            profile: Profile {
                uuid: who.0,
                name: format!("Player{number}"),
            },
            requested_view_distance: None,
            outbound,
            awaiting_teleport: Arc::new(AtomicI32::new(NO_TELEPORT)),
        };
        self.command(join).await;
        loop {
            let message = self.read(WEST).await;
            if matches!(message.body, EdgeToWorker::PlayerJoin(_)) {
                break;
            }
            self.ports[WEST.0 as usize].said.push_back(message);
        }
        let person = Person {
            entity,
            region: WEST,
            chunk: HOME,
        };
        self.people.insert(who, person);
        self.model.centres.insert(who, HOME);
        let spawned = WorkerToEdge::ToPlayer {
            player: who,
            event: PlayerEvent::Spawned {
                riding: None,
                attempt: session.0,
                entity_id: entity,
                pose: Pose::at(within(HOME)),
                flying: false,
                inventory: Inventory::default(),
                selected_slot: 0,
            },
        };
        self.tell(WEST, spawned);
        self.settle(WEST).await;
        Client {
            player: who,
            entity,
            session,
            packets,
            queue,
            chunks: BTreeMap::new(),
            told: Vec::new(),
            acknowledged: Vec::new(),
            batches: 0,
            confirms: true,
            disconnected: None,
            closed: false,
        }
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
    /// send it without anything else happening. Chunks come in batches, each of
    /// which the client confirms before the next comes; one that does not confirm
    /// ([`Client::confirm`]) is brought up to what came without. The edge is asked
    /// to acknowledge an action of the player that no region hears of, and what it
    /// sent before the acknowledgement is what it had to send. Panics if the edge
    /// ends the connection: [`Script::removed`] waits for that.
    pub(crate) async fn sync(&mut self, client: &mut Client) {
        loop {
            client.drain();
            let mut confirmed = false;
            while client.confirms && client.batches > 0 {
                client.batches -= 1;
                confirmed = true;
                let received = Command::ChunkBatchReceived {
                    session: client.session,
                    player: client.player,
                    chunks_per_tick: 64.0,
                };
                self.command(received).await;
            }
            let sequence = self.sequence();
            let handled = Command::Handled {
                session: client.session,
                player: client.player,
                sequence,
            };
            self.command(handled).await;
            while client.acknowledged.last() != Some(&sequence) {
                let packet = self.packet(client).await;
                client.take(packet.expect("the edge ended the connection"));
            }
            // A batch that came meanwhile is confirmed in another round.
            if !confirmed && (client.batches == 0 || !client.confirms) {
                return;
            }
        }
    }

    /// The script has the player no longer.
    fn gone(&mut self, who: PlayerId) {
        self.people.remove(&who);
        self.model.centres.remove(&who);
        self.model.forget_the_unwanted();
    }

    /// The player's connection ends from the client's side. The edge has taken that
    /// and has dropped its end of the client's queue when this returns.
    pub(crate) async fn leave(&mut self, client: &mut Client) {
        let leave = Command::Leave {
            session: client.session,
            player: client.player,
        };
        self.command(leave).await;
        self.removed(client).await;
    }

    /// Waits until the edge has ended the client's connection, as it does with a
    /// client whose queue was full when it had a packet for it. The client takes
    /// what was in its queue.
    pub(crate) async fn removed(&mut self, client: &mut Client) {
        while let Some(packet) = self.packet(client).await {
            client.take(packet);
        }
        client.closed = true;
        self.gone(client.player);
    }

    // ---- Jobs, the model, and the waits on the whole edge ----

    /// The test's handle on the jobs the edge dispatches. Panics unless the script
    /// was started with [`End::Held`].
    pub(crate) fn held(&mut self) -> &mut Held {
        let held = self.held.as_mut();
        held.expect("the script was started with the end of lighting that holds jobs")
    }

    /// The replica as the test fed it.
    pub(crate) fn model(&self) -> &Model {
        &self.model
    }

    /// Waits until the edge has ended a turn since the call, by settling a region
    /// that has a link. The edge takes what was said of jobs at the end of every
    /// turn, so every answer given before the call has been taken, and what it made
    /// room for dispatched.
    pub(crate) async fn turn(&mut self) {
        let linked = self.linked();
        let region = linked.first().expect("a region has a link");
        self.settle(*region).await;
    }

    /// Waits until the edge has taken every command and handled everything the
    /// linked regions have said so far, and what that had it say to other regions.
    /// With jobs held this is not the end of everything: see the page.
    pub(crate) async fn quiet(&mut self) {
        self.drained().await;
        // Twice, as what one region said can make the edge say something to a
        // region that was settled before.
        for _ in 0..2 {
            for region in self.linked() {
                self.settle(region).await;
            }
        }
    }

    /// Answers every job that is out with the real result, and those dispatched
    /// for that, until the edge is quiet with no job out. Returns the jobs it
    /// answered, in order.
    pub(crate) async fn answer_all(&mut self) -> Vec<LightJob> {
        let mut answered = Vec::new();
        loop {
            self.quiet().await;
            let out = self.held().out();
            if out.is_empty() {
                return answered;
            }
            for job in out {
                self.held().answer(job.basis);
                answered.push(job);
            }
        }
    }

    /// Makes sure that the edge has got through everything said to it without
    /// finding its own account wrong.
    pub(crate) async fn end(mut self) {
        self.quiet().await;
        assert!(!self.task.is_finished(), "the edge's task ended");
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::Instant;

    use super::*;
    use crate::light::{FromThread, LightOutcome};

    /// A chunk two east of [`HOME`]: in a view from there, and not beside it.
    const APART: ChunkPos = ChunkPos::new(2, 0);

    /// Heights within light are counted from the bottom of the section below the
    /// chunk.
    fn up(y: i32) -> usize {
        (y - MIN_Y + 16) as usize
    }

    /// The west sends every chunk it is asked for and that the model lacks, as
    /// `chunk` gives it.
    fn serve(script: &mut Script, chunk: impl Fn(ChunkPos) -> Chunk) {
        for position in script.asked(WEST) {
            if !script.model().has(position) {
                script.snapshot(WEST, position, chunk(position));
            }
        }
    }

    fn lit(outcome: LightOutcome) -> Light {
        match outcome {
            LightOutcome::Lit { light, .. } => (*light).clone(),
            LightOutcome::Failed { why } => panic!("the job failed: {why}"),
        }
    }

    // ---- The chunks ----

    #[test]
    fn ground_comes_with_the_light_of_the_rules_beside_its_like_and_says_so() {
        let chunk = ground();
        assert_eq!(chunk.generated(), GENERATOR);
        assert_eq!(chunk.get(5, FLOOR - 1, 5), Some(blocks::STONE));
        assert_eq!(chunk.get(5, FLOOR, 5), Some(blocks::AIR));
        let light = chunk.light().expect("ground has light");
        assert_eq!(
            (light.made_by, light.beside),
            (clustine_light::VERSION, GENERATOR)
        );
        let bare = unlit(ground());
        let due = rules(&bare, [Some(&bare); 9]);
        assert_eq!((&light.sky, &light.block), (&due.sky, &due.block));
        assert_eq!(light.sky.len(), LIGHT_SECTIONS);
        assert_eq!(chunk.sections().len(), SECTIONS);
        assert_eq!(chunk.min_y(), MIN_Y);
    }

    #[test]
    fn a_chunk_made_unlit_has_no_light_and_is_still_as_generated() {
        let chunk = unlit(ground());
        assert!(chunk.light().is_none());
        assert_eq!(chunk.generated(), GENERATOR);
        assert!(chunk.shows_the_same(&ground()));
    }

    #[test]
    fn blocks_set_that_light_sees_take_the_light_and_others_leave_it() {
        let unseen = with(ground(), &[(1, FLOOR - 1, 1, blocks::GRANITE)]);
        assert!(unseen.light().is_some());
        assert_eq!(unseen.generated(), 0);
        let seen = with(
            ground(),
            &[(1, FLOOR, 1, blocks::STONE), (2, FLOOR, 2, blocks::GLASS)],
        );
        assert!(seen.light().is_none());
        assert_eq!(seen.get(2, FLOOR, 2), Some(blocks::GLASS));
    }

    #[test]
    fn a_lamp_shines_and_a_roof_casts_a_shadow() {
        let lamp = lamp(ground(), 8, FLOOR, 8);
        assert!(lamp.light().is_none());
        let light = as_held(&rules(&lamp, [None; 9]));
        assert_eq!(light.block(8, up(FLOOR), 8), Some(15));
        assert_eq!(light.block(9, up(FLOOR), 8), Some(14));

        let roofed = roof(ground(), FLOOR + 3, (0, 0), (15, 15));
        let light = as_held(&rules(&roofed, [None; 9]));
        assert_eq!(light.sky(8, up(FLOOR), 8), Some(0));
        assert_eq!(light.sky(8, up(FLOOR + 4), 8), Some(15));
        let partly = roof(ground(), FLOOR + 3, (0, 0), (7, 15));
        assert_eq!(partly.get(7, FLOOR + 3, 0), Some(blocks::STONE));
        assert_eq!(partly.get(8, FLOOR + 3, 0), Some(blocks::AIR));
    }

    #[test]
    fn light_among_neighbours_is_the_rules_for_them_with_each_in_its_place() {
        let east = lamp(ground(), 0, FLOOR, 8);
        let mut nine = [None; 9];
        nine[5] = Some(&east);
        let chunk = lit_among(unlit(ground()), nine);
        let light = as_held(chunk.light().expect("it was given light"));
        // The lamp is beside the eastern border, and nowhere near the western one.
        assert_eq!(light.block(15, up(FLOOR), 8), Some(14));
        assert_eq!(light.block(0, up(FLOOR), 8), Some(0));
        assert_eq!(chunk.light().unwrap().beside, GENERATOR);
    }

    #[test]
    fn the_nine_around_a_chunk_are_north_west_first_and_row_by_row() {
        let nine = nine_around(ChunkPos::new(10, 20));
        assert_eq!(nine[0], ChunkPos::new(9, 19));
        assert_eq!(nine[2], ChunkPos::new(11, 19));
        assert_eq!(nine[3], ChunkPos::new(9, 20));
        assert_eq!(nine[4], ChunkPos::new(10, 20));
        assert_eq!(nine[8], ChunkPos::new(11, 21));
        assert_eq!(at(ChunkPos::new(-1, 2), 3, 7, 5), BlockPos::new(-13, 7, 37));
    }

    // ---- Regions, clients and the model, with lighting done in the turn ----

    #[tokio::test]
    async fn the_home_region_is_asked_for_the_view_the_model_has_of_a_player_who_joins() {
        let mut script = Script::start(End::Here).await;
        assert_eq!(script.linked(), REGIONS);
        let client = script.join(1).await;
        assert_eq!(client.player, player(1));
        assert_eq!(script.asked(WEST), view(HOME, VIEW));
        assert_eq!(script.model().view(player(1)), view(HOME, VIEW));
        assert_eq!(script.model().wanted(), view(HOME, VIEW));
        assert_eq!(script.role(WEST, HOME), Some(Role::Viewer));
        assert!(script.asked(EAST).is_empty());
        assert!(script.model().positions().is_empty());
        script.end().await;
    }

    /// The first example of the page. Section 4 of the record: a chunk with carried
    /// light that is current is sent at once, neighbours or none; section 5, the
    /// table's first row, and the note on L1's comparison: a chunk that is lit anew
    /// because a chunk came beside it is sent as an update to those who have it.
    #[tokio::test]
    async fn a_chunk_and_then_its_neighbour_with_a_lamp_leave_a_client_the_light_of_both() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let carried = ground().take_light().unwrap();
        assert_eq!(client.light(HOME), Some(&as_held(&carried)));
        assert_eq!(client.block(at(HOME, 0, FLOOR - 1, 0)), Some(blocks::STONE));

        script.snapshot(WEST, EAST_OF_HOME, lamp(ground(), 0, FLOOR, 8));
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(
            client.block(at(EAST_OF_HOME, 0, FLOOR, 8)),
            Some(blocks::GLOWSTONE)
        );
        for chunk in [HOME, EAST_OF_HOME] {
            let due = script.model().light(chunk).unwrap();
            assert_eq!(client.light(chunk), Some(&as_held(&due)), "{chunk:?}");
        }
        // The lamp's light has come over the border.
        let light = client.light(HOME).unwrap();
        assert_eq!(light.block(15, up(FLOOR), 8), Some(14));
        assert_eq!(client.times_sent(HOME), 1);
        assert!(matches!(
            client.told_of(HOME)[..],
            [Told::Sent(_), Told::Light(..)]
        ));
        assert_eq!(client.chunks(), script.model().positions());
        script.end().await;
    }

    #[tokio::test]
    async fn block_changes_of_one_message_reach_the_client_and_the_model() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, ground());
        let (stone, glass) = (at(HOME, 3, FLOOR, 3), at(HOME, 4, FLOOR, 4));
        // One for a chunk that has not come, which the edge and the model pass over.
        let nowhere = at(APART, 1, FLOOR, 1);
        script.change(
            WEST,
            &[
                (stone, blocks::STONE),
                (glass, blocks::GLASS),
                (nowhere, blocks::STONE),
            ],
        );
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.block(stone), Some(blocks::STONE));
        assert_eq!(client.block(glass), Some(blocks::GLASS));
        assert_eq!(client.block(nowhere), None);
        let model = script.model();
        let (now, came) = (model.chunk(HOME).unwrap(), model.came(HOME).unwrap());
        assert_eq!(now.get(4, FLOOR, 4), Some(blocks::GLASS));
        assert_eq!(came.get(4, FLOOR, 4), Some(blocks::AIR));
        assert!(came.light().is_some());
        assert!(!model.has(APART));
        let told = client.told_of(HOME);
        assert!(
            told.contains(&Told::Block(stone, blocks::STONE)),
            "{told:?}"
        );
        assert!(
            told.contains(&Told::Block(glass, blocks::GLASS)),
            "{told:?}"
        );
        script.end().await;
    }

    /// Section 6 of the record: a batch is offered only while none is outstanding.
    #[tokio::test]
    async fn a_client_that_does_not_confirm_is_sent_one_batch_and_the_rest_when_it_does() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        client.confirm(false);
        serve(&mut script, |_| ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let wanted = script.model().wanted();
        let finished = |client: &Client| {
            let batch = |told: &&Told| matches!(told, Told::BatchFinished(_));
            client.told().iter().filter(batch).count()
        };
        assert_eq!(finished(&client), 1);
        assert!(client.chunks().len() < wanted.len());
        assert_eq!(client.told().first(), Some(&Told::BatchStarted));

        client.confirm(true);
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), wanted);
        assert!(finished(&client) > 1);
        // What a batch says it had is what came between its marks.
        let mut within = 0;
        for told in client.told() {
            match told {
                Told::BatchStarted => within = 0,
                Told::Sent(_) => within += 1,
                Told::BatchFinished(count) => assert_eq!(*count, within),
                _ => {}
            }
        }
        script.end().await;
    }

    /// Section 3 of the record, E6, and section 6: a view that moves away unloads.
    #[tokio::test]
    async fn a_walk_moves_the_view_of_the_edge_and_of_the_model() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        serve(&mut script, |_| ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let before = view(HOME, VIEW);
        assert_eq!(client.chunks(), before);

        script.walk(player(1), EAST_OF_HOME);
        script.quiet().await;
        let after = view(EAST_OF_HOME, VIEW);
        assert_eq!(script.asked(WEST), after);
        assert_eq!(script.model().view(player(1)), after);
        let kept: BTreeSet<_> = before.intersection(&after).copied().collect();
        assert_eq!(script.model().positions(), kept);
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), kept);
        let left = ChunkPos::new(-3, 0);
        assert_eq!(client.told_of(left).last(), Some(&Told::Unloaded(left)));

        serve(&mut script, |_| ground());
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), after);
        script.end().await;
    }

    /// Section 3 of the record, E2: a snapshot that shows the same is nothing,
    /// whichever region it is from.
    #[tokio::test]
    async fn the_region_a_player_is_handed_to_is_asked_for_their_view_and_can_send_the_same() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let told = client.told_of(HOME);

        script.hand(player(1), EAST, EAST_OF_HOME).await;
        script.quiet().await;
        assert_eq!(script.asked(EAST), view(EAST_OF_HOME, VIEW));
        assert_eq!(script.role(EAST, HOME), Some(Role::Viewer));
        script.snapshot(EAST, HOME, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.told_of(HOME), told);
        assert!(client.has(HOME));

        // And the east is the player's region now: its word moves their view.
        script.walk(player(1), HOME);
        script.quiet().await;
        assert_eq!(script.asked(EAST), view(HOME, VIEW));
        script.end().await;
    }

    /// Section 3 of the record, E2 and E3; section 6: a chunk sent again is
    /// unloaded first.
    #[tokio::test]
    async fn a_region_that_resumes_sends_the_same_chunk_for_nothing_and_a_changed_one_anew() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, ground());
        script.snapshot(WEST, APART, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let told = client.told().len();

        script.lose(WEST).await;
        assert!(!script.linked().contains(&WEST));
        let hello = script.link(WEST).await;
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(hello.since, 1);
        let named: BTreeSet<_> = hello.chunks.iter().copied().collect();
        assert_eq!(named, view(HOME, VIEW));
        assert!(hello.guests.is_empty());
        assert_eq!(script.asked(WEST), view(HOME, VIEW));
        script.resume(WEST);
        script.snapshot(WEST, HOME, ground());
        let changed = with(ground(), &[(2, FLOOR, 2, blocks::STONE)]);
        script.snapshot(WEST, APART, changed);
        script.quiet().await;
        script.sync(&mut client).await;
        assert!(client.connected());
        let of_chunks = |told: &&Told| told.chunk().is_some();
        let since: Vec<_> = client.told()[told..].iter().filter(of_chunks).collect();
        assert_eq!(since, [&Told::Unloaded(APART), &Told::Sent(APART)]);
        assert_eq!(client.block(at(APART, 2, FLOOR, 2)), Some(blocks::STONE));
        let due = script.model().light(APART).unwrap();
        assert_eq!(client.light(APART), Some(&as_held(&due)));
        let now = script.model().chunk(APART).unwrap();
        assert_eq!(now.get(2, FLOOR, 2), Some(blocks::STONE));
        script.end().await;
    }

    /// A chunk that another region holds, served by it; then that region is
    /// absorbed, and the survivor serves the same chunk. Section 3 of the record,
    /// E2: it is the same whichever region the snapshot is from.
    #[tokio::test]
    async fn after_a_merge_the_survivor_is_asked_for_the_chunk_and_sends_the_same_for_nothing() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.elsewhere(WEST, APART, NORTH);
        script.settle(WEST).await;
        script.settle(NORTH).await;
        assert_eq!(script.role(NORTH, APART), Some(Role::Guest));
        script.snapshot(NORTH, APART, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        assert!(client.has(APART));
        let told = client.told().to_vec();

        script.merge(NORTH, EAST).await;
        assert!(!script.linked().contains(&NORTH));
        assert_eq!(script.role(EAST, APART), Some(Role::Guest));
        script.snapshot(EAST, APART, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.told(), told);

        // The survivor's word reaches the chunk as the absorbed region's did.
        script.change(EAST, &[(at(APART, 1, FLOOR, 1), blocks::STONE)]);
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.block(at(APART, 1, FLOOR, 1)), Some(blocks::STONE));
        script.end().await;
    }

    /// Section 3 of the record, E2, after a split: the part sends what the region
    /// it was split off had sent.
    #[tokio::test]
    async fn after_a_split_the_part_is_asked_for_its_players_view_and_sends_the_same_for_nothing() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.hand(player(1), EAST, EAST_OF_HOME).await;
        script.snapshot(EAST, HOME, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let told = client.told().to_vec();
        assert_eq!(
            told,
            [Told::BatchStarted, Told::Sent(HOME), Told::BatchFinished(1)]
        );

        script.split(EAST, PART, &[player(1)]).await;
        script.quiet().await;
        assert_eq!(script.linked(), [WEST, EAST, NORTH, SOUTH, PART]);
        assert_eq!(script.asked(PART), view(EAST_OF_HOME, VIEW));
        script.snapshot(PART, HOME, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        assert!(client.connected());
        assert_eq!(client.told(), told);
        // The part is the player's region now.
        script.walk(player(1), HOME);
        script.quiet().await;
        assert_eq!(script.asked(PART), view(HOME, VIEW));
        script.end().await;
    }

    /// Section 3 of the record, E2, after `NotMine`: the region that held a chunk
    /// for another gives it up, the viewer's region is asked again and sends the
    /// same chunk itself.
    #[tokio::test(start_paused = true)]
    async fn after_not_mine_the_region_asked_again_sends_the_same_for_nothing() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join(1).await;
        script.elsewhere(WEST, APART, EAST);
        script.settle(WEST).await;
        script.settle(EAST).await;
        script.snapshot(EAST, APART, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        let told = client.told().to_vec();
        assert!(client.has(APART));
        // What the west was told up to here is looked at no more.
        script.said(WEST).await;

        script.not_mine(EAST, APART);
        assert_eq!(script.role(EAST, APART), None);
        script.quiet().await;
        // The edge asks a region again at most once a second.
        tokio::time::advance(Duration::from_secs(2)).await;
        let said = script.said(WEST).await;
        let again = |message: &EdgeMessage| matches!(&message.body, EdgeToWorker::Subscribe { chunks, .. } if chunks == &[APART]);
        assert!(said.iter().any(again), "{said:?}");
        script.snapshot(WEST, APART, ground());
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.told(), told);
        script.end().await;
    }

    #[tokio::test]
    async fn a_queue_has_the_room_it_was_given_and_is_filled_and_drained_by_the_test() {
        let mut script = Script::start(End::Here).await;
        let mut client = script.join_with_room(1, 64).await;
        script.sync(&mut client).await;
        assert_eq!(client.room(), 64);
        assert_eq!(client.fill_leaving(3), 61);
        assert_eq!(client.room(), 3);
        assert_eq!(client.fill(), 3);
        assert_eq!(client.room(), 0);
        client.drain();
        assert_eq!(client.room(), 64);
        assert!(client.told().is_empty());
        assert!(client.connected());
        assert_eq!(client.disconnected(), None);
        assert_eq!(client.entity, EntityId(101));

        script.leave(&mut client).await;
        assert!(!client.connected());
        assert_eq!(client.room(), 0);
        assert!(script.model().wanted().is_empty());
        script.end().await;
    }

    // ---- Jobs that are held ----

    /// The second example of the page. The record's L3: no chunk is sent without
    /// light; section 5, the table's second row: under a block change those who
    /// wait are sent the chunk as its blocks are now, with the job's light, and the
    /// chunk is stale, so that another job follows.
    #[tokio::test]
    async fn a_block_change_under_a_held_job_is_in_the_chunk_that_is_sent_and_has_a_job_follow() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, unlit(ground()));
        let job = script.held().next().await;
        assert_eq!(job.position, HOME);
        assert!(!job.whole && job.old.is_none());
        assert!(script.model().same_nine(&job));
        assert!(job.nine[4].is_some());
        assert_eq!(job.nine.iter().flatten().count(), 1);
        script.quiet().await;
        script.sync(&mut client).await;
        assert!(!client.has(HOME));

        let torch = at(HOME, 3, FLOOR, 3);
        script.change(WEST, &[(torch, blocks::TORCH)]);
        script.settle(WEST).await;
        assert!(!script.model().same_nine(&job));
        let first = lit(script.held().answer(job.basis));
        script.turn().await;
        script.sync(&mut client).await;
        assert_eq!(client.block(torch), Some(blocks::TORCH));
        assert_eq!(client.light(HOME), Some(&as_held(&first)));

        let next = script.held().next().await;
        assert_eq!(next.position, HOME);
        assert!(next.basis > job.basis);
        assert!(script.model().same_nine(&next));
        assert_eq!(script.answer_all().await.len(), 1);
        script.sync(&mut client).await;
        // The record's L1, for a client at the rim of whose view the chunk is.
        let due = script.model().light(HOME).unwrap();
        assert_eq!(client.light(HOME), Some(&as_held(&due)));
        assert_eq!(client.light(HOME).unwrap().block(3, up(FLOOR), 3), Some(14));
        assert_eq!(client.times_sent(HOME), 1);
        script.end().await;
    }

    /// The third example of the page. Section 5 of the record: a player whose queue
    /// is full at their turn is removed and the others still get theirs.
    #[tokio::test]
    async fn a_waiter_whose_queue_is_full_is_removed_and_the_one_behind_is_sent_the_chunk() {
        let mut script = Script::start(End::Held).await;
        let mut first = script.join(1).await;
        let mut second = script.join(2).await;
        script.snapshot(WEST, HOME, unlit(ground()));
        let job = script.held().next().await;
        script.sync(&mut first).await;
        assert!(first.fill() > 0);
        script.held().answer(job.basis);
        script.turn().await;
        script.removed(&mut first).await;
        assert!(!first.connected());
        assert!(!first.has(HOME));
        assert_eq!(script.model().view(player(1)), BTreeSet::new());
        script.sync(&mut second).await;
        assert!(second.has(HOME));
        assert!(script.model().has(HOME));
        script.end().await;
    }

    /// Section 5 of the record: a result is taken by its job, in whatever order
    /// results come; one that is late is dropped; a job that is not answered leaves
    /// its chunk unsent.
    #[tokio::test]
    async fn held_jobs_are_answered_late_out_of_order_or_never() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        let far = ChunkPos::new(0, 2);
        for chunk in [HOME, APART, far] {
            script.snapshot(WEST, chunk, unlit(ground()));
        }
        script.quiet().await;
        let fresh = script.held().fresh();
        let places: Vec<_> = fresh.iter().map(|job| job.position).collect();
        assert_eq!(places, [HOME, APART, far]);
        assert!(script.held().fresh().is_empty());
        assert_eq!(script.held().out().len(), 3);
        let [home, apart, never] = &fresh[..] else {
            panic!("three jobs");
        };
        let found = script.held().job_for(APART);
        assert_eq!(found.map(|job| job.basis), Some(apart.basis));

        script.held().started(home.basis);
        let light = lit(script.held().answer(apart.basis));
        script.turn().await;
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), BTreeSet::from([APART]));
        assert_eq!(client.light(APART), Some(&as_held(&light)));

        script.held().answer(home.basis);
        assert_eq!(script.held().never(never.basis).position, far);
        assert!(script.held().out().is_empty());
        script.turn().await;
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), BTreeSet::from([HOME, APART]));

        // A result that comes a second time is nobody's.
        let told = client.told().to_vec();
        script.held().say(FromThread::Done {
            basis: home.basis,
            position: HOME,
            outcome: LightOutcome::Failed {
                why: "said twice".to_owned(),
            },
        });
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(client.told(), told);
        assert!(script.held().fresh().is_empty());
        script.end().await;
    }

    /// Section 5 of the record, "A failed job": those who wait are sent the chunk,
    /// lit by its heights; no job is made for it until something marks it.
    #[tokio::test]
    async fn a_failed_job_has_the_chunk_sent_with_the_light_of_its_heights_and_no_job_follow() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        let chunk = roof(unlit(ground()), FLOOR + 3, (0, 0), (15, 15));
        script.snapshot(WEST, HOME, chunk.clone());
        let job = script.held().next().await;
        script.held().fail(job.basis, "the test has it fail");
        script.quiet().await;
        script.sync(&mut client).await;
        assert!(client.has(HOME));
        let by_heights = Light {
            made_by: 0,
            beside: 0,
            sky: clustine_world::sky_light(&chunk),
            block: Vec::new(),
        };
        assert_eq!(client.light(HOME).unwrap().sky, as_held(&by_heights).sky);
        assert!(script.held().out().is_empty());

        // A change that light sees marks it, and the job for that is answered.
        script.change(WEST, &[(at(HOME, 3, FLOOR, 3), blocks::TORCH)]);
        let next = script.held().next().await;
        assert_eq!(next.position, HOME);
        script.answer_all().await;
        script.sync(&mut client).await;
        let due = script.model().light(HOME).unwrap();
        assert_eq!(client.light(HOME), Some(&as_held(&due)));
        script.end().await;
    }

    /// Section 5 of the record: with no thread left, a job fails at its dispatch.
    #[tokio::test]
    async fn with_the_end_shut_a_job_fails_at_its_dispatch_and_the_chunk_is_sent() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        script.held().shut(true);
        script.snapshot(WEST, HOME, unlit(ground()));
        script.quiet().await;
        script.sync(&mut client).await;
        assert!(client.has(HOME));
        assert!(script.held().out().is_empty());

        script.held().shut(false);
        script.snapshot(WEST, APART, unlit(ground()));
        assert_eq!(script.held().next().await.position, APART);
        script.end().await;
    }

    /// Section 4 of the record: a job is dispatched while fewer than `IN_FLIGHT`
    /// are under way.
    #[tokio::test]
    async fn no_more_jobs_are_out_than_the_test_allows() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        // With none allowed, nothing is dispatched until the test allows one.
        script.held().set_in_flight(0);
        script.snapshot(WEST, HOME, unlit(ground()));
        script.snapshot(WEST, APART, unlit(ground()));
        script.quiet().await;
        assert!(script.held().out().is_empty());
        script.held().set_in_flight(1);
        script.turn().await;
        let out = script.held().out();
        assert_eq!(out.len(), 1);
        script.held().answer(out[0].basis);
        script.turn().await;
        let then = script.held().out();
        assert_eq!(then.len(), 1);
        assert_ne!(then[0].position, out[0].position);
        let answered = script.answer_all().await;
        assert_eq!(answered.len(), 1);
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), BTreeSet::from([HOME, APART]));
        script.end().await;
    }

    #[tokio::test]
    async fn a_job_answered_with_a_given_light_leaves_the_client_that_light() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, unlit(ground()));
        let job = script.held().next().await;
        let mut given = script.model().light(HOME).unwrap();
        given.block[3] = SectionLight::Full;
        assert_ne!(Some(&given), script.model().light(HOME).as_ref());
        let said = lit(script.held().answer_with(job.basis, given.clone()));
        assert_eq!(said, given);
        script.turn().await;
        script.sync(&mut client).await;
        assert_eq!(client.light(HOME), Some(&as_held(&given)));
        assert_eq!(client.light(HOME).unwrap().block(0, 3 * 16, 0), Some(15));
        script.end().await;
    }

    /// The record's L1 and L2 where the script has made the edge quiet: a join into
    /// terrain without light, every job answered.
    #[tokio::test]
    async fn with_every_job_answered_a_client_holds_the_light_of_the_model_for_its_whole_view() {
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        serve(&mut script, |position| {
            let chunk = unlit(ground());
            // A lamp in every other chunk, at its eastern border.
            if position.x % 2 == 0 {
                lamp(chunk, 15, FLOOR, 8)
            } else {
                chunk
            }
        });
        let answered = script.answer_all().await;
        assert!(answered.len() >= view(HOME, VIEW).len());
        assert!(script.held().out().is_empty());
        script.sync(&mut client).await;
        assert_eq!(client.chunks(), view(HOME, VIEW));
        for chunk in client.chunks() {
            let due = script.model().light(chunk).unwrap();
            assert_eq!(client.light(chunk), Some(&as_held(&due)), "{chunk:?}");
        }
        script.end().await;
    }

    /// The page's word on the clock: no wait moves a paused clock while what it
    /// waits for comes, and the edge is still there when the test moves it.
    #[tokio::test(start_paused = true)]
    async fn the_waits_do_not_move_a_paused_clock() {
        let began = Instant::now();
        let mut script = Script::start(End::Held).await;
        let mut client = script.join(1).await;
        script.snapshot(WEST, HOME, unlit(ground()));
        let job = script.held().next().await;
        script.change(WEST, &[(at(HOME, 3, FLOOR, 3), blocks::TORCH)]);
        script.settle(WEST).await;
        script.held().answer(job.basis);
        script.turn().await;
        script.sync(&mut client).await;
        script.answer_all().await;
        script.lose(EAST).await;
        script.link(EAST).await;
        script.resume(EAST);
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(Instant::now(), began);

        tokio::time::advance(Duration::from_secs(3600)).await;
        script.quiet().await;
        script.sync(&mut client).await;
        assert_eq!(Instant::now(), began + Duration::from_secs(3600));
        assert!(client.connected());
        script.end().await;
    }
}
