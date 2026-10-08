//! World store service: serves and persists chunks, snapshots and write-ahead logs.
//!
//! The store runs on its own thread and is spoken to through messages, so that slow
//! storage can never delay a tick and so that it can live in another process.
//!
//! One store serves every region of a world. Whoever runs a region opens it with a hello
//! and speaks to the store through the [`StoreHandle`] it gets in return. The store sees
//! to it that a region has one owner at a time, and keeps a write-ahead log per region.
//!
//! Only chunks that were changed are stored. Any other chunk is generated again when it
//! is needed, which is why a world is tied to the generator settings it was created with.
//!
//! Whoever runs a region may be in another process than the store: [`serve`] offers a
//! store over TCP, and [`StoreHandle::connect`] opens a region of a store that is served.

mod local;
mod tcp;

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clustine_format::{BlockChanges, FormatError};
use clustine_region::{Layout, RegionId};
use clustine_rpc::{RegionHello, StoreReply, StoreRequest};
use clustine_world::{Chunk, ChunkGenerator, ChunkPos};
use tracing::{error, info};

use crate::local::LocalFs;
pub use crate::tcp::{Server, serve};

/// Why a world could not be opened, read or written, or a region not be opened.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Format(#[from] FormatError),
    #[error("the world's metadata is malformed: {0}")]
    MalformedMeta(String),
    #[error("the world was made with {setting} `{stored}`, but this server uses `{current}`")]
    Incompatible {
        setting: &'static str,
        stored: String,
        current: String,
    },
    /// The region has, or has had, an owner that the one saying hello does not replace.
    #[error(
        "region {region} has been opened with epoch {seen}, which epoch {offered} does not replace"
    )]
    EpochRefused {
        region: RegionId,
        offered: u64,
        /// The highest epoch the region has been opened with.
        seen: u64,
    },
    /// The store's regions are part of another layout than the one in the hello.
    #[error("the store's regions are part of layout {expected:016x}, not of layout {offered:016x}")]
    LayoutMismatch { expected: u64, offered: u64 },
    /// A store in another process did not accept the hello, for the reason it gave.
    #[error("the store refused the hello: {0}")]
    Refused(String),
}

/// Where chunks are kept.
trait Backend: Send {
    /// The stored chunk, or `None` if it was never stored.
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError>;
    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError>;

    /// Appends block changes to the write-ahead log of `region`. They need not be
    /// durable before [`Backend::commit`] is called.
    fn log(&mut self, _region: RegionId, _changes: &BlockChanges) -> Result<(), StoreError> {
        Ok(())
    }

    /// Makes everything logged so far durable, whichever logs it went to.
    fn commit(&mut self) -> Result<(), StoreError> {
        Ok(())
    }

    /// Empties the write-ahead log of `region`: every change logged there is in a saved
    /// chunk.
    fn checkpoint(&mut self, _region: RegionId) -> Result<(), StoreError> {
        Ok(())
    }

    /// What is in the write-ahead log of `region`.
    fn pending(&mut self, _region: RegionId) -> Result<Vec<BlockChanges>, StoreError> {
        Ok(Vec::new())
    }
}

/// Keeps chunks in memory: they last as long as the store. There are no logs.
#[derive(Default)]
struct Memory(BTreeMap<ChunkPos, Chunk>);

impl Backend for Memory {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        Ok(self.0.get(&position).cloned())
    }

    fn save(&mut self, position: ChunkPos, _tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        self.0.insert(position, chunk.clone());
        Ok(())
    }
}

/// Tells a handle from every other: the region it was opened for and a number that the
/// store gives to one handle only.
#[derive(Clone, Copy)]
struct Session {
    region: RegionId,
    number: u64,
}

/// What a handle is made of besides the way to the store thread.
struct Opened {
    session: Session,
    replies: Receiver<StoreReply>,
    /// Set by the store once another owner has taken the region over.
    lost: Arc<AtomicBool>,
}

/// What the store thread is told.
enum Message {
    /// Someone says hello to become the owner of a region.
    Open {
        hello: RegionHello,
        answer: Sender<Result<Opened, StoreError>>,
    },
    /// A request made through a handle.
    Request {
        session: Session,
        request: StoreRequest,
    },
    /// A handle was dropped.
    Close { session: Session },
}

/// A running store. It stops when it and every [`StoreHandle`] it has handed out have
/// been dropped; clones count as the store itself.
#[derive(Clone)]
pub struct Store {
    messages: Sender<Message>,
}

impl Store {
    /// Starts a store that keeps changed chunks in memory only.
    pub fn memory(generator: Arc<dyn ChunkGenerator>) -> Store {
        start(Box::new(Memory::default()), generator)
    }

    /// Starts a store that keeps changed chunks in the directory `root`, creating the
    /// world there if there is none. If the server died last time, the changes its
    /// regions had logged but not yet saved are applied first. The regions need not be
    /// the same this time.
    pub fn local(root: &Path, generator: Arc<dyn ChunkGenerator>) -> Result<Store, StoreError> {
        let mut backend = LocalFs::open(root, &generator.settings())?;
        backend.drain_logs(|backend, pending| {
            let (chunks, changes) = recover(backend, generator.as_ref(), pending)?;
            info!(chunks, changes, "recovered changes that had not been saved");
            Ok(())
        })?;
        Ok(start(Box::new(backend), generator))
    }

    /// Opens a region for its owner. The hello takes its turn among the requests made
    /// through handles.
    ///
    /// The first hello decides which layout the store's regions are part of; one that
    /// names another layout is refused. A region has one owner at a time. A hello with a
    /// higher epoch than the owner's replaces the owner, whose handle is lost from then
    /// on: what it asks for is not done and it is not answered. A hello with the same or
    /// a lower epoch is refused. Once the owner has dropped its handle, the region can be
    /// opened again with the same epoch or a higher one.
    ///
    /// What the last owner logged and did not get to checkpoint is applied to the stored
    /// chunks before the handle is handed out.
    pub fn open_region(&self, hello: RegionHello) -> Result<StoreHandle, StoreError> {
        let (answer, answered) = mpsc::channel();
        // The store runs for as long as there is a `Store`, so it is still there.
        let _ = self.messages.send(Message::Open { hello, answer });
        let opened = answered
            .recv()
            .expect("the store thread answers every hello")?;
        Ok(StoreHandle::local(opened, self.messages.clone()))
    }
}

/// An open region: what its owner speaks to the store through. Dropping it gives the
/// region up, after everything requested before has been done.
///
/// The store is in this process or, for a handle made by [`StoreHandle::connect`], in
/// another one. A handle is used in the same way and does the same either way.
pub struct StoreHandle {
    link: Link,
    replies: Receiver<StoreReply>,
    /// Set once nothing asked through the handle will be done any more.
    lost: Arc<AtomicBool>,
}

/// How what a handle asks for gets to the store. Dropping it gives the region up.
enum Link {
    /// Through the messages of a store in this process.
    Local {
        session: Session,
        messages: Sender<Message>,
    },
    /// Through the thread that writes to the connection to a store in another process.
    /// That thread closes the connection once this is gone.
    Remote(Sender<StoreRequest>),
}

impl Link {
    fn request(&self, request: StoreRequest) {
        match self {
            Link::Local { session, messages } => {
                // The store runs for as long as there is a handle, so it is still there.
                let _ = messages.send(Message::Request {
                    session: *session,
                    request,
                });
            }
            Link::Remote(requests) => {
                // The thread is gone if the connection has failed. The handle is lost
                // then, and nothing it asks for is done.
                let _ = requests.send(request);
            }
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Link::Local { session, messages } = self {
            let _ = messages.send(Message::Close { session: *session });
        }
    }
}

impl StoreHandle {
    /// The handle of a region that the store behind `messages` has opened.
    fn local(opened: Opened, messages: Sender<Message>) -> StoreHandle {
        let Opened {
            session,
            replies,
            lost,
        } = opened;
        StoreHandle {
            link: Link::Local { session, messages },
            replies,
            lost,
        }
    }

    /// Queues a request. An answer, if the request has one, arrives later through
    /// [`StoreHandle::try_reply`].
    pub fn request(&self, request: StoreRequest) {
        self.link.request(request);
    }

    /// Returns the next answer if one is ready, without blocking. A handle that is lost
    /// has no answers any more.
    pub fn try_reply(&self) -> Option<StoreReply> {
        if self.is_lost() {
            return None;
        }
        self.replies.try_recv().ok()
    }

    /// Waits until everything requested so far has been done. Answers to earlier load
    /// requests that are still unread are discarded, so this is for shutting down.
    ///
    /// If the handle is lost, nothing requested is done any more and this returns
    /// without waiting.
    pub fn flush(&self) {
        self.request(StoreRequest::Flush);
        while let Ok(reply) = self.replies.recv() {
            if reply == StoreReply::Flushed {
                return;
            }
        }
    }

    /// Whether nothing asked through this handle will be done any more: the region was
    /// taken over by another owner, or the store can no longer be reached.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

/// The hello of the first owner of a region that covers the whole world.
fn whole_world() -> RegionHello {
    RegionHello {
        region: RegionId(0),
        epoch: 1,
        layout: Layout::single().fingerprint(),
    }
}

/// Starts a store that keeps changed chunks in memory only, and opens the one region of
/// a world that is not divided.
pub fn spawn(generator: Arc<dyn ChunkGenerator>) -> StoreHandle {
    Store::memory(generator)
        .open_region(whole_world())
        .expect("a store that has just been started accepts any hello")
}

/// Starts a store as [`Store::local`] does, and opens the one region of a world that is
/// not divided.
pub fn spawn_local(
    root: &Path,
    generator: Arc<dyn ChunkGenerator>,
) -> Result<StoreHandle, StoreError> {
    Store::local(root, generator)?.open_region(whole_world())
}

/// Applies logged block changes to the chunks they are in and saves those chunks, after
/// which the log they are from can be emptied. Returns the number of chunks and of
/// changes.
///
/// A log holds every change since the last checkpoint, in order. A chunk may have been
/// saved in between with some of them already in it; applying all of them again in
/// order ends in the same state, because each one sets a block to a definite state.
fn recover(
    backend: &mut dyn Backend,
    generator: &dyn ChunkGenerator,
    pending: &[BlockChanges],
) -> Result<(usize, usize), StoreError> {
    let mut chunks = BTreeMap::new();
    let mut changes = 0;
    let mut tick = 0;
    for record in pending {
        tick = tick.max(record.tick);
        for (position, state) in &record.changes {
            let chunk_position = position.chunk();
            let chunk = match chunks.entry(chunk_position) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(
                    backend
                        .load(chunk_position)?
                        .unwrap_or_else(|| generator.generate(chunk_position)),
                ),
            };
            let (x, z) = position.in_chunk();
            chunk.set(x, position.y, z, *state);
            changes += 1;
        }
    }
    for (position, chunk) in &chunks {
        backend.save(*position, tick, chunk)?;
    }
    Ok((chunks.len(), changes))
}

/// What the store knows about a region that has been opened.
struct RegionState {
    /// The highest epoch the region has been opened with.
    epoch: u64,
    /// The owner, unless it has dropped its handle.
    owner: Option<Owner>,
}

/// The store's end of a handle.
struct Owner {
    /// The number of the handle's session.
    number: u64,
    epoch: u64,
    replies: Sender<StoreReply>,
    /// Tells the handle that another owner has taken the region over.
    lost: Arc<AtomicBool>,
}

/// What the store thread works with.
struct Service {
    backend: Box<dyn Backend>,
    generator: Arc<dyn ChunkGenerator>,
    /// The fingerprint of the layout the regions are part of, known from the first hello.
    layout: Option<u64>,
    regions: BTreeMap<RegionId, RegionState>,
    /// How many handles have been handed out.
    handles: u64,
}

impl Service {
    fn run(mut self, messages: Receiver<Message>) {
        // Messages are taken in batches: everything that is waiting is handled, then
        // what was logged is made durable in one go.
        while let Ok(first) = messages.recv() {
            let batch = std::iter::once(first).chain(messages.try_iter());
            let mut replies = Vec::new();
            for message in batch {
                match message {
                    Message::Open { hello, answer } => {
                        // Whoever said hello is waiting for the answer.
                        let _ = answer.send(self.open(hello));
                    }
                    Message::Request { session, request } => {
                        // What a handle asks for after its region was taken over is
                        // not done.
                        let Some(epoch) = self.owner(session).map(|owner| owner.epoch) else {
                            continue;
                        };
                        if let Some(reply) = self.serve(session.region, epoch, request) {
                            replies.push((session, reply));
                        }
                    }
                    Message::Close { session } => {
                        // A region that was taken over is not the handle's to give up.
                        if let Some(state) = self.regions.get_mut(&session.region) {
                            state.owner.take_if(|owner| owner.number == session.number);
                        }
                    }
                }
            }
            // Before any answer, so that a flush means "durable".
            if let Err(error) = self.backend.commit() {
                error!(%error, "the logs could not be made durable");
            }
            for (session, reply) in replies {
                // A handle whose region was taken over later in the batch is not
                // answered either. One that was dropped does not listen any more.
                if let Some(owner) = self.owner(session) {
                    let _ = owner.replies.send(reply);
                }
            }
        }
    }

    /// The store's end of the handle `session`, if the handle still owns its region.
    fn owner(&self, session: Session) -> Option<&Owner> {
        let owner = self.regions.get(&session.region)?.owner.as_ref()?;
        (owner.number == session.number).then_some(owner)
    }

    /// Makes whoever said `hello` the owner of the region, if nothing speaks against it.
    fn open(&mut self, hello: RegionHello) -> Result<Opened, StoreError> {
        let RegionHello {
            region,
            epoch,
            layout,
        } = hello;
        let expected = *self.layout.get_or_insert(layout);
        if layout != expected {
            return Err(StoreError::LayoutMismatch {
                expected,
                offered: layout,
            });
        }
        let state = match self.regions.entry(region) {
            Entry::Occupied(entry) => {
                let state = entry.into_mut();
                // An owner that is there only gives way to a later one. One that has
                // let go may come back.
                let accepted = match state.owner {
                    Some(_) => epoch > state.epoch,
                    None => epoch >= state.epoch,
                };
                if !accepted {
                    return Err(StoreError::EpochRefused {
                        region,
                        offered: epoch,
                        seen: state.epoch,
                    });
                }
                state
            }
            Entry::Vacant(entry) => entry.insert(RegionState { epoch, owner: None }),
        };
        // From here on the owner that was there, if any, gets nothing done and hears
        // nothing: its handle is lost, and its end of the replies is closed, which also
        // ends a flush it waits in. In this order, so that the flush finds the handle lost.
        if let Some(replaced) = state.owner.take() {
            replaced.lost.store(true, Ordering::Relaxed);
            info!(%region, epoch, "the owner of a region was replaced");
        }
        state.epoch = epoch;

        // What the last owner logged and did not get to checkpoint is not in the stored
        // chunks, which are what the new owner will load.
        let pending = self.backend.pending(region)?;
        if !pending.is_empty() {
            let (chunks, changes) =
                recover(self.backend.as_mut(), self.generator.as_ref(), &pending)?;
            self.backend.checkpoint(region)?;
            info!(%region, chunks, changes, "recovered changes that had not been saved");
        }

        self.handles += 1;
        let session = Session {
            region,
            number: self.handles,
        };
        let (sender, replies) = mpsc::channel();
        let lost = Arc::new(AtomicBool::new(false));
        state.owner = Some(Owner {
            number: session.number,
            epoch,
            replies: sender,
            lost: Arc::clone(&lost),
        });
        Ok(Opened {
            session,
            replies,
            lost,
        })
    }

    /// Does what the owner of `region` asks for and returns the answer, if there is one.
    fn serve(&mut self, region: RegionId, epoch: u64, request: StoreRequest) -> Option<StoreReply> {
        match request {
            StoreRequest::Load { position } => match self.backend.load(position) {
                Ok(stored) => Some(StoreReply::Loaded {
                    position,
                    chunk: stored.unwrap_or_else(|| self.generator.generate(position)),
                }),
                // Generating the chunk instead would look fine at first and then
                // overwrite what players built once it is saved. The region leaves a
                // hole in the world that someone can look into.
                Err(error) => {
                    error!(?position, %error, "a stored chunk cannot be read");
                    Some(StoreReply::Unreadable { position })
                }
            },
            StoreRequest::Save {
                position,
                tick,
                chunk,
            } => {
                if let Err(error) = self.backend.save(position, tick, &chunk) {
                    error!(?position, %error, "a chunk could not be stored");
                }
                None
            }
            // The state of the region is not kept yet, and the answer does not wait for
            // the disk yet either; see docs/adr/0008-durable-regions-and-resuming.md for
            // what a commit is to become.
            StoreRequest::Commit {
                tick,
                changes,
                state: _,
            } => {
                if !changes.is_empty() {
                    let record = BlockChanges {
                        tick,
                        epoch,
                        changes,
                    };
                    if let Err(error) = self.backend.log(region, &record) {
                        error!(%region, %error, "block changes could not be logged");
                    }
                }
                Some(StoreReply::Committed { tick })
            }
            StoreRequest::Checkpoint { .. } => {
                if let Err(error) = self.backend.checkpoint(region) {
                    error!(%region, %error, "the log could not be emptied");
                }
                None
            }
            StoreRequest::Flush => Some(StoreReply::Flushed),
        }
    }
}

fn start(backend: Box<dyn Backend>, generator: Arc<dyn ChunkGenerator>) -> Store {
    let (sender, receiver) = mpsc::channel();
    let service = Service {
        backend,
        generator,
        layout: None,
        regions: BTreeMap::new(),
        handles: 0,
    };
    thread::Builder::new()
        .name("worldstore".to_owned())
        .spawn(move || service.run(receiver))
        .expect("spawning the worldstore thread");
    Store { messages: sender }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Barrier;
    use std::time::Duration;

    use clustine_data::blocks;
    use clustine_worldgen::FlatGenerator;

    use super::*;

    pub(crate) fn generator() -> Arc<dyn ChunkGenerator> {
        Arc::new(FlatGenerator::classic())
    }

    /// Waits for the next answer to `store` other than that a commit is done, which the
    /// tests here do not look at.
    pub(crate) fn reply(store: &StoreHandle) -> StoreReply {
        for _ in 0..30_000 {
            match store.try_reply() {
                Some(StoreReply::Committed { .. }) => {}
                Some(reply) => return reply,
                None => thread::sleep(Duration::from_millis(1)),
            }
        }
        panic!("the store did not answer");
    }

    /// Asks for a chunk and waits for it.
    pub(crate) fn load(store: &StoreHandle, position: ChunkPos) -> Chunk {
        store.request(StoreRequest::Load { position });
        match reply(store) {
            StoreReply::Loaded {
                position: loaded,
                chunk,
            } => {
                assert_eq!(loaded, position);
                chunk
            }
            other => panic!("expected the chunk, got {other:?}"),
        }
    }

    pub(crate) fn save(store: &StoreHandle, position: ChunkPos, chunk: &Chunk) {
        store.request(StoreRequest::Save {
            position,
            tick: 5,
            chunk: chunk.clone(),
        });
    }

    pub(crate) fn edited() -> Chunk {
        let mut chunk = generator().generate(ChunkPos::new(0, 0));
        chunk.set(3, -61, 4, blocks::AIR);
        chunk.set(3, 100, 4, blocks::GLASS);
        chunk
    }

    /// The hello of an owner of one of two regions: 0 is west of x = 0, 1 east of it.
    pub(crate) fn hello(region: u32, epoch: u64) -> RegionHello {
        RegionHello {
            region: RegionId(region),
            epoch,
            layout: Layout::new(vec![0]).unwrap().fingerprint(),
        }
    }

    /// A store of each kind.
    pub(crate) fn stores(directory: &Path) -> [Store; 2] {
        [
            Store::memory(generator()),
            Store::local(directory, generator()).unwrap(),
        ]
    }

    #[test]
    fn chunks_that_were_never_stored_are_generated() {
        let directory = tempfile::tempdir().unwrap();
        let stores = [
            spawn(generator()),
            spawn_local(directory.path(), generator()).unwrap(),
        ];
        for store in stores {
            let position = ChunkPos::new(7, -9);
            assert_eq!(load(&store, position), generator().generate(position));
        }
    }

    #[test]
    fn a_saved_chunk_is_what_is_loaded_from_then_on() {
        let directory = tempfile::tempdir().unwrap();
        let stores = [
            spawn(generator()),
            spawn_local(directory.path(), generator()).unwrap(),
        ];
        for store in stores {
            let position = ChunkPos::new(-2, 5);
            save(&store, position, &edited());
            assert_eq!(load(&store, position), edited());
            // Other chunks are not affected.
            let other = ChunkPos::new(-2, 6);
            assert_eq!(load(&store, other), generator().generate(other));
        }
    }

    /// Generating the chunk instead would overwrite what was built there once it is
    /// saved, and saying nothing would leave the region waiting for it.
    #[test]
    fn a_stored_chunk_that_cannot_be_read_is_answered_as_unreadable() {
        let directory = tempfile::tempdir().unwrap();
        let position = ChunkPos::new(3, 4);
        let store = spawn_local(directory.path(), generator()).unwrap();
        save(&store, position, &edited());
        store.flush();
        let manifest = directory
            .path()
            .join("manifests/overworld/0.0/3.4.manifest");
        std::fs::write(&manifest, b"not a manifest").unwrap();

        store.request(StoreRequest::Load { position });
        assert_eq!(reply(&store), StoreReply::Unreadable { position });
    }

    #[test]
    fn a_world_on_disk_outlives_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let position = ChunkPos::new(100, -100);
        {
            let store = spawn_local(directory.path(), generator()).unwrap();
            save(&store, position, &edited());
            store.flush();
        }
        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, position), edited());
    }

    #[test]
    fn flush_waits_for_everything_before_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = spawn_local(directory.path(), generator()).unwrap();
        for x in 0..50 {
            save(&store, ChunkPos::new(x, 0), &edited());
        }
        // A load that is still unanswered does not confuse the flush.
        store.request(StoreRequest::Load {
            position: ChunkPos::new(0, 0),
        });
        store.flush();
        let manifests = directory.path().join("manifests/overworld");
        let stored: usize = fs::read_dir(manifests)
            .unwrap()
            .map(|region| fs::read_dir(region.unwrap().path()).unwrap().count())
            .sum();
        assert_eq!(stored, 50);
    }

    #[test]
    fn equal_sections_are_stored_once() {
        let directory = tempfile::tempdir().unwrap();
        let store = spawn_local(directory.path(), generator()).unwrap();
        for x in 0..20 {
            save(&store, ChunkPos::new(x, 3), &edited());
        }
        store.flush();
        // The edited chunk has two sections that are not plain air.
        let blobs: usize = fs::read_dir(directory.path().join("blobs"))
            .unwrap()
            .map(|prefix| fs::read_dir(prefix.unwrap().path()).unwrap().count())
            .sum();
        assert_eq!(blobs, 2);
    }

    pub(crate) fn log(
        store: &StoreHandle,
        tick: u64,
        changes: &[(i32, i32, i32, clustine_data::BlockState)],
    ) {
        store.request(StoreRequest::Commit {
            state: Vec::new(),
            tick,
            changes: changes
                .iter()
                .map(|(x, y, z, state)| (clustine_world::BlockPos::new(*x, *y, *z), *state))
                .collect(),
        });
    }

    /// Changes that were logged but never saved, as after a crash, are in the world
    /// when it is opened again.
    #[test]
    fn logged_changes_are_recovered_when_the_world_is_opened() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = spawn_local(directory.path(), generator()).unwrap();
            // The same block twice: the later change wins. And a second chunk.
            log(
                &store,
                1,
                &[(3, -61, 4, blocks::STONE), (40, -61, 4, blocks::AIR)],
            );
            log(
                &store,
                2,
                &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
            );
            store.flush();
            // The store is dropped without the chunks ever being saved.
        }

        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, ChunkPos::new(0, 0)), edited());
        let other = load(&store, ChunkPos::new(2, 0));
        assert_eq!(other.get(8, -61, 4), Some(blocks::AIR));

        // Recovery saved the chunks and emptied the log.
        assert_eq!(fs::read(directory.path().join("logs/0.wal")).unwrap(), b"");
        drop(store);
        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, ChunkPos::new(0, 0)), edited());
    }

    /// A chunk saved in the middle of the logged changes ends up right all the same.
    #[test]
    fn recovery_copes_with_chunks_saved_in_between() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        {
            let store = spawn_local(directory.path(), generator()).unwrap();
            log(&store, 1, &[(3, -61, 4, blocks::STONE)]);
            let mut saved = generator().generate(origin);
            saved.set(3, -61, 4, blocks::STONE);
            save(&store, origin, &saved);
            log(
                &store,
                2,
                &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
            );
            store.flush();
        }
        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, origin), edited());
    }

    #[test]
    fn a_checkpoint_empties_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let wal = directory.path().join("logs/0.wal");
        let store = spawn_local(directory.path(), generator()).unwrap();
        log(&store, 1, &[(3, -61, 4, blocks::STONE)]);
        store.flush();
        assert!(!fs::read(&wal).unwrap().is_empty());

        store.request(StoreRequest::Checkpoint {
            tick: 0,
            state: Vec::new(),
        });
        store.flush();
        assert_eq!(fs::read(&wal).unwrap(), b"");

        // Logging goes on after a checkpoint.
        log(&store, 2, &[(3, -61, 4, blocks::AIR)]);
        store.flush();
        assert!(!fs::read(&wal).unwrap().is_empty());
    }

    #[test]
    fn logged_changes_carry_the_epoch_of_the_owner() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::local(directory.path(), generator()).unwrap();
        let owner = store.open_region(hello(1, 7)).unwrap();
        log(&owner, 3, &[(3, -61, 4, blocks::AIR)]);
        owner.flush();
        let logged = fs::read(directory.path().join("logs/1.wal")).unwrap();
        let (records, _) = clustine_format::read_log(&logged).unwrap();
        let expected = BlockChanges {
            tick: 3,
            epoch: 7,
            changes: vec![(clustine_world::BlockPos::new(3, -61, 4), blocks::AIR)],
        };
        assert_eq!(records, [expected]);
    }

    /// The process can die in the middle of appending to the log. Whatever is left of
    /// the last record is ignored and the records before it are recovered.
    #[test]
    fn a_log_cut_off_in_the_middle_of_a_record_is_recovered_up_to_there() {
        let directory = tempfile::tempdir().unwrap();
        let wal = directory.path().join("logs/0.wal");
        let origin = ChunkPos::new(0, 0);
        {
            let store = spawn_local(directory.path(), generator()).unwrap();
            log(&store, 1, &[(3, -61, 4, blocks::AIR)]);
            log(&store, 2, &[(3, 100, 4, blocks::GLASS)]);
            log(&store, 3, &[(5, 100, 5, blocks::STONE)]);
            store.flush();
        }
        let complete = fs::read(&wal).unwrap();
        // Three records of the same length; cut at the start of, just into, and just
        // before the end of the third.
        let record = complete.len() / 3;
        for length in [2 * record, 2 * record + 1, 3 * record - 1] {
            fs::write(&wal, &complete[..length]).unwrap();
            // Recovery from an earlier round has saved the chunk; start from scratch.
            let _ = fs::remove_dir_all(directory.path().join("manifests"));
            let store = spawn_local(directory.path(), generator()).unwrap();
            assert_eq!(load(&store, origin), edited(), "cut at {length}");
        }
    }

    /// Each region has a log of its own, and a checkpoint only says something about the
    /// region that makes it.
    #[test]
    fn regions_log_and_checkpoint_independently() {
        let directory = tempfile::tempdir().unwrap();
        let logs = ["logs/0.wal", "logs/1.wal"].map(|path| directory.path().join(path));
        let (west_chunk, east_chunk) = (ChunkPos::new(-1, 0), ChunkPos::new(0, 0));
        let mut dug = generator().generate(west_chunk);
        dug.set(13, -61, 4, blocks::AIR);
        {
            let store = Store::local(directory.path(), generator()).unwrap();
            let west = store.open_region(hello(0, 1)).unwrap();
            let east = store.open_region(hello(1, 1)).unwrap();
            log(&west, 1, &[(-3, -61, 4, blocks::AIR)]);
            log(
                &east,
                1,
                &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
            );
            west.flush();
            east.flush();
            let logged = logs.clone().map(|path| fs::read(path).unwrap());
            assert!(!logged[0].is_empty() && !logged[1].is_empty());
            assert_ne!(logged[0], logged[1]);

            // The west saves what it changed and makes a checkpoint.
            save(&west, west_chunk, &dug);
            west.request(StoreRequest::Checkpoint {
                tick: 0,
                state: Vec::new(),
            });
            west.flush();
            assert_eq!(fs::read(&logs[0]).unwrap(), b"");
            assert_eq!(fs::read(&logs[1]).unwrap(), logged[1]);
            // The server dies before the east has saved anything.
        }

        let store = Store::local(directory.path(), generator()).unwrap();
        let west = store.open_region(hello(0, 1)).unwrap();
        let east = store.open_region(hello(1, 1)).unwrap();
        assert_eq!(load(&east, east_chunk), edited());
        assert_eq!(load(&west, west_chunk), dug);
        for path in logs {
            assert_eq!(fs::read(path).unwrap(), b"");
        }
    }

    /// When a world is opened, what is in its logs goes into the chunks, whichever
    /// regions the logs are of: the world may be divided in another way from now on.
    #[test]
    fn the_logs_of_all_regions_are_recovered_when_the_world_is_opened() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = Store::local(directory.path(), generator()).unwrap();
            let west = store.open_region(hello(0, 3)).unwrap();
            let east = store.open_region(hello(1, 4)).unwrap();
            log(&west, 1, &[(-3, -61, 4, blocks::AIR)]);
            log(
                &east,
                1,
                &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
            );
            west.flush();
            east.flush();
        }

        // A single region this time, and a first owner again.
        let store = spawn_local(directory.path(), generator()).unwrap();
        for path in ["logs/0.wal", "logs/1.wal"] {
            assert_eq!(fs::read(directory.path().join(path)).unwrap(), b"");
        }
        assert_eq!(load(&store, ChunkPos::new(0, 0)), edited());
        let west_chunk = load(&store, ChunkPos::new(-1, 0));
        assert_eq!(west_chunk.get(13, -61, 4), Some(blocks::AIR));
    }

    #[test]
    fn a_log_cut_off_in_one_region_leaves_the_log_of_another_whole() {
        let directory = tempfile::tempdir().unwrap();
        let (west_chunk, east_chunk) = (ChunkPos::new(-1, 0), ChunkPos::new(0, 0));
        {
            let store = Store::local(directory.path(), generator()).unwrap();
            let west = store.open_region(hello(0, 1)).unwrap();
            let east = store.open_region(hello(1, 1)).unwrap();
            for (handle, x) in [(&west, -3), (&east, 3)] {
                log(handle, 1, &[(x, -61, 4, blocks::AIR)]);
                log(handle, 2, &[(x, 100, 4, blocks::GLASS)]);
                handle.flush();
            }
        }
        // The server died while the west was appending its second record.
        let wal = directory.path().join("logs/0.wal");
        let complete = fs::read(&wal).unwrap();
        fs::write(&wal, &complete[..complete.len() - 1]).unwrap();

        let store = Store::local(directory.path(), generator()).unwrap();
        let west = store.open_region(hello(0, 1)).unwrap();
        let east = store.open_region(hello(1, 1)).unwrap();
        let mut dug = generator().generate(west_chunk);
        dug.set(13, -61, 4, blocks::AIR);
        assert_eq!(load(&west, west_chunk), dug);
        assert_eq!(load(&east, east_chunk), edited());
    }

    #[test]
    fn answers_reach_only_the_handle_that_asked() {
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            let west = store.open_region(hello(0, 1)).unwrap();
            let east = store.open_region(hello(1, 1)).unwrap();
            let (here, there) = (ChunkPos::new(-4, 2), ChunkPos::new(6, 2));
            let loaded = |position| StoreReply::Loaded {
                position,
                chunk: generator().generate(position),
            };
            west.request(StoreRequest::Load { position: here });
            east.request(StoreRequest::Load { position: there });
            west.request(StoreRequest::Flush);
            east.request(StoreRequest::Load { position: there });
            east.request(StoreRequest::Flush);

            assert_eq!(reply(&east), loaded(there));
            assert_eq!(reply(&east), loaded(there));
            assert_eq!(reply(&east), StoreReply::Flushed);
            assert_eq!(east.try_reply(), None);
            // The east asked last, so the west has been answered by now.
            assert_eq!(west.try_reply(), Some(loaded(here)));
            assert_eq!(west.try_reply(), Some(StoreReply::Flushed));
            assert_eq!(west.try_reply(), None);
        }
    }

    /// Each region is run by a thread of its own, which opens the region and uses it.
    #[test]
    fn regions_are_opened_and_used_from_threads_of_their_own() {
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            thread::scope(|scope| {
                for (region, x) in [(0, -1), (1, 0)] {
                    let store = store.clone();
                    scope.spawn(move || {
                        let handle = store.open_region(hello(region, 1)).unwrap();
                        let position = ChunkPos::new(x, 0);
                        let mut built = generator().generate(position);
                        built.set(1, 80, 1, blocks::STONE);
                        save(&handle, position, &built);
                        assert_eq!(load(&handle, position), built);
                        handle.flush();
                    });
                }
            });
        }
    }

    /// A world that was last opened before it could have several regions has a single
    /// log, `wal`, next to `meta`.
    #[test]
    fn the_log_of_a_world_from_before_regions_is_recovered_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        drop(spawn_local(directory.path(), generator()).unwrap());
        fs::remove_dir_all(directory.path().join("logs")).unwrap();
        let wal = directory.path().join("wal");
        let record = |tick, (x, y, z), state| BlockChanges {
            tick,
            epoch: 1,
            changes: vec![(clustine_world::BlockPos::new(x, y, z), state)],
        };
        let mut logged = record(1, (3, -61, 4), blocks::AIR).encode();
        logged.extend(record(2, (3, 100, 4), blocks::GLASS).encode());
        // And a record the server died in the middle of.
        logged.extend(&record(3, (5, 100, 5), blocks::STONE).encode()[..9]);
        fs::write(&wal, logged).unwrap();

        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, origin), edited());
        assert!(!wal.exists());
        assert_eq!(fs::read(directory.path().join("logs/0.wal")).unwrap(), b"");

        // The changes are in a saved chunk, which is there without any log.
        drop(store);
        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, origin), edited());
    }

    /// An owner can go away without a checkpoint while the store runs on, as when its
    /// process dies. What it logged is then in the log only.
    #[test]
    fn a_region_opened_again_has_what_its_last_owner_logged() {
        let directory = tempfile::tempdir().unwrap();
        let wal = directory.path().join("logs/1.wal");
        let origin = ChunkPos::new(0, 0);
        let store = Store::local(directory.path(), generator()).unwrap();
        let first = store.open_region(hello(1, 1)).unwrap();
        log(&first, 1, &[(3, -61, 4, blocks::STONE)]);
        log(
            &first,
            2,
            &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
        );
        first.flush();
        drop(first);
        assert!(!fs::read(&wal).unwrap().is_empty());
        assert!(!directory.path().join("manifests/overworld").exists());

        // The log is replayed before the region is handed out: its changes are in a
        // saved chunk and no longer in the log.
        let second = store.open_region(hello(1, 1)).unwrap();
        assert_eq!(fs::read(&wal).unwrap(), b"");
        assert!(
            directory
                .path()
                .join("manifests/overworld/0.0/0.0.manifest")
                .exists()
        );
        assert_eq!(load(&second, origin), edited());
    }

    #[test]
    fn an_owner_with_a_higher_epoch_replaces_the_one_there_is() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let old = store.open_region(hello(1, 1)).unwrap();
            // Done, because the region is still its own.
            save(&old, origin, &edited());
            assert!(!old.is_lost());
            // Whether or not this is answered before the new owner is there, the answer
            // is not given out afterwards.
            old.request(StoreRequest::Load { position: origin });
            let new = store.open_region(hello(1, 2)).unwrap();
            assert!(old.is_lost() && !new.is_lost());

            // Nothing the old owner asks for is done any more, and none of it answered.
            save(&old, origin, &generator().generate(origin));
            old.request(StoreRequest::Load { position: origin });
            // Returns although the store does not answer it.
            old.flush();
            // Once this is answered, the store has been through all of the above.
            new.flush();
            assert_eq!(old.try_reply(), None);
            assert_eq!(load(&new, origin), edited());

            // The region is not the old owner's to give up either.
            drop(old);
            assert!(matches!(
                store.open_region(hello(1, 2)),
                Err(StoreError::EpochRefused { .. })
            ));
            assert_eq!(load(&new, origin), edited());
        }
    }

    /// Generates what [`generator`] does, but stops before the chunk at [`HELD`] until
    /// it is let go on. The store is busy meanwhile, and what it is asked for queues up.
    pub(crate) struct Held(pub(crate) Barrier);

    pub(crate) const HELD: ChunkPos = ChunkPos::new(1000, 1000);

    impl ChunkGenerator for Held {
        fn generate(&self, position: ChunkPos) -> Chunk {
            if position == HELD {
                // Once to say that the store has got here, once to be let go on.
                self.0.wait();
                self.0.wait();
            }
            generator().generate(position)
        }

        fn settings(&self) -> String {
            generator().settings()
        }
    }

    /// The hello of a new owner takes its turn among the requests. What the old owner
    /// asked for before it is done; what is behind it, already waiting or not, is not.
    #[test]
    fn what_a_replaced_owner_has_queued_or_asks_for_later_is_not_done() {
        let directory = tempfile::tempdir().unwrap();
        let wal = directory.path().join("logs/1.wal");
        let origin = ChunkPos::new(0, 0);
        let mut dug = generator().generate(origin);
        dug.set(3, -61, 4, blocks::AIR);
        // What the old owner tries once it has been replaced.
        let mut overwritten = generator().generate(origin);
        overwritten.set(9, 90, 9, blocks::STONE);
        let meddle = |old: &StoreHandle| {
            save(old, origin, &overwritten);
            log(old, 9, &[(5, 100, 5, blocks::STONE)]);
            old.request(StoreRequest::Checkpoint {
                tick: 0,
                state: Vec::new(),
            });
            old.request(StoreRequest::Load { position: origin });
        };

        let held = Arc::new(Held(Barrier::new(2)));
        let store = Store::local(directory.path(), held.clone()).unwrap();
        let old = store.open_region(hello(1, 1)).unwrap();
        log(&old, 1, &[(3, -61, 4, blocks::AIR)]);

        // While the store is busy, the hello of a new owner arrives, and after it more
        // requests of the old one.
        old.request(StoreRequest::Load { position: HELD });
        held.0.wait();
        let (answer, answered) = mpsc::channel();
        let open = Message::Open {
            hello: hello(1, 2),
            answer,
        };
        store.messages.send(open).unwrap();
        meddle(&old);
        held.0.wait();
        let opened = answered.recv().unwrap().unwrap();
        let new = StoreHandle::local(opened, store.messages.clone());

        // The new owner hears nothing that was meant for the old one, finds what the old
        // one logged before the hello, and nothing of what was waiting behind it.
        new.request(StoreRequest::Flush);
        assert_eq!(reply(&new), StoreReply::Flushed);
        assert_eq!(load(&new, origin), dug);
        assert_eq!(fs::read(&wal).unwrap(), b"");
        // Not even the chunk the store was busy with is answered.
        assert_eq!(old.try_reply(), None);

        // The same goes for what the old owner asks for from now on.
        log(&new, 2, &[(3, 100, 4, blocks::GLASS)]);
        new.flush();
        let logged = fs::read(&wal).unwrap();
        assert!(!logged.is_empty());
        meddle(&old);
        old.flush();
        new.flush();
        assert_eq!(old.try_reply(), None);
        assert_eq!(load(&new, origin), dug);
        assert_eq!(fs::read(&wal).unwrap(), logged);

        // After a crash the world is as the new owner left it.
        drop((old, new, store));
        let store = Store::local(directory.path(), generator()).unwrap();
        let owner = store.open_region(hello(1, 1)).unwrap();
        assert_eq!(load(&owner, origin), edited());
    }

    #[test]
    fn an_owner_gives_way_only_to_a_higher_epoch() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let owner = store.open_region(hello(1, 5)).unwrap();
            save(&owner, origin, &edited());
            for epoch in [5, 4] {
                let Err(error) = store.open_region(hello(1, epoch)) else {
                    panic!("the region was opened a second time with epoch {epoch}");
                };
                assert!(
                    matches!(
                        error,
                        StoreError::EpochRefused {
                            region: RegionId(1),
                            offered,
                            seen: 5,
                        } if offered == epoch
                    ),
                    "{error}"
                );
            }
            // The owner is none the worse for it.
            assert_eq!(load(&owner, origin), edited());
            // Another region has an owner and epochs of its own.
            let neighbour = store.open_region(hello(0, 1)).unwrap();
            let position = ChunkPos::new(-1, 0);
            assert_eq!(load(&neighbour, position), generator().generate(position));
        }
    }

    #[test]
    fn a_region_given_up_is_opened_again_with_the_same_epoch_or_a_higher_one() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let refused = |epoch, highest| {
                matches!(
                    store.open_region(hello(1, epoch)),
                    Err(StoreError::EpochRefused {
                        region: RegionId(1),
                        offered,
                        seen,
                    }) if offered == epoch && seen == highest
                )
            };
            let owner = store.open_region(hello(1, 5)).unwrap();
            save(&owner, origin, &edited());
            drop(owner);
            assert!(refused(4, 5));

            let owner = store.open_region(hello(1, 5)).unwrap();
            assert_eq!(load(&owner, origin), edited());
            drop(owner);

            let owner = store.open_region(hello(1, 7)).unwrap();
            assert_eq!(load(&owner, origin), edited());
            drop(owner);
            // The highest epoch counts, not the one the region was opened with first.
            assert!(refused(6, 7));
        }
    }

    #[test]
    fn a_hello_with_another_layout_than_the_first_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            let first = hello(0, 1);
            let other = RegionHello {
                layout: Layout::single().fingerprint(),
                ..hello(1, 1)
            };
            let refused = || {
                matches!(
                    store.open_region(other),
                    Err(StoreError::LayoutMismatch { expected, offered })
                        if expected == first.layout && offered == other.layout
                )
            };
            let west = store.open_region(first).unwrap();
            assert!(refused());
            // The layout stays when no region is open any more.
            drop(west);
            assert!(refused());
            // The refused hello has not taken the region either.
            let east = store.open_region(hello(1, 1)).unwrap();
            let position = ChunkPos::new(0, 0);
            assert_eq!(load(&east, position), generator().generate(position));
        }
    }

    #[test]
    fn a_world_is_tied_to_its_generator() {
        struct Other;
        impl ChunkGenerator for Other {
            fn generate(&self, position: ChunkPos) -> Chunk {
                FlatGenerator::classic().generate(position)
            }
            fn settings(&self) -> String {
                "something else".to_owned()
            }
        }

        let directory = tempfile::tempdir().unwrap();
        drop(spawn_local(directory.path(), generator()).unwrap());
        // Opening it again with the same generator is fine.
        drop(spawn_local(directory.path(), generator()).unwrap());
        let Err(error) = spawn_local(directory.path(), Arc::new(Other)) else {
            panic!("the world was opened with another generator");
        };
        assert!(
            matches!(
                error,
                StoreError::Incompatible {
                    setting: "generator",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_damaged_chunk_is_not_replaced_by_a_generated_one() {
        let directory = tempfile::tempdir().unwrap();
        let position = ChunkPos::new(1, 1);
        let store = spawn_local(directory.path(), generator()).unwrap();
        save(&store, position, &edited());
        store.flush();

        let manifest = directory
            .path()
            .join("manifests/overworld/0.0/1.1.manifest");
        let mut bytes = fs::read(&manifest).unwrap();
        bytes[10] ^= 0xFF;
        fs::write(&manifest, bytes).unwrap();

        store.request(StoreRequest::Load { position });
        store.flush();
        assert_eq!(store.try_reply(), None, "the damaged chunk was answered");
        // The store carries on with other chunks.
        let other = ChunkPos::new(2, 2);
        assert_eq!(load(&store, other), generator().generate(other));
    }
}
