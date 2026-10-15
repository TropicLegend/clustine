//! The scheduler's book of tasks (ADR-0021, section 5): which are wanted and by whom,
//! which wait for which, which are ready, and the cache of what is done.
//!
//! A task is held by the requests whose chunk it is, and by every task not yet started
//! that needs it. A task that nothing holds and that has not started is forgotten; one
//! that is done and that nothing holds may be dropped from the cache, least recently
//! used first. So what a request under way still needs is never dropped, and a
//! request that is dropped leaves no task behind that has not started.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use clustine_world::generation::{Done, Failed};
use clustine_world::{Chunk, ChunkPos};

use super::lattice::Key;
use super::output::Output;
use super::{Figures, Order, Pick, Recorded};

/// Which request a task serves first: what a request wants before what only the
/// look-ahead wants, and the oldest of either first.
pub(crate) type Priority = (bool, u64);

/// How many positions of lit chunks that were handed out are remembered, so that the
/// look-ahead does not light them again for a store that has them.
const HANDED_REMEMBERED: usize = 1 << 16;

#[derive(Debug)]
enum State {
    /// Waiting for this many of its needs.
    Waiting(usize),
    Ready,
    Running,
    Done {
        output: Arc<Output>,
        bytes: usize,
        stamp: u64,
    },
}

#[derive(Debug)]
struct Node {
    state: State,
    /// Requests whose chunk this is, and tasks not yet started that need it.
    holds: usize,
    priority: Priority,
    /// What it needs, until it starts.
    needs: Vec<Key>,
    /// Tasks not yet started that wait for this one to be done.
    waiters: Vec<Key>,
}

/// Someone who wants a task done: a request, which is answered, or the look-ahead,
/// which is not.
struct Claim {
    root: Key,
    done: Option<Done>,
    /// The look-ahead the request started, given up with it if it is dropped before
    /// it is answered.
    ahead: Vec<u64>,
}

/// An answer to be given once the lock is let go.
pub(crate) type Answer = (Done, Result<Chunk, Failed>);

/// A task taken to be run, with the output of each thing it needs, in the order of
/// [`Key::needs`].
pub(crate) type Taken = (Key, Vec<(Key, Arc<Output>)>);

pub(crate) struct Graph {
    order: Order,
    pick: Pick,
    bound: Option<usize>,
    look_ahead: bool,
    nodes: BTreeMap<Key, Node>,
    /// Tasks that failed, or that need one that did, for the scheduler's life.
    failed: BTreeSet<Key>,
    ready: BTreeSet<(Priority, Key)>,
    /// Done tasks that nothing holds, by when they were last used.
    unheld: BTreeMap<u64, Key>,
    claims: BTreeMap<u64, Claim>,
    roots: BTreeMap<Key, BTreeSet<u64>>,
    handed: BTreeSet<ChunkPos>,
    handed_order: VecDeque<ChunkPos>,
    next_id: u64,
    next_stamp: u64,
    bytes: usize,
    pending: usize,
    running: usize,
    random: u64,
    over_bound_said: bool,
    runs_done: BTreeSet<ChunkPos>,
    pub(crate) figures: Figures,
    pub(crate) stopping: bool,
}

impl Graph {
    pub(crate) fn new(order: Order, pick: Pick, bound: Option<usize>, look_ahead: bool) -> Self {
        let random = match pick {
            Pick::Oldest => 0,
            Pick::Shuffled(seed) => seed,
        };
        Self {
            order,
            pick,
            bound,
            look_ahead,
            nodes: BTreeMap::new(),
            failed: BTreeSet::new(),
            ready: BTreeSet::new(),
            unheld: BTreeMap::new(),
            claims: BTreeMap::new(),
            roots: BTreeMap::new(),
            handed: BTreeSet::new(),
            handed_order: VecDeque::new(),
            next_id: 0,
            next_stamp: 0,
            bytes: 0,
            pending: 0,
            running: 0,
            random,
            over_bound_said: false,
            runs_done: BTreeSet::new(),
            figures: Figures::default(),
            stopping: false,
        }
    }

    /// Whether a task is queued or running.
    pub(crate) fn busy(&self) -> bool {
        self.pending > 0 || self.running > 0
    }

    pub(crate) fn figures(&self) -> Figures {
        Figures {
            queued: self.pending as u64,
            running: self.running as u64,
            cache_bytes: self.bytes as u64,
            ..self.figures
        }
    }

    pub(crate) fn has_ready(&self) -> bool {
        !self.ready.is_empty()
    }

    fn stamp(&mut self) -> u64 {
        self.next_stamp += 1;
        self.next_stamp
    }

    /// The output of a task that is done, if the cache has it.
    pub(crate) fn cached(&self, key: Key) -> Option<Arc<Output>> {
        match &self.nodes.get(&key)?.state {
            State::Done { output, .. } => Some(Arc::clone(output)),
            _ => None,
        }
    }

    /// Asks for `root`, answered through `done`. Answers given at once (from the cache,
    /// or a failure remembered) are put into `answers`; otherwise the request's number
    /// is returned, to be given up with [`Graph::cancel`].
    pub(crate) fn ask(&mut self, root: Key, done: Done, answers: &mut Vec<Answer>) -> Option<u64> {
        self.next_id += 1;
        let id = self.next_id;
        let asked = if let Some(output) = self.cached(root) {
            answers.push((done, Ok(output.answer())));
            self.handed_out(root);
            None
        } else if self.hold(root, (false, id)) {
            let claim = Claim {
                root,
                done: Some(done),
                ahead: Vec::new(),
            };
            self.claims.insert(id, claim);
            self.roots.entry(root).or_default().insert(id);
            Some(id)
        } else {
            answers.push((done, Err(Failed)));
            None
        };
        if let (true, Key::Lit(position)) = (self.look_ahead, root) {
            for neighbour in super::lattice::around(position, 1).filter(|n| *n != position) {
                self.ahead(neighbour, asked);
            }
        }
        asked
    }

    /// Wants `Lit(position)` ahead for nobody in particular, as the look-ahead wants
    /// a neighbour: after everything a request wants, and kept in the cache when done.
    pub(crate) fn prepare(&mut self, position: ChunkPos) {
        self.ahead(position, None);
    }

    /// Wants `Lit(position)` ahead, after everything a request wants.
    fn ahead(&mut self, position: ChunkPos, request: Option<u64>) {
        let key = Key::Lit(position);
        if self.handed.contains(&position) || self.failed.contains(&key) {
            return;
        }
        if self.cached(key).is_some() {
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        if !self.hold(key, (true, id)) {
            return;
        }
        let claim = Claim {
            root: key,
            done: None,
            ahead: Vec::new(),
        };
        self.claims.insert(id, claim);
        self.roots.entry(key).or_default().insert(id);
        if let Some(claim) = request.and_then(|request| self.claims.get_mut(&request)) {
            claim.ahead.push(id);
        }
    }

    /// Gives up a request that has not been answered, and the look-ahead it started.
    pub(crate) fn cancel(&mut self, id: u64) {
        let Some(claim) = self.claims.remove(&id) else {
            return;
        };
        self.unroot(claim.root, id);
        for ahead in claim.ahead {
            if let Some(claim) = self.claims.remove(&ahead) {
                self.unroot(claim.root, ahead);
            }
        }
        self.evict();
    }

    fn unroot(&mut self, root: Key, id: u64) {
        if let Some(ids) = self.roots.get_mut(&root) {
            ids.remove(&id);
            if ids.is_empty() {
                self.roots.remove(&root);
            }
        }
        self.release(root);
    }

    /// A lit chunk that was handed out is not kept: the store has it.
    fn handed_out(&mut self, key: Key) {
        let Key::Lit(position) = key else {
            return;
        };
        if let Some(node) = self.nodes.get(&key)
            && node.holds == 0
            && let State::Done { bytes, stamp, .. } = node.state
        {
            self.unheld.remove(&stamp);
            self.bytes -= bytes;
            self.nodes.remove(&key);
        }
        if self.handed.insert(position) {
            self.handed_order.push_back(position);
            if self.handed_order.len() > HANDED_REMEMBERED
                && let Some(oldest) = self.handed_order.pop_front()
            {
                self.handed.remove(&oldest);
            }
        }
    }

    /// Holds `key` for someone of `priority`, making it and what it needs wanted if it
    /// is not. False if it failed, or needs something that failed.
    fn hold(&mut self, key: Key, priority: Priority) -> bool {
        if self.failed.contains(&key) {
            return false;
        }
        if let Some(node) = self.nodes.get_mut(&key) {
            node.holds += 1;
            if node.holds == 1
                && let State::Done { stamp, .. } = node.state
            {
                self.unheld.remove(&stamp);
            }
            self.improve(key, priority);
            return true;
        }
        let needs = key.needs(self.order);
        self.nodes.insert(
            key,
            Node {
                state: State::Waiting(0),
                holds: 1,
                priority,
                needs: needs.clone(),
                waiters: Vec::new(),
            },
        );
        self.pending += 1;
        let mut missing = 0;
        for (taken, need) in needs.iter().enumerate() {
            if !self.hold(*need, priority) {
                for need in &needs[..taken] {
                    self.unwait(*need, key);
                    self.release(*need);
                }
                self.nodes.remove(&key);
                self.pending -= 1;
                self.failed.insert(key);
                return false;
            }
            let node = self.nodes.get_mut(need).expect("held above");
            if !matches!(node.state, State::Done { .. }) {
                node.waiters.push(key);
                missing += 1;
            }
        }
        let node = self.nodes.get_mut(&key).expect("made above");
        if missing == 0 {
            node.state = State::Ready;
            self.ready.insert((priority, key));
        } else {
            node.state = State::Waiting(missing);
        }
        true
    }

    /// Lets `key` and what it waits for be taken earlier, for a newer claim of an
    /// older or more urgent request.
    fn improve(&mut self, key: Key, priority: Priority) {
        let Some(node) = self.nodes.get_mut(&key) else {
            return;
        };
        if priority >= node.priority {
            return;
        }
        let old = std::mem::replace(&mut node.priority, priority);
        match node.state {
            State::Ready => {
                self.ready.remove(&(old, key));
                self.ready.insert((priority, key));
            }
            State::Waiting(_) => {
                for need in node.needs.clone() {
                    self.improve(need, priority);
                }
            }
            State::Running | State::Done { .. } => {}
        }
    }

    fn unwait(&mut self, need: Key, waiter: Key) {
        if let Some(node) = self.nodes.get_mut(&need) {
            node.waiters.retain(|key| *key != waiter);
        }
    }

    /// Lets go of one hold of `key`.
    fn release(&mut self, key: Key) {
        let Some(node) = self.nodes.get_mut(&key) else {
            return;
        };
        node.holds -= 1;
        if node.holds > 0 {
            return;
        }
        match node.state {
            State::Waiting(_) | State::Ready => self.forget(key),
            State::Done { stamp, .. } => {
                self.unheld.insert(stamp, key);
            }
            State::Running => {}
        }
    }

    /// A task that nothing wants any more and that has not started.
    fn forget(&mut self, key: Key) {
        let node = self.nodes.remove(&key).expect("a task to forget");
        debug_assert!(node.waiters.is_empty(), "a waiter holds what it waits for");
        self.pending -= 1;
        if let State::Ready = node.state {
            self.ready.remove(&(node.priority, key));
        }
        for need in node.needs {
            self.unwait(need, key);
            self.release(need);
        }
    }

    /// The next ready task, by the pick, marked running, with what it needs.
    pub(crate) fn take(&mut self) -> Option<Taken> {
        let entry = match self.pick {
            Pick::Oldest => *self.ready.first()?,
            Pick::Shuffled(_) => {
                if self.ready.is_empty() {
                    return None;
                }
                let at = (next_random(&mut self.random) % self.ready.len() as u64) as usize;
                *self.ready.iter().nth(at).expect("within the length")
            }
        };
        self.ready.remove(&entry);
        let key = entry.1;
        let node = self.nodes.get_mut(&key).expect("a ready task is known");
        node.state = State::Running;
        let needs = std::mem::take(&mut node.needs);
        self.pending -= 1;
        self.running += 1;
        let mut inputs = Vec::with_capacity(needs.len());
        for need in &needs {
            let stamp = self.stamp();
            let node = self.nodes.get_mut(need).expect("a need is held");
            let State::Done {
                output,
                stamp: used,
                ..
            } = &mut node.state
            else {
                panic!("{need:?} is not done, and {key:?} was ready");
            };
            *used = stamp;
            inputs.push((*need, Arc::clone(output)));
        }
        for need in needs {
            self.release(need);
        }
        Some((key, inputs))
    }

    /// A task that ran to its end.
    pub(crate) fn complete(&mut self, key: Key, output: Arc<Output>, answers: &mut Vec<Answer>) {
        self.count(key, &output);
        let bytes = output.bytes();
        let stamp = self.stamp();
        let node = self.nodes.get_mut(&key).expect("a running task is known");
        node.state = State::Done {
            output: Arc::clone(&output),
            bytes,
            stamp,
        };
        self.running -= 1;
        self.bytes += bytes;
        let waiters = std::mem::take(&mut node.waiters);
        if node.holds == 0 {
            self.unheld.insert(stamp, key);
        }
        for waiter in waiters {
            let Some(node) = self.nodes.get_mut(&waiter) else {
                continue;
            };
            if let State::Waiting(missing) = &mut node.state {
                *missing -= 1;
                if *missing == 0 {
                    node.state = State::Ready;
                    self.ready.insert((node.priority, waiter));
                }
            }
        }
        if let Some(ids) = self.roots.remove(&key) {
            let mut handed = false;
            for id in ids {
                let claim = self.claims.remove(&id).expect("a root's claim is known");
                if let Some(done) = claim.done {
                    answers.push((done, Ok(output.answer())));
                    handed = true;
                }
                self.release(key);
            }
            if handed {
                self.handed_out(key);
            }
        }
        self.evict();
    }

    fn count(&mut self, key: Key, output: &Output) {
        let figures = &mut self.figures;
        match key {
            Key::Starts(_) => figures.starts += 1,
            Key::Terrain(_) => figures.terrain += 1,
            Key::Run(position) => {
                figures.runs += 1;
                if !self.runs_done.insert(position) {
                    figures.runs_made_again += 1;
                }
                let run = output.run();
                figures.failed_placements += run.failures as u64;
                add(&mut figures.recorded, &run.recorded);
            }
            Key::Finished(_) => figures.finished += 1,
            Key::Lit(_) => figures.lit += 1,
        }
    }

    /// A task that panicked: it fails, and so does everything waiting for it.
    pub(crate) fn fail(&mut self, key: Key, answers: &mut Vec<Answer>) {
        self.figures.failed_tasks += 1;
        self.fail_with(key, answers);
        self.evict();
    }

    fn fail_with(&mut self, key: Key, answers: &mut Vec<Answer>) {
        self.failed.insert(key);
        let Some(node) = self.nodes.remove(&key) else {
            return;
        };
        match node.state {
            State::Running => self.running -= 1,
            State::Waiting(_) | State::Ready => {
                self.pending -= 1;
                if let State::Ready = node.state {
                    self.ready.remove(&(node.priority, key));
                }
                for need in node.needs {
                    self.unwait(need, key);
                    self.release(need);
                }
            }
            State::Done { .. } => unreachable!("a task that is done does not fail"),
        }
        if let Some(ids) = self.roots.remove(&key) {
            for id in ids {
                let claim = self.claims.remove(&id).expect("a root's claim is known");
                if let Some(done) = claim.done {
                    answers.push((done, Err(Failed)));
                }
            }
        }
        for waiter in node.waiters {
            if self.nodes.contains_key(&waiter) {
                self.fail_with(waiter, answers);
            }
        }
    }

    /// Drops what nothing holds, least recently used first, until the cache is within
    /// its bound.
    fn evict(&mut self) {
        let Some(bound) = self.bound else {
            return;
        };
        while self.bytes > bound {
            let Some((_, key)) = self.unheld.pop_first() else {
                if !self.over_bound_said {
                    self.over_bound_said = true;
                    tracing::info!(
                        bytes = self.bytes,
                        bound,
                        "the cache of generation holds more than its bound, all of it \
                         needed by requests under way"
                    );
                }
                return;
            };
            let node = self.nodes.remove(&key).expect("an unheld task is known");
            if let State::Done { bytes, .. } = node.state {
                self.bytes -= bytes;
            }
        }
        self.over_bound_said = false;
    }

    /// The number of tasks queued, by iterating them: for tests of the counts kept.
    #[cfg(test)]
    pub(crate) fn counted(&self) -> (usize, usize) {
        let pending = self
            .nodes
            .values()
            .filter(|node| matches!(node.state, State::Waiting(_) | State::Ready))
            .count();
        let running = self
            .nodes
            .values()
            .filter(|node| matches!(node.state, State::Running))
            .count();
        (pending, running)
    }

    #[cfg(test)]
    pub(crate) fn is_failed(&self, key: Key) -> bool {
        self.failed.contains(&key)
    }
}

fn add(sum: &mut Recorded, run: &Recorded) {
    sum.far_reads += run.far_reads;
    sum.beyond_margin += run.beyond_margin;
    sum.refused_writes += run.refused_writes;
    sum.dropped += run.dropped;
    sum.far_piece_reads += run.far_piece_reads;
    sum.far_pieces += run.far_pieces;
}

/// Splitmix64: a seeded source for the shuffled pick, of the scheduler's own so that a
/// seed gives the same choices wherever it runs.
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
