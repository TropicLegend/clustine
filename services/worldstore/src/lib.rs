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
//! The store also says which regions there are and which of them holds a chunk: it is
//! started with a [`Division`] of the world, only the holder of a chunk loads and saves
//! it, and [`Store::regions`] lists the regions. See
//! `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! Only chunks that were changed are stored. Any other chunk is generated again when it
//! is needed, which is why a world is tied to the generator settings it was created with.
//!
//! Whoever runs a region may be in another process than the store: [`serve`] offers a
//! store over TCP, [`StoreHandle::connect`] opens a region of a store that is served,
//! and [`regions`] reads its list of regions.

mod chunks;
mod disk;
mod lanes;
mod local;
mod table;
mod tcp;

#[cfg(test)]
mod kill;
#[cfg(test)]
mod kill_regions;
#[cfg(test)]
mod kill_unpinned;
#[cfg(test)]
mod regions;
#[cfg(test)]
mod rest;
#[cfg(test)]
mod rounds;
#[cfg(test)]
mod scenarios;
#[cfg(test)]
mod stripes_end;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unpinned;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clustine_format::{FormatError, Hash};
use clustine_region::RegionId;
use clustine_rpc::{RegionHello, RegionList, Restored, StoreReply, StoreRequest};
use clustine_world::{ChunkArea, ChunkGenerator, ChunkPos};

use crate::chunks::{ChunkService, Chunks, FileChunks, MemoryChunks};
use crate::disk::{Disk, MemoryDisk, OsDisk};
use crate::lanes::Lanes;
pub use crate::table::{Division, NotAscending};
pub use crate::tcp::{Server, regions, serve};

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
    /// The region has been absorbed by another, and is none any more.
    #[error("region {region} has been absorbed by region {into}")]
    Absorbed { region: RegionId, into: RegionId },
    /// The world has no such region: the hello is for a region of another division
    /// than the store was started with, or for one that was absorbed long ago.
    #[error("the world has no region {region}")]
    UnknownRegion { region: RegionId },
    /// The store was started with a division in which two pinned regions have a chunk
    /// in common.
    #[error("the areas of the pinned regions {first} and {second} overlap")]
    Division { first: RegionId, second: RegionId },
    /// The table of regions and the log do not fit together, or the table says what
    /// cannot be.
    #[error("the table of regions is not in order: {0}")]
    Table(String),
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
    /// The thread for chunks has made the saves durable that were asked for before the
    /// return with this number, of these chunks.
    Returned {
        session: Session,
        number: u64,
        chunks: Vec<ChunkPos>,
    },
    /// Someone wants the list of regions.
    Regions {
        answer: Sender<Result<RegionList, StoreError>>,
    },
    /// Someone waits for the store to be at rest with what was asked of it before.
    /// The commit thread passes it on to the thread for chunks, behind what it has
    /// for that thread; `reply_to` is how that thread gets back to this one.
    Barrier {
        reply_to: Sender<Message>,
        answer: Sender<Result<(), StoreError>>,
    },
    /// The thread for chunks has done everything it was given before a barrier, and
    /// has said what it had to say of it.
    Passed {
        answer: Sender<Result<(), StoreError>>,
    },
}

/// A running store. It stops when it and every [`StoreHandle`] it has handed out have
/// been dropped; clones count as the store itself. Nobody waits for its threads to
/// end: whoever needs it to have done with what it was asked calls [`Store::flush`].
#[derive(Clone)]
pub struct Store {
    messages: Sender<Message>,
}

impl Store {
    /// Starts a store that keeps everything in memory only, for a world that is one
    /// region: [`Store::memory_divided`] with one region pinned to the whole world and
    /// the home chunk at the origin.
    pub fn memory(generator: Arc<dyn ChunkGenerator>) -> Store {
        Self::memory_divided(generator, undivided())
            .expect("a world of one region has no areas that overlap")
    }

    /// Starts a store that keeps everything in memory only, for a world divided as
    /// `division` says.
    pub fn memory_divided(
        generator: Arc<dyn ChunkGenerator>,
        division: Division,
    ) -> Result<Store, StoreError> {
        start(
            Arc::new(MemoryDisk::default()),
            Path::new("/world"),
            Box::new(MemoryChunks::default()),
            generator,
            &division,
        )
    }

    /// Starts a store as [`Store::local_divided`] does, for a world that is one region,
    /// as [`Store::memory`] does.
    pub fn local(root: &Path, generator: Arc<dyn ChunkGenerator>) -> Result<Store, StoreError> {
        Self::local_divided(root, generator, undivided())
    }

    /// Starts a store that keeps the world in the directory `root`, creating the world
    /// there if there is none, divided as `division` says. What the regions committed
    /// and have not checkpointed is left in the log, for each region to be restored
    /// with when it is opened.
    ///
    /// If the world there was divided otherwise, by other areas or with another home
    /// chunk, it is made over: what its regions committed is put into the stored
    /// chunks, their states are dropped, and its regions are those of `division`. So
    /// is a world from before there was a table of regions, whatever it was divided
    /// into. The store says in its log, once, that it has made the world over, and
    /// that whoever was in the world has to join again: whoever still runs a region
    /// of it finds the region gone, or begun anew.
    pub fn local_divided(
        root: &Path,
        generator: Arc<dyn ChunkGenerator>,
        division: Division,
    ) -> Result<Store, StoreError> {
        let disk: Arc<dyn Disk> = Arc::new(OsDisk::default());
        local::prepare(&disk, root, generator.as_ref())?;
        let chunks = FileChunks::new(Arc::clone(&disk), root);
        start(disk, root, Box::new(chunks), generator, &division)
    }

    /// The regions of the world, with what each is pinned to and roughly holds, and
    /// those that were absorbed. Everything asked for before is durable when the list
    /// is made, so that it has nothing a crash would take back.
    ///
    /// The error is [`StoreError::Io`] for as long as what a failed write left in the
    /// log is not durably gone, as it is for a hello.
    pub fn regions(&self) -> Result<RegionList, StoreError> {
        let (answer, answered) = mpsc::channel();
        // The store runs for as long as there is a `Store`, so it is still there.
        let _ = self.messages.send(Message::Regions { answer });
        answered
            .recv()
            .expect("the store answers every request for the list")
    }

    /// Waits until the store is at rest with everything that was asked of it before
    /// this call, through this store, a clone of it or any handle: each such request
    /// has been done, or will never be (what a handle asks after it is lost is not
    /// done); what it wrote is as durable as the store makes it; every answer the
    /// store owes for it has been sent to its handle; and neither of the store's
    /// threads is in the middle of any of it.
    ///
    /// It closes nothing. Handles and clones that live are served on, and what they
    /// ask after this call is not waited for. Neither is what a handle in another
    /// process has asked that has not reached this one yet.
    ///
    /// Saved chunks are not written to their files or made durable by it: that is
    /// what a checkpoint, a return and [`StoreHandle::flush`] do. A region's commits
    /// stay in the log until its checkpoint, so nothing is lost by that.
    ///
    /// It takes as long as what the two threads have before them, and may wait for
    /// the disk: it is not for a thread that must not block.
    ///
    /// The error is [`StoreError::Io`] for as long as what a failed write left in
    /// the log is not durably gone, as for [`Store::regions`]. Nothing is under way
    /// then either. It is also [`StoreError::Io`] if one of the store's threads has
    /// died, of which nothing more can be said.
    pub fn flush(&self) -> Result<(), StoreError> {
        Self::rested(&self.barrier())
    }

    /// The first half of [`Store::flush`]: sends the barrier, and returns where its
    /// answer arrives.
    pub(crate) fn barrier(&self) -> Receiver<Result<(), StoreError>> {
        let (answer, answered) = mpsc::channel();
        // The store runs for as long as there is a `Store`, so it is still there.
        let _ = self.messages.send(Message::Barrier {
            reply_to: self.messages.clone(),
            answer,
        });
        answered
    }

    /// The second half: waits for the answer to a barrier.
    pub(crate) fn rested(barrier: &Receiver<Result<(), StoreError>>) -> Result<(), StoreError> {
        // Only a thread that has died lets go of a barrier without answering it.
        barrier
            .recv()
            .map_err(|_| io::Error::other("a thread of the world store has gone"))?
    }

    /// Opens a region for its owner, and returns the handle with what the region is to
    /// be restored with. The hello takes its turn among the requests made through
    /// handles: what the previous owner asked for before it is done first, and the
    /// commits among that are what the region is restored with.
    ///
    /// The regions are those the store's table has: a hello for a region there is
    /// none of is refused. A region has one owner at a time. A hello with
    /// the epoch of the owner, or a higher one, replaces the owner, whose handle is lost
    /// from then on: what it asks for is not done and it is not answered. A hello with a
    /// lower epoch than the highest the region has been opened with is refused, also
    /// after the store was started again.
    ///
    /// After a write or a sync of the log has failed, every region has lost its owner,
    /// and a hello is answered with [`StoreError::Io`] for as long as what was cut off
    /// the log is not durably gone. It is worth saying again.
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
    /// The chunks saved before it, through whatever handle, are in the store's files
    /// then, and durable. If they could not be written, every handle that saved since
    /// they were last made durable is lost.
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

/// A world that is one region, which players enter at the origin.
fn undivided() -> Division {
    Division {
        home: ChunkPos::new(0, 0),
        pinned: vec![ChunkArea::EVERYWHERE],
    }
}

/// The hello of the first owner of a region that covers the whole world.
fn whole_world() -> RegionHello {
    RegionHello {
        region: RegionId(0),
        epoch: 1,
    }
}

/// Starts a store that keeps everything in memory only, and opens the one region of a
/// world that is not divided.
pub fn spawn(generator: Arc<dyn ChunkGenerator>) -> StoreHandle {
    let (handle, _) = Store::memory(generator)
        .open_region(whole_world())
        .expect("a store that has just been started accepts the hello for its one region");
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
/// `root`, and its chunks in `chunks`, for a world divided as `division` says.
///
/// The thread for chunks runs first: reading the world may have it put what the regions
/// of another division committed into the stored chunks. It ends by itself if the world
/// cannot be read, when nothing can give it work any more.
fn start(
    disk: Arc<dyn Disk>,
    root: &Path,
    chunks: Box<dyn Chunks>,
    generator: Arc<dyn ChunkGenerator>,
    division: &Division,
) -> Result<Store, StoreError> {
    let (jobs, queued) = mpsc::channel();
    let service = ChunkService {
        chunks,
        generator,
        disk: Arc::clone(&disk),
        regions: root.join("regions"),
        saved: Vec::new(),
        states: 0,
    };
    thread::Builder::new()
        .name("worldstore-chunks".to_owned())
        .spawn(move || service.run(queued))?;
    let lanes = Lanes::load(disk, root, jobs, division)?;
    let (messages, received) = mpsc::channel();
    thread::Builder::new()
        .name("worldstore".to_owned())
        .spawn(move || lanes.run(received))?;
    Ok(Store { messages })
}
