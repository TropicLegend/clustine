//! Chunks: where they are kept, and the thread that saves and loads them.
//!
//! That thread is apart from the one that commits, so that saving a great many chunks
//! at a checkpoint never keeps a commit waiting. It does what it is given in order,
//! which is what makes a load that follows a save find what was saved, and a checkpoint
//! wait for the saves before it.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Weak};

use clustine_data::BlockState;
use clustine_format::{ChunkManifest, Hash, StateFile, pack, unpack};
use clustine_rpc::{Restored, StoreReply};
use clustine_world::{BlockPos, Chunk, ChunkGenerator, ChunkPos};
use tracing::{error, warn};

use crate::disk::{Disk, parent, replace};
use crate::{Message, Opened, Peer, StoreError};

/// Where chunks are kept.
pub(crate) trait Chunks: Send {
    /// The stored chunk, or `None` if it was never stored.
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError>;
    /// Stores the chunk, so that it is what is loaded from now on. It need not be
    /// durable before [`Chunks::sync`].
    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError>;
    /// Makes every chunk saved so far durable.
    fn sync(&mut self) -> Result<(), StoreError>;
}

/// Keeps chunks in memory: they last as long as the store.
#[derive(Default)]
pub(crate) struct MemoryChunks(BTreeMap<ChunkPos, Chunk>);

impl Chunks for MemoryChunks {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        Ok(self.0.get(&position).cloned())
    }

    fn save(&mut self, position: ChunkPos, _tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        self.0.insert(position, chunk.clone());
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Keeps chunks in files of the world:
///
/// ```text
/// blobs/ab/abcdef…                                sections, by the hash of their content
/// manifests/overworld/<rx>.<rz>/<x>.<z>.manifest  chunks, grouped by 32×32 chunks
/// ```
///
/// Section files are never changed once written, and are durable before a manifest
/// names them. A manifest is durable once its directory has been synced, which
/// [`Chunks::sync`] does for every directory that a manifest was put in since.
pub(crate) struct FileChunks {
    disk: Arc<dyn Disk>,
    root: PathBuf,
    /// Directories that a manifest has been put in since they were last synced.
    unsynced: BTreeSet<PathBuf>,
    /// Section files that may be there without being durably so: their directory could
    /// not be synced, and then they could not be removed either. They are written
    /// again by the next chunk that has them, instead of being taken for stored.
    unsure: BTreeSet<PathBuf>,
}

impl FileChunks {
    pub(crate) fn new(disk: Arc<dyn Disk>, root: &Path) -> Self {
        Self {
            disk,
            root: root.to_owned(),
            unsynced: BTreeSet::new(),
            unsure: BTreeSet::new(),
        }
    }

    fn blob_path(&self, hash: &Hash) -> PathBuf {
        let name = hash.to_string();
        self.root.join("blobs").join(&name[..2]).join(name)
    }

    pub(crate) fn manifest_path(&self, position: ChunkPos) -> PathBuf {
        self.root
            .join("manifests/overworld")
            .join(format!("{}.{}", position.x >> 5, position.z >> 5))
            .join(format!("{}.{}.manifest", position.x, position.z))
    }

    /// Writes the sections that are not stored yet, and makes them durable.
    fn store_sections(&mut self, sections: Vec<(Hash, Vec<u8>)>) -> Result<(), StoreError> {
        let mut created = Vec::new();
        let stored = (|| -> Result<(), StoreError> {
            let mut directories = BTreeSet::new();
            for (hash, canonical) in sections {
                let path = self.blob_path(&hash);
                if self.disk.exists(&path)? && !self.unsure.contains(&path) {
                    continue;
                }
                let directory = parent(&path).to_owned();
                self.disk.create_dir_all(&directory)?;
                replace(self.disk.as_ref(), &path, &pack(&canonical))?;
                created.push(path);
                directories.insert(directory);
            }
            for directory in directories {
                self.disk.sync_directory(&directory)?;
            }
            Ok(())
        })();
        match &stored {
            Ok(()) => {
                for path in &created {
                    self.unsure.remove(path);
                }
            }
            // A section file that is there but perhaps not durably so would be taken
            // for a stored one by the next chunk that has it, whose manifest could then
            // name a section that a crash takes away. If it cannot be removed, it is
            // remembered as that.
            Err(_) => {
                for path in created {
                    if self.disk.remove(&path).is_err() {
                        self.unsure.insert(path);
                    }
                }
            }
        }
        stored
    }
}

impl Chunks for FileChunks {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        let Some(bytes) = self.disk.read(&self.manifest_path(position))? else {
            return Ok(None);
        };
        let manifest = ChunkManifest::decode(&bytes)?;
        let chunk = manifest.restore(|hash| -> Result<Vec<u8>, StoreError> {
            let path = self.blob_path(hash);
            let packed = self
                .disk
                .read(&path)?
                .ok_or(StoreError::MissingSection(*hash))?;
            Ok(unpack(&packed)?)
        })?;
        Ok(Some(chunk))
    }

    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        let (manifest, sections) = ChunkManifest::describe(position, chunk, tick);
        // Sections first: a manifest must never name a section that is not there.
        self.store_sections(sections)?;
        let path = self.manifest_path(position);
        let directory = parent(&path).to_owned();
        self.disk.create_dir_all(&directory)?;
        replace(self.disk.as_ref(), &path, &manifest.encode())?;
        self.unsynced.insert(directory);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        // Taken in any case: after a failed sync, what it was to make durable may be
        // lost although a later one succeeds. Whoever saved into these directories is
        // told, and saves it again.
        for directory in std::mem::take(&mut self.unsynced) {
            self.disk.sync_directory(&directory)?;
        }
        Ok(())
    }
}

/// What the thread for chunks is given to do.
pub(crate) enum Job {
    /// Answered with the chunk, or with [`StoreReply::Unreadable`].
    Load { position: ChunkPos, peer: Arc<Peer> },
    Save {
        position: ChunkPos,
        tick: u64,
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
    /// Everything before it is done; the commit thread answers.
    Flush { peer: Arc<Peer> },
    /// Once the saves before it are durable, tells the commit thread, which frees the
    /// chunks of the return with this number that are still to be freed by it.
    Return {
        number: u64,
        chunks: Vec<ChunkPos>,
        peer: Arc<Peer>,
    },
    /// Applies the block changes of the commits a region is restored with to the stored
    /// chunks, then hands the opened region to whoever asked for it.
    Restore {
        changes: Vec<(BlockPos, BlockState)>,
        tick: u64,
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
}

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
                chunk,
                peer,
            } => {
                // Done even if the handle has been lost since: it was asked for before
                // that, and only once the commits before it were durable, so it holds
                // nothing that a restored region does not know of. And it comes before
                // whatever the next owner does with the chunk.
                match self.chunks.save(position, tick, &chunk) {
                    Ok(()) => self.saved.push(Arc::downgrade(&peer)),
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
                if !self.sync() {
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
                if !self.sync() {
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
                );
                if let Err(error) = applied {
                    error!(region = %peer.session.region, %error, "a region could not be restored");
                    let _ = answer.send(Err(error));
                    peer.lose();
                    return;
                }
                self.saved.push(Arc::downgrade(&peer));
                if answer.send(Ok((opened, restored))).is_err() {
                    // Whoever asked has gone; nobody will give the region up for them.
                    peer.send(Message::Close {
                        session: peer.session,
                    });
                }
            }
            Job::Fold { changes, done } => {
                // The tick of a saved chunk is only for whoever looks at the files.
                let folded = apply(self.chunks.as_mut(), self.generator.as_ref(), &changes, 0)
                    .and_then(|()| self.chunks.sync());
                let _ = done.send(folded);
            }
        }
    }

    /// Makes the saves so far durable. If that fails, every handle that saved since the
    /// last time is lost, and opening its region again saves its chunks again.
    fn sync(&mut self) -> bool {
        let saved = std::mem::take(&mut self.saved);
        match self.chunks.sync() {
            Ok(()) => true,
            Err(error) => {
                error!(%error, "saved chunks could not be made durable");
                for peer in saved.iter().filter_map(Weak::upgrade) {
                    peer.lose();
                }
                false
            }
        }
    }
}

/// Applies block changes, in order, to the chunks they are in and saves those chunks as
/// of `tick`.
///
/// A chunk may have been saved with some of them in it already, or with all of them;
/// applying all of them again in order ends in the same state, because each one sets a
/// block to a definite state and the last one for a block wins.
pub(crate) fn apply(
    chunks: &mut dyn Chunks,
    generator: &dyn ChunkGenerator,
    changes: &[(BlockPos, BlockState)],
    tick: u64,
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
        chunks.save(*position, tick, chunk)?;
    }
    Ok(())
}
