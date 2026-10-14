//! Lighting a chunk away from the fan-out task
//! (`docs/adr/0023-light-at-the-edge.md`, section 5).
//!
//! The fan-out task never runs the light rules itself: a chunk is a couple of
//! milliseconds, and a join into changed terrain is hundreds of chunks. It makes a
//! [`LightJob`] of nine shared chunks that nobody changes, hands it to a [`Lighting`],
//! and reads what comes back ([`FromThread`]) as one more arm of its loop. What a job
//! does is [`relight`], a function of the job alone.

use std::any::Any;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;

use bytes::Bytes;
use clustine_light::{Around, Scratch, Unlisted, light_with};
use clustine_protocol::packets;
use clustine_protocol::packets::play::{LightData, LightUpdate};
use clustine_world::{Chunk, ChunkPos, Light, SectionLight};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::error;

use crate::encode::{Named, light_data};

/// One chunk to light, with all that the light is made from.
#[derive(Debug, Clone)]
pub(crate) struct LightJob {
    /// The fan-out task's moment at the dispatch. Nothing here reads it: it comes
    /// back unchanged with everything that is said of the job, and the task tells by
    /// it whether the result is still wanted.
    pub basis: u64,
    pub position: ChunkPos,
    /// North-west first, row by row; the centre, the fifth, is always there. One that
    /// is not there is a wall to light.
    pub nine: [Option<Arc<Chunk>>; 9],
    /// Whether the dimension has a sky.
    pub sky: bool,
    /// The light the chunk has now. What it lists stays listed, and the update names
    /// what differs from it.
    pub old: Option<Arc<Light>>,
    /// Whether the update is to name every listed section, or those that differ from
    /// `old`.
    pub whole: bool,
}

/// What became of a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LightOutcome {
    Lit {
        light: Arc<Light>,
        /// The encoded light update for those who have the chunk; none if it would
        /// name nothing.
        update: Option<Bytes>,
    },
    /// The rules did not give light for these chunks. `why` is for the log.
    Failed { why: String },
}

/// What whoever runs jobs says of them, in the order it happened: `Started` before a
/// job is begun, so that the task knows which thread has which job and since when,
/// and `Done` with its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FromThread {
    Started {
        basis: u64,
        thread: usize,
    },
    Done {
        basis: u64,
        position: ChunkPos,
        outcome: LightOutcome,
    },
}

/// The light of the job's chunk and the update that says it to a client.
///
/// Pure: the same job gives the same result, whatever `scratch` was used for before.
/// Nothing but the job is read, and of its chunks only the blocks.
///
/// The light is the rules' for the nine chunks, and in addition lists every section
/// `old` lists: a client that was once sent a section cannot be told to forget it, so
/// a section the rules now leave out is kept with the levels it has today (section 6
/// of the record).
///
/// Panics where the rules do (a neighbour of another height than the centre); that
/// is the caller's to catch.
pub(crate) fn relight(job: &LightJob, scratch: &mut Scratch) -> LightOutcome {
    let [nw, n, ne, w, centre, e, sw, s, se] = &job.nine;
    let Some(centre) = centre else {
        return LightOutcome::Failed {
            why: "the job has no chunk to light".into(),
        };
    };
    let around = Around {
        centre: centre.as_ref(),
        neighbours: [nw, n, ne, w, e, sw, s, se].map(|chunk| chunk.as_deref()),
        sky: job.sky,
    };
    let old = job.old.as_deref();
    let also = old.map_or(Unlisted::NONE, Unlisted::listed_in);
    let (mut light, kept) = light_with(&around, also, scratch);
    kept.put_into(&mut light);

    let named = if job.whole {
        Named::ALL
    } else {
        differing(old, &light)
    };
    let data = light_data(&light.sky, &light.block, named);
    let update = names_any(&data).then(|| {
        let packet = LightUpdate {
            chunk_x: job.position.x,
            chunk_z: job.position.z,
            light: data,
        };
        Bytes::from(packets::encode(&packet))
    });
    LightOutcome::Lit {
        light: Arc::new(light),
        update,
    }
}

/// The sections in which `new` is not what `old` is, for the sky and for blocks
/// apart. Without old light every section differs: the client was sent none.
fn differing(old: Option<&Light>, new: &Light) -> Named {
    let Some(old) = old else {
        return Named::ALL;
    };
    let differing = |old: &[SectionLight], new: &[SectionLight]| {
        new.iter()
            .take(Unlisted::SECTIONS)
            .enumerate()
            // A section the old light did not have was never said.
            .filter(|(index, section)| old.get(*index).unwrap_or(&SectionLight::Absent) != *section)
            .fold(0, |mask, (index, _)| mask | 1 << index)
    };
    Named {
        sky: differing(&old.sky, &new.sky),
        block: differing(&old.block, &new.block),
    }
}

/// Whether a packet's light names any section at all.
fn names_any(light: &LightData) -> bool {
    [
        &light.sky_mask,
        &light.empty_sky_mask,
        &light.block_mask,
        &light.empty_block_mask,
    ]
    .into_iter()
    .flatten()
    .any(|word| *word != 0)
}

/// How many threads light chunks on a machine of `processors`: a quarter of them, and
/// at least two, so that one long job does not hold up every other.
pub(crate) fn threads_for(processors: usize) -> usize {
    (processors / 4).max(2)
}

/// [`threads_for`] this machine.
pub(crate) fn default_threads() -> usize {
    threads_for(thread::available_parallelism().map_or(1, usize::from))
}

/// What a thread runs for a job: [`relight`], or what a test hands it.
type Work = dyn Fn(&LightJob, &mut Scratch) -> LightOutcome + Send + Sync;

/// Whoever runs jobs one after the other: a thread, or the turn that dispatches.
struct Worker {
    thread: usize,
    /// Made at the first job and not before: most edges of the end-to-end tests never
    /// light a chunk, and a scratch that was used is megabytes. Made anew after a job
    /// that panicked.
    scratch: Option<Scratch>,
    results: UnboundedSender<FromThread>,
}

impl Worker {
    /// Runs one job and says so before and after. A panic of the work is the job's
    /// failure and not the worker's. False if nobody listens any more.
    fn run(&mut self, job: LightJob, work: &Work) -> bool {
        let (basis, position) = (job.basis, job.position);
        let started = FromThread::Started {
            basis,
            thread: self.thread,
        };
        if self.results.send(started).is_err() {
            return false;
        }
        let scratch = self.scratch.get_or_insert_with(Scratch::new);
        // The job is shared chunks nobody changes and the scratch is thrown away
        // below, so nothing a panic left half done is looked at again.
        let outcome = match catch_unwind(AssertUnwindSafe(|| work(&job, scratch))) {
            Ok(outcome) => outcome,
            Err(panic) => {
                self.scratch = None;
                LightOutcome::Failed { why: said(&*panic) }
            }
        };
        // The nine chunks are let go before the result is told: a block change that
        // follows the result then finds the chunk its own and copies nothing.
        drop(job);
        let done = FromThread::Done {
            basis,
            position,
            outcome,
        };
        self.results.send(done).is_ok()
    }
}

/// What a panic said.
fn said(panic: &(dyn Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic that said nothing".to_owned()
    }
}

/// Where jobs go. What is said of them comes out of the receiver that was made with
/// it.
///
/// Neither the queue of jobs nor that of results has a bound of its own: whoever
/// dispatches has at most [`Lighting::in_flight`] jobs out at a time, so neither
/// holds more.
pub(crate) struct Lighting {
    to: To,
    threads: usize,
}

enum To {
    /// The queue the threads take from.
    Threads(Threads),
    /// Run where [`Lighting::send`] is called. Only the edge's tests ask for that.
    #[cfg(test)]
    Here(Box<Worker>),
    /// Handed to a test, which answers when and how it likes: see [`Held`].
    #[cfg(test)]
    Held(held::Door),
}

/// The threads' queue, and what it takes to start one more in place of one that
/// was written off.
struct Threads {
    jobs: mpsc::Sender<LightJob>,
    queue: Arc<Mutex<mpsc::Receiver<LightJob>>>,
    work: Arc<Work>,
    results: UnboundedSender<FromThread>,
    /// The threads that are counted on, each with the word that tells it to end
    /// after the job it is in.
    running: BTreeMap<usize, Arc<AtomicBool>>,
    /// The number the next thread gets. No two threads share one, so that what a
    /// thread written off says later is never taken for its successor's.
    next: usize,
    /// How many threads were started in place of one written off.
    replaced: usize,
}

/// How many threads are started in place of threads written off, over the life of
/// an edge. A thread that hangs was not meant to be possible; one that hangs again
/// and again is not mended by a fifth.
const REPLACEMENTS: usize = 4;

impl Threads {
    /// Starts one more thread. False if the machine gave none.
    fn start(&mut self) -> bool {
        let thread = self.next;
        self.next += 1;
        let queue = self.queue.clone();
        let work = self.work.clone();
        let retired = Arc::new(AtomicBool::new(false));
        let told = retired.clone();
        let mut worker = Worker {
            thread,
            scratch: None,
            results: self.results.clone(),
        };
        let run = move || {
            loop {
                // The lock is held while waiting: the others wait for the lock in
                // place of the queue, and the first to have it has the next job.
                // No thread panics with it, since a job's panic is caught.
                let job = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
                let Ok(job) = job else {
                    // The `Lighting` is gone.
                    break;
                };
                // A thread that was written off and came back all the same has
                // been replaced: it ends, and what it said of the job was dropped.
                if !worker.run(job, &*work) || told.load(Ordering::Relaxed) {
                    break;
                }
            }
        };
        match thread::Builder::new()
            .name(format!("light-{thread}"))
            .spawn(run)
        {
            Ok(_) => {
                self.running.insert(thread, retired);
                true
            }
            Err(why) => {
                error!(thread, %why, "A thread for light could not be started");
                false
            }
        }
    }
}

impl Lighting {
    /// `count` threads that take jobs from one queue, each with a scratch of its own.
    /// They end when the `Lighting` is dropped, each after the job it is in; nobody
    /// joins them.
    pub(crate) fn threads(count: usize) -> (Lighting, UnboundedReceiver<FromThread>) {
        Lighting::threads_running(count, Arc::new(relight))
    }

    /// As [`Lighting::threads`], with what the threads run for a job handed in.
    pub(crate) fn threads_running(
        count: usize,
        work: Arc<Work>,
    ) -> (Lighting, UnboundedReceiver<FromThread>) {
        let (jobs, queue) = mpsc::channel::<LightJob>();
        let (results, from_threads) = unbounded_channel();
        let mut threads = Threads {
            jobs,
            queue: Arc::new(Mutex::new(queue)),
            work,
            results,
            running: BTreeMap::new(),
            next: 0,
            replaced: 0,
        };
        for _ in 0..count {
            threads.start();
        }
        let lighting = Lighting {
            threads: threads.running.len(),
            to: To::Threads(threads),
        };
        (lighting, from_threads)
    }

    /// The thread numbered `thread` is counted on no longer: it has had a job for
    /// longer than a job may take. It cannot be stopped; it ends if it ever comes
    /// back. One is started in its place, unless [`REPLACEMENTS`] were already.
    /// Returns how many threads are left to take jobs.
    pub(crate) fn write_off(&mut self, thread: usize) -> usize {
        if let To::Threads(threads) = &mut self.to
            && let Some(retired) = threads.running.remove(&thread)
        {
            retired.store(true, Ordering::Relaxed);
            if threads.replaced < REPLACEMENTS && threads.start() {
                threads.replaced += 1;
            }
            self.threads = threads.running.len();
        }
        self.threads
    }

    /// Jobs are run where [`Lighting::send`] is called, and the result is in the
    /// receiver when it returns: for whoever must have sent all it will by the end of
    /// a turn.
    #[cfg(test)]
    pub(crate) fn here() -> (Lighting, UnboundedReceiver<FromThread>) {
        let (results, from_here) = unbounded_channel();
        let worker = Worker {
            thread: 0,
            scratch: None,
            results,
        };
        let lighting = Lighting {
            to: To::Here(Box::new(worker)),
            threads: 1,
        };
        (lighting, from_here)
    }

    /// Hands a job over. It is given back if no thread is left to take it.
    pub(crate) fn send(&mut self, job: LightJob) -> Result<(), LightJob> {
        match &mut self.to {
            // With every thread written off the queue would still take the job,
            // and nobody would ever take it out.
            To::Threads(threads) if threads.running.is_empty() => Err(job),
            To::Threads(threads) => threads.jobs.send(job).map_err(|refused| refused.0),
            #[cfg(test)]
            To::Here(worker) => {
                worker.run(job, &relight);
                Ok(())
            }
            #[cfg(test)]
            To::Held(door) => door.take(job),
        }
    }

    /// How many jobs may be out at a time: two a thread, one that is worked on and
    /// one that waits, so that a thread that finishes finds its next job there.
    pub(crate) fn in_flight(&self) -> usize {
        #[cfg(test)]
        if let To::Held(door) = &self.to {
            return door.in_flight();
        }
        // At least one, also with no thread left: such a job fails where it is
        // dispatched and holds its slot for no longer than that.
        (2 * self.threads).max(1)
    }
}

#[cfg(test)]
pub(crate) use held::Held;

/// The end of [`Lighting`] that hands its jobs to a test.
#[cfg(test)]
mod held {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    /// How long [`Held::next`] waits for a job that has to come.
    const SOON: Duration = Duration::from_secs(10);

    /// The dispatching side: where [`Lighting::send`] puts a job.
    pub(super) struct Door {
        jobs: UnboundedSender<LightJob>,
        slots: Arc<AtomicUsize>,
        shut: Arc<AtomicBool>,
    }

    impl Door {
        /// Hands the job to the test, or gives it back as an end without a thread
        /// does.
        pub(super) fn take(&self, job: LightJob) -> Result<(), LightJob> {
            if self.shut.load(Ordering::Relaxed) {
                return Err(job);
            }
            self.jobs.send(job).map_err(|refused| refused.0)
        }

        pub(super) fn in_flight(&self) -> usize {
            self.slots.load(Ordering::Relaxed)
        }
    }

    /// A test's handle on the jobs of a [`Lighting::held`]: it sees each job as it is
    /// dispatched and answers it when and how it likes, or never, in any order.
    ///
    /// A job is named by its `basis`, which no two jobs share. A job that was
    /// dispatched and not answered is **out**; it holds its nine chunks for as long
    /// as it is.
    pub(crate) struct Held {
        dispatched: UnboundedReceiver<LightJob>,
        /// The jobs that are out, by basis: in the order they were dispatched, since
        /// a later job has the greater basis.
        out: BTreeMap<u64, LightJob>,
        /// Those of them that neither [`Held::next`] nor [`Held::fresh`] has given
        /// yet.
        fresh: VecDeque<u64>,
        results: UnboundedSender<FromThread>,
        slots: Arc<AtomicUsize>,
        shut: Arc<AtomicBool>,
        /// Made at the first real answer: megabytes, which most tests never need.
        scratch: Option<Scratch>,
    }

    impl Lighting {
        /// Jobs are handed to the [`Held`] and nothing is said of them but what the
        /// test has it say. Four jobs may be out at a time, as with the two threads
        /// the server has at least, until [`Held::set_in_flight`] says otherwise.
        pub(crate) fn held() -> (Lighting, Held, UnboundedReceiver<FromThread>) {
            let (jobs, dispatched) = unbounded_channel();
            let (results, from_held) = unbounded_channel();
            let slots = Arc::new(AtomicUsize::new(4));
            let shut = Arc::new(AtomicBool::new(false));
            let door = Door {
                jobs,
                slots: slots.clone(),
                shut: shut.clone(),
            };
            let lighting = Lighting {
                to: To::Held(door),
                threads: 0,
            };
            let held = Held {
                dispatched,
                out: BTreeMap::new(),
                fresh: VecDeque::new(),
                results,
                slots,
                shut,
                scratch: None,
            };
            (lighting, held, from_held)
        }
    }

    impl Held {
        /// How many jobs may be out at a time from now on
        /// ([`Lighting::in_flight`]). Jobs that are out already stay out.
        pub(crate) fn set_in_flight(&self, jobs: usize) {
            self.slots.store(jobs, Ordering::Relaxed);
        }

        /// While `shut`, a job is given back at its dispatch, as when no thread is
        /// left to take it.
        pub(crate) fn shut(&self, shut: bool) {
            self.shut.store(shut, Ordering::Relaxed);
        }

        /// Takes in what was dispatched since this was last asked.
        fn look(&mut self) {
            while let Ok(job) = self.dispatched.try_recv() {
                self.fresh.push_back(job.basis);
                self.out.insert(job.basis, job);
            }
        }

        /// The next job that was dispatched and that neither this nor
        /// [`Held::fresh`] has given yet, in the order of dispatch. Waits for one;
        /// panics if none comes.
        pub(crate) async fn next(&mut self) -> LightJob {
            self.look();
            if self.fresh.is_empty() {
                let job = tokio::time::timeout(SOON, self.dispatched.recv())
                    .await
                    .expect("no job was dispatched")
                    .expect("whoever dispatches jobs is gone");
                self.fresh.push_back(job.basis);
                self.out.insert(job.basis, job);
            }
            let basis = self.fresh.pop_front().expect("put there above");
            self.out[&basis].clone()
        }

        /// The jobs dispatched so far that neither this nor [`Held::next`] has given
        /// yet, in the order of dispatch. Waits for nothing.
        pub(crate) fn fresh(&mut self) -> Vec<LightJob> {
            self.look();
            let fresh: Vec<u64> = self.fresh.drain(..).collect();
            fresh.iter().map(|basis| self.out[basis].clone()).collect()
        }

        /// Every job that is out, in the order of dispatch. Waits for nothing.
        pub(crate) fn out(&mut self) -> Vec<LightJob> {
            self.look();
            self.out.values().cloned().collect()
        }

        /// The job that is out for the chunk at `position`: the last dispatched, if
        /// there are several.
        pub(crate) fn job_for(&mut self, position: ChunkPos) -> Option<LightJob> {
            self.look();
            let mut jobs = self.out.values().rev();
            jobs.find(|job| job.position == position).cloned()
        }

        /// Says that thread 0 has begun the job, as a thread does before it works.
        pub(crate) fn started(&self, basis: u64) {
            self.say(FromThread::Started { basis, thread: 0 });
        }

        /// Answers the job with what [`relight`] gives for it, or with a failure if
        /// the rules panic, as a thread does. Returns what was said.
        pub(crate) fn answer(&mut self, basis: u64) -> LightOutcome {
            let job = self.taken_out(basis);
            let scratch = self.scratch.get_or_insert_with(Scratch::new);
            let outcome = match catch_unwind(AssertUnwindSafe(|| relight(&job, scratch))) {
                Ok(outcome) => outcome,
                Err(panic) => {
                    // It may be half written.
                    self.scratch = None;
                    LightOutcome::Failed { why: said(&*panic) }
                }
            };
            self.done(job, outcome)
        }

        /// Answers the job with `light`, whatever the rules would give, and with the
        /// update that says it: the sections in which it differs from the job's old
        /// light, or every listed one if the job is `whole`. Returns what was said.
        pub(crate) fn answer_with(&mut self, basis: u64, light: Light) -> LightOutcome {
            let job = self.taken_out(basis);
            let named = if job.whole {
                Named::ALL
            } else {
                differing(job.old.as_deref(), &light)
            };
            let data = light_data(&light.sky, &light.block, named);
            let update = names_any(&data).then(|| {
                let packet = LightUpdate {
                    chunk_x: job.position.x,
                    chunk_z: job.position.z,
                    light: data,
                };
                Bytes::from(packets::encode(&packet))
            });
            let light = Arc::new(light);
            self.done(job, LightOutcome::Lit { light, update })
        }

        /// Answers the job with a failure.
        pub(crate) fn fail(&mut self, basis: u64, why: &str) {
            let job = self.taken_out(basis);
            let why = why.to_owned();
            self.done(job, LightOutcome::Failed { why });
        }

        /// The job is never answered: it is out no longer and its chunks are let go,
        /// and whoever dispatched it goes on counting it. Returns the job.
        pub(crate) fn never(&mut self, basis: u64) -> LightJob {
            self.taken_out(basis)
        }

        /// Says anything at all as a thread: a result for a job that was answered
        /// before, or for one that never was.
        pub(crate) fn say(&self, said: FromThread) {
            // Whoever dispatches may be gone; then nobody is there to mind.
            let _ = self.results.send(said);
        }

        fn taken_out(&mut self, basis: u64) -> LightJob {
            self.look();
            self.fresh.retain(|fresh| *fresh != basis);
            let job = self.out.remove(&basis);
            job.unwrap_or_else(|| panic!("no job with the basis {basis} is out"))
        }

        /// Says the outcome of `job`, which is let go first, as a thread does.
        fn done(&self, job: LightJob, outcome: LightOutcome) -> LightOutcome {
            let (basis, position) = (job.basis, job.position);
            drop(job);
            self.say(FromThread::Done {
                basis,
                position,
                outcome: outcome.clone(),
            });
            outcome
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Barrier;

    use clustine_botswarm::{ChunkLight, Named as Seen, SectionLight as Said};
    use clustine_data::blocks;
    use clustine_protocol::packets::play::ClientboundPlay;
    use clustine_world::{ChunkGenerator, LIGHT_ARRAY_LENGTH};
    use clustine_worldgen::FlatGenerator;

    use super::*;

    const POSITION: ChunkPos = ChunkPos::new(3, -5);

    /// The lowest air of the flat world, in light section 1 (the chunk's lowest).
    const GROUND: i32 = -60;

    fn flat() -> Chunk {
        FlatGenerator::classic().generate(ChunkPos::new(0, 0))
    }

    /// `centre` among eight flat chunks.
    fn among_flat(centre: Chunk) -> [Option<Arc<Chunk>>; 9] {
        let flat = Arc::new(flat());
        let mut nine = [(); 9].map(|()| Some(flat.clone()));
        nine[4] = Some(Arc::new(centre));
        nine
    }

    fn job(nine: [Option<Arc<Chunk>>; 9], old: Option<Light>, whole: bool) -> LightJob {
        LightJob {
            basis: 1,
            position: POSITION,
            nine,
            sky: true,
            old: old.map(Arc::new),
            whole,
        }
    }

    /// A chunk with light of every case: a torch on the ground, a pillar that casts a
    /// shadow, and a stone high up that makes sections there listed.
    fn varied() -> Chunk {
        let mut chunk = flat();
        chunk.set(3, GROUND, 3, blocks::TORCH);
        for y in GROUND..-20 {
            chunk.set(8, y, 8, blocks::STONE);
        }
        chunk.set(12, 100, 4, blocks::STONE);
        chunk
    }

    /// What the rules give for a job's nine chunks, asked of the crate directly.
    fn rules(job: &LightJob) -> Light {
        let chunk = |slot: usize| job.nine[slot].as_deref();
        clustine_light::light(&Around {
            centre: chunk(4).unwrap(),
            neighbours: [0, 1, 2, 3, 5, 6, 7, 8].map(chunk),
            sky: job.sky,
        })
    }

    fn lit(outcome: LightOutcome) -> (Light, Option<Bytes>) {
        match outcome {
            LightOutcome::Lit { light, update } => ((*light).clone(), update),
            LightOutcome::Failed { why } => panic!("the job failed: {why}"),
        }
    }

    fn relit(job: &LightJob) -> (Light, Option<Bytes>) {
        lit(relight(job, &mut Scratch::new()))
    }

    /// The update as a client reads it, which must be for the job's chunk.
    fn decoded(update: &Bytes) -> LightData {
        match ClientboundPlay::decode(update) {
            Ok(ClientboundPlay::LightUpdate(packet)) => {
                assert_eq!((packet.chunk_x, packet.chunk_z), (POSITION.x, POSITION.z));
                packet.light
            }
            other => panic!("not a light update: {other:?}"),
        }
    }

    /// Light as a client holds it that was sent all of `light`.
    fn held(light: &Light) -> ChunkLight {
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

    /// The sections of each case: (with levels or full, dark), as bits.
    fn cases(sections: &[SectionLight]) -> (u64, u64) {
        let bits = |wanted: fn(&SectionLight) -> bool| {
            sections
                .iter()
                .enumerate()
                .filter(|(_, section)| wanted(section))
                .fold(0u64, |mask, (index, _)| mask | 1 << index)
        };
        (
            bits(|section| matches!(section, SectionLight::Full | SectionLight::Levels(_))),
            bits(|section| *section == SectionLight::Dark),
        )
    }

    /// The sections in which two lists differ, as bits.
    fn differ(old: &[SectionLight], new: &[SectionLight]) -> u64 {
        assert_eq!(old.len(), new.len());
        (0..new.len())
            .filter(|index| old[*index] != new[*index])
            .fold(0, |mask, index| mask | 1 << index)
    }

    fn level(section: &SectionLight, x: usize, y: usize, z: usize) -> u8 {
        match section {
            SectionLight::Absent => panic!("the section is not listed"),
            SectionLight::Dark => 0,
            SectionLight::Full => 15,
            SectionLight::Levels(levels) => {
                let index = y << 8 | z << 4 | x;
                levels[index / 2] >> (index % 2 * 4) & 15
            }
        }
    }

    /// Levels that are neither dark nor full and that no flood gives.
    fn other_levels() -> SectionLight {
        let mut levels = Box::new([0x21; LIGHT_ARRAY_LENGTH]);
        levels[5] = 0xc3;
        SectionLight::Levels(levels)
    }

    /// A job the rules panic on: a neighbour that does not span the centre's heights.
    fn job_the_rules_panic_on(basis: u64) -> LightJob {
        let flat = flat();
        let short = Chunk::from_sections(flat.min_y(), flat.sections()[..12].to_vec());
        let mut nine = among_flat(flat);
        nine[7] = Some(Arc::new(short));
        LightJob {
            basis,
            ..job(nine, None, false)
        }
    }

    fn next(results: &mut UnboundedReceiver<FromThread>) -> FromThread {
        results
            .blocking_recv()
            .expect("whoever runs the jobs is still there")
    }

    #[test]
    fn the_same_job_gives_the_same_result_whatever_the_scratch_was_used_for() {
        let first = job(among_flat(varied()), None, false);
        let mut other = flat();
        other.set(0, 250, 0, blocks::GLOWSTONE);
        let other = job(among_flat(other), None, true);

        let fresh = relight(&first, &mut Scratch::new());
        assert_eq!(relight(&first, &mut Scratch::new()), fresh);

        let mut scratch = Scratch::new();
        assert_eq!(relight(&first, &mut scratch), fresh);
        assert_eq!(relight(&first, &mut scratch), fresh);
        let between = relight(&other, &mut scratch);
        assert_ne!(between, fresh);
        assert_eq!(relight(&first, &mut scratch), fresh);
        assert_eq!(relight(&other, &mut Scratch::new()), between);
    }

    /// With old light too, and after a job that panicked half way in the scratch.
    #[test]
    fn the_same_job_gives_the_same_result_after_a_job_that_panicked_in_the_scratch() {
        let first = job(among_flat(varied()), None, false);
        let (old, _) = relit(&job(among_flat(flat()), None, false));
        let first = LightJob {
            old: Some(Arc::new(old)),
            ..first
        };
        let fresh = relight(&first, &mut Scratch::new());

        let mut scratch = Scratch::new();
        let panicking = job_the_rules_panic_on(2);
        let panicked = catch_unwind(AssertUnwindSafe(|| relight(&panicking, &mut scratch)));
        assert!(panicked.is_err());
        assert_eq!(relight(&first, &mut scratch), fresh);
    }

    #[test]
    fn a_job_leaves_its_nine_chunks_as_they_were() {
        let job = job(among_flat(varied()), None, true);
        let before: Vec<Chunk> = job
            .nine
            .iter()
            .map(|chunk| (**chunk.as_ref().unwrap()).clone())
            .collect();
        relit(&job);
        for (chunk, before) in job.nine.iter().zip(&before) {
            assert_eq!(chunk.as_deref(), Some(before));
        }
    }

    #[test]
    fn the_light_is_what_the_rules_give_for_the_nine_chunks() {
        let job = job(among_flat(varied()), None, false);
        let (light, _) = relit(&job);
        assert_eq!(light, rules(&job));
        assert_eq!(light.made_by, clustine_light::VERSION);
        // The chunk has light of every case, so the comparison says something.
        for sections in [&light.sky, &light.block] {
            let (levels, dark) = cases(sections);
            assert!(levels != 0 && dark != 0, "{levels:#b} {dark:#b}");
            assert!(sections.contains(&SectionLight::Absent));
        }
        assert!(light.sky.contains(&SectionLight::Full));
    }

    /// Each of the eight places around the centre is the neighbour the record says:
    /// north-west first, row by row. A torch at the near edge of the chunk in that
    /// place lights the centre's column beside it, and not the column opposite.
    #[test]
    fn each_of_the_nine_is_the_neighbour_in_its_place() {
        for slot in [0, 1, 2, 3, 5, 6, 7, 8] {
            // Where the neighbour is: -1, 0 or 1 along x (west to east) and along z
            // (north to south).
            let (dx, dz) = (slot as i32 % 3 - 1, slot as i32 / 3 - 1);
            // The neighbour's column nearest the centre, the centre's nearest the
            // neighbour, and the centre's furthest from it.
            let columns = |d: i32| match d {
                -1 => (15, 0, 15),
                0 => (8, 8, 8),
                _ => (0, 15, 0),
            };
            let (torch_x, near_x, far_x) = columns(dx);
            let (torch_z, near_z, far_z) = columns(dz);
            let mut neighbour = flat();
            neighbour.set(torch_x, GROUND, torch_z, blocks::TORCH);
            let mut nine = among_flat(flat());
            nine[slot] = Some(Arc::new(neighbour));

            let job = job(nine, None, false);
            let (light, _) = relit(&job);
            assert_eq!(light, rules(&job), "place {slot}");
            // A torch gives 14, and a level falls by one a step through air.
            let steps = (dx.abs() + dz.abs()) as u8;
            let at = |x, z| level(&light.block[1], x, (GROUND + 64) as usize, z);
            assert_eq!(at(near_x, near_z), 14 - steps, "place {slot}");
            if dx != 0 {
                assert_eq!(at(far_x, near_z), 0, "place {slot}");
            }
            if dz != 0 {
                assert_eq!(at(near_x, far_z), 0, "place {slot}");
            }
        }
    }

    /// A neighbour that is not there is a wall: it gives no light and takes none.
    #[test]
    fn a_missing_neighbour_is_a_wall() {
        let mut bright = flat();
        bright.set(0, GROUND, 8, blocks::TORCH);
        let mut nine = among_flat(flat());
        nine[5] = Some(Arc::new(bright));
        let with = job(nine.clone(), None, false);
        nine[5] = None;
        nine[0] = None;
        let without = job(nine, None, false);

        let (light, update) = relit(&without);
        assert_eq!(light, rules(&without));
        assert_ne!(light, relit(&with).0);
        assert_eq!(light.block[1], SectionLight::Dark);
        assert!(update.is_some());

        // With no neighbour at all, too.
        let mut alone = [(); 9].map(|()| None);
        alone[4] = Some(Arc::new(varied()));
        let alone = job(alone, None, false);
        assert_eq!(relit(&alone).0, rules(&alone));
    }

    #[test]
    fn a_job_without_its_centre_fails_and_does_not_panic() {
        let mut nine = among_flat(flat());
        nine[4] = None;
        let outcome = relight(&job(nine, None, true), &mut Scratch::new());
        assert!(matches!(outcome, LightOutcome::Failed { .. }));
    }

    /// Without a sky there is no sky light: none in the light, none in the update,
    /// whichever sections the update names, and none kept from old light that had
    /// some.
    #[test]
    fn a_dimension_without_a_sky_has_no_sky_light() {
        let with_sky = job(among_flat(varied()), None, true);
        let (sky_lit, _) = relit(&with_sky);
        for (old, whole) in [
            (None, true),
            (None, false),
            (Some(sky_lit.clone()), true),
            (Some(sky_lit.clone()), false),
        ] {
            let skyless = LightJob {
                sky: false,
                ..job(among_flat(varied()), old.clone(), whole)
            };
            let (light, update) = relit(&skyless);
            assert_eq!(light, rules(&skyless));
            assert!(light.sky.iter().all(|s| *s == SectionLight::Absent));
            assert_eq!(light.block, sky_lit.block);

            let data = update.as_deref().map(Bytes::copy_from_slice);
            match (old, whole) {
                // The block light is what the old light had: nothing to name.
                (Some(_), false) => assert_eq!(data, None),
                _ => {
                    let data = decoded(&data.unwrap());
                    let seen = Seen::of(&data, light.sky.len()).unwrap();
                    assert_eq!(seen.any_sky(), 0);
                    assert!(data.sky.is_empty());
                    let (levels, dark) = cases(&light.block);
                    assert_eq!((seen.block, seen.dark_block), (levels, dark));
                }
            }
        }
    }

    /// An update that is to name every listed section does, whatever the old light
    /// was, and a client that had nothing of the chunk's light holds exactly the light
    /// after it.
    #[test]
    fn a_whole_update_names_every_listed_section_and_decodes_to_the_light() {
        let (flat_light, _) = relit(&job(among_flat(flat()), None, false));
        let fresh = job(among_flat(varied()), None, true);
        let (same, _) = relit(&fresh);
        for old in [None, Some(flat_light), Some(same)] {
            let job = job(among_flat(varied()), old, true);
            let (light, update) = relit(&job);
            assert_eq!(light, rules(&job));

            let data = decoded(&update.expect("light is listed, so it is named"));
            let seen = Seen::of(&data, light.sky.len()).unwrap();
            assert_eq!((seen.sky, seen.dark_sky), cases(&light.sky));
            assert_eq!((seen.block, seen.dark_block), cases(&light.block));
            assert_eq!(
                ChunkLight::of(&data, light.sky.len()).unwrap(),
                held(&light)
            );
            // It is the light of the chunk's packet: the same function, all named.
            assert_eq!(data, light_data(&light.sky, &light.block, Named::ALL));
        }
    }

    /// Without old light a client was sent none of it, so every listed section
    /// differs.
    #[test]
    fn without_old_light_the_update_names_every_listed_section() {
        let whole = relit(&job(among_flat(varied()), None, true));
        let differing = relit(&job(among_flat(varied()), None, false));
        assert!(whole.1.is_some());
        assert_eq!(whole, differing);
    }

    /// Block changes between the old light and the new: the update names the
    /// sections that differ and no others, each in the mask of its case, sky and
    /// block apart, and laid over the old light it gives the new.
    #[test]
    fn the_update_names_the_sections_that_differ_and_only_those() {
        let (flat_light, _) = relit(&job(among_flat(flat()), None, false));
        let mut torch = flat();
        torch.set(3, GROUND, 3, blocks::TORCH);
        let mut stone = flat();
        stone.set(5, 100, 5, blocks::STONE);
        let (torch_light, _) = relit(&job(among_flat(torch.clone()), None, false));

        // (what, the chunk now, the light before, whether sky and block light differ)
        let steps = [
            ("a torch placed", torch, flat_light.clone(), false, true),
            ("a stone in the sky", stone, flat_light.clone(), true, true),
            (
                "both and a pillar",
                varied(),
                flat_light.clone(),
                true,
                true,
            ),
            (
                "a torch taken away",
                flat(),
                torch_light.clone(),
                false,
                true,
            ),
            ("a pillar beside a torch", varied(), torch_light, true, true),
        ];
        for (what, chunk, old, sky_differs, block_differs) in steps {
            let job = job(among_flat(chunk), Some(old.clone()), false);
            let (light, update) = relit(&job);
            let data = decoded(&update.unwrap_or_else(|| panic!("{what}: no update")));
            let seen = Seen::of(&data, light.sky.len()).unwrap();

            let sky = differ(&old.sky, &light.sky);
            let block = differ(&old.block, &light.block);
            assert_eq!(sky != 0, sky_differs, "{what}");
            assert_eq!(block != 0, block_differs, "{what}");
            let (sky_levels, sky_dark) = cases(&light.sky);
            let (block_levels, block_dark) = cases(&light.block);
            assert_eq!(seen.sky, sky & sky_levels, "{what}");
            assert_eq!(seen.dark_sky, sky & sky_dark, "{what}");
            assert_eq!(seen.block, block & block_levels, "{what}");
            assert_eq!(seen.dark_block, block & block_dark, "{what}");
            // Fewer than are listed: this is not the whole update under another name.
            let listed = Unlisted::listed_in(&light);
            assert_ne!((sky, block), (listed.sky, listed.block), "{what}");

            let mut client = held(&old);
            client.lay_over(&data).unwrap();
            assert_eq!(client, held(&light), "{what}");
        }
    }

    /// For each listed section, and each case the old light could have had there in
    /// its place: the update names that section alone, in the mask of the case it has
    /// now, for the light it is of and not for the other.
    #[test]
    fn the_update_names_a_section_for_each_pair_of_cases_that_differ() {
        let nine = among_flat(varied());
        let (now, _) = relit(&job(nine.clone(), None, false));
        let mut pairs = BTreeSet::new();
        let name = |section: &SectionLight| match section {
            SectionLight::Absent => "absent",
            SectionLight::Dark => "dark",
            SectionLight::Full => "full",
            SectionLight::Levels(_) => "levels",
        };
        let mut scratch = Scratch::new();
        for of_sky in [true, false] {
            let sections = if of_sky { &now.sky } else { &now.block };
            for (index, section) in sections.iter().enumerate() {
                if *section == SectionLight::Absent {
                    continue;
                }
                let before = [
                    SectionLight::Absent,
                    SectionLight::Dark,
                    SectionLight::Full,
                    other_levels(),
                ];
                for before in before.into_iter().filter(|before| before != section) {
                    pairs.insert((of_sky, name(&before), name(section)));
                    let mut old = now.clone();
                    if of_sky {
                        old.sky[index] = before;
                    } else {
                        old.block[index] = before;
                    }
                    let job = job(nine.clone(), Some(old.clone()), false);
                    let (light, update) = lit(relight(&job, &mut scratch));
                    assert_eq!(light, now);

                    let data = decoded(&update.expect("a section differs"));
                    let seen = Seen::of(&data, light.sky.len()).unwrap();
                    let bit = 1u64 << index;
                    let dark = *section == SectionLight::Dark;
                    let expected = Seen {
                        sky: if of_sky && !dark { bit } else { 0 },
                        dark_sky: if of_sky && dark { bit } else { 0 },
                        block: if !of_sky && !dark { bit } else { 0 },
                        dark_block: if !of_sky && dark { bit } else { 0 },
                    };
                    assert_eq!(seen, expected, "section {index}, sky: {of_sky}");
                    let mut client = held(&old);
                    client.lay_over(&data).unwrap();
                    assert_eq!(client, held(&light));
                }
            }
        }
        // Every pair of cases whose second is listed was met, for both lights, but
        // that no block light is full.
        let mut expected = BTreeSet::new();
        for of_sky in [true, false] {
            for before in ["absent", "dark", "full", "levels"] {
                for after in ["dark", "full", "levels"] {
                    if before != after && (of_sky || after != "full") {
                        expected.insert((of_sky, before, after));
                    }
                }
            }
            expected.insert((of_sky, "levels", "levels"));
        }
        assert_eq!(pairs, expected);
    }

    #[test]
    fn there_is_no_update_when_nothing_differs() {
        for chunk in [flat(), varied()] {
            let nine = among_flat(chunk);
            let (light, first) = relit(&job(nine.clone(), None, false));
            assert!(first.is_some());
            let again = relight(&job(nine, Some(light.clone()), false), &mut Scratch::new());
            let expected = LightOutcome::Lit {
                light: Arc::new(light),
                update: None,
            };
            assert_eq!(again, expected);
        }
    }

    /// Nothing is listed where no block is anywhere near: nothing to name, even
    /// whole.
    #[test]
    fn there_is_no_update_when_no_section_is_listed() {
        let flat = flat();
        let dimension_of_air = Chunk::from_sections(flat.min_y(), flat.sections()[1..].to_vec());
        assert!(
            dimension_of_air
                .sections()
                .iter()
                .all(|section| section.non_air_count() == 0)
        );
        let air = Arc::new(dimension_of_air);
        let nine = [(); 9].map(|()| Some(air.clone()));
        for whole in [true, false] {
            let (light, update) = relit(&job(nine.clone(), None, whole));
            assert_eq!(Unlisted::listed_in(&light), Unlisted::NONE);
            assert_eq!(update, None);
        }
    }

    /// A section the old light listed and the rules now leave out stays listed, with
    /// the levels it has today: under a roof far above, the sky's light as it comes
    /// in from the sides, and dark for blocks.
    #[test]
    fn a_section_that_was_listed_stays_listed_with_todays_levels() {
        // A roof over the whole chunk in its section 22, seven sections above the
        // light section 15 that the test is about.
        let mut roofed = flat();
        for z in 0..16 {
            for x in 0..16 {
                roofed.set(x, 300, z, blocks::STONE);
            }
        }
        let nine = among_flat(roofed);
        const KEPT: usize = 15;
        let fresh = job(nine.clone(), None, false);
        let ruled = rules(&fresh);
        assert_eq!(ruled.sky[KEPT], SectionLight::Absent);
        assert_eq!(ruled.block[KEPT], SectionLight::Absent);
        assert_eq!(relit(&fresh).0, ruled);

        // The client was once sent the section, as open sky and with a lamp's light.
        let (mut old, _) = relit(&job(among_flat(flat()), None, false));
        old.sky[KEPT] = SectionLight::Full;
        old.block[KEPT] = other_levels();

        for whole in [false, true] {
            let job = job(nine.clone(), Some(old.clone()), whole);
            let (light, update) = relit(&job);
            for index in (0..light.sky.len()).filter(|index| *index != KEPT) {
                assert_eq!(light.sky[index], ruled.sky[index], "section {index}");
                assert_eq!(light.block[index], ruled.block[index], "section {index}");
            }
            // The open sky is one column to the west of x = 0, and eight columns
            // from the middle on the nearest side.
            let sky = &light.sky[KEPT];
            assert!(matches!(sky, SectionLight::Levels(_)), "{sky:?}");
            for y in [0, 9, 15] {
                assert_eq!(level(sky, 0, y, 8), 14);
                assert_eq!(level(sky, 15, y, 8), 14);
                assert_eq!(level(sky, 7, y, 8), 7);
            }
            assert_eq!(light.block[KEPT], SectionLight::Dark);

            // The section is named like any other, and the client ends with it.
            let data = decoded(&update.unwrap());
            let seen = Seen::of(&data, light.sky.len()).unwrap();
            assert_ne!(seen.sky & 1 << KEPT, 0);
            assert_ne!(seen.dark_block & 1 << KEPT, 0);
            let mut client = held(&old);
            client.lay_over(&data).unwrap();
            assert_eq!(client, held(&light));

            // From then on it is the entry's light: listed again by the next job,
            // with nothing to say while nothing changes.
            let next = LightJob {
                old: Some(Arc::new(light.clone())),
                whole: false,
                ..job
            };
            assert_eq!(relit(&next), (light, None));
        }
    }

    /// One light kept listed and not the other, where the old light listed one.
    #[test]
    fn sky_and_block_light_are_kept_listed_apart() {
        let nine = among_flat(flat());
        let (ruled, _) = relit(&job(nine.clone(), None, false));
        const KEPT: usize = 20;
        assert_eq!(ruled.sky[KEPT], SectionLight::Absent);

        let mut old = ruled.clone();
        old.block[KEPT] = SectionLight::Dark;
        let (light, update) = relit(&job(nine.clone(), Some(old), false));
        assert_eq!(light.sky, ruled.sky);
        assert_eq!(light.block[KEPT], SectionLight::Dark);
        // Dark it was and dark it is.
        assert_eq!(update, None);

        let mut old = ruled.clone();
        old.sky[KEPT] = other_levels();
        let (light, update) = relit(&job(nine, Some(old), false));
        assert_eq!(light.block, ruled.block);
        assert_eq!(light.sky[KEPT], SectionLight::Full);
        let data = decoded(&update.unwrap());
        let seen = Seen::of(&data, light.sky.len()).unwrap();
        let only_the_sky = Seen {
            sky: 1 << KEPT,
            ..Seen::default()
        };
        assert_eq!(seen, only_the_sky);
    }

    #[test]
    fn a_thread_says_started_and_then_done_and_names_the_job() {
        let (mut lighting, mut results) = Lighting::threads(1);
        let job = LightJob {
            basis: 41,
            ..job(among_flat(varied()), None, true)
        };
        let expected = relight(&job, &mut Scratch::new());
        lighting.send(job).unwrap();
        let started = FromThread::Started {
            basis: 41,
            thread: 0,
        };
        assert_eq!(next(&mut results), started);
        let done = FromThread::Done {
            basis: 41,
            position: POSITION,
            outcome: expected,
        };
        assert_eq!(next(&mut results), done);
    }

    /// The rules' panic is the job's failure: the thread says so, lives, and gives
    /// the next job the right light, also for a job that waited behind the failing
    /// one in the queue.
    #[test]
    fn a_job_whose_rules_panic_fails_and_the_thread_goes_on_to_light_the_next() {
        let (mut lighting, mut results) = Lighting::threads(1);
        let good = |basis| LightJob {
            basis,
            ..job(among_flat(varied()), None, false)
        };
        let expected = relight(&good(0), &mut Scratch::new());

        lighting.send(good(1)).unwrap();
        lighting.send(job_the_rules_panic_on(2)).unwrap();
        lighting.send(good(3)).unwrap();
        lighting.send(job_the_rules_panic_on(4)).unwrap();
        lighting.send(job_the_rules_panic_on(5)).unwrap();
        lighting.send(good(6)).unwrap();

        for basis in 1..=6 {
            let started = FromThread::Started { basis, thread: 0 };
            assert_eq!(next(&mut results), started);
            match next(&mut results) {
                FromThread::Done {
                    basis: done,
                    position,
                    outcome,
                } => {
                    assert_eq!((done, position), (basis, POSITION));
                    if [2, 4, 5].contains(&basis) {
                        let LightOutcome::Failed { why } = outcome else {
                            panic!("job {basis} did not fail: {outcome:?}");
                        };
                        assert!(why.contains("span the same heights"), "{why}");
                    } else {
                        assert_eq!(outcome, expected, "job {basis}");
                    }
                }
                other => panic!("job {basis}: {other:?}"),
            }
        }
    }

    /// A worker has no scratch until its first job, keeps it from job to job, and
    /// throws it away after a job that panicked, which may have left it half written.
    #[test]
    fn the_scratch_is_made_at_the_first_job_and_anew_after_a_panic() {
        let (results, mut from_worker) = unbounded_channel();
        let mut worker = Worker {
            thread: 7,
            scratch: None,
            results,
        };
        let good = job(among_flat(flat()), None, false);
        assert!(worker.scratch.is_none());
        assert!(worker.run(good.clone(), &relight));
        assert!(worker.scratch.is_some());
        assert!(worker.run(job_the_rules_panic_on(2), &relight));
        assert!(worker.scratch.is_none());
        assert!(worker.run(good.clone(), &relight));
        assert!(worker.scratch.is_some());

        let mut said = Vec::new();
        while let Ok(message) = from_worker.try_recv() {
            said.push(message);
        }
        assert_eq!(said.len(), 6);
        assert_eq!(said[1], said[5]);
        assert!(matches!(
            &said[3],
            FromThread::Done {
                basis: 2,
                outcome: LightOutcome::Failed { .. },
                ..
            }
        ));
        let started = FromThread::Started {
            basis: 2,
            thread: 7,
        };
        assert_eq!(said[2], started);

        // Nobody listens: the worker says so, and whoever loops over it stops.
        drop(from_worker);
        assert!(!worker.run(good, &relight));
    }

    /// Jobs for different chunks handed to several threads all come back, each once
    /// and each with its own light, and each is started once, before it is done.
    #[test]
    fn jobs_handed_to_several_threads_all_come_back_each_once() {
        let (mut lighting, mut results) = Lighting::threads(3);
        let jobs: Vec<LightJob> = (0..12)
            .map(|index| {
                let mut chunk = flat();
                chunk.set(index, GROUND + index as i32, 15 - index, blocks::GLOWSTONE);
                LightJob {
                    basis: 100 + index as u64,
                    position: ChunkPos::new(index as i32, -1),
                    ..job(among_flat(chunk), None, index % 2 == 0)
                }
            })
            .collect();
        for job in &jobs {
            lighting.send(job.clone()).unwrap();
        }

        let mut started = BTreeSet::new();
        let mut done = BTreeSet::new();
        while done.len() < jobs.len() {
            match next(&mut results) {
                FromThread::Started { basis, thread } => {
                    assert!(thread < 3);
                    assert!(started.insert(basis), "job {basis} was started twice");
                }
                FromThread::Done {
                    basis,
                    position,
                    outcome,
                } => {
                    assert!(started.contains(&basis), "job {basis} was not started");
                    assert!(done.insert(basis), "job {basis} came back twice");
                    let job = &jobs[(basis - 100) as usize];
                    assert_eq!(position, job.position);
                    assert_eq!(outcome, relight(job, &mut Scratch::new()));
                }
            }
        }
        assert_eq!(started, done);
        // Nothing more is said, and the threads end when the `Lighting` is dropped:
        // the channel closes only when the last of them has let go of it.
        drop(lighting);
        assert_eq!(results.blocking_recv(), None);
    }

    /// The threads work at the same time: each of four jobs waits, in what the
    /// threads were handed to run, until four threads have one.
    #[test]
    fn each_thread_takes_a_job_while_the_others_have_theirs() {
        let together = Arc::new(Barrier::new(4));
        let work = {
            let together = together.clone();
            move |job: &LightJob, scratch: &mut Scratch| {
                together.wait();
                relight(job, scratch)
            }
        };
        let (mut lighting, mut results) = Lighting::threads_running(4, Arc::new(work));
        assert_eq!(lighting.in_flight(), 8);
        let expected = relight(&job(among_flat(flat()), None, false), &mut Scratch::new());
        for basis in 0..8 {
            let job = LightJob {
                basis,
                ..job(among_flat(flat()), None, false)
            };
            lighting.send(job).unwrap();
        }
        let mut threads = BTreeSet::new();
        let mut done = BTreeSet::new();
        while done.len() < 8 {
            match next(&mut results) {
                FromThread::Started { thread, .. } => {
                    threads.insert(thread);
                }
                FromThread::Done { basis, outcome, .. } => {
                    assert!(done.insert(basis));
                    assert_eq!(outcome, expected);
                }
            }
        }
        assert_eq!(threads, BTreeSet::from([0, 1, 2, 3]));
    }

    /// The end that runs a job where it is called says what a thread says, in the
    /// same order, and has said it when `send` returns.
    #[test]
    fn a_job_run_where_it_is_called_gives_what_a_thread_gives() {
        let (mut here, mut from_here) = Lighting::here();
        let (mut threads, mut from_threads) = Lighting::threads(1);
        let (flat_light, _) = relit(&job(among_flat(flat()), None, false));
        let jobs = [
            job(among_flat(varied()), None, false),
            job_the_rules_panic_on(8),
            job(among_flat(varied()), Some(flat_light), false),
            job(among_flat(flat()), None, true),
        ];
        for job in jobs {
            // The job the rules panic on has no result to compare with.
            let direct = (job.basis != 8).then(|| relight(&job, &mut Scratch::new()));
            let (basis, position) = (job.basis, job.position);
            threads.send(job.clone()).unwrap();
            let by_thread = [next(&mut from_threads), next(&mut from_threads)];

            here.send(job).unwrap();
            let by_turn = [
                from_here.try_recv().expect("started within the call"),
                from_here.try_recv().expect("done within the call"),
            ];
            assert!(from_here.try_recv().is_err());
            assert_eq!(by_turn, by_thread);
            assert_eq!(by_turn[0], FromThread::Started { basis, thread: 0 });
            let outcome = match &by_turn[1] {
                FromThread::Done {
                    basis: done,
                    position: at,
                    outcome,
                } => {
                    assert_eq!((*done, *at), (basis, position));
                    outcome
                }
                other => panic!("{other:?}"),
            };
            match direct {
                Some(direct) => assert_eq!(*outcome, direct),
                None => assert!(matches!(outcome, LightOutcome::Failed { .. })),
            }
        }
        assert_eq!(here.in_flight(), 2);
    }

    /// A job handed to threads that are not there is given back and not queued. One
    /// job may still be out, so that whoever dispatches finds that out for each.
    #[test]
    fn a_job_is_given_back_when_no_thread_is_left() {
        let (mut lighting, mut results) = Lighting::threads(0);
        assert_eq!(lighting.in_flight(), 1);
        let job = LightJob {
            basis: 77,
            ..job(among_flat(flat()), None, false)
        };
        let back = lighting.send(job).expect_err("nobody takes the job");
        assert_eq!(back.basis, 77);
        assert!(results.try_recv().is_err());
    }

    /// Work that never ends for the job with the basis 1 until `release` is dropped,
    /// and is the rules' for every other.
    fn hangs_on_the_first(release: mpsc::Receiver<()>) -> Arc<Work> {
        let release = Mutex::new(release);
        Arc::new(move |job: &LightJob, scratch: &mut Scratch| {
            if job.basis == 1 {
                let _ = release.lock().unwrap().recv();
            }
            relight(job, scratch)
        })
    }

    fn numbered(basis: u64) -> LightJob {
        LightJob {
            basis,
            ..job(among_flat(flat()), None, false)
        }
    }

    #[test]
    fn a_thread_written_off_is_replaced_and_ends_when_it_comes_back() {
        let (release, held) = mpsc::channel();
        let (mut lighting, mut results) = Lighting::threads_running(1, hangs_on_the_first(held));
        lighting.send(numbered(1)).unwrap();
        let started = FromThread::Started {
            basis: 1,
            thread: 0,
        };
        assert_eq!(next(&mut results), started);
        // The one thread hangs. Written off, another takes the next job.
        assert_eq!(lighting.write_off(0), 1);
        assert_eq!(lighting.in_flight(), 2);
        lighting.send(numbered(2)).unwrap();
        let started = FromThread::Started {
            basis: 2,
            thread: 1,
        };
        assert_eq!(next(&mut results), started);
        assert!(matches!(
            next(&mut results),
            FromThread::Done { basis: 2, .. }
        ));
        // The first comes back after all: it says so, and takes no job after.
        drop(release);
        assert!(matches!(
            next(&mut results),
            FromThread::Done { basis: 1, .. }
        ));
        for basis in 3..20 {
            lighting.send(numbered(basis)).unwrap();
            let started = FromThread::Started { basis, thread: 1 };
            assert_eq!(next(&mut results), started);
            assert!(matches!(next(&mut results), FromThread::Done { .. }));
        }
        // Writing off a thread that is not counted on changes nothing.
        assert_eq!(lighting.write_off(0), 1);
    }

    #[test]
    fn after_four_replacements_a_thread_written_off_leaves_one_fewer_and_then_none() {
        let (mut lighting, _results) = Lighting::threads(2);
        // Threads 0 and 1, and four more as they are written off: 2, 3, 4, 5.
        for thread in 0..4 {
            assert_eq!(lighting.write_off(thread), 2);
        }
        assert_eq!(lighting.write_off(4), 1);
        assert_eq!(lighting.in_flight(), 2);
        assert_eq!(lighting.write_off(5), 0);
        assert_eq!(lighting.in_flight(), 1);
        let back = lighting
            .send(numbered(9))
            .expect_err("nobody takes the job");
        assert_eq!(back.basis, 9);
    }

    #[test]
    fn the_threads_end_when_the_lighting_is_dropped() {
        let (lighting, mut results) = Lighting::threads(3);
        drop(lighting);
        // Each thread holds a sender of results for as long as it runs.
        assert_eq!(results.blocking_recv(), None);
    }

    #[test]
    fn a_quarter_of_the_processors_light_and_at_least_two() {
        assert_eq!([1, 2, 4, 7, 8].map(threads_for), [2; 5]);
        assert_eq!([12, 16, 64, 255].map(threads_for), [3, 4, 16, 63]);
        let processors = thread::available_parallelism().map_or(1, usize::from);
        assert_eq!(default_threads(), threads_for(processors));
        assert!(default_threads() >= 2);
    }
}
