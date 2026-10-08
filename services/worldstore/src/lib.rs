//! World store service: serves and persists chunks, the commits of regions and their
//! states.
//!
//! The store runs on threads of its own and is spoken to through messages, so that slow
//! storage can never delay a tick and so that it can live in another process.
//!
//! One store serves every region of a world. Whoever runs a region opens it with a hello
//! and speaks to the store through the [`StoreHandle`] it gets in return, together with
//! what the region is to be [`Restored`] with. The store sees to it that a region has one
//! owner at a time; see `docs/adr/0008-durable-regions-and-resuming.md`, section 3, for
//! what it promises.
//!
//! Only chunks that were changed are stored. Any other chunk is generated again when it
//! is needed, which is why a world is tied to the generator settings it was created with.
//!
//! Whoever runs a region may be in another process than the store: [`serve`] offers a
//! store over TCP, and [`StoreHandle::connect`] opens a region of a store that is served.

mod chunks;
mod disk;
mod lanes;
mod local;
mod tcp;

#[cfg(test)]
mod kill;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clustine_format::{FormatError, Hash};
use clustine_region::{Layout, RegionId};
use clustine_rpc::{RegionHello, Restored, StoreReply, StoreRequest};
use clustine_world::ChunkGenerator;

use crate::chunks::{ChunkService, Chunks, FileChunks, MemoryChunks};
use crate::disk::{Disk, MemoryDisk, OsDisk};
use crate::lanes::Lanes;
pub use crate::tcp::{Server, serve};

/// Why a world could not be opened, read or written, or a region not be opened.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Format(#[from] FormatError),
    #[error("{path}: {error}")]
    Damaged { path: PathBuf, error: FormatError },
    #[error("a stored chunk names section {0}, which is not there")]
    MissingSection(Hash),
    #[error("the world's metadata is malformed: {0}")]
    MalformedMeta(String),
    #[error("the world was made with {setting} `{stored}`, but this server uses `{current}`")]
    Incompatible {
        setting: &'static str,
        stored: String,
        current: String,
    },
    /// The region has been opened with a higher epoch than the one offered: whoever
    /// offered it has been replaced.
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
    /// Every block of entity ids has been issued.
    #[error("there are no entity ids left for another region")]
    OutOfEntityIds,
    /// A store in another process did not accept the hello, for the reason it gave.
    #[error("the store refused the hello: {0}")]
    Refused(String),
}

/// Tells a handle from every other: the region it was opened for and a number that the
/// store gives to one handle only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Session {
    region: RegionId,
    number: u64,
}

/// The store's way to a handle, which the commit thread and the thread for chunks share.
struct Peer {
    session: Session,
    replies: Sender<StoreReply>,
    /// Set once nothing asked through the handle will be done any more.
    lost: Arc<AtomicBool>,
    /// The way back to the commit thread.
    messages: Sender<Message>,
}

impl Peer {
    fn new(
        session: Session,
        replies: Sender<StoreReply>,
        lost: Arc<AtomicBool>,
        messages: Sender<Message>,
    ) -> Self {
        Self {
            session,
            replies,
            lost,
            messages,
        }
    }

    fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// Answers the handle, unless it is lost.
    fn answer(&self, reply: StoreReply) {
        if !self.is_lost() {
            // A handle that was dropped does not listen any more.
            let _ = self.replies.send(reply);
        }
    }

    /// Marks the handle lost. For the commit thread, which has let go of the owner.
    fn mark_lost(&self) {
        self.lost.store(true, Ordering::SeqCst);
    }

    /// Marks the handle lost and has the commit thread let go of the owner.
    fn lose(&self) {
        if !self.lost.swap(true, Ordering::SeqCst) {
            self.send(Message::Lost {
                session: self.session,
            });
        }
    }

    fn send(&self, message: Message) {
        // The commit thread runs for as long as there is a peer.
        let _ = self.messages.send(message);
    }
}

/// What a handle is made of besides the way to the commit thread.
struct Opened {
    session: Session,
    replies: Receiver<StoreReply>,
    /// Set by the store once nothing the handle asks for will be done any more.
    lost: Arc<AtomicBool>,
}

/// What the commit thread is told.
enum Message {
    /// Someone says hello to become the owner of a region. `reply_to` is how the store
    /// gets back to the commit thread on the handle's behalf.
    Open {
        hello: RegionHello,
        reply_to: Sender<Message>,
        answer: Sender<Result<(Opened, Restored), StoreError>>,
    },
    /// A request made through a handle.
    Request {
        session: Session,
        request: StoreRequest,
    },
    /// A handle was dropped.
    Close { session: Session },
    /// The thread for chunks could not do what a handle asked for, which has lost the
    /// handle.
    Lost { session: Session },
    /// The thread for chunks has written the state of a checkpoint to `temporary`, after
    /// the saves before it were durable.
    Checkpointed {
        session: Session,
        tick: u64,
        temporary: PathBuf,
    },
    /// The thread for chunks has done everything the handle asked for before a flush.
    Flushed(Arc<Peer>),
}

/// A running store. It stops when it and every [`StoreHandle`] it has handed out have
/// been dropped; clones count as the store itself.
#[derive(Clone)]
pub struct Store {
    messages: Sender<Message>,
}

impl Store {
    /// Starts a store that keeps everything in memory only.
    pub fn memory(generator: Arc<dyn ChunkGenerator>) -> Store {
        start(
            Arc::new(MemoryDisk::default()),
            Path::new("/world"),
            Box::new(MemoryChunks::default()),
            generator,
        )
        .expect("a store in memory starts with nothing to read")
    }

    /// Starts a store that keeps the world in the directory `root`, creating the world
    /// there if there is none. What the regions committed and have not checkpointed is
    /// left in the log, for each region to be restored with when it is opened.
    pub fn local(root: &Path, generator: Arc<dyn ChunkGenerator>) -> Result<Store, StoreError> {
        let disk: Arc<dyn Disk> = Arc::new(OsDisk::default());
        local::prepare(&disk, root, generator.as_ref())?;
        let chunks = FileChunks::new(Arc::clone(&disk), root);
        start(disk, root, Box::new(chunks), generator)
    }

    /// Opens a region for its owner, and returns the handle with what the region is to
    /// be restored with. The hello takes its turn among the requests made through
    /// handles: what the previous owner asked for before it is done first, and the
    /// commits among that are what the region is restored with.
    ///
    /// The first hello decides which layout the store's regions are part of; one that
    /// names another layout is refused. A region has one owner at a time. A hello with
    /// the epoch of the owner, or a higher one, replaces the owner, whose handle is lost
    /// from then on: what it asks for is not done and it is not answered. A hello with a
    /// lower epoch than the highest the region has been opened with is refused, also
    /// after the store was started again.
    pub fn open_region(&self, hello: RegionHello) -> Result<(StoreHandle, Restored), StoreError> {
        let (answer, answered) = mpsc::channel();
        // The store runs for as long as there is a `Store`, so it is still there.
        let _ = self.messages.send(Message::Open {
            hello,
            reply_to: self.messages.clone(),
            answer,
        });
        let (opened, restored) = answered.recv().expect("the store answers every hello")?;
        Ok((StoreHandle::local(opened, self.messages.clone()), restored))
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

    /// Waits until everything requested so far has been done. Answers to earlier
    /// requests that are still unread are discarded, so this is for shutting down.
    ///
    /// If the handle is lost, nothing requested is done any more and this returns
    /// without waiting for it.
    pub fn flush(&self) {
        self.request(StoreRequest::Flush);
        while let Ok(reply) = self.replies.recv() {
            if reply == StoreReply::Flushed {
                return;
            }
        }
    }

    /// Whether nothing asked through this handle will be done any more: the region was
    /// taken over by another owner, the store could not keep what was asked of it, or
    /// it can no longer be reached. The region is to be opened again then, and restored
    /// from what the store has.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
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

/// Starts a store that keeps everything in memory only, and opens the one region of a
/// world that is not divided.
pub fn spawn(generator: Arc<dyn ChunkGenerator>) -> StoreHandle {
    let (handle, _) = Store::memory(generator)
        .open_region(whole_world())
        .expect("a store that has just been started accepts any hello");
    handle
}

/// Starts a store as [`Store::local`] does, and opens the one region of a world that is
/// not divided.
pub fn spawn_local(
    root: &Path,
    generator: Arc<dyn ChunkGenerator>,
) -> Result<StoreHandle, StoreError> {
    let (handle, _) = Store::local(root, generator)?.open_region(whole_world())?;
    Ok(handle)
}

/// Starts the threads of a store that keeps its log and region files on `disk` under
/// `root`, and its chunks in `chunks`.
fn start(
    disk: Arc<dyn Disk>,
    root: &Path,
    chunks: Box<dyn Chunks>,
    generator: Arc<dyn ChunkGenerator>,
) -> Result<Store, StoreError> {
    let (jobs, queued) = mpsc::channel();
    let lanes = Lanes::load(Arc::clone(&disk), root, jobs)?;
    let service = ChunkService {
        chunks,
        generator,
        disk,
        regions: root.join("regions"),
        saved: Vec::new(),
        states: 0,
    };
    thread::Builder::new()
        .name("worldstore-chunks".to_owned())
        .spawn(move || service.run(queued))?;
    let (messages, received) = mpsc::channel();
    thread::Builder::new()
        .name("worldstore".to_owned())
        .spawn(move || lanes.run(received))?;
    Ok(Store { messages })
}
