//! Chunks: where they are kept, and the thread that saves and loads them.
//!
//! That thread is apart from the one that commits, so that saving a great many chunks
//! at a checkpoint never keeps a commit waiting. It does what it is given in order,
//! which is what makes a load that follows a save find what was saved, and a checkpoint
//! wait for the saves before it.
//!
//! Chunks that were saved are written to their files together, when they are made
//! durable: syncs that wait at the same time are made durable together by the file
//! system, and syncs in turn each pay for the disk. See
//! `docs/adr/0018-a-checkpoints-chunks-written-together.md`, and for the one file a
//! chunk is, `docs/adr/0022-what-a-chunk-carries.md`, section 3.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Weak};

use clustine_data::BlockState;
use clustine_format::{ChunkFile, FormatError, StateFile};
use clustine_rpc::{Restored, StoreReply};
use clustine_world::{BlockPos, Chunk, ChunkGenerator, ChunkPos};
use tracing::{error, warn};

use crate::disk::{Disk, parent, temporary};
use crate::{Message, Opened, Peer, StoreError};

/// The epoch a chunk file has that no region saved: the store wrote it itself, when it
/// made a world over for another division. No region is ever opened with epoch 0.
pub(crate) const NO_REGION: u64 = 0;

/// The directory of the world's dimension under `chunks/`: its name without the
/// namespace. A world has one dimension, and until a generator says which
/// (`docs/adr/0021-generation-is-a-function.md`, section 8) every world is the
/// overworld; the store learns the name from the generator's settings then.
const DIMENSION: &str = "overworld";

/// Where chunks are kept.
pub(crate) trait Chunks: Send {
    /// The stored chunk, or `None` if it was never stored.
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError>;
    /// Stores the chunk, so that it is what is loaded from now on. It need not be
    /// durable before [`Chunks::sync`]. `tick` and `epoch` are those of the region
    /// that saves it, for whoever looks at what is stored.
    fn save(
        &mut self,
        position: ChunkPos,
        tick: u64,
        epoch: u64,
        chunk: &Chunk,
    ) -> Result<(), StoreError>;
    /// Makes every chunk saved so far durable. After an error none of those saved
    /// since the last time may be taken for durable, and they are to be saved again.
    fn sync(&mut self) -> Result<(), StoreError>;
    /// How many saved chunks are held to be written by the next [`Chunks::sync`]: none,
    /// where a save is all there is to storing a chunk.
    fn pending(&self) -> usize {
        0
    }
}

/// Keeps chunks in memory: they last as long as the store.
#[derive(Default)]
pub(crate) struct MemoryChunks(BTreeMap<ChunkPos, Chunk>);

impl Chunks for MemoryChunks {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        Ok(self.0.get(&position).cloned())
    }

    fn save(
        &mut self,
        position: ChunkPos,
        _tick: u64,
        _epoch: u64,
        chunk: &Chunk,
    ) -> Result<(), StoreError> {
        self.0.insert(position, chunk.clone());
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Keeps chunks in files of the world, one file a chunk:
///
/// ```text
/// chunks/overworld/<rx>.<rz>/<x>.<z>.chunk   grouped by 32×32 chunks
/// ```
///
/// A saved chunk is held here and touches no file until [`Chunks::sync`], which writes
/// all that were saved since the last one in one round: the files are written under
/// temporary names and synced at the same time, each is renamed into place, and their
/// directories are synced at the same time. Until then a load is answered with what
/// is held.
///
/// A file names nothing but its own chunk and is put in place by renaming a file that
/// is durable, so a chunk on disk is whole, as it was before or as it was saved,
/// wherever the round stops and whichever of its files a crash keeps. It is durably
/// as it was saved once the sync has ended well.
pub(crate) struct FileChunks {
    disk: Arc<dyn Disk>,
    root: PathBuf,
    /// The chunks saved since the last sync, each as it was last saved.
    pending: BTreeMap<ChunkPos, Saved>,
}

/// A chunk that was saved and is not in the files yet.
struct Saved {
    /// What a load is answered with.
    chunk: Chunk,
    /// The encoded file.
    file: Vec<u8>,
}

/// The files of a round of a sync: all are written under temporary names, those are
/// synced together, each is renamed into place, and their directories are synced
/// together.
#[derive(Default)]
struct Round {
    /// The temporary names that were written to, in the order of the files. One that
    /// could not be written in full is among them.
    temporaries: Vec<PathBuf>,
    /// How many of the first files were put in place.
    placed: usize,
}

impl Round {
    /// Puts each of the contents in the file it is paired with. They are durably
    /// there only if this returns without an error; `self` says what a failure left.
    fn put(&mut self, disk: &dyn Disk, files: &[(PathBuf, Vec<u8>)]) -> io::Result<()> {
        let mut directories = BTreeSet::new();
        for (path, contents) in files {
            let directory = parent(path);
            if !directories.contains(directory) {
                disk.create_dir_all(directory)?;
                directories.insert(directory.to_owned());
            }
            let temporary = temporary(path);
            self.temporaries.push(temporary.clone());
            disk.write(&temporary, contents)?;
        }
        // On disk before any takes the place of what was there, or a crash could leave
        // an empty file under a final name.
        disk.sync_files(&self.temporaries)?;
        for ((path, _), temporary) in files.iter().zip(&self.temporaries) {
            disk.rename(temporary, path)?;
            self.placed += 1;
        }
        disk.sync_directories(&Vec::from_iter(directories))
    }

    /// After a failure: removes the temporary files that were not put in place, as far
    /// as they can be. One that stays is never read, and is written over by the next
    /// sync of the same chunk.
    fn withdraw(&self, disk: &dyn Disk) {
        for temporary in &self.temporaries[self.placed..] {
            let _ = disk.remove(temporary);
        }
    }
}

impl FileChunks {
    pub(crate) fn new(disk: Arc<dyn Disk>, root: &Path) -> Self {
        Self {
            disk,
            root: root.to_owned(),
            pending: BTreeMap::new(),
        }
    }

    pub(crate) fn chunk_path(&self, position: ChunkPos) -> PathBuf {
        self.root
            .join("chunks")
            .join(DIMENSION)
            .join(format!("{}.{}", position.x >> 5, position.z >> 5))
            .join(format!("{}.{}.chunk", position.x, position.z))
    }
}

impl Chunks for FileChunks {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        // What was saved is what is loaded from then on, also while no file has it.
        if let Some(saved) = self.pending.get(&position) {
            return Ok(Some(saved.chunk.clone()));
        }
        let Some(bytes) = self.disk.read(&self.chunk_path(position))? else {
            return Ok(None);
        };
        let (file, unfit) = ChunkFile::decode(&bytes)?;
        if file.position != position {
            // A file that was moved or copied by hand. Its blocks are another chunk's,
            // however well its checksum fits.
            return Err(FormatError::Corrupt("the file is of another chunk").into());
        }
        if unfit > 0 {
            // The reader has put them right: an entity without data where the blocks
            // ask for one, none where they do not. Whoever looks after the world
            // should know that a file said otherwise.
            warn!(
                ?position,
                block_entities = unfit,
                "a stored chunk had block entities that did not fit its blocks"
            );
        }
        Ok(Some(file.chunk))
    }

    fn save(
        &mut self,
        position: ChunkPos,
        tick: u64,
        epoch: u64,
        chunk: &Chunk,
    ) -> Result<(), StoreError> {
        let file = ChunkFile {
            position,
            tick,
            epoch,
            chunk: chunk.clone(),
        };
        // Only noted. Written by itself a chunk costs two syncs, one after the other,
        // and a checkpoint saves a great many; the next sync writes all of them in a
        // round whose syncs wait together. A later save of the chunk takes the place
        // of this one.
        let saved = Saved {
            file: file.encode(),
            chunk: file.chunk,
        };
        self.pending.insert(position, saved);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        // Taken in any case: after a failed sync, what it was to make durable may be
        // lost although a later one succeeds. Whoever saved these chunks is told, and
        // saves them again. The directories it was to sync are forgotten with them.
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return Ok(());
        }
        let files: Vec<_> = pending
            .into_iter()
            .map(|(position, saved)| (self.chunk_path(position), saved.file))
            .collect();
        let mut round = Round::default();
        if let Err(error) = round.put(self.disk.as_ref(), &files) {
            round.withdraw(self.disk.as_ref());
            // Nothing that was put in place is taken back: such a file is whole and
            // names nothing else, so it is right whether or not a crash keeps it.
            return Err(error.into());
        }
        Ok(())
    }

    fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// What the thread for chunks is given to do.
pub(crate) enum Job {
    /// Answered with the chunk, or with [`StoreReply::Unreadable`].
    Load { position: ChunkPos, peer: Arc<Peer> },
    /// The chunk is what is loaded from now on. It is written and made durable with
    /// the next checkpoint, return, flush or restoring of whatever handle, or with this
    /// save if it leaves [`PENDING_LIMIT`] chunks to be written.
    Save {
        position: ChunkPos,
        tick: u64,
        /// The epoch the handle opened its region with.
        epoch: u64,
        chunk: Chunk,
        peer: Arc<Peer>,
    },
    /// Once the saves before it are durable, writes `state` under a name of its own and
    /// makes it durable; the commit thread puts it in place.
    Checkpoint {
        tick: u64,
        state: Vec<u8>,
        peer: Arc<Peer>,
    },
    /// Everything before it is done, and the saves before it are durable in the files;
    /// the commit thread answers.
    Flush { peer: Arc<Peer> },
    /// Once the saves before it are durable, tells the commit thread, which frees the
    /// chunks of the return with this number that are still to be freed by it.
    Return {
        number: u64,
        chunks: Vec<ChunkPos>,
        peer: Arc<Peer>,
    },
    /// Applies the block changes of the commits a region is restored with to the stored
    /// chunks, makes the saves so far durable, and then hands the opened region to
    /// whoever asked for it; or the error, if either could not be done.
    Restore {
        changes: Vec<(BlockPos, BlockState)>,
        tick: u64,
        /// The epoch the region is opened with.
        epoch: u64,
        peer: Arc<Peer>,
        opened: Opened,
        restored: Restored,
        answer: Sender<Result<(Opened, Restored), StoreError>>,
    },
    /// Applies block changes to the stored chunks and makes them durable.
    Fold {
        changes: Vec<(BlockPos, BlockState)>,
        done: Sender<Result<(), StoreError>>,
    },
    /// Everything before it is done, of whatever handle, and what there was to say of
    /// it to the commit thread is said: tells that thread so, through `reply_to`,
    /// which answers whoever waits for the store to be at rest.
    Barrier {
        reply_to: Sender<Message>,
        answer: Sender<Result<(), StoreError>>,
    },
}

/// How many saved chunks wait to be written at most: the save that makes them so many
/// has them written. A region saves a chunk also when its last ticket goes, and
/// nothing else writes it before the next checkpoint, which may be an interval away,
/// or the next return. Without a limit, what an interval changed would be held in
/// memory.
const PENDING_LIMIT: usize = 128;

/// The thread for chunks.
pub(crate) struct ChunkService {
    pub(crate) chunks: Box<dyn Chunks>,
    pub(crate) generator: Arc<dyn ChunkGenerator>,
    pub(crate) disk: Arc<dyn Disk>,
    /// Where state files are written.
    pub(crate) regions: PathBuf,
    /// The handles that saved chunks since they were last made durable. A failure to
    /// make them durable loses all of them. Held weakly, so that a handle that has gone
    /// is not kept from closing: its saves are in the log until a checkpoint of its own,
    /// which would keep it here.
    pub(crate) saved: Vec<Weak<Peer>>,
    /// How many state files have been written, which names them apart.
    pub(crate) states: u64,
}

impl ChunkService {
    pub(crate) fn run(mut self, jobs: Receiver<Job>) {
        while let Ok(job) = jobs.recv() {
            self.work(job);
        }
    }

    fn work(&mut self, job: Job) {
        match job {
            Job::Load { position, peer } => {
                // Nobody listens to what a lost handle asked for.
                if peer.is_lost() {
                    return;
                }
                let reply = match self.chunks.load(position) {
                    Ok(stored) => StoreReply::Loaded {
                        position,
                        chunk: stored.unwrap_or_else(|| self.generator.generate(position)),
                    },
                    // Generating the chunk instead would look fine at first and then
                    // overwrite what players built once it is saved. The region leaves
                    // a hole in the world that someone can look into.
                    Err(error) => {
                        error!(?position, %error, "a stored chunk cannot be read");
                        StoreReply::Unreadable { position }
                    }
                };
                peer.answer(reply);
            }
            Job::Save {
                position,
                tick,
                epoch,
                chunk,
                peer,
            } => {
                // Done even if the handle has been lost since: it was asked for before
                // that, and only once the commits before it were durable, so it holds
                // nothing that a restored region does not know of. And it comes before
                // whatever the next owner does with the chunk.
                match self.chunks.save(position, tick, epoch, &chunk) {
                    Ok(()) => {
                        self.saved.push(Arc::downgrade(&peer));
                        if self.chunks.pending() >= PENDING_LIMIT {
                            // Whoever saved them is lost if they cannot be written,
                            // this handle among them.
                            let _ = self.sync();
                        }
                    }
                    Err(error) => {
                        error!(?position, %error, "a chunk could not be stored");
                        // A checkpoint that came after this would drop commits that are
                        // in no stored chunk.
                        peer.lose();
                    }
                }
            }
            Job::Checkpoint { tick, state, peer } => {
                if peer.is_lost() {
                    return;
                }
                if self.sync().is_err() {
                    // Its saves may be among those that are not durable.
                    peer.lose();
                    return;
                }
                self.states += 1;
                let region = peer.session.region;
                let temporary = self
                    .regions
                    .join(format!("{region}.state.{}.tmp", self.states));
                let file = StateFile { tick, state }.encode();
                let written = self
                    .disk
                    .write(&temporary, &file)
                    .and_then(|()| self.disk.sync(&temporary));
                match written {
                    Ok(()) => peer.send(Message::Checkpointed {
                        session: peer.session,
                        tick,
                        temporary,
                    }),
                    Err(error) => {
                        error!(%region, %error, "the state of a region could not be written");
                        let _ = self.disk.remove(&temporary);
                        peer.lose();
                    }
                }
            }
            Job::Flush { peer } => {
                // Nobody is answered for a lost handle, and nothing is written for it.
                if peer.is_lost() {
                    return;
                }
                // Whoever flushes takes every save before it to be in the files: a
                // release, a merge and a split right after a checkpoint, when there is
                // nothing left to write, and whoever looks at the files afterwards.
                // If they cannot be written, the handles that saved are lost, and this
                // one is answered only if it is not among them.
                let _ = self.sync();
                if !peer.is_lost() {
                    peer.send(Message::Flushed(Arc::clone(&peer)));
                }
            }
            Job::Return {
                number,
                chunks,
                peer,
            } => {
                // The region is read afresh when it is opened again, and holds the
                // chunks still.
                if peer.is_lost() {
                    return;
                }
                if self.sync().is_err() {
                    // Its saves may be among those that are not durable, and with them
                    // what it changed in the chunks it gives back.
                    peer.lose();
                    return;
                }
                peer.send(Message::Returned {
                    session: peer.session,
                    number,
                    chunks,
                });
            }
            Job::Restore {
                changes,
                tick,
                epoch,
                peer,
                opened,
                restored,
                answer,
            } => {
                let applied = apply(
                    self.chunks.as_mut(),
                    self.generator.as_ref(),
                    &changes,
                    tick,
                    epoch,
                )
                // In the files before the region is handed over: a world whose chunk
                // files cannot be written is then not opened, instead of being opened
                // and lost at its first checkpoint, and what an opening applied is not
                // held in memory until one.
                .and_then(|()| self.sync());
                if let Err(error) = applied {
                    error!(region = %peer.session.region, %error, "a region could not be restored");
                    let _ = answer.send(Err(error));
                    peer.lose();
                    return;
                }
                if answer.send(Ok((opened, restored))).is_err() {
                    // Whoever asked has gone; nobody will give the region up for them.
                    peer.send(Message::Close {
                        session: peer.session,
                    });
                }
            }
            Job::Fold { changes, done } => {
                // The tick and the epoch of a saved chunk are only for whoever looks at
                // the files, and no region saves these.
                let folded = apply(
                    self.chunks.as_mut(),
                    self.generator.as_ref(),
                    &changes,
                    0,
                    NO_REGION,
                )
                .and_then(|()| self.chunks.sync());
                let _ = done.send(folded);
            }
            Job::Barrier { reply_to, answer } => {
                // Nothing is made durable for it: saved chunks become that with a
                // checkpoint, a return or a handle's own flush, whose owner is told if
                // they do not. Nothing that passes a barrier looks at files.
                let _ = reply_to.send(Message::Passed { answer });
            }
        }
    }

    /// Writes the saves so far and makes them durable. If that fails, every handle that
    /// saved since the last time is lost, and opening its region again saves its chunks
    /// again.
    fn sync(&mut self) -> Result<(), StoreError> {
        let saved = std::mem::take(&mut self.saved);
        let synced = self.chunks.sync();
        if let Err(error) = &synced {
            error!(%error, "saved chunks could not be made durable");
            for peer in saved.iter().filter_map(Weak::upgrade) {
                peer.lose();
            }
        }
        synced
    }
}

/// Applies block changes, in order, to the chunks they are in and saves those chunks as
/// of `tick`, for the region opened with `epoch`.
///
/// A chunk may have been saved with some of them in it already, or with all of them;
/// applying all of them again in order ends in the same state, because each one sets a
/// block to a definite state and the last one for a block wins.
pub(crate) fn apply(
    chunks: &mut dyn Chunks,
    generator: &dyn ChunkGenerator,
    changes: &[(BlockPos, BlockState)],
    tick: u64,
    epoch: u64,
) -> Result<(), StoreError> {
    let mut changed = BTreeMap::new();
    let mut unreadable = BTreeSet::new();
    for (position, state) in changes {
        let chunk_position = position.chunk();
        if unreadable.contains(&chunk_position) {
            continue;
        }
        let chunk = match changed.entry(chunk_position) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => match chunks.load(chunk_position) {
                Ok(stored) => {
                    entry.insert(stored.unwrap_or_else(|| generator.generate(chunk_position)))
                }
                // It is answered as unreadable when it is loaded, and stays as it is:
                // not generated, not saved.
                Err(error) => {
                    warn!(position = ?chunk_position, %error, "changes to a chunk that cannot be read are not applied");
                    unreadable.insert(chunk_position);
                    continue;
                }
            },
        };
        let (x, z) = position.in_chunk();
        chunk.set(x, position.y, z, *state);
    }
    for (position, chunk) in &changed {
        chunks.save(*position, tick, epoch, chunk)?;
    }
    Ok(())
}
