//! The scheduler itself: its pool of threads, how a request is asked and answered,
//! and what each task does with what it needs.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak, mpsc};
use std::thread::JoinHandle;

use clustine_light::{Around, Scratch, Unlisted};
use clustine_world::generation::{Done, Failed, GeneratorSettings, Wanted};
use clustine_world::{Chunk, ChunkGenerator, ChunkPos};

use super::graph::{Answer, Graph};
use super::lattice::{Key, around, rank};
use super::output::{MadeTerrain, Output, finish};
use super::view::{self, Context, Fetch};
use super::{Figures, Options};
use crate::stages::{Stages, Starts, StartsNear};

/// Makes chunks from `S` on a pool of its own. Dropping it ends its threads and waits
/// for them.
pub struct Scheduler<S: Stages> {
    shared: Arc<Shared<S>>,
    threads: Vec<JoinHandle<()>>,
}

/// What the scheduler and its threads share.
struct Shared<S> {
    stages: S,
    context: Context,
    graph: Mutex<Graph>,
    /// Told when a task may have become ready, and when the scheduler stops.
    work: Condvar,
    /// Told when no task may be queued or running any more.
    idle: Condvar,
}

impl<S: Stages> Scheduler<S> {
    pub fn new(stages: S, options: Options) -> Self {
        let GeneratorSettings { identity, version } = stages.settings();
        let context = Context {
            identity,
            version,
            dimension: stages.dimension().name,
        };
        let graph = Graph::new(
            options.order,
            options.pick,
            options.cache_bytes,
            options.look_ahead,
        );
        let shared = Arc::new(Shared {
            stages,
            context,
            graph: Mutex::new(graph),
            work: Condvar::new(),
            idle: Condvar::new(),
        });
        let threads = (0..options.threads.max(1))
            .map(|index| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("clustine-generation-{index}"))
                    .spawn(move || work(&shared))
                    .expect("a thread for generation can be started")
            })
            .collect();
        Self { shared, threads }
    }

    /// The finished chunk at `position` (definition D, before light), waited for.
    /// For tests and measurements: the store asks through [`ChunkGenerator`].
    pub fn finished(&self, position: ChunkPos) -> Result<Chunk, Failed> {
        self.wait(Key::Finished(position))
    }

    pub fn figures(&self) -> Figures {
        self.shared.lock().figures()
    }

    /// Waits until no task is queued or running.
    pub fn wait_idle(&self) {
        let mut graph = self.shared.lock();
        while graph.busy() && !graph.stopping {
            graph = self
                .shared
                .idle
                .wait(graph)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub fn stages(&self) -> &S {
        &self.shared.stages
    }

    /// Asks for `root`, answered through `done`.
    fn ask(&self, root: Key, done: Done) -> Wanted {
        let mut answers = Vec::new();
        let asked = {
            let mut graph = self.shared.lock();
            let asked = graph.ask(root, done, &mut answers);
            if graph.has_ready() {
                self.shared.work.notify_all();
            }
            asked
        };
        give(answers);
        let Some(id) = asked else {
            return Wanted::nothing();
        };
        let shared: Weak<Shared<S>> = Arc::downgrade(&self.shared);
        Wanted::new(move || {
            if let Some(shared) = shared.upgrade() {
                let mut graph = shared.lock();
                graph.cancel(id);
                if !graph.busy() {
                    shared.idle.notify_all();
                }
            }
        })
    }

    fn wait(&self, root: Key) -> Result<Chunk, Failed> {
        let (sender, receiver) = mpsc::channel();
        let _wanted = self.ask(
            root,
            Box::new(move |made| {
                // The receiver is gone only if the waiter was unwound; nobody asks then.
                let _ = sender.send(made);
            }),
        );
        // Without an answer only when the scheduler stops under the request.
        receiver.recv().unwrap_or(Err(Failed))
    }

    #[cfg(test)]
    pub(crate) fn graph(&self) -> MutexGuard<'_, Graph> {
        self.shared.lock()
    }
}

impl<S: Stages> Drop for Scheduler<S> {
    fn drop(&mut self) {
        self.shared.lock().stopping = true;
        self.shared.work.notify_all();
        self.shared.idle.notify_all();
        for thread in self.threads.drain(..) {
            // A thread of the pool panics only outside a task, by a defect that was
            // reported where it happened; the others are joined all the same.
            let _ = thread.join();
        }
    }
}

impl<S: Stages> ChunkGenerator for Scheduler<S> {
    /// The lit chunk, waited for; panics if generation failed.
    fn generate(&self, position: ChunkPos) -> Chunk {
        match self.wait(Key::Lit(position)) {
            Ok(chunk) => chunk,
            Err(Failed) => panic!(
                "generation failed for the chunk at ({}, {})",
                position.x, position.z
            ),
        }
    }

    fn settings(&self) -> clustine_world::generation::GeneratorSettings {
        clustine_world::generation::GeneratorSettings {
            identity: self.shared.context.identity.clone(),
            version: self.shared.context.version,
        }
    }

    fn origin(&self) -> ChunkPos {
        self.shared.stages.origin()
    }

    /// Lit chunks, made after whatever a request wants and kept in the cache.
    fn prepare(&self, positions: &[ChunkPos]) {
        let mut graph = self.shared.lock();
        for position in positions {
            graph.prepare(*position);
        }
        if graph.has_ready() {
            self.shared.work.notify_all();
        }
    }

    fn request(&self, position: ChunkPos, done: Done) -> Wanted {
        self.ask(Key::Lit(position), done)
    }
}

/// Calls the answers, each by itself: one that panics does not keep the others.
fn give(answers: Vec<Answer>) {
    for (done, made) in answers {
        if catch_unwind(AssertUnwindSafe(move || done(made))).is_err() {
            tracing::error!("an answer to a request for a generated chunk panicked");
        }
    }
}

/// A thread of the pool: takes ready tasks until the scheduler stops.
fn work<S: Stages>(shared: &Shared<S>) {
    let mut scratch = Scratch::default();
    let mut graph = shared.lock();
    loop {
        if graph.stopping {
            return;
        }
        let Some((key, inputs)) = graph.take() else {
            graph = shared
                .work
                .wait(graph)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        drop(graph);
        let made = catch_unwind(AssertUnwindSafe(|| {
            shared.perform(key, &inputs, &mut scratch)
        }));
        drop(inputs);
        let mut answers = Vec::new();
        graph = shared.lock();
        match made {
            Ok(output) => graph.complete(key, Arc::new(output), &mut answers),
            Err(payload) => {
                shared.log_failure(key, &view::message(&*payload));
                graph.fail(key, &mut answers);
            }
        }
        if graph.has_ready() {
            shared.work.notify_all();
        }
        if !graph.busy() {
            shared.idle.notify_all();
        }
        if !answers.is_empty() {
            drop(graph);
            give(answers);
            graph = shared.lock();
        }
    }
}

/// The starts handed to a chunk's terrain.
struct Near<'a>(&'a BTreeMap<ChunkPos, Arc<Output>>);

impl StartsNear for Near<'_> {
    fn starts(&self, source: ChunkPos) -> &Starts {
        match self.0.get(&source) {
            Some(starts) => starts.starts(),
            None => panic!(
                "terrain asked for the starts of ({}, {}), which is not within 8",
                source.x, source.z
            ),
        }
    }
}

impl<S: Stages> Shared<S> {
    fn lock(&self) -> MutexGuard<'_, Graph> {
        // Nothing panics while it holds the lock but a broken invariant of the graph,
        // and the threads may as well go on with what is there then.
        self.graph.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn log_failure(&self, key: Key, message: &str) {
        let position = key.position();
        tracing::error!(
            identity = %self.context.identity,
            version = self.context.version,
            dimension = self.context.dimension,
            chunk = ?position,
            placing = key.kind(),
            message,
            "a task of generation failed, and every chunk that needs it with it"
        );
    }

    /// A task, from what it needs: each its output, in the order of [`Key::needs`].
    fn perform(&self, key: Key, inputs: &[(Key, Arc<Output>)], scratch: &mut Scratch) -> Output {
        match key {
            Key::Starts(source) => Output::Starts(self.stages.starts(source)),
            Key::Terrain(position) => {
                let starts = inputs
                    .iter()
                    .map(|(need, output)| (need.position(), Arc::clone(output)))
                    .collect();
                Output::Terrain(self.terrain(position, starts))
            }
            Key::Run(centre) => {
                let mut terrain = BTreeMap::new();
                let mut seen = Vec::new();
                for (need, output) in inputs {
                    match need {
                        Key::Terrain(at) => {
                            terrain.insert(*at, Arc::clone(output));
                        }
                        // In ascending rank, as `Key::needs` lists them.
                        Key::Run(d) => seen.push((*d, Arc::clone(output))),
                        other => panic!("a run does not need {other:?}"),
                    }
                }
                Output::Run(view::run(
                    &self.stages,
                    centre,
                    &terrain,
                    &seen,
                    self,
                    &self.context,
                ))
            }
            Key::Finished(e) => {
                let (_, terrain) = &inputs[0];
                let mut runs: Vec<(ChunkPos, Arc<Output>)> = inputs[1..]
                    .iter()
                    .map(|(need, output)| (need.position(), Arc::clone(output)))
                    .collect();
                runs.sort_by_key(|(d, _)| rank(*d));
                Output::Finished(finish(e, &terrain.terrain().terrain, &runs))
            }
            Key::Lit(_) => {
                // In the order of `around`, which is `Around`'s with the centre fifth.
                let finished: Vec<_> = inputs.iter().map(|(_, output)| output.finished()).collect();
                let centre = finished[4];
                let neighbours = [0, 1, 2, 3, 5, 6, 7, 8].map(|index| Some(&finished[index].chunk));
                let around = Around {
                    centre: &centre.chunk,
                    neighbours,
                    sky: has_sky(self.context.dimension),
                };
                let (mut light, _) = clustine_light::light_with(&around, Unlisted::NONE, scratch);
                // The nine were made by this generator, which an edge goes by when it
                // asks whether the light still holds beside the chunks it has.
                light.beside = self.context.version;
                let mut chunk = centre.chunk.clone();
                chunk.set_light(light);
                // A chunk is marked if any run it depends on failed (section 5): the
                // runs of the nine it is lit from, and the runs those saw.
                let with_failure = finished.iter().any(|finished| finished.tainted);
                chunk.mark_generated(self.context.version, with_failure);
                Output::Lit(chunk)
            }
        }
    }

    fn terrain(&self, position: ChunkPos, starts: BTreeMap<ChunkPos, Arc<Output>>) -> MadeTerrain {
        let terrain = self.stages.terrain(position, &Near(&starts));
        let referenced = terrain
            .references
            .iter()
            .map(|reference| {
                let source = reference.source;
                match starts.get(&source) {
                    Some(made) => (source, Arc::clone(made)),
                    None => panic!(
                        "terrain references a start of ({}, {}), which is not within 8",
                        source.x, source.z
                    ),
                }
            })
            .collect();
        MadeTerrain {
            terrain,
            starts: referenced,
        }
    }
}

impl<S: Stages> Fetch for Shared<S> {
    fn terrain(&self, at: ChunkPos) -> Arc<Output> {
        let cached = self.lock().cached(Key::Terrain(at));
        if let Some(made) = cached {
            return made;
        }
        let starts = around(at, 8)
            .map(|source| (source, Fetch::starts(self, source)))
            .collect();
        Arc::new(Output::Terrain(Shared::terrain(self, at, starts)))
    }

    fn starts(&self, source: ChunkPos) -> Arc<Output> {
        let cached = self.lock().cached(Key::Starts(source));
        if let Some(made) = cached {
            return made;
        }
        Arc::new(Output::Starts(self.stages.starts(source)))
    }
}

/// Whether a dimension has a sky, as the game's dimension types say; the data here
/// does not carry it yet.
fn has_sky(dimension: &str) -> bool {
    !matches!(dimension, "minecraft:the_nether" | "minecraft:the_end")
}
