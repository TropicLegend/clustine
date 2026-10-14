//! The orders of events around light at the edge: test 4 of
//! `docs/adr/0023-light-at-the-edge.md` ("the record" below), written from the record
//! and the page at the head of `script.rs` by someone who has not seen the fan-out.
//!
//! **What a test here is.** A [`Play`] is a script (regions, clients and held light
//! jobs) and beside it an account of what the record says the edge's replica is after
//! each thing said: for every chunk whether it may be sent, which light it has, why it
//! is marked, and for every client which chunks it has and for which of them the
//! record promises that it holds exactly the entry's light. One thing is said at a
//! time; then the edge is waited for, every client brought up to date, and the account
//! held against what the clients were sent and which jobs were dispatched. The account
//! is made from the record alone. Nothing in it follows what the edge happens to do
//! but which jobs it dispatches and when, which the record leaves open in part (the
//! hold and the rest are not built), and which are then held to the record in turn.
//!
//! **What is checked after every step** (the record's "What it can see"):
//!
//! - L3: a chunk is sent only from an entry that is sendable by the record: it has
//!   light, and no chunk came or was replaced in its three by three since that light
//!   was made (section 1). And the other way round: a chunk a client waits for that is
//!   sendable has been sent once the client is brought up to date (section 6).
//! - A chunk that is sent is sent with the entry's light and the blocks as they are.
//! - No batch is empty; a chunk a client has is not sent again without an unload, and
//!   that unload comes immediately before the batch that carries the chunk
//!   (section 6).
//! - A light update comes only in the step in which a result for its chunk is taken,
//!   once at most, never for a chunk the client does not have or waits to have
//!   replaced, never at a withheld or failed result (sections 5 and 6); it names every
//!   listed section if its job was to (section 5).
//! - A job is for a chunk of the replica that is marked, one a chunk at a time; its
//!   nine chunks are the model's, its old light the entry's, and it is whole if a
//!   result was withheld from a holder who was promised it, or failed (sections 4
//!   and 5; section 5 says "if anyone has the chunk", which since "Found while
//!   building" can only mean anyone who would have been sent the result).
//! - Whoever has the chunk of a block that changed, or waits to have it replaced, is
//!   sent the block update (sections 3 and 6).
//! - L1, for every client and chunk the record promises it for: see below.
//!
//! **And where every job is answered** ([`Play::whole_check`]): L2 and L4 as far as
//! they show. Every chunk of the replica is sendable and nothing is left marked; an
//! entry whose light was computed has the light of the rules for the model's chunks
//! as they are now (and may list more sections, if it listed them before: section 6).
//!
//! **Which light a holder is promised.** L1 says: exactly the entry's light, or the
//! entry owes them. "Found while building" narrows who is sent an update, and the
//! account follows it as far as it is said:
//!
//! - A client that is sent a chunk holds the entry's light of that moment.
//! - At a result that is taken and not withheld, a holder is **promised** the update
//!   if a chunk came or was replaced beside the chunk since its light was made (every
//!   holder then), or if a block change the job was owed for found the holder lacking
//!   a chunk beside this one that the change reaches (or that the change was in), and
//!   it still lacks it. A promised holder that held the entry's light, or whose update
//!   is whole, holds the entry's light afterwards.
//! - A holder that is not promised is sent the update or not, and nothing is asked of
//!   its light from then on (its own client lit the change) until it is sent the
//!   chunk again or a whole update it is promised.
//! - At a withheld or failed result holders are sent nothing, and what the job was
//!   owed for is owed by the next.
//! - A client that is sent a chunk with light older than its blocks (section 5, the
//!   table's second row) has not lit that change by itself. It is promised the next
//!   result if it is at the rim; whether it is when it has every chunk around is
//!   where section 8 and "Found while building" differ, and is a finding below. The
//!   random orders ask nothing of it.
//!
//! **Findings** are the tests marked `#[ignore = "finding: ..."]`: each is kept as the
//! record has it and fails today.
//!
//! What the record does not say is listed in the report of step L6; where it is
//! silent the account asks nothing.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use clustine_botswarm::Named;
use clustine_data::{BlockState, blocks};
use clustine_region::RegionId;
use clustine_world::{
    BlockPos, Chunk, ChunkPos, Light, SectionLight, light_tells_apart, sky_light,
};

use crate::light::{LightJob, LightOutcome};
use crate::script::{
    Client, EAST, EAST_OF_HOME, End, FLOOR, HOME, MIN_Y, NORTH, Script, Told, VIEW, WEST, as_held,
    at, ground, lamp, lit_among, nine_around, roof, unlit, view, with,
};

// ---------------------------------------------------------------------------------
// The steps of a test, for the message of a failure
// ---------------------------------------------------------------------------------

thread_local! {
    /// What the test on this thread has said so far, in order.
    static STEPS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn note(step: String) {
    STEPS.with(|steps| steps.borrow_mut().push(step));
}

fn steps() -> String {
    STEPS.with(|steps| steps.borrow().join("\n"))
}

/// The edge did what the record does not allow: panics with why. The steps before
/// are printed as the play is dropped, as they are when the edge's own check of its
/// account ends its task and a wait of the script panics.
macro_rules! broken {
    ($($why:tt)*) => {
        panic!($($why)*)
    };
}

impl Drop for Play {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("--- the steps before:\n{}\n---", steps());
        }
    }
}

// ---------------------------------------------------------------------------------
// The account
// ---------------------------------------------------------------------------------

/// What an entry's light is (the record's `LightKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The chunk has come and has no light yet.
    Unlit,
    Carried,
    Computed,
    Fallback,
}

/// A block change that light sees and that reached a chunk: for each client that had
/// the chunk then, the chunks beside it that the change reaches and the client lacked.
#[derive(Debug, Clone)]
struct Mark {
    /// The chunks beside the marked one that the change reaches, or was in.
    reached: BTreeSet<ChunkPos>,
    lacking: BTreeMap<usize, BTreeSet<ChunkPos>>,
}

/// Why an entry is marked.
#[derive(Debug, Clone, Default)]
struct Causes {
    /// Its own chunk came, without light that may be sent.
    arrived: bool,
    /// A chunk came or was replaced beside it.
    beside: bool,
    changes: Vec<Mark>,
}

impl Causes {
    fn any(&self) -> bool {
        self.shaped() || !self.changes.is_empty()
    }

    /// Whether the chunks light is made from have changed (the record's `shaped`).
    fn shaped(&self) -> bool {
        self.arrived || self.beside
    }

    fn merge(&mut self, other: Causes) {
        self.arrived |= other.arrived;
        self.beside |= other.beside;
        self.changes.extend(other.changes);
    }
}

/// An entry of the replica as the record has it, as far as a test can hold it to.
#[derive(Debug)]
struct Entry {
    /// The generator's version whose blocks, as light sees them, the chunk has.
    as_made: Option<u16>,
    /// The `beside` of the light it carries, while it is `Carried`.
    beside: Option<u16>,
    light: Option<Light>,
    kind: Kind,
    /// It has light, and no chunk came or was replaced in its three by three since.
    sendable: bool,
    /// The job under way whose result would be taken.
    job: Option<u64>,
    /// What that job was dispatched for.
    job_causes: Causes,
    /// What has marked the entry since the last dispatch.
    pending: Causes,
    /// What results that were withheld or failed were owed for: the next result
    /// that is sent is owed to the same holders.
    carry: Causes,
    owed: bool,
    /// The clients that were sent the chunk with light older than its blocks.
    sent_stale: BTreeSet<usize>,
    /// The sections its light has listed since the chunk came: sky, block.
    listed: (u64, u64),
    /// The nine chunks its computed light was made from.
    made_from: Option<[Option<Arc<Chunk>>; 9]>,
}

impl Entry {
    fn current(&self) -> bool {
        self.sendable && !self.pending.any() && self.job.is_none()
    }

    /// Marked by a block change, with or without a job for it under way.
    fn stale(&self) -> bool {
        !self.pending.changes.is_empty()
            || (self.job.is_some() && !self.job_causes.changes.is_empty())
    }

    fn put(&mut self, light: Light, kind: Kind) {
        self.listed.0 |= listed(&light.sky);
        self.listed.1 |= listed(&light.block);
        self.light = Some(light);
        self.kind = kind;
    }
}

/// The sections of a list that are not absent, as bits.
fn listed(sections: &[SectionLight]) -> u64 {
    let listed = |(_, section): &(usize, &SectionLight)| **section != SectionLight::Absent;
    let bits = sections.iter().enumerate().filter(listed);
    bits.fold(0, |mask, (index, _)| mask | 1 << index)
}

fn same_light(one: &Light, other: &Light) -> bool {
    one.sky == other.sky && one.block == other.block
}

/// What a failed job leaves an entry that had no light (section 5): the sky's light
/// by the chunk's heights, and block light dark wherever sky light is listed.
fn by_heights(chunk: &Chunk) -> Light {
    let sky = sky_light(chunk);
    let dark = |section: &SectionLight| match section {
        SectionLight::Absent => SectionLight::Absent,
        _ => SectionLight::Dark,
    };
    let block = sky.iter().map(dark).collect();
    Light {
        made_by: 0,
        beside: 0,
        sky,
        block,
    }
}

/// The eight places around `position`, north-west first, row by row: the order of
/// `clustine_light::reach`.
fn around(position: ChunkPos) -> [ChunkPos; 8] {
    let nine = nine_around(position);
    [
        nine[0], nine[1], nine[2], nine[3], nine[5], nine[6], nine[7], nine[8],
    ]
}

/// Heights within light are counted from the bottom of the section below the chunk.
fn up(y: i32) -> usize {
    (y - MIN_Y + 16) as usize
}

/// What a client is to be sent of light in a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Nothing: a withheld or failed result, or one in which nothing differs.
    Nothing,
    /// The update: the record promises it to this holder.
    One,
    /// The record does not say whether this holder is sent the update.
    Either,
}

/// What a step may bring besides chunks and blocks.
#[derive(Debug, Default)]
struct Ctx {
    /// The chunk a result was taken for in this step, and what each client that has
    /// it is to be sent of it.
    result: Option<(ChunkPos, BTreeMap<usize, Expect>)>,
    /// The sections the update names if it is whole: sky, block.
    whole: Option<(u64, u64)>,
    /// The blocks that changed in this step: whoever has the chunk of one, or waits
    /// to have it replaced, is sent the block update (sections 3 and 6).
    blocks: Vec<(BlockPos, BlockState)>,
}

/// What a test keeps of a client beside the client itself.
#[derive(Debug, Default)]
struct Watch {
    /// How much of what the client was told has been looked at.
    seen: usize,
    /// The chunks it has, by what it was told.
    has: BTreeSet<ChunkPos>,
    /// Those of them that were replaced in the replica since (the record's `replace`).
    replace: BTreeSet<ChunkPos>,
    /// The chunks of which the record promises that it holds the entry's light.
    must: BTreeSet<ChunkPos>,
    /// The edge serves it no longer.
    gone: bool,
    /// It does not confirm its batches now, so it is not sent all it could be.
    lags: bool,
    /// The chunks it was sent in this step, whose blocks are yet to be looked at.
    fresh: Vec<ChunkPos>,
}

/// How much of each thing a play has seen, so that a run can say what it looked at.
#[derive(Debug, Default, Clone, Copy)]
struct Seen {
    steps: usize,
    chunks_sent: usize,
    chunks_sent_again: usize,
    light_updates: usize,
    updates_promised: usize,
    results_withheld: usize,
    results_dropped: usize,
    jobs_failed: usize,
    lights_compared: usize,
}

impl Seen {
    fn add(&mut self, other: Seen) {
        self.steps += other.steps;
        self.chunks_sent += other.chunks_sent;
        self.chunks_sent_again += other.chunks_sent_again;
        self.light_updates += other.light_updates;
        self.updates_promised += other.updates_promised;
        self.results_withheld += other.results_withheld;
        self.results_dropped += other.results_dropped;
        self.jobs_failed += other.jobs_failed;
        self.lights_compared += other.lights_compared;
    }
}

/// How a held job is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    Rules,
    Failure,
}

/// A script and the account of what the record says of it; see the head of the file.
struct Play {
    script: Script,
    clients: Vec<Client>,
    watch: Vec<Watch>,
    entries: BTreeMap<ChunkPos, Entry>,
    /// The blocks a test has set or changed, by chunk: what a client's blocks are
    /// compared at.
    touched: BTreeMap<ChunkPos, BTreeSet<BlockPos>>,
    /// Every job dispatched, in order.
    dispatched: Vec<(u64, ChunkPos)>,
    in_flight: usize,
    /// Whether a holder that was sent a chunk with light older than its blocks is
    /// taken to be promised the next result (section 8's table of outcomes).
    stale_is_promised: bool,
    seen: Seen,
}

impl Play {
    async fn start() -> Self {
        STEPS.with(|steps| steps.borrow_mut().clear());
        Self {
            script: Script::start(End::Held).await,
            clients: Vec::new(),
            watch: Vec::new(),
            entries: BTreeMap::new(),
            touched: BTreeMap::new(),
            dispatched: Vec::new(),
            in_flight: 4,
            stale_is_promised: true,
            seen: Seen::default(),
        }
    }

    // ---- What a test says ----

    /// A player joins at [`HOME`]. Returns the number of their client here.
    async fn join(&mut self, number: u128) -> usize {
        note(format!("player {number} joins"));
        let client = self.script.join(number).await;
        self.clients.push(client);
        self.watch.push(Watch::default());
        self.step(Ctx::default()).await;
        self.clients.len() - 1
    }

    fn id(&self, who: usize) -> clustine_world::PlayerId {
        self.clients[who].player
    }

    fn live(&self) -> Vec<usize> {
        let live = |who: &usize| !self.watch[*who].gone;
        (0..self.clients.len()).filter(live).collect()
    }

    /// The region sends `chunk` for `position`: E1, E2 or E3 of section 3.
    async fn snapshot(&mut self, region: RegionId, position: ChunkPos, chunk: Chunk) {
        self.snapshot_called(region, position, chunk, "a chunk")
            .await;
    }

    async fn snapshot_called(&mut self, region: RegionId, at: ChunkPos, chunk: Chunk, what: &str) {
        let model = self.script.model();
        let event = match model.chunk(at) {
            _ if !model.wanted().contains(&at) => "nobody wants it",
            None => "E1",
            Some(there) if there.shows_the_same(&chunk) => "E2",
            Some(_) => "E3",
        };
        note(format!(
            "{region:?} sends {what} for {at:?} ({event}); light: {}, generated by {}",
            chunk.light().map_or("none".to_owned(), |light| format!(
                "made by {} beside {}",
                light.made_by, light.beside
            )),
            chunk.generated(),
        ));
        self.script.snapshot(region, at, chunk.clone());
        if event == "E1" || event == "E3" {
            self.arrive(at, &chunk);
        }
        self.step(Ctx::default()).await;
    }

    /// Section 3, arrive.
    fn arrive(&mut self, position: ChunkPos, chunk: &Chunk) {
        let light = chunk.light().cloned();
        let as_made = match &light {
            Some(light) if light.beside != 0 => Some(light.beside),
            _ => (chunk.generated() != 0).then(|| chunk.generated()),
        };
        // Section 7: acceptable light, with a `beside`.
        let acceptable = |light: &Light| {
            (light.made_by == clustine_light::VERSION || light.made_by == Light::IMPORTED)
                && light.beside != 0
        };
        let mut entry = Entry {
            as_made,
            beside: None,
            light: None,
            kind: Kind::Unlit,
            sendable: false,
            job: None,
            job_causes: Causes::default(),
            pending: Causes::default(),
            carry: Causes::default(),
            owed: false,
            sent_stale: BTreeSet::new(),
            listed: (0, 0),
            made_from: None,
        };
        if let Some(light) = light.filter(acceptable) {
            let beside = Some(light.beside);
            let made_so = |place: &ChunkPos| {
                let entry = self.entries.get(place);
                entry.is_none_or(|entry| entry.as_made == beside)
            };
            // T of section 7.
            if around(position).iter().all(made_so) {
                entry.beside = beside;
                entry.sendable = true;
                entry.put(light, Kind::Carried);
            }
        }
        entry.pending.arrived = !entry.sendable;
        self.entries.insert(position, entry);
        // Step 4: the eight around it.
        for place in around(position) {
            let Some(entry) = self.entries.get(&place) else {
                continue;
            };
            let made_so = |beside: &ChunkPos| {
                let other = self.entries.get(beside);
                other.is_none_or(|other| other.as_made == entry.beside)
            };
            let kept =
                entry.kind == Kind::Carried && entry.current() && around(place).iter().all(made_so);
            if !kept {
                let entry = self.entries.get_mut(&place).expect("found above");
                entry.sendable = false;
                entry.pending.beside = true;
            }
        }
        // E3: a client that has the chunk has it replaced, after an unload.
        for watch in &mut self.watch {
            if watch.has.contains(&position) {
                watch.replace.insert(position);
                watch.must.remove(&position);
            }
        }
    }

    /// The region says that blocks have changed, in one tick's message: E4 and E5.
    async fn change(&mut self, region: RegionId, changes: &[(BlockPos, BlockState)]) {
        note(format!("{region:?} says of blocks: {changes:?}"));
        // The chunks as the changes before in the message leave them.
        let mut chunks: BTreeMap<ChunkPos, Chunk> = BTreeMap::new();
        let mut said = Vec::new();
        for (block, state) in changes {
            let place = block.chunk();
            // Section 3: a change for a chunk that has not come is passed over.
            let Some(now) = self.script.model().chunk(place) else {
                continue;
            };
            let chunk = chunks.entry(place).or_insert_with(|| now.clone());
            let (x, z) = (
                block.x.rem_euclid(16) as usize,
                block.z.rem_euclid(16) as usize,
            );
            let Some(old) = chunk.get(x, block.y, z) else {
                continue;
            };
            if old == *state {
                continue;
            }
            self.touched.entry(place).or_default().insert(*block);
            said.push((*block, *state));
            let section = ((block.y - MIN_Y) / 16) as usize;
            let before = chunk.sections()[section].non_air_count();
            chunk.set(x, block.y, z, *state);
            let after = chunk.sections()[section].non_air_count();
            if !light_tells_apart(old, *state) {
                continue;
            }
            // Reach, of section 3: by distance, and all eight for the first block of
            // a section or its last.
            let everywhere = (before == 0) != (after == 0);
            let by_distance = clustine_light::reach(x, z);
            let mut reached = BTreeSet::from([place]);
            for (beside, near) in around(place).into_iter().zip(by_distance) {
                if near || everywhere {
                    reached.insert(beside);
                }
            }
            if let Some(entry) = self.entries.get_mut(&place) {
                entry.as_made = None;
            }
            for marked in &reached {
                let Some(entry) = self.entries.get_mut(marked) else {
                    continue;
                };
                let mut lacking = BTreeMap::new();
                for (who, watch) in self.watch.iter().enumerate() {
                    if watch.gone || !watch.has.contains(marked) || watch.replace.contains(marked) {
                        continue;
                    }
                    let lacked =
                        |beside: &ChunkPos| reached.contains(beside) && !watch.has.contains(beside);
                    lacking.insert(who, around(*marked).into_iter().filter(lacked).collect());
                }
                let beside = around(*marked).into_iter();
                let reached = beside.filter(|beside| reached.contains(beside)).collect();
                entry.pending.changes.push(Mark { reached, lacking });
            }
        }
        self.script.change(region, changes);
        let ctx = Ctx {
            blocks: said,
            ..Ctx::default()
        };
        self.step(ctx).await;
    }

    /// The player's region says that they moved into `to`.
    async fn walk(&mut self, who: usize, to: ChunkPos) {
        note(format!("client {who} walks to {to:?}"));
        self.script.walk(self.id(who), to);
        self.prune();
        self.step(Ctx::default()).await;
    }

    /// The player walks from chunk to chunk along the row `z = 0` until they are in
    /// the chunk `x`.
    async fn walk_along(&mut self, who: usize, from: i32, x: i32) {
        let mut now = from;
        while now != x {
            now += (x - now).signum();
            self.walk(who, ChunkPos::new(now, 0)).await;
        }
    }

    /// The player's connection ends from the client's side.
    async fn leave(&mut self, who: usize) {
        note(format!("client {who} leaves"));
        self.script.leave(&mut self.clients[who]).await;
        self.watch[who].gone = true;
        self.prune();
        self.step(Ctx::default()).await;
    }

    /// E6: what no view wants any more is gone from the replica.
    fn prune(&mut self) {
        let wanted = self.script.model().wanted();
        self.entries.retain(|position, _| wanted.contains(position));
    }

    fn set_in_flight(&mut self, jobs: usize) {
        note(format!("{jobs} jobs may be out at a time"));
        self.in_flight = jobs;
        self.script.held().set_in_flight(jobs);
    }

    /// The west sends each of `places` that it is asked for and has not sent, as
    /// `chunk` makes it.
    async fn serve(
        &mut self,
        places: impl IntoIterator<Item = ChunkPos>,
        chunk: impl Fn(ChunkPos) -> Chunk,
    ) {
        for place in places {
            if self.script.asked(WEST).contains(&place) && !self.script.model().has(place) {
                self.snapshot(WEST, place, chunk(place)).await;
            }
        }
    }

    // ---- Jobs ----

    /// The jobs that are out, in the order of their dispatch.
    fn out(&mut self) -> Vec<LightJob> {
        self.script.held().out()
    }

    /// The one job that is out for `position`.
    fn job(&mut self, position: ChunkPos) -> LightJob {
        let out = self.out();
        let mut jobs = out.iter().filter(|job| job.position == position);
        let (Some(job), None) = (jobs.next(), jobs.next()) else {
            broken!("not one job is out for {position:?}: {:?}", places(&out));
        };
        job.clone()
    }

    /// How many jobs have been dispatched for `position` so far.
    fn jobs_for(&self, position: ChunkPos) -> usize {
        let for_it = |(_, place): &&(u64, ChunkPos)| *place == position;
        self.dispatched.iter().filter(for_it).count()
    }

    async fn answer(&mut self, basis: u64) -> LightOutcome {
        self.result(basis, How::Rules, None).await
    }

    async fn fail(&mut self, basis: u64) -> LightOutcome {
        self.result(basis, How::Failure, None).await
    }

    /// Answers the job and waits until the edge has removed the client `who`, whose
    /// queue the test has made full: the result's fan-out is to find it so.
    async fn answer_removing(&mut self, basis: u64, who: usize) -> LightOutcome {
        self.result(basis, How::Rules, Some(who)).await
    }

    async fn result(&mut self, basis: u64, how: How, removing: Option<usize>) -> LightOutcome {
        let out = self.out();
        let Some(job) = out.into_iter().find(|job| job.basis == basis) else {
            broken!("the test answers the job {basis}, which is not out");
        };
        note(format!(
            "the job {basis} for {:?} is answered: {how:?}",
            job.position
        ));
        let outcome = match how {
            How::Rules => self.script.held().answer(basis),
            How::Failure => {
                let why = "the test has it fail";
                self.script.held().fail(basis, why);
                LightOutcome::Failed { why: why.into() }
            }
        };
        if let Some(who) = removing {
            self.script.turn().await;
            self.script.removed(&mut self.clients[who]).await;
            note(format!("  client {who} was removed"));
            self.watch[who].gone = true;
            self.prune();
        }
        let ctx = self.take(&job, &outcome);
        self.step(ctx).await;
        outcome
    }

    /// Section 5: what becomes of a result.
    fn take(&mut self, job: &LightJob, outcome: &LightOutcome) -> Ctx {
        let place = job.position;
        let taken = |entry: &&mut Entry| entry.job == Some(job.basis);
        let Some(entry) = self.entries.get_mut(&place).filter(taken) else {
            note("  (the result is nobody's: section 5, a result that is not taken)".into());
            self.seen.results_dropped += 1;
            return Ctx::default();
        };
        let has = |watch: &Watch| {
            !watch.gone && watch.has.contains(&place) && !watch.replace.contains(&place)
        };
        let holders: Vec<usize> = (0..self.watch.len())
            .filter(|who| has(&self.watch[*who]))
            .collect();
        entry.job = None;
        let causes = std::mem::take(&mut entry.job_causes);
        let reshaped = entry.pending.shaped();
        let rechanged = !entry.pending.changes.is_empty();
        // Whom the record promises this result, were it sent: every holder if a chunk
        // came or was replaced beside the chunk; otherwise the holders at the rim.
        let mut promised = BTreeSet::new();
        for who in &holders {
            let watch = &self.watch[*who];
            let still = |lacked: &BTreeSet<ChunkPos>| {
                lacked.iter().any(|beside| !watch.has.contains(beside))
            };
            let at_the_rim = |mark: &Mark| mark.lacking.get(who).is_some_and(still);
            // One who was sent the chunk after the change has not lit it by itself:
            // it is at the rim if it lacks a chunk the change reaches now, and
            // section 8 names it either way.
            let came_late = entry.sent_stale.contains(who)
                && (self.stale_is_promised
                    || causes.changes.iter().any(|mark| still(&mark.reached)));
            if causes.beside || causes.changes.iter().any(at_the_rim) || came_late {
                promised.insert(*who);
            }
        }
        let mut expect = BTreeMap::new();
        let mut whole = None;
        match outcome {
            LightOutcome::Lit { light, update } => {
                let differs = (entry.light.as_ref()).is_none_or(|old| !same_light(old, light));
                entry.put((**light).clone(), Kind::Computed);
                entry.made_from = Some(job.nine.clone());
                entry.sendable = !reshaped;
                if rechanged {
                    // Withheld. Section 5 has `owed` set "if anyone has the chunk";
                    // since "Found while building" that can only mean anyone who
                    // would have been sent the result.
                    self.seen.results_withheld += usize::from(!promised.is_empty());
                    entry.owed |= !promised.is_empty();
                    entry.carry.merge(causes);
                    for who in holders {
                        expect.insert(who, Expect::Nothing);
                        if differs {
                            self.watch[who].must.remove(&place);
                        }
                    }
                } else {
                    entry.owed = false;
                    entry.sent_stale.clear();
                    for who in holders {
                        let watch = &mut self.watch[who];
                        if promised.contains(&who) {
                            let sent = if update.is_some() {
                                Expect::One
                            } else {
                                Expect::Nothing
                            };
                            self.seen.updates_promised += usize::from(update.is_some());
                            expect.insert(who, sent);
                            if job.whole {
                                watch.must.insert(place);
                            }
                        } else {
                            expect.insert(who, Expect::Either);
                            if differs {
                                watch.must.remove(&place);
                            }
                        }
                    }
                    if job.whole {
                        whole = Some((listed(&light.sky), listed(&light.block)));
                    }
                }
            }
            LightOutcome::Failed { .. } => {
                self.seen.jobs_failed += 1;
                if entry.light.is_none() {
                    let chunk = self.script.model().chunk(place);
                    entry.put(
                        by_heights(chunk.expect("an entry has a chunk")),
                        Kind::Fallback,
                    );
                }
                entry.kind = Kind::Fallback;
                entry.sendable = !reshaped;
                entry.owed |= !promised.is_empty();
                entry.carry.merge(causes);
                for who in holders {
                    expect.insert(who, Expect::Nothing);
                }
            }
        }
        Ctx {
            result: Some((place, expect)),
            whole,
            blocks: Vec::new(),
        }
    }

    /// Answers every job that is out as the rules have it, oldest first, and those
    /// dispatched for that, until none is out. Returns how many.
    async fn answer_all(&mut self) -> usize {
        let mut answered = 0;
        while let Some(job) = self.out().first() {
            self.answer(job.basis).await;
            answered += 1;
        }
        answered
    }

    // ---- The checks ----

    /// Waits for the edge, brings every client up to date and holds the account
    /// against what was dispatched and sent.
    async fn step(&mut self, ctx: Ctx) {
        self.seen.steps += 1;
        self.script.quiet().await;
        self.see_jobs();
        for who in self.live() {
            self.script.sync(&mut self.clients[who]).await;
        }
        self.see_jobs();
        for who in self.live() {
            self.see_told(who, &ctx);
        }
        self.standing();
    }

    /// Takes in the jobs dispatched since it last looked.
    fn see_jobs(&mut self) {
        let fresh = self.script.held().fresh();
        if fresh.is_empty() {
            return;
        }
        for job in fresh {
            let (basis, place) = (job.basis, job.position);
            note(format!(
                "  a job for {place:?} is dispatched: basis {basis}, whole: {}, old light: {}",
                job.whole,
                job.old.is_some()
            ));
            self.dispatched.push((basis, place));
            if !self.script.model().same_nine(&job) {
                broken!("the nine chunks of the job {basis} for {place:?} are not the model's");
            }
            let Some(entry) = self.entries.get_mut(&place) else {
                broken!("a job for {place:?}, where the replica has no chunk");
            };
            if let Some(under_way) = entry.job {
                broken!("a job for {place:?} while its job {under_way} is under way");
            }
            if !entry.pending.any() {
                broken!(
                    "a job for {place:?}, which nothing has marked since its last job \
                     (kind {:?})",
                    entry.kind
                );
            }
            let same = match (&job.old, &entry.light) {
                (Some(old), Some(light)) => same_light(old, light),
                (None, None) => true,
                _ => false,
            };
            if !same {
                broken!("the old light of the job for {place:?} is not the entry's light");
            }
            if entry.owed && !job.whole {
                broken!(
                    "a result was withheld from those who have {place:?} (or failed), and \
                     the next job is not told to name every section"
                );
            }
            entry.job = Some(basis);
            entry.job_causes = std::mem::take(&mut entry.pending);
            let carried = std::mem::take(&mut entry.carry);
            entry.job_causes.merge(carried);
        }
        let out = self.script.held().out();
        if out.len() > self.in_flight {
            broken!(
                "{} jobs are out where {} may be: {:?}",
                out.len(),
                self.in_flight,
                places(&out)
            );
        }
    }

    /// Goes through what the client was told since it was last looked at.
    fn see_told(&mut self, who: usize, ctx: &Ctx) {
        let told = self.clients[who].told()[self.watch[who].seen..].to_vec();
        self.watch[who].seen += told.len();
        let view = self.script.model().view(self.id(who));
        let (watch, seen) = (&mut self.watch[who], &mut self.seen);
        for (block, state) in &ctx.blocks {
            if watch.has.contains(&block.chunk()) && !told.contains(&Told::Block(*block, *state)) {
                broken!(
                    "client {who}: has the chunk of {block:?}, or waits to have it \
                     replaced, and was not sent that it became {state:?} (section 3)"
                );
            }
        }
        // How many chunks the batch that is open has brought.
        let mut batch: Option<i32> = None;
        // Chunks that were unloaded to be sent again, and have not been yet.
        let mut again: BTreeSet<ChunkPos> = BTreeSet::new();
        let mut lights: BTreeMap<ChunkPos, Vec<Named>> = BTreeMap::new();
        for said in &told {
            if !again.is_empty()
                && !matches!(said, Told::BatchStarted | Told::Sent(_) | Told::Unloaded(_))
            {
                broken!(
                    "client {who}: the unload of {again:?} is not immediately before the \
                     batch that carries the chunk again (section 6): then came {said:?}"
                );
            }
            match *said {
                Told::BatchStarted => {
                    if batch.replace(0).is_some() {
                        broken!("client {who}: a batch begins within a batch");
                    }
                }
                Told::BatchFinished(count) => {
                    let Some(brought) = batch.take() else {
                        broken!("client {who}: a batch ends that did not begin");
                    };
                    if brought == 0 || count != brought {
                        broken!(
                            "client {who}: a batch of {brought} chunks that says it has \
                             {count} (section 6: no empty batch)"
                        );
                    }
                }
                Told::Sent(place) => {
                    let Some(brought) = batch.as_mut() else {
                        broken!("client {who}: {place:?} was sent outside a batch");
                    };
                    *brought += 1;
                    if watch.has.contains(&place) {
                        broken!(
                            "client {who}: {place:?} was sent again without an unload \
                             before it (section 6)"
                        );
                    }
                    let Some(entry) = self.entries.get_mut(&place) else {
                        broken!("client {who}: was sent {place:?}, which the replica has not");
                    };
                    if !entry.sendable {
                        broken!(
                            "client {who}: was sent {place:?} from an entry that is not \
                             sendable (L3): light {:?}, marked by {:?}, job {:?}",
                            entry.kind,
                            (entry.pending.arrived, entry.pending.beside),
                            entry.job
                        );
                    }
                    if !view.contains(&place) {
                        broken!("client {who}: was sent {place:?}, which is not in its view");
                    }
                    if entry.stale() {
                        entry.sent_stale.insert(who);
                    }
                    watch.fresh.push(place);
                    seen.chunks_sent += 1;
                    seen.chunks_sent_again += usize::from(again.remove(&place));
                    watch.has.insert(place);
                    watch.must.insert(place);
                }
                Told::Unloaded(place) => {
                    watch.has.remove(&place);
                    watch.must.remove(&place);
                    if watch.replace.remove(&place) && view.contains(&place) {
                        again.insert(place);
                    }
                }
                Told::Block(block, _) => {
                    if !watch.has.contains(&block.chunk()) {
                        broken!(
                            "client {who}: a block update for {block:?}, in a chunk it \
                             does not have"
                        );
                    }
                }
                Told::Light(place, named) => {
                    if !watch.has.contains(&place) {
                        broken!(
                            "client {who}: a light update for {place:?}, which it does not \
                             have (section 6)"
                        );
                    }
                    if watch.replace.contains(&place) {
                        broken!(
                            "client {who}: a light update for {place:?}, of which it holds \
                             the chunk of before (section 6)"
                        );
                    }
                    seen.light_updates += 1;
                    lights.entry(place).or_default().push(named);
                }
            }
        }
        if batch.is_some() || !again.is_empty() {
            broken!("client {who}: a batch was left open, or {again:?} unloaded and not sent");
        }
        let none = BTreeMap::new();
        let (result, expect) = match &ctx.result {
            Some((place, expect)) => (Some(*place), expect),
            None => (None, &none),
        };
        for (place, named) in &lights {
            let expected = expect.get(&who).filter(|_| result == Some(*place));
            let Some(expected) = expected else {
                broken!(
                    "client {who}: a light update for {place:?} in a step that took no \
                     result for it that this client is sent (section 6): {named:?}"
                );
            };
            if *expected == Expect::Nothing || named.len() > 1 {
                broken!(
                    "client {who}: {} light updates for {place:?} where the record has \
                     {expected:?}: {named:?}",
                    named.len()
                );
            }
            if let Some((sky, block)) = ctx.whole
                && (named[0].any_sky() != sky || named[0].any_block() != block)
            {
                broken!(
                    "client {who}: the update for {place:?} was to name every listed \
                     section (sky {sky:#b}, block {block:#b}) and names {}",
                    named[0]
                );
            }
        }
        if let Some(place) = result
            && expect.get(&who) == Some(&Expect::One)
            && !lights.contains_key(&place)
        {
            broken!(
                "client {who}: no light update for {place:?}, which it has and whose result \
                 was taken and not withheld; the record promises it one (section 6, and \
                 \"Found while building\")"
            );
        }
    }

    /// What holds after every step.
    fn standing(&mut self) {
        self.look();
        for watch in &mut self.watch {
            watch.fresh.clear();
        }
    }

    fn look(&mut self) {
        for who in self.live() {
            let view = self.script.model().view(self.id(who));
            let (watch, client) = (&self.watch[who], &self.clients[who]);
            if client.chunks() != watch.has {
                broken!("client {who}: the test's own account of what it has is wrong");
            }
            for place in &watch.has {
                if !view.contains(place) {
                    broken!("client {who}: still has {place:?}, which left its view");
                }
            }
            for place in &view {
                let Some(entry) = self.entries.get(place) else {
                    continue;
                };
                let has = watch.has.contains(place) && !watch.replace.contains(place);
                if entry.sendable && !has && !watch.lags {
                    broken!(
                        "client {who}: {place:?} is sendable and in its view, no batch is \
                         outstanding, and it was not sent (section 6)"
                    );
                }
            }
            for place in &watch.must {
                let entry = self
                    .entries
                    .get(place)
                    .expect("a chunk a client has is there");
                let light = entry
                    .light
                    .as_ref()
                    .expect("a chunk that was sent has light");
                let (due, held) = (as_held(light), client.light(*place));
                self.seen.lights_compared += 1;
                if held != Some(&due) {
                    let held = held.expect("the client has the chunk");
                    let differs = |one: &[_], other: &[_]| -> Vec<usize> {
                        (0..one.len()).filter(|at| one[*at] != other[*at]).collect()
                    };
                    broken!(
                        "client {who}: its light for {place:?} is not the entry's (L1; \
                         kind {:?}): sky sections {:?} and block sections {:?} differ",
                        entry.kind,
                        differs(&held.sky, &due.sky),
                        differs(&held.block, &due.block)
                    );
                }
            }
            // A chunk that was just sent: the blocks a test sets, wherever it sets them.
            for place in &watch.fresh {
                let chunk = self.script.model().chunk(*place);
                let chunk = chunk.expect("a chunk a client was sent is there");
                for (x, z) in COLUMNS.into_iter().flat_map(|x| COLUMNS.map(|z| (x, z))) {
                    for y in HEIGHTS {
                        let shown = client.block(at(*place, x as i32, y, z as i32));
                        if shown != chunk.get(x, y, z) {
                            broken!(
                                "client {who}: was sent {place:?} with {shown:?} at {x}, {y}, \
                                 {z}, where the chunk has {:?}",
                                chunk.get(x, y, z)
                            );
                        }
                    }
                }
            }
            for place in watch.has.difference(&watch.replace) {
                let chunk = self.script.model().chunk(*place);
                let chunk = chunk.expect("a chunk a client has is there");
                for block in self.touched.get(place).into_iter().flatten() {
                    let (x, z) = (block.x.rem_euclid(16), block.z.rem_euclid(16));
                    let due = chunk.get(x as usize, block.y, z as usize);
                    if client.block(*block) != due {
                        broken!(
                            "client {who}: shows {:?} at {block:?}, where the chunk has {due:?}",
                            client.block(*block)
                        );
                    }
                }
            }
        }
    }

    /// The client confirms its batches, or does so no longer.
    async fn confirm(&mut self, who: usize, confirms: bool) {
        note(format!("client {who} confirms its batches: {confirms}"));
        self.clients[who].confirm(confirms);
        self.watch[who].lags = !confirms;
        self.step(Ctx::default()).await;
    }

    /// Answers every job and holds the replica to L2 and L4 as far as they show.
    async fn whole_check(&mut self) {
        self.answer_all().await;
        note("  (every job is answered: the whole check)".into());
        for (place, entry) in &self.entries {
            if !entry.sendable {
                broken!(
                    "{place:?} is not sendable with every job answered (L4): kind {:?}, \
                     marked by {:?}",
                    entry.kind,
                    entry.pending
                );
            }
            if entry.kind == Kind::Fallback {
                continue;
            }
            if entry.pending.any() {
                broken!(
                    "{place:?} is marked and no job is under way for it (L2): {:?}",
                    entry.pending
                );
            }
            if entry.kind != Kind::Computed {
                continue;
            }
            // Section 2, what "current" promises of computed light: the chunk's blocks
            // now beside the neighbours of its job, none of which has come anew or
            // changed within reach since. One that has left marks nothing (section
            // 3): the light stays as it was made beside it.
            let mut nine = self.script.model().nine(*place);
            let made_from = entry.made_from.as_ref().expect("computed light had a job");
            for (now, then) in nine.iter_mut().zip(made_from) {
                *now = now.or(then.as_deref());
            }
            let [nw, n, ne, w, centre, e, sw, s, se] = nine;
            let rules = clustine_light::light(&clustine_light::Around {
                centre: centre.expect("an entry has a chunk"),
                neighbours: [nw, n, ne, w, e, sw, s, se],
                sky: true,
            });
            let light = entry.light.as_ref().expect("computed light is there");
            let lists = [
                (&rules.sky, &light.sky, entry.listed.0, "sky"),
                (&rules.block, &light.block, entry.listed.1, "block"),
            ];
            for (rules, light, listed, which) in lists {
                for (section, (due, has)) in rules.iter().zip(light.iter()).enumerate() {
                    // Section 6: a section once listed stays listed, as its levels.
                    let kept = *due == SectionLight::Absent && listed >> section & 1 == 1;
                    if due != has && !kept {
                        broken!(
                            "the computed light of {place:?} is not the rules' for the \
                             chunks as they are now (L2): {which} section {section}"
                        );
                    }
                }
            }
        }
    }

    /// The light updates the client was sent for `place`, in order.
    fn lights(&self, who: usize, place: ChunkPos) -> Vec<Named> {
        let light = |told: &Told| match told {
            Told::Light(at, named) if *at == place => Some(*named),
            _ => None,
        };
        self.clients[who].told().iter().filter_map(light).collect()
    }

    /// Whether the client holds, for `place`, the light of the rules for the model's
    /// chunks as they are now.
    fn holds_the_rules(&self, who: usize, place: ChunkPos) -> bool {
        let due = self
            .script
            .model()
            .light(place)
            .expect("the model has the chunk");
        self.clients[who].light(place) == Some(&as_held(&due))
    }

    async fn end(mut self) -> Seen {
        self.whole_check().await;
        // Every wait panics if the edge's task has ended over its own account.
        self.script.quiet().await;
        self.seen
    }
}

fn places(jobs: &[LightJob]) -> Vec<ChunkPos> {
    jobs.iter().map(|job| job.position).collect()
}

/// The chunks around [`HOME`] and itself.
fn home_and_around() -> Vec<ChunkPos> {
    nine_around(HOME).to_vec()
}

// ---------------------------------------------------------------------------------
// A chunk and its neighbours, in every order
// ---------------------------------------------------------------------------------

/// Test 4, first item: a chunk first, then its eight neighbours, in every position of
/// the changed one among them; the neighbours first. Section 3, arrive: a chunk that
/// comes changed (its `as_made` is none) fails T for itself and for every chunk beside
/// it that carries light, so all nine are lit by the edge; L1 and L2 where every job
/// is answered. Each order is played twice: with every job answered as it is
/// dispatched, and with all of them held until the nine have come.
#[tokio::test]
async fn a_changed_chunk_among_its_eight_neighbours_is_lit_right_wherever_it_comes_among_them() {
    for place in 0..9 {
        for held in [false, true] {
            let mut play = Play::start().await;
            let client = play.join(1).await;
            let mut order: Vec<_> = around(HOME).to_vec();
            order.insert(place, HOME);
            for position in order {
                let chunk = if position == HOME {
                    lamp(ground(), 15, FLOOR, 8)
                } else {
                    ground()
                };
                play.snapshot(WEST, position, chunk).await;
                if !held {
                    play.answer_all().await;
                }
            }
            play.whole_check().await;
            for position in home_and_around() {
                assert!(play.clients[client].has(position), "{position:?}");
                assert!(play.holds_the_rules(client, position), "{position:?}");
                assert_eq!(play.clients[client].times_sent(position), 1);
            }
            // The lamp's light is over the border.
            let east = play.clients[client].light(EAST_OF_HOME).unwrap();
            assert_eq!(east.block(0, up(FLOOR), 8), Some(14));
            play.end().await;
        }
    }
}

/// Test 4, first item: one neighbour never. Section 4, "Held": a chunk waits only so
/// long for a neighbour (no hold at all is built yet), and is then lit with a wall
/// where the neighbour would be; L4: with every job answered it is sent.
#[tokio::test]
async fn a_chunk_whose_neighbour_never_comes_is_sent_lit_beside_a_wall() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    let west = ChunkPos::new(-1, 0);
    play.snapshot(WEST, HOME, lamp(ground(), 0, FLOOR, 8)).await;
    let but_west = around(HOME).into_iter().filter(|place| *place != west);
    play.serve(but_west, |_| ground()).await;
    play.whole_check().await;
    assert!(play.clients[client].has(HOME));
    assert!(!play.clients[client].has(west));
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Block changes that light sees (E5), at every point of a job's life
// ---------------------------------------------------------------------------------

/// A chunk two east of [`HOME`]: in a view from there, and not beside it.
const APART: ChunkPos = ChunkPos::new(2, 0);

/// Test 4, second item: E5 before the dispatch. Section 4, dispatch: it runs at the
/// end of a turn, from the replica as it is then, so a change that came before is in
/// the job's chunks and no second job is owed.
#[tokio::test]
async fn a_block_change_before_the_dispatch_is_in_the_job_and_no_second_job_follows() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.set_in_flight(0);
    play.snapshot(WEST, HOME, unlit(ground())).await;
    let torch = at(HOME, 3, FLOOR, 3);
    play.change(WEST, &[(torch, blocks::TORCH)]).await;
    assert!(play.out().is_empty());
    assert!(!play.clients[client].has(HOME));
    play.set_in_flight(4);
    play.step(Ctx::default()).await;
    let job = play.job(HOME);
    let centre = job.nine[4].as_ref().expect("a job has its chunk");
    assert_eq!(centre.get(3, FLOOR, 3), Some(blocks::TORCH));
    play.answer(job.basis).await;
    assert!(play.out().is_empty());
    assert_eq!(play.jobs_for(HOME), 1);
    assert_eq!(play.clients[client].block(torch), Some(blocks::TORCH));
    assert!(play.holds_the_rules(client, HOME));
    let light = play.clients[client].light(HOME).unwrap();
    assert_eq!(light.block(3, up(FLOOR), 3), Some(14));
    assert!(play.lights(client, HOME).is_empty());
    play.end().await;
}

/// Test 4, second item: E5 between dispatch and result, for one who waits. Section 5,
/// the table's second row: the chunk is sent as its blocks are now with the job's
/// light, the entry is stale and the next job follows. The client lacks every chunk
/// beside this one, so by "Found while building" it is sent that job's light.
#[tokio::test]
async fn a_block_change_under_a_job_sends_the_waiter_the_chunk_and_then_the_next_light() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, unlit(ground())).await;
    let job = play.job(HOME);
    let torch = at(HOME, 3, FLOOR, 3);
    play.change(WEST, &[(torch, blocks::TORCH)]).await;
    assert!(!play.clients[client].has(HOME));
    play.answer(job.basis).await;
    assert_eq!(play.clients[client].block(torch), Some(blocks::TORCH));
    // The job's light is of before the torch.
    let light = play.clients[client].light(HOME).unwrap();
    assert_eq!(light.block(3, up(FLOOR), 3), Some(0));
    let next = play.job(HOME);
    assert!(next.basis > job.basis);
    play.answer(next.basis).await;
    assert!(play.holds_the_rules(client, HOME));
    assert_eq!(play.lights(client, HOME).len(), 1);
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    assert_eq!(play.jobs_for(HOME), 2);
    play.end().await;
}

/// The same for a client that has every chunk around: section 5, the table's second
/// row, sends it the chunk with light older than its blocks. Its own client has not
/// lit that change (it had no chunk when the block update would have come), so
/// nothing but the edge can put the light right: L1 (the client holds the entry's
/// light), and section 8's table of outcomes, which names "who was sent the chunk
/// with light older than its blocks" beside the holder at the rim. "Found while
/// building" names only the holder at the rim.
#[tokio::test]
#[ignore = "finding: a waiter with every chunk around it that is sent a chunk whose block \
            changed under its job is never sent the light of that block"]
async fn a_waiter_inside_its_view_sent_light_older_than_the_blocks_is_sent_the_next_light() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.serve(around(HOME), |_| ground()).await;
    play.snapshot(WEST, HOME, unlit(ground())).await;
    let job = play.job(HOME);
    let torch = at(HOME, 8, FLOOR, 8);
    play.change(WEST, &[(torch, blocks::TORCH)]).await;
    play.answer(job.basis).await;
    assert_eq!(play.clients[client].block(torch), Some(blocks::TORCH));
    assert_eq!(
        play.clients[client]
            .light(HOME)
            .unwrap()
            .block(8, up(FLOOR), 8),
        Some(0)
    );
    play.whole_check().await;
    assert_eq!(
        play.clients[client]
            .light(HOME)
            .unwrap()
            .block(8, up(FLOOR), 8),
        Some(14),
        "the torch the client was sent is dark for good"
    );
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

/// Test 4, second item: E5 after the result, for a holder at the rim. "Found while
/// building": one who lacks a chunk beside it that the change reaches is sent the
/// light update, which names the sections that differ: a torch changes no sky light.
#[tokio::test]
async fn a_block_change_after_the_result_sends_a_holder_at_the_rim_the_differing_sections() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, unlit(ground())).await;
    play.answer_all().await;
    assert!(play.clients[client].has(HOME));
    play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    let job = play.job(HOME);
    assert!(job.old.is_some() && !job.whole);
    play.answer(job.basis).await;
    let lights = play.lights(client, HOME);
    assert_eq!(lights.len(), 1);
    assert_eq!(lights[0].any_sky(), 0, "{}", lights[0]);
    assert_ne!(lights[0].any_block(), 0);
    assert!(play.holds_the_rules(client, HOME));
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    play.end().await;
}

/// The same for a holder that has every chunk around the change. "Found while
/// building": everyone else has the block update and the chunks around it, and is
/// sent nothing of light, as the official server does.
#[tokio::test]
async fn a_block_change_sends_a_holder_with_every_chunk_around_the_block_and_no_light() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.serve(view(HOME, VIEW), |_| ground()).await;
    let torch = at(HOME, 8, FLOOR, 8);
    play.change(WEST, &[(torch, blocks::TORCH)]).await;
    // The chunk and the four beside it, which the change reaches.
    assert_eq!(play.out().len(), 4);
    play.whole_check().await;
    assert_eq!(play.dispatched.len(), 5);
    assert_eq!(play.clients[client].block(torch), Some(blocks::TORCH));
    for place in home_and_around() {
        assert!(play.lights(client, place).is_empty(), "{place:?}");
    }
    play.end().await;
}

/// Test 4, second item: two in one `TickDelta`. Section 4, dispatch: all the block
/// changes of one tick are in the replica before a job is made from it: one job.
#[tokio::test]
async fn two_block_changes_of_one_tick_are_one_job() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    let (one, other) = (at(HOME, 1, FLOOR, 8), at(HOME, 8, FLOOR, 1));
    play.change(WEST, &[(one, blocks::TORCH), (other, blocks::GLOWSTONE)])
        .await;
    let job = play.job(HOME);
    let centre = job.nine[4].as_ref().unwrap();
    assert_eq!(centre.get(1, FLOOR, 8), Some(blocks::TORCH));
    assert_eq!(centre.get(8, FLOOR, 1), Some(blocks::GLOWSTONE));
    play.answer(job.basis).await;
    assert!(play.out().is_empty());
    assert_eq!(play.jobs_for(HOME), 1);
    assert_eq!(play.lights(client, HOME).len(), 1);
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

/// Test 4, second item: one in a neighbour within reach and one out of it. Section 3,
/// reach: a change marks the neighbour at offset `dx` only if `hx + hz <= 14`.
#[tokio::test]
async fn a_block_change_in_a_neighbour_marks_the_chunk_only_within_reach() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    play.snapshot(WEST, EAST_OF_HOME, ground()).await;
    // Sixteen columns from the border with home: out of reach.
    play.change(WEST, &[(at(EAST_OF_HOME, 15, FLOOR, 8), blocks::GLOWSTONE)])
        .await;
    assert_eq!(places(&play.out()), [EAST_OF_HOME]);
    play.answer_all().await;
    assert_eq!(play.jobs_for(HOME), 0);
    // One column from it: within.
    play.change(WEST, &[(at(EAST_OF_HOME, 0, FLOOR, 8), blocks::GLOWSTONE)])
        .await;
    let out: BTreeSet<_> = places(&play.out()).into_iter().collect();
    assert_eq!(out, BTreeSet::from([HOME, EAST_OF_HOME]));
    play.whole_check().await;
    assert_eq!(play.jobs_for(HOME), 1);
    assert!(play.holds_the_rules(client, HOME));
    assert!(play.holds_the_rules(client, EAST_OF_HOME));
    let light = play.clients[client].light(HOME).unwrap();
    assert_eq!(light.block(15, up(FLOOR), 8), Some(14));
    play.end().await;
}

/// Test 4, second item: one that fills an empty section. Section 3, reach: the first
/// block of a section that held only air reaches all eight, wherever it is, and so
/// does taking the last.
#[tokio::test]
async fn the_first_block_of_a_section_and_its_last_mark_all_eight_neighbours() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.serve(home_and_around(), |_| ground()).await;
    let high = at(HOME, 8, 100, 8);
    for state in [blocks::STONE, blocks::AIR] {
        let before = play.dispatched.len();
        play.change(WEST, &[(high, state)]).await;
        play.whole_check().await;
        let lit: BTreeSet<_> = play.dispatched[before..].iter().map(|job| job.1).collect();
        let nine: BTreeSet<_> = home_and_around().into_iter().collect();
        assert_eq!(lit, nine, "{state:?}");
        assert_eq!(play.dispatched.len() - before, 9);
    }
    assert_eq!(play.clients[client].block(high), Some(blocks::AIR));
    play.end().await;
}

/// Test 4, second item: a torch between dispatch and result with a client that has
/// the chunk: that client is sent no light until the next job's, and then every
/// section. Section 5, "Why a result is withheld", and `owed` of section 1.
#[tokio::test]
async fn a_torch_under_a_job_withholds_its_light_from_a_holder_and_the_next_names_every_section() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    let first = play.job(HOME);
    play.change(WEST, &[(at(HOME, 8, FLOOR, 1), blocks::TORCH)])
        .await;
    play.answer(first.basis).await;
    assert!(play.lights(client, HOME).is_empty());
    let next = play.job(HOME);
    assert!(next.whole, "the next job is to name every section");
    let outcome = play.answer(next.basis).await;
    let LightOutcome::Lit { light, .. } = outcome else {
        panic!("the rules light a flat chunk");
    };
    let lights = play.lights(client, HOME);
    assert_eq!(lights.len(), 1);
    assert_eq!(lights[0].any_sky(), listed(&light.sky));
    assert_eq!(lights[0].any_block(), listed(&light.block));
    assert!(play.holds_the_rules(client, HOME));
    assert!(play.out().is_empty());
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Block changes that light does not see (E4)
// ---------------------------------------------------------------------------------

/// Test 4, third item: E4 before the dispatch, between dispatch and result and after
/// it. Section 3, E4: the block update is sent and no moment is raised: no job is
/// made for it, and a result under which it came is not withheld.
#[tokio::test]
async fn a_block_change_light_does_not_see_makes_no_job_and_withholds_no_result() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    let under = |x| at(HOME, x, FLOOR - 1, 8);
    // With nothing marked.
    play.change(WEST, &[(under(1), blocks::GRANITE)]).await;
    assert!(play.out().is_empty());
    // Before the dispatch.
    play.set_in_flight(0);
    play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    play.change(WEST, &[(under(2), blocks::GRANITE)]).await;
    play.set_in_flight(4);
    play.step(Ctx::default()).await;
    let job = play.job(HOME);
    // Between dispatch and result.
    play.change(WEST, &[(under(3), blocks::GRANITE)]).await;
    play.answer(job.basis).await;
    assert_eq!(
        play.lights(client, HOME).len(),
        1,
        "the result was withheld"
    );
    assert!(play.out().is_empty());
    // After the result.
    play.change(WEST, &[(under(4), blocks::GRANITE)]).await;
    assert!(play.out().is_empty());
    assert_eq!(play.jobs_for(HOME), 1);
    for x in 1..=4 {
        assert_eq!(play.clients[client].block(under(x)), Some(blocks::GRANITE));
    }
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

/// Test 4, third item: the same change said a second time by another region raises
/// nothing. Section 3, E4: a change that puts the state that is there already is that
/// case too. Said under the job for the first, it does not withhold the result; said
/// afterwards, it makes no job.
#[tokio::test]
async fn the_same_change_said_again_by_another_region_raises_nothing() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    let torch = [(at(HOME, 1, FLOOR, 8), blocks::TORCH)];
    play.change(WEST, &torch).await;
    let job = play.job(HOME);
    play.change(EAST, &torch).await;
    assert_eq!(play.out().len(), 1);
    play.answer(job.basis).await;
    assert_eq!(
        play.lights(client, HOME).len(),
        1,
        "the result was withheld"
    );
    assert!(play.out().is_empty());
    play.change(EAST, &torch).await;
    assert!(play.out().is_empty());
    assert_eq!(play.jobs_for(HOME), 1);
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Chunks that come, are replaced or leave between dispatch and result
// ---------------------------------------------------------------------------------

/// Test 4, fourth item: E1 at a neighbour between dispatch and result. Section 5, the
/// table's third row: the result is taken, the chunk is not sendable and those who
/// wait go on waiting; the next job, made beside the neighbour, sends it. L3.
#[tokio::test]
async fn a_neighbour_that_comes_under_a_job_keeps_the_waiter_waiting_for_the_next() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, unlit(ground())).await;
    let job = play.job(HOME);
    play.snapshot(WEST, EAST_OF_HOME, lamp(ground(), 0, FLOOR, 8))
        .await;
    play.answer(job.basis).await;
    assert!(
        !play.clients[client].has(HOME),
        "light made beside a wall was sent"
    );
    let next = play.job(HOME);
    assert!(next.nine[5].is_some());
    play.answer(next.basis).await;
    assert!(play.clients[client].has(HOME));
    assert!(play.holds_the_rules(client, HOME));
    let light = play.clients[client].light(HOME).unwrap();
    assert_eq!(light.block(15, up(FLOOR), 8), Some(14));
    play.whole_check().await;
    assert_eq!(play.jobs_for(HOME), 2);
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    play.end().await;
}

/// The same for one who has the chunk: section 5, the table's third row: they are sent
/// the job's update, and the next job's, which is made beside the new neighbour.
#[tokio::test]
async fn a_neighbour_that_comes_under_a_job_leaves_a_holder_the_light_of_the_next() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    let job = play.job(HOME);
    play.snapshot(WEST, EAST_OF_HOME, lamp(ground(), 0, FLOOR, 8))
        .await;
    play.answer(job.basis).await;
    play.whole_check().await;
    assert_eq!(play.jobs_for(HOME), 2);
    assert!(play.holds_the_rules(client, HOME));
    let light = play.clients[client].light(HOME).unwrap();
    assert_eq!(light.block(15, up(FLOOR), 8), Some(14));
    assert_eq!(light.block(1, up(FLOOR), 8), Some(14));
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    play.end().await;
}

/// Test 4, fourth item: E3 at the chunk between dispatch and result. Section 5, "a
/// result that is not taken": arrive forgot the job, the result is dropped, and the
/// chunk is sent with the light of the job made from the chunk that replaced it.
#[tokio::test]
async fn a_result_for_a_chunk_that_was_replaced_is_dropped() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, unlit(ground())).await;
    let job = play.job(HOME);
    play.snapshot(WEST, HOME, lamp(ground(), 8, FLOOR, 8)).await;
    play.answer(job.basis).await;
    assert!(!play.clients[client].has(HOME));
    let next = play.job(HOME);
    play.answer(next.basis).await;
    let lamp = at(HOME, 8, FLOOR, 8);
    assert_eq!(play.clients[client].block(lamp), Some(blocks::GLOWSTONE));
    assert!(play.holds_the_rules(client, HOME));
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    play.end().await;
}

/// Test 4, fourth item: E6 between dispatch and result, and E6 then E1. Section 5: a
/// result for a chunk that left is dropped, and so is one for a chunk that left and
/// came again: the new entry has no job of that moment.
#[tokio::test]
async fn a_result_for_a_chunk_that_left_is_dropped_also_when_the_chunk_has_come_again() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    let rim = ChunkPos::new(-3, 0);
    play.snapshot(WEST, rim, unlit(ground())).await;
    let job = play.job(rim);
    play.walk(client, EAST_OF_HOME).await;
    let other = ChunkPos::new(4, 0);
    play.snapshot(WEST, other, unlit(ground())).await;
    let kept = play.job(other);
    play.walk(client, HOME).await;
    play.walk(client, EAST_OF_HOME).await;
    // The first chunk has left and has not come again; the second left and comes.
    play.snapshot(WEST, other, unlit(ground())).await;
    play.answer(job.basis).await;
    assert!(!play.clients[client].has(rim));
    play.answer(kept.basis).await;
    assert!(
        !play.clients[client].has(other),
        "the old job's result was taken"
    );
    assert_eq!(play.jobs_for(other), 2);
    play.whole_check().await;
    assert!(play.clients[client].has(other));
    assert_eq!(play.clients[client].times_sent(rim), 0);
    assert_eq!(play.clients[client].times_sent(other), 1);
    play.end().await;
}

/// Test 4, fifth item: E2 between dispatch and result, from the region that served
/// the chunk. Section 3, E2: nothing; a job under way goes on and its result is taken.
#[tokio::test]
async fn the_same_chunk_from_its_region_under_a_job_leaves_the_job_and_its_result() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, unlit(ground())).await;
    let job = play.job(HOME);
    play.snapshot(WEST, HOME, ground()).await;
    assert_eq!(play.out().len(), 1);
    assert!(
        !play.clients[client].has(HOME),
        "the snapshot's light was put in"
    );
    play.answer(job.basis).await;
    assert!(play.clients[client].has(HOME));
    assert!(play.out().is_empty());
    assert_eq!(play.jobs_for(HOME), 1);
    play.end().await;
}

/// Test 4, fifth item: E2 between dispatch and result from another region, after a
/// merge. Section 3, E2: it is the same whichever region the snapshot is from.
#[tokio::test]
async fn the_same_chunk_from_the_survivor_of_a_merge_under_a_job_leaves_the_job_and_its_result() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.script.elsewhere(WEST, APART, NORTH);
    play.script.settle(WEST).await;
    play.script.settle(NORTH).await;
    play.snapshot(NORTH, APART, unlit(ground())).await;
    let job = play.job(APART);
    note("the east absorbs the north".into());
    play.script.merge(NORTH, EAST).await;
    play.step(Ctx::default()).await;
    play.snapshot(EAST, APART, unlit(ground())).await;
    assert_eq!(play.out().len(), 1);
    play.answer(job.basis).await;
    assert!(play.clients[client].has(APART));
    assert_eq!(play.jobs_for(APART), 1);
    play.end().await;
}

/// Test 4, fifth item: E2 between dispatch and result from another region, after
/// `NotMine`. On a paused clock: the edge asks a region again a second later, and no
/// wait here lets the clock run.
#[tokio::test(start_paused = true)]
async fn the_same_chunk_from_the_region_asked_after_not_mine_leaves_the_job_and_its_result() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.script.elsewhere(WEST, APART, EAST);
    play.script.settle(WEST).await;
    play.script.settle(EAST).await;
    play.snapshot(EAST, APART, unlit(ground())).await;
    let job = play.job(APART);
    note("the east says that the chunk is not its own; two seconds pass".into());
    play.script.not_mine(EAST, APART);
    play.script.quiet().await;
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    play.step(Ctx::default()).await;
    assert!(play.script.asked(WEST).contains(&APART));
    play.snapshot(WEST, APART, unlit(ground())).await;
    assert_eq!(play.out().len(), 1);
    play.answer(job.basis).await;
    assert!(play.clients[client].has(APART));
    assert_eq!(play.jobs_for(APART), 1);
    play.end().await;
}

/// Test 4, sixth item: E3 twice for one chunk before its light is there: one unload,
/// one chunk, the second snapshot's blocks. Section 6: a second E3 before the light is
/// there leaves the chunk in `replace`, once.
#[tokio::test]
async fn a_chunk_replaced_twice_before_its_light_is_unloaded_once_and_sent_once() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    play.snapshot(WEST, HOME, lamp(ground(), 3, FLOOR, 3)).await;
    let first = play.job(HOME);
    assert!(play.clients[client].has(HOME));
    play.snapshot(WEST, HOME, lamp(ground(), 9, FLOOR, 9)).await;
    play.answer(first.basis).await;
    assert_eq!(play.clients[client].told_of(HOME), [Told::Sent(HOME)]);
    play.whole_check().await;
    assert_eq!(
        play.clients[client].told_of(HOME),
        [Told::Sent(HOME), Told::Unloaded(HOME), Told::Sent(HOME)]
    );
    let block = |x| play.clients[client].block(at(HOME, x, FLOOR, x));
    assert_eq!(
        (block(3), block(9)),
        (Some(blocks::AIR), Some(blocks::GLOWSTONE))
    );
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

/// Test 4, sixth item: E3 and then the view moves away before the light is there: the
/// client is sent the unload it is owed, and nothing of the chunk after it.
/// Section 6: a view that moves away unloads what is in `replace`.
#[tokio::test]
async fn a_replaced_chunk_whose_view_moves_away_before_its_light_is_unloaded_and_no_more() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    let rim = ChunkPos::new(-3, 0);
    play.snapshot(WEST, rim, ground()).await;
    play.snapshot(WEST, rim, lamp(ground(), 3, FLOOR, 3)).await;
    let job = play.job(rim);
    assert!(play.clients[client].has(rim));
    play.walk(client, EAST_OF_HOME).await;
    let told = [Told::Sent(rim), Told::Unloaded(rim)];
    assert_eq!(play.clients[client].told_of(rim), told);
    play.answer(job.basis).await;
    assert_eq!(play.clients[client].told_of(rim), told);
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Jobs that fail, are never answered or answered in another order
// ---------------------------------------------------------------------------------

/// Test 4, seventh item: a result that fails for an entry that was `Carried`, with
/// someone who has the chunk. Section 5, "A failed job": those who have it are sent
/// nothing, no job is made for it until it is marked, a neighbour that then comes
/// gives it a job, and that job's result names every section.
#[tokio::test]
async fn a_failed_job_for_carried_light_sends_a_holder_nothing_until_a_neighbour_brings_a_job() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    let before = play.clients[client].light(HOME).cloned();
    play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    let job = play.job(HOME);
    play.fail(job.basis).await;
    assert!(play.lights(client, HOME).is_empty());
    assert_eq!(play.clients[client].light(HOME), before.as_ref());
    assert!(
        play.out().is_empty(),
        "a job by itself after one that failed"
    );
    // Nothing comes of a change light does not see, either.
    play.change(WEST, &[(at(HOME, 5, FLOOR - 1, 5), blocks::GRANITE)])
        .await;
    assert!(play.out().is_empty());

    play.snapshot(WEST, EAST_OF_HOME, ground()).await;
    let next = play.job(HOME);
    assert!(
        next.whole,
        "those who have the chunk are owed every section"
    );
    let LightOutcome::Lit { light, .. } = play.answer(next.basis).await else {
        panic!("the rules light a flat chunk");
    };
    let lights = play.lights(client, HOME);
    assert_eq!(lights.len(), 1);
    assert_eq!(lights[0].any_sky(), listed(&light.sky));
    assert_eq!(lights[0].any_block(), listed(&light.block));
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

/// Test 4, seventh item: a result that fails for an entry that had no light.
/// Section 5: those who wait are sent the chunk, with the sky's light by its heights
/// and block light dark; no job follows by itself; a neighbour that then comes gives
/// it a job, whose result leaves the one who has the chunk the light of the rules.
#[tokio::test]
async fn a_failed_job_for_a_chunk_without_light_sends_it_lit_by_heights_until_a_neighbour_comes() {
    let (play, client, _) = failed_without_light_and_then_a_neighbour().await;
    assert_eq!(play.lights(client, HOME).len(), 1);
    assert!(play.holds_the_rules(client, HOME));
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    play.end().await;
}

/// The same order, held to the letter of test 4 and of section 5 ("the result of
/// that job goes to those who have the chunk with every section named"). Section 5
/// also says that `owed` is set at a failure "if anyone has it", and at this failure
/// nobody has the chunk yet: the waiter is sent it by the failure itself. The client's
/// light is right either way (the test above), since it holds exactly the light the
/// update differs from.
#[tokio::test]
#[ignore = "finding: after a failed job for a chunk nobody had yet, the next job's update \
            names the differing sections, where test 4 and section 5 say every section"]
async fn the_job_after_a_failed_one_for_a_chunk_without_light_names_every_section() {
    let (play, client, light) = failed_without_light_and_then_a_neighbour().await;
    let lights = play.lights(client, HOME);
    assert_eq!(lights[0].any_sky(), listed(&light.sky), "{}", lights[0]);
    assert_eq!(lights[0].any_block(), listed(&light.block), "{}", lights[0]);
    play.end().await;
}

/// A chunk without light whose job fails, and then a neighbour and the job it brings.
/// Returns the light of that job.
async fn failed_without_light_and_then_a_neighbour() -> (Play, usize, Light) {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    let chunk = roof(unlit(ground()), FLOOR + 3, (0, 0), (15, 15));
    play.snapshot(WEST, HOME, chunk.clone()).await;
    let job = play.job(HOME);
    play.fail(job.basis).await;
    assert!(play.clients[client].has(HOME));
    let due = as_held(&by_heights(&chunk));
    assert_eq!(play.clients[client].light(HOME), Some(&due));
    assert!(
        play.out().is_empty(),
        "a job by itself after one that failed"
    );

    play.snapshot(WEST, EAST_OF_HOME, ground()).await;
    let next = play.job(HOME);
    let LightOutcome::Lit { light, .. } = play.answer(next.basis).await else {
        panic!("the rules light a flat chunk");
    };
    (play, client, (*light).clone())
}

/// Test 4, seventh item: a result that fails while a result is owed already (`owed`
/// stays set). The first result is withheld by a second torch, the next job fails,
/// and the job a third change brings names every section.
#[tokio::test]
async fn a_failed_job_after_a_withheld_result_still_owes_a_holder_every_section() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    let first = play.job(HOME);
    play.change(WEST, &[(at(HOME, 8, FLOOR, 1), blocks::TORCH)])
        .await;
    play.answer(first.basis).await;
    let second = play.job(HOME);
    play.fail(second.basis).await;
    assert!(play.lights(client, HOME).is_empty());
    assert!(play.out().is_empty());
    play.change(WEST, &[(at(HOME, 14, FLOOR, 8), blocks::TORCH)])
        .await;
    let third = play.job(HOME);
    assert!(third.whole);
    play.answer(third.basis).await;
    assert_eq!(play.lights(client, HOME).len(), 1);
    assert!(play.holds_the_rules(client, HOME));
    play.end().await;
}

/// Test 4, seventh item: a job that is never answered, and results in another order
/// than the jobs. Section 5: a result is taken by its job. (That a job is given up
/// after `JOB_PATIENCE` is not built, and the fan-out has no setting for it yet.)
#[tokio::test]
async fn results_in_another_order_are_each_taken_and_a_job_never_answered_holds_up_no_other() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    let places = [HOME, APART, ChunkPos::new(0, 2), ChunkPos::new(-2, 0)];
    for place in places {
        play.snapshot(WEST, place, unlit(ground())).await;
    }
    let jobs = play.out();
    assert_eq!(self::places(&jobs), places);
    note("the job for the first is never answered".into());
    play.script.held().never(jobs[0].basis);
    for job in jobs[1..].iter().rev() {
        play.answer(job.basis).await;
        assert!(play.clients[client].has(job.position));
    }
    assert!(!play.clients[client].has(HOME));
    assert_eq!(play.clients[client].chunks().len(), 3);
    play.script.quiet().await;
}

/// Test 4, seventh item: a job with no thread left. Section 5: it fails at its
/// dispatch, in the dispatching turn, and the chunk is sent at once with the light a
/// failed job leaves; with a thread again, what marks the chunk gives it a job.
#[tokio::test]
async fn with_no_thread_left_a_chunk_is_sent_at_once_lit_by_heights_and_a_mark_later_lights_it() {
    let mut script = Script::start(End::Held).await;
    let mut client = script.join(1).await;
    script.held().shut(true);
    let chunk = roof(unlit(ground()), FLOOR + 3, (0, 0), (15, 15));
    script.snapshot(WEST, HOME, chunk.clone());
    script.quiet().await;
    script.sync(&mut client).await;
    assert!(script.held().out().is_empty());
    assert_eq!(client.light(HOME), Some(&as_held(&by_heights(&chunk))));
    script.held().shut(false);
    script.change(WEST, &[(at(HOME, 3, FLOOR, 3), blocks::TORCH)]);
    script.quiet().await;
    assert_eq!(places(&script.held().out()), [HOME]);
    script.answer_all().await;
    script.sync(&mut client).await;
    let due = script.model().light(HOME).unwrap();
    assert_eq!(client.light(HOME), Some(&as_held(&due)));
    assert_eq!(client.times_sent(HOME), 1);
    script.end().await;
}

// ---------------------------------------------------------------------------------
// A client whose queue is full when a result is fanned out
// ---------------------------------------------------------------------------------

/// Test 4, eighth item: a waiter whose queue is full when the result of a chunk is
/// fanned out, with a second waiter behind them: the second is sent the chunk.
/// Section 4, R2 and R3. Played with each of the two as the one whose queue is full,
/// since the record does not say in which order waiters are sent to.
#[tokio::test]
async fn a_waiter_whose_queue_is_full_at_a_result_is_removed_and_the_other_is_sent_the_chunk() {
    for full in [0, 1] {
        let mut play = Play::start().await;
        let both = [play.join(1).await, play.join(2).await];
        play.snapshot(WEST, HOME, unlit(ground())).await;
        let job = play.job(HOME);
        assert!(play.clients[both[full]].fill() > 0);
        play.answer_removing(job.basis, both[full]).await;
        let other = both[1 - full];
        assert!(play.clients[other].has(HOME));
        assert!(!play.clients[both[full]].has(HOME));
        play.end().await;
    }
}

/// Test 4, eighth item: the same for one who has the chunk, at a light update, with a
/// second behind.
#[tokio::test]
async fn a_holder_whose_queue_is_full_at_a_light_update_is_removed_and_the_other_is_sent_it() {
    for full in [0, 1] {
        let mut play = Play::start().await;
        let both = [play.join(1).await, play.join(2).await];
        play.snapshot(WEST, HOME, ground()).await;
        play.change(WEST, &[(at(HOME, 1, FLOOR, 8), blocks::TORCH)])
            .await;
        let job = play.job(HOME);
        assert!(play.clients[both[full]].fill() > 0);
        play.answer_removing(job.basis, both[full]).await;
        let other = both[1 - full];
        assert_eq!(play.lights(other, HOME).len(), 1);
        assert!(play.holds_the_rules(other, HOME));
        play.end().await;
    }
}

/// Test 4, eighth item: the same where the player who is removed was the last viewer
/// of the entry itself, of an entry in `waiting` and of an entry whose job is under
/// way. Section 4, R1 and R3: the entries nobody views any more are gone, their jobs
/// answer to nobody, and the edge goes on (its own account holds, or its task ends
/// and every wait here says so).
#[tokio::test]
async fn removing_the_last_viewer_at_a_result_drops_the_entry_one_in_waiting_and_one_with_a_job() {
    let mut play = Play::start().await;
    let (stays, goes) = (play.join(1).await, play.join(2).await);
    play.walk_along(goes, 0, 4).await;
    // Three chunks only the second player sees, and one both do.
    let own = [
        ChunkPos::new(6, 0),
        ChunkPos::new(6, 2),
        ChunkPos::new(6, -2),
    ];
    let shared = ChunkPos::new(3, 0);
    play.set_in_flight(2);
    for place in own {
        play.snapshot(WEST, place, unlit(ground())).await;
    }
    // Two have a job under way, and the third waits for a slot.
    let jobs = play.out();
    assert_eq!(places(&jobs), own[..2]);
    play.snapshot(WEST, shared, ground()).await;
    assert!(play.clients[stays].has(shared));
    assert!(play.clients[goes].fill() > 0);
    // The result of the entry itself: its one waiter is its last viewer.
    play.answer_removing(jobs[0].basis, goes).await;
    assert!(play.entries.keys().all(|place| place.x <= 4));
    // The job of an entry that went with them answers to nobody.
    play.answer(jobs[1].basis).await;
    assert!(play.out().is_empty(), "a job for an entry nobody views");
    // And the edge still serves the other.
    play.change(WEST, &[(at(shared, 1, FLOOR, 8), blocks::TORCH)])
        .await;
    play.whole_check().await;
    assert!(play.clients[stays].has(shared));
    play.end().await;
}

/// Test 4, ninth item: two results taken in turns that follow each other, for chunks
/// beside each other, where the first's fan-out removes a waiter of the second.
/// Section 4, R3: after a send nothing is taken for granted.
#[tokio::test]
async fn a_waiter_removed_at_one_result_is_not_sent_the_next_which_the_other_waiter_is() {
    for full in [0, 1] {
        let mut play = Play::start().await;
        let both = [play.join(1).await, play.join(2).await];
        play.set_in_flight(0);
        play.snapshot(WEST, HOME, unlit(ground())).await;
        play.snapshot(WEST, EAST_OF_HOME, unlit(ground())).await;
        play.set_in_flight(4);
        play.step(Ctx::default()).await;
        let (one, other) = (play.job(HOME), play.job(EAST_OF_HOME));
        assert!(play.clients[both[full]].fill() > 0);
        // Both results are there before the edge has a turn.
        note("the job for the chunk beside it is answered in the same breath".into());
        play.script.held().answer(other.basis);
        let entry = play.entries.get_mut(&EAST_OF_HOME).unwrap();
        let light = play.script.model().light(EAST_OF_HOME).unwrap();
        entry.job = None;
        entry.job_causes = Causes::default();
        entry.sendable = true;
        entry.put(light, Kind::Computed);
        entry.made_from = Some(other.nine.clone());
        play.answer_removing(one.basis, both[full]).await;
        let left = both[1 - full];
        for place in [HOME, EAST_OF_HOME] {
            assert!(play.clients[left].has(place), "{place:?}");
            assert!(play.holds_the_rules(left, place), "{place:?}");
        }
        play.end().await;
    }
}

// ---------------------------------------------------------------------------------
// Which job goes first
// ---------------------------------------------------------------------------------

/// Test 4, tenth item: forty chunks that change at every tick and a player who joins
/// beside them, with one slot: every chunk the player waits for is dispatched before
/// any of the forty. Section 4, dispatch: the first of `waiting`, always, while there
/// is one (the first review's finding 1).
#[tokio::test]
async fn chunks_someone_waits_for_are_dispatched_before_any_of_forty_that_keep_changing() {
    let mut play = Play::start().await;
    let (_first, second) = (play.join(1).await, play.join(2).await);
    play.serve(view(HOME, VIEW), |_| ground()).await;
    play.set_in_flight(1);
    // Forty chunks whose changes, in their second column from the west, reach no
    // chunk east of them: nothing beside the chunks the second player will wait for.
    let forty: Vec<_> = view(HOME, VIEW)
        .into_iter()
        .filter(|place| place.x <= 2)
        .take(40)
        .collect();
    assert_eq!(forty.len(), 40);
    let mut ticks = 0;
    let tick = |ticks: &mut u32| -> Vec<(BlockPos, BlockState)> {
        *ticks += 1;
        let state = if *ticks % 2 == 1 {
            blocks::GLOWSTONE
        } else {
            blocks::AIR
        };
        let change = |place: &ChunkPos| (at(*place, 1, FLOOR, 8), state);
        forty.iter().map(change).collect()
    };
    play.change(WEST, &tick(&mut ticks)).await;
    assert_eq!(play.out().len(), 1);

    // The second player moves on, and waits for a column of chunks without light.
    play.walk(second, EAST_OF_HOME).await;
    let waited: Vec<_> = (-3..=3).map(|z| ChunkPos::new(4, z)).collect();
    for place in &waited {
        play.snapshot(WEST, *place, unlit(ground())).await;
        play.change(WEST, &tick(&mut ticks)).await;
    }
    let before = play.dispatched.len();
    for _ in 0..waited.len() {
        let job = play.out()[0].clone();
        play.answer(job.basis).await;
        play.change(WEST, &tick(&mut ticks)).await;
    }
    let next: BTreeSet<_> = play.dispatched[before..].iter().map(|job| job.1).collect();
    let waited: BTreeSet<_> = waited.into_iter().collect();
    assert_eq!(
        next, waited,
        "a chunk nobody waits for was dispatched before one somebody waits for"
    );
    play.whole_check().await;
    for place in &waited {
        assert!(play.clients[second].has(*place));
    }
    play.end().await;
}

/// Test 4, eleventh item: with one slot and more waited-for chunks than it serves, a
/// player who flies towards lower coordinates and one who flies towards higher: for
/// each, among the chunks they wait for, one nearer to them as they now stand is
/// dispatched before one farther, and of two equally near the older. Section 4, rank
/// (the second review's finding 3).
#[tokio::test]
#[ignore = "not built: the settings of section 4 (step L4's second part)"]
async fn waited_for_chunks_are_dispatched_nearest_first_as_the_view_stands_and_then_oldest_first() {
    for towards in [-1, 1] {
        let mut play = Play::start().await;
        let client = play.join(1).await;
        play.set_in_flight(0);
        // Every chunk of the view, in ascending order: the far west is the oldest.
        let mut age = BTreeMap::new();
        for place in view(HOME, VIEW) {
            age.insert(place, age.len());
            play.snapshot(WEST, place, unlit(ground())).await;
        }
        let centre = ChunkPos::new(towards, 0);
        play.walk(client, centre).await;
        play.set_in_flight(1);
        play.step(Ctx::default()).await;
        let far = |place: ChunkPos| (place.x - centre.x).pow(2) + (place.z - centre.z).pow(2);
        let waited = play.entries.len();
        let mut order = Vec::new();
        // No chunk comes while these jobs are under way, so each chunk has one.
        for _ in 0..waited {
            let job = play.out()[0].clone();
            order.push(job.position);
            play.answer(job.basis).await;
        }
        for pair in order.windows(2) {
            let (one, next) = (pair[0], pair[1]);
            assert!(
                (far(one), age[&one]) < (far(next), age[&next]),
                "flying towards {towards}: {one:?} was dispatched before {next:?}"
            );
        }
        play.end().await;
    }
}

// ---------------------------------------------------------------------------------
// Two players
// ---------------------------------------------------------------------------------

/// Test 4, twelfth item: a second player who comes to have the neighbour of a changed
/// chunk and then the chunk (ADR-0022's finding 2). They have the neighbour and not
/// the changed chunk, so no block update reaches them: by "Found while building" they
/// are at the rim and are sent the neighbour's light; then they are sent the chunk.
#[tokio::test]
async fn a_player_with_the_neighbour_of_a_changed_chunk_is_sent_its_light_and_then_the_chunk() {
    let mut play = Play::start().await;
    let (first, second) = (play.join(1).await, play.join(2).await);
    play.serve(view(HOME, VIEW), |_| ground()).await;
    let (changed, beside) = (ChunkPos::new(-3, 0), ChunkPos::new(-2, 0));
    play.walk_along(second, 0, 1).await;
    assert!(play.clients[second].has(beside) && !play.clients[second].has(changed));
    let lamp = at(changed, 15, FLOOR, 8);
    play.change(WEST, &[(lamp, blocks::GLOWSTONE)]).await;
    play.whole_check().await;
    assert_eq!(play.lights(second, beside).len(), 1);
    assert!(play.holds_the_rules(second, beside));
    let light = play.clients[second].light(beside).unwrap();
    assert_eq!(light.block(0, up(FLOOR), 8), Some(14));
    // The first has both chunks and every chunk around the neighbour.
    assert!(play.lights(first, beside).is_empty());

    play.walk_along(second, 1, 0).await;
    play.whole_check().await;
    assert_eq!(play.clients[second].block(lamp), Some(blocks::GLOWSTONE));
    assert!(play.holds_the_rules(second, changed));
    assert!(play.holds_the_rules(second, beside));
    play.end().await;
}

/// The other way round: a second player who has a chunk and not the changed chunk
/// beside it, and then comes to have that one.
#[tokio::test]
async fn a_player_with_a_chunk_is_sent_its_light_when_the_neighbour_it_lacks_changes() {
    let mut play = Play::start().await;
    let (_first, second) = (play.join(1).await, play.join(2).await);
    play.serve(view(HOME, VIEW), |_| ground()).await;
    let (has, changed) = (ChunkPos::new(-3, 0), ChunkPos::new(-2, 0));
    play.walk_along(second, 0, -6).await;
    assert!(play.clients[second].has(has) && !play.clients[second].has(changed));
    let lamp = at(changed, 0, FLOOR, 8);
    play.change(WEST, &[(lamp, blocks::GLOWSTONE)]).await;
    play.whole_check().await;
    assert_eq!(play.lights(second, has).len(), 1);
    assert!(play.holds_the_rules(second, has));
    let light = play.clients[second].light(has).unwrap();
    assert_eq!(light.block(15, up(FLOOR), 8), Some(14));

    play.walk_along(second, -6, -5).await;
    play.whole_check().await;
    assert_eq!(play.clients[second].block(lamp), Some(blocks::GLOWSTONE));
    assert!(play.holds_the_rules(second, changed));
    assert!(play.holds_the_rules(second, has));
    play.end().await;
}

/// Test 4, twelfth item: a player whose view leaves a chunk and returns while its job
/// is under way. Another player keeps the entry, so the job stays the entry's
/// (section 3, E6 is only the last viewer's leaving) and both are sent its result.
#[tokio::test]
async fn a_player_whose_view_leaves_a_chunk_and_returns_under_its_job_is_sent_the_result() {
    let mut play = Play::start().await;
    let (first, second) = (play.join(1).await, play.join(2).await);
    let rim = ChunkPos::new(3, 0);
    play.snapshot(WEST, rim, unlit(ground())).await;
    let job = play.job(rim);
    play.walk(second, ChunkPos::new(-1, 0)).await;
    play.walk(second, HOME).await;
    assert_eq!(play.out().len(), 1);
    play.answer(job.basis).await;
    for client in [first, second] {
        assert!(play.clients[client].has(rim));
        assert_eq!(play.clients[client].times_sent(rim), 1);
    }
    assert_eq!(play.jobs_for(rim), 1);
    play.end().await;
}

// ---------------------------------------------------------------------------------
// A link that is lost and comes again
// ---------------------------------------------------------------------------------

/// Test 4, thirteenth item: a resume that brings the same chunks (E2) and one that
/// brings a changed one: an unload before the chunk, block updates in between.
/// Section 6: the client goes on showing the old chunk, and block updates for it
/// still go, until the new one is sendable; then the unload and the chunk go together.
#[tokio::test]
async fn a_resume_brings_the_same_chunk_for_nothing_and_a_changed_one_after_an_unload() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    play.snapshot(WEST, APART, ground()).await;
    let told = play.clients[client].told().len();

    note("the west's link is lost and comes again".into());
    play.script.lose(WEST).await;
    play.script.link(WEST).await;
    play.script.resume(WEST);
    play.step(Ctx::default()).await;
    play.snapshot(WEST, HOME, ground()).await;
    let stone = at(APART, 2, FLOOR, 2);
    play.snapshot(WEST, APART, with(ground(), &[(2, FLOOR, 2, blocks::STONE)]))
        .await;
    let job = play.job(APART);
    assert_eq!(play.clients[client].told().len(), told);
    assert_eq!(play.clients[client].block(stone), Some(blocks::AIR));

    // A block update for the chunk the client still shows.
    let torch = at(APART, 9, FLOOR, 9);
    play.change(WEST, &[(torch, blocks::TORCH)]).await;
    assert_eq!(play.clients[client].block(torch), Some(blocks::TORCH));
    assert_eq!(play.clients[client].times_sent(APART), 1);
    play.answer(job.basis).await;
    let of_chunks = |told: &&Told| told.chunk().is_some();
    let since: Vec<_> = play.clients[client].told()[told..]
        .iter()
        .filter(of_chunks)
        .copied()
        .collect();
    assert_eq!(
        since,
        [
            Told::Block(torch, blocks::TORCH),
            Told::Unloaded(APART),
            Told::Sent(APART)
        ]
    );
    assert_eq!(play.clients[client].block(stone), Some(blocks::STONE));
    assert_eq!(play.clients[client].block(torch), Some(blocks::TORCH));
    play.whole_check().await;
    assert!(play.holds_the_rules(client, APART));
    assert_eq!(play.clients[client].times_sent(HOME), 1);
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Sections that stay listed, and batches
// ---------------------------------------------------------------------------------

/// Test 4, fourteenth item: a section that is listed, becomes `Absent`, and whose
/// levels then change (a roof widened far above): it is named again. Section 6: a
/// section that was once listed stays listed for the life of the entry, as its
/// levels, and is compared and named like any other.
#[tokio::test]
async fn a_section_that_was_listed_and_is_absent_by_the_rules_is_named_again_when_its_levels_change()
 {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.snapshot(WEST, HOME, ground()).await;
    // The light section of a block at height 100.
    let section = up(100) / 16;
    let top = at(HOME, 8, 100, 8);
    play.change(WEST, &[(top, blocks::STONE)]).await;
    play.whole_check().await;
    assert!(
        play.clients[client]
            .light(HOME)
            .unwrap()
            .sky(8, up(99), 8)
            .is_some()
    );
    play.change(WEST, &[(top, blocks::AIR)]).await;
    play.whole_check().await;
    // By the rules the section is absent now; the client still has it, as its levels.
    let rules = play.script.model().light(HOME).unwrap();
    assert_eq!(rules.sky[section], SectionLight::Absent);
    let light = play.clients[client].light(HOME).unwrap();
    assert_eq!(light.sky(8, up(100), 8), Some(15));

    // A roof far above, and then wider.
    let mut wide = 0;
    for reach in [1, 3] {
        let mut stones = Vec::new();
        for x in 8 - reach..=8 + reach {
            for z in 8 - reach..=8 + reach {
                stones.push((at(HOME, x, 200, z), blocks::STONE));
            }
        }
        let before = play.lights(client, HOME).len();
        play.change(WEST, &stones).await;
        play.whole_check().await;
        let lights = play.lights(client, HOME);
        assert_eq!(lights.len(), before + 1);
        let named = lights.last().unwrap();
        assert_ne!(named.any_sky() >> section & 1, 0, "{named}");
        // Under the middle of the roof the sky's light comes in from its sides.
        let light = play.clients[client].light(HOME).unwrap();
        let level = light.sky(8, up(100), 8).expect("the section is listed");
        assert_eq!(level, 15 - (reach as u8 + 1));
        wide += 1;
    }
    assert_eq!(wide, 2);
    let rules = play.script.model().light(HOME).unwrap();
    assert_eq!(rules.sky[section], SectionLight::Absent);
    play.end().await;
}

/// Test 4, last item: a batch confirmed while everything pending waits for light.
/// Section 6: if none is sendable nothing is sent, no start and no end, and the next
/// chunk that becomes sendable is offered.
#[tokio::test]
async fn a_batch_confirmed_while_all_that_is_pending_waits_for_light_is_followed_by_no_empty_one() {
    let mut play = Play::start().await;
    let client = play.join(1).await;
    play.confirm(client, false).await;
    play.snapshot(WEST, HOME, ground()).await;
    assert!(play.clients[client].has(HOME));
    for place in [APART, ChunkPos::new(0, 2)] {
        play.snapshot(WEST, place, unlit(ground())).await;
    }
    assert_eq!(play.out().len(), 2);
    play.confirm(client, true).await;
    let batches = |play: &Play| {
        let begun = |told: &&Told| **told == Told::BatchStarted;
        play.clients[client].told().iter().filter(begun).count()
    };
    assert_eq!(batches(&play), 1);
    play.answer_all().await;
    assert!(play.clients[client].has(APART));
    assert_eq!(batches(&play), 3);
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Found by the random orders
// ---------------------------------------------------------------------------------

/// Found by the random orders. Two players wait for a chunk; one is sent it and the
/// other has a batch outstanding, so the entry keeps its packet (section 1: the
/// packet is kept while the entry has a waiter). Then a chunk comes beside it, and
/// the entry is unsendable until its next job (section 3, arrive, step 4, which says
/// nothing of the packet). Then a view moves, before that job is answered. Nothing
/// in the record ends the edge there: the waiter goes on waiting, and is sent the
/// chunk with the light of the next job (section 5: the result's light is put in and
/// the packet dropped). Without the walk this order passes.
#[tokio::test]
#[ignore = "finding: the edge's own check ends its task (\"keeps a packet of light that may \
            not be sent\") when a view moves while a chunk a slow client waits for has \
            become unsendable"]
async fn a_chunk_that_comes_beside_one_a_slow_client_still_waits_for_leaves_the_edge_running() {
    let mut play = Play::start().await;
    let (quick, slow) = (play.join(1).await, play.join(2).await);
    play.confirm(slow, false).await;
    play.snapshot(WEST, APART, ground()).await;
    // The slow client has a batch outstanding from here on.
    assert!(play.clients[slow].has(APART));
    play.snapshot(WEST, HOME, unlit(ground())).await;
    play.answer_all().await;
    assert!(play.clients[quick].has(HOME) && !play.clients[slow].has(HOME));
    play.snapshot(WEST, EAST_OF_HOME, lamp(ground(), 0, FLOOR, 8))
        .await;
    play.walk(quick, ChunkPos::new(0, 1)).await;
    play.confirm(slow, true).await;
    play.whole_check().await;
    assert!(play.clients[slow].has(HOME));
    assert!(play.holds_the_rules(slow, HOME));
    assert!(play.holds_the_rules(quick, HOME));
    play.end().await;
}

// ---------------------------------------------------------------------------------
// Random orders from a seed
// ---------------------------------------------------------------------------------

/// A small generator of numbers that gives the same ones for the same seed.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `bound`.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn of<T: Copy>(&mut self, choices: &[T]) -> T {
        choices[self.below(choices.len())]
    }
}

/// The columns a random block is in: at the borders, where light crosses, and in the
/// middle, from where it reaches less.
const COLUMNS: [usize; 6] = [0, 1, 7, 8, 14, 15];
/// And its heights: in the ground, on it, above it, and in a section that is empty.
const HEIGHTS: [i32; 5] = [FLOOR - 1, FLOOR, FLOOR + 1, 100, 101];

/// A chunk as a region might send it, and what to call it.
fn terrain(dice: &mut Dice) -> (Chunk, String) {
    let (x, z) = (dice.of(&COLUMNS), dice.of(&COLUMNS));
    let bare = unlit(ground());
    match dice.below(12) {
        0..=3 => (ground(), "ground with its light".into()),
        4 => (unlit(ground()), "ground without light".into()),
        5 | 6 => (
            lamp(ground(), x, FLOOR, z),
            format!("ground with a lamp at {x}, {z} and no light"),
        ),
        7 => (
            lit_among(lamp(ground(), x, FLOOR, z), [Some(&bare); 9]),
            format!("ground with a lamp at {x}, {z}, stored with its light"),
        ),
        8 => (
            with(ground(), &[(x, FLOOR - 1, z, blocks::GRANITE)]),
            "ground with granite in it, which keeps its light".into(),
        ),
        9 => (
            roof(unlit(ground()), 100, (x, z), (x, z)),
            format!("ground with a stone high above {x}, {z} and no light"),
        ),
        10 => {
            let mut chunk = ground();
            let mut light = chunk.take_light().expect("ground has light");
            light.made_by += 1;
            chunk.set_light(light);
            (chunk, "ground with light of other rules".into())
        }
        _ => {
            let mut chunk = ground();
            let mut light = chunk.take_light().expect("ground has light");
            chunk.mark_generated(2, false);
            light.beside = 2;
            chunk.set_light(light);
            (chunk, "ground of another generator's version".into())
        }
    }
}

/// A block of `chunk` and a state for it: of every kind light sees and does not.
fn block(dice: &mut Dice, chunk: ChunkPos) -> (BlockPos, BlockState) {
    let (x, z) = (dice.of(&COLUMNS), dice.of(&COLUMNS));
    let y = dice.of(&HEIGHTS);
    let state = dice.of(&[
        blocks::TORCH,
        blocks::GLOWSTONE,
        blocks::STONE,
        blocks::GLASS,
        blocks::AIR,
        blocks::AIR,
        blocks::GRANITE,
        blocks::CAVE_AIR,
        blocks::DIRT,
    ]);
    (at(chunk, x as i32, y, z as i32), state)
}

/// Plays `steps` random steps from `seed` and holds the edge to the record after
/// each: see the head of the file for what is checked.
///
/// With slow clients, they stop confirming their batches now and then, and confirm
/// again: a chunk can then wait for a client while it is sendable, and stop being so.
async fn random_orders(seed: u64, steps: usize, slow: Slow) -> Seen {
    let mut dice = Dice(seed);
    let mut play = Play::start().await;
    note(format!("seed {seed}"));
    // A holder inside its view that was sent light older than the blocks is not
    // promised the next light by "Found while building": see the finding above.
    play.stale_is_promised = false;
    play.set_in_flight(dice.of(&[1, 2, 4]));
    let mut joined = 0;
    let mut centres: BTreeMap<usize, ChunkPos> = BTreeMap::new();
    for _ in 0..steps {
        let live = play.live();
        if live.is_empty() {
            joined += 1;
            let who = play.join(joined).await;
            centres.insert(who, HOME);
            continue;
        }
        let asked: Vec<_> = play.script.asked(WEST).into_iter().collect();
        let (come, awaited): (Vec<ChunkPos>, Vec<ChunkPos>) = asked
            .iter()
            .partition(|place| play.script.model().has(**place));
        let lagging = play.watch.iter().any(|watch| watch.lags && !watch.gone);
        let stand = slow == Slow::WhileViewsStand && lagging;
        let roll = dice.below(100);
        if slow == Slow::WhileViewsStand && matches!(roll, 0..=11 | 90..=91) {
            play.answer_all().await;
        }
        let out = play.out();
        match roll {
            0..=2 if live.len() < 3 && !stand => {
                joined += 1;
                let who = play.join(joined).await;
                centres.insert(who, HOME);
            }
            3 if live.len() > 1 && !stand => play.leave(dice.of(&live)).await,
            95..=96 if slow != Slow::Never => {
                // A client that stops confirming its batches, or confirms again.
                let who = dice.of(&live);
                let lags = play.watch[who].lags;
                play.confirm(who, lags).await;
            }
            4..=11 if !stand => {
                let who = dice.of(&live);
                let from = centres[&who];
                let to = ChunkPos::new(
                    (from.x + dice.below(3) as i32 - 1).clamp(-2, 2),
                    (from.z + dice.below(3) as i32 - 1).clamp(-2, 2),
                );
                if to != from {
                    centres.insert(who, to);
                    play.walk(who, to).await;
                }
            }
            12..=37 if !awaited.is_empty() => {
                let (chunk, what) = terrain(&mut dice);
                let place = dice.of(&awaited);
                play.snapshot_called(WEST, place, chunk, &what).await;
            }
            38..=40 if !come.is_empty() => {
                let place = dice.of(&come);
                let chunk = play.script.model().chunk(place).expect("it has come");
                play.snapshot_called(WEST, place, chunk.clone(), "the same chunk")
                    .await;
            }
            41..=43 if !come.is_empty() => {
                let place = dice.of(&come);
                play.snapshot_called(WEST, place, other(&mut dice, &play, place), "another")
                    .await;
            }
            44..=65 if !asked.is_empty() => {
                let mut changes = Vec::new();
                for _ in 0..=dice.below(3) {
                    // Mostly in a chunk that has come.
                    let places = if come.is_empty() || dice.below(10) == 0 {
                        &asked
                    } else {
                        &come
                    };
                    let place = dice.of(places);
                    changes.push(block(&mut dice, place));
                }
                play.change(WEST, &changes).await;
                // Now and then the east says the same again.
                if dice.below(8) == 0 {
                    play.change(EAST, &changes).await;
                }
            }
            66..=87 if !out.is_empty() => {
                // The oldest, the newest or any.
                let job = match dice.below(3) {
                    0 => &out[0],
                    1 => &out[out.len() - 1],
                    _ => &out[dice.below(out.len())],
                };
                play.answer(job.basis).await;
            }
            88..=89 if !out.is_empty() => {
                play.fail(out[dice.below(out.len())].basis).await;
            }
            90..=91 if !stand => {
                note("the west's link is lost and comes again".into());
                play.script.lose(WEST).await;
                play.script.link(WEST).await;
                play.script.resume(WEST);
                play.step(Ctx::default()).await;
                // The region owes every chunk it had sent: most as they were.
                for place in come {
                    if dice.below(6) == 0 {
                        let chunk = other(&mut dice, &play, place);
                        play.snapshot_called(WEST, place, chunk, "another").await;
                    } else {
                        let chunk = play.script.model().chunk(place).expect("it has come");
                        play.snapshot_called(WEST, place, chunk.clone(), "the same chunk")
                            .await;
                    }
                }
            }
            92..=94 => play.whole_check().await,
            _ => {
                play.answer_all().await;
            }
        }
    }
    play.end().await
}

/// The chunk the model has at `place` with one block that shows otherwise, without
/// light: as after changes that were missed.
fn other(dice: &mut Dice, play: &Play, place: ChunkPos) -> Chunk {
    let chunk = play.script.model().chunk(place).expect("it has come");
    loop {
        let (block, state) = block(dice, place);
        let (x, z) = (
            block.x.rem_euclid(16) as usize,
            block.z.rem_euclid(16) as usize,
        );
        if chunk.get(x, block.y, z) != Some(state) {
            return with(chunk.clone(), &[(x, block.y, z, state)]);
        }
    }
}

/// Whether the clients of a random run stop confirming their batches for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slow {
    Never,
    Sometimes,
    /// And no view moves and no link ends while one of them does not confirm, nor
    /// before every job is answered: the orders that are left when the finding
    /// above is kept out of the way.
    WhileViewsStand,
}

/// How many steps a seed of the ordinary run plays.
const STEPS_A_SEED: usize = 150;

/// Test 4, "in random orders from a seed": snapshots of every kind, block changes of
/// every kind, views that move, players who join and leave, a link lost and resumed,
/// jobs answered late, out of order and failing. A failure prints the seed and the
/// steps; `CLUSTINE_LIGHT_ORDERS_SEED=<seed>` plays that one in the long run below.
#[tokio::test]
async fn random_orders_of_events_keep_the_standing_rule() {
    let mut seen = Seen::default();
    for seed in 1..=16 {
        seen.add(random_orders(seed, STEPS_A_SEED, Slow::Never).await);
    }
    for seed in 17..=24 {
        seen.add(random_orders(seed, STEPS_A_SEED, Slow::WhileViewsStand).await);
    }
    println!("{seen:?}");
    // A run that looked at nothing has shown nothing.
    assert!(
        seen.light_updates > 100 && seen.results_withheld > 10,
        "{seen:?}"
    );
    assert!(
        seen.chunks_sent_again > 10 && seen.jobs_failed > 10,
        "{seen:?}"
    );
}

/// The same with clients that stop confirming their batches for a while.
#[tokio::test]
#[ignore = "finding: the edge's own check ends its task (\"keeps a packet of light that may \
            not be sent\") when a view moves while a chunk a slow client waits for has \
            become unsendable"]
async fn random_orders_of_events_with_slow_clients_keep_the_standing_rule() {
    for seed in 1..=16 {
        random_orders(seed, STEPS_A_SEED, Slow::Sometimes).await;
    }
}

/// The long run: `CLUSTINE_LIGHT_ORDERS_SEEDS` seeds (200 unless said) of
/// `CLUSTINE_LIGHT_ORDERS_STEPS` steps each (300), from the seed
/// `CLUSTINE_LIGHT_ORDERS_SEED` on (1,000); with slow clients if
/// `CLUSTINE_LIGHT_ORDERS_SLOW` is 1, and if it is 2 with slow clients while no
/// view moves.
#[tokio::test]
#[ignore = "long: some minutes of random orders; see the environment variables above it"]
async fn many_random_orders_of_events_keep_the_standing_rule() {
    let number = |name: &str, otherwise: u64| -> u64 {
        let said = std::env::var(name).ok();
        said.map_or(otherwise, |said| said.parse().expect("a number"))
    };
    let first = number("CLUSTINE_LIGHT_ORDERS_SEED", 1_000);
    let seeds = number("CLUSTINE_LIGHT_ORDERS_SEEDS", 200);
    let steps = number("CLUSTINE_LIGHT_ORDERS_STEPS", 300) as usize;
    let slow = match number("CLUSTINE_LIGHT_ORDERS_SLOW", 0) {
        0 => Slow::Never,
        1 => Slow::Sometimes,
        _ => Slow::WhileViewsStand,
    };
    let mut seen = Seen::default();
    for seed in first..first + seeds {
        seen.add(random_orders(seed, steps, slow).await);
    }
    println!("{seen:?}");
}
