//! Chunks: where they are kept, and the thread that saves and loads them.
//!
//! That thread is apart from the one that commits, so that saving a great many chunks
//! at a checkpoint never keeps a commit waiting. It does each job it is given once the
//! earlier jobs that job waits for are done, which is what makes a load that follows a
//! save find what was saved, and a checkpoint wait for the saves before it; a chunk
//! that is not stored is made by the generator meanwhile, and holds back only what
//! waits for it (`docs/adr/0021-generation-is-a-function.md`, section 7).
//!
//! Chunks that were saved, and chunks that were made and handed to a region, are written to their files together, when they are made
//! durable: syncs that wait at the same time are made durable together by the file
//! system, and syncs in turn each pay for the disk. See
//! `docs/adr/0018-a-checkpoints-chunks-written-together.md`, and for the one file a
//! chunk is, `docs/adr/0022-what-a-chunk-carries.md`, section 3.
//!
//! A chunk is named by its dimension and its position ([`LevelChunk`]): each dimension
//! has its chunks in a directory of its own, and a generator of its own makes those not
//! stored (`docs/groundwork/p-plan.md`, section 3.3). The changes a region's commits
//! make, which name positions alone, are of the region's dimension, which the jobs that
//! carry them name.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use clustine_data::BlockState;
use clustine_format::{ChunkFile, EntitiesFile, FormatError, StateFile, read_log};
use clustine_rpc::{Parts, Restored, StoreReply};
use clustine_world::generation::{Failed, Wanted};
use clustine_world::poi::Claim;
use clustine_world::{
    BlockPos, Change, Chunk, ChunkPos, Dimension, EntityChange, KeptEvent, LevelChunk,
    ScheduledTick, Seam,
};
use tracing::{error, info, warn};

use crate::disk::{Disk, parent, temporary};
use crate::generators::Generators;
use crate::lanes::commit_changes;
use crate::local::Meta;
use crate::{Message, Opened, Peer, StoreError};

/// The epoch a chunk file has that no region saved: the store wrote it itself, when it
/// made a world over for another division. No region is ever opened with epoch 0.
pub(crate) const NO_REGION: u64 = 0;

/// The directory of `dimension`'s chunks under `chunks/`: its name in the game's
/// registries without the namespace, as the format names it
/// (`docs/world-format.md`, "Directory layout"): `overworld`, where every world kept
/// its chunks when it had the overworld alone, `the_nether` and `the_end`.
fn directory(dimension: Dimension) -> &'static str {
    let name = dimension.type_name();
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// Where chunks are kept.
pub(crate) trait Chunks: Send {
    /// The stored chunk, or `None` if it was never stored.
    fn load(&mut self, position: LevelChunk) -> Result<Option<Chunk>, StoreError>;
    /// Whether a chunk is stored at `position`, as [`Chunks::load`] would find it.
    fn has(&mut self, position: LevelChunk) -> Result<bool, StoreError> {
        Ok(self.load(position)?.is_some())
    }
    /// Stores the `parts` of the chunk, so that the chunk is what is loaded from now on:
    /// its blocks (with everything else it keeps) and its entities are kept apart, and a
    /// part not named stays as it was stored (`docs/adr/0028-entities.md`, section 2).
    /// It need not be durable before [`Chunks::sync`]. `tick` and `epoch` are those of
    /// the region that saves it, for whoever looks at what is stored.
    fn save_parts(
        &mut self,
        position: LevelChunk,
        tick: u64,
        epoch: u64,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError>;
    /// Stores the whole chunk, as [`Chunks::save_parts`] does with every part.
    fn save(
        &mut self,
        position: LevelChunk,
        tick: u64,
        epoch: u64,
        chunk: &Chunk,
    ) -> Result<(), StoreError> {
        self.save_parts(position, tick, epoch, chunk, Parts::ALL)
    }
    /// Makes every chunk saved so far durable. After an error none of those saved
    /// since the last time may be taken for durable, and they are to be saved again.
    fn sync(&mut self) -> Result<(), StoreError>;
    /// How many saved chunks are held to be written by the next [`Chunks::sync`]: none,
    /// where a save is all there is to storing a chunk. Notes are not among them.
    fn pending(&self) -> usize {
        0
    }
    /// Notes a chunk the generator made and a region was answered with, to be stored
    /// by the next [`Chunks::sync`] (`docs/adr/0021-generation-is-a-function.md`,
    /// section 6, S2). A save of the position that is pending is kept instead: it is
    /// the chunk with a change in it. Unlike a save, a note that a failed sync was to
    /// write is kept, and written by the next one (S5). A chunk made for the first time
    /// is noted whole, its entities with it (`docs/groundwork/n7l-plan.md`, section 2.4);
    /// one made again over a stored one (S8) only its blocks, its stored entities kept.
    fn note(
        &mut self,
        position: LevelChunk,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError> {
        if self.load(position)?.is_none() {
            self.save(position, 0, NO_REGION, chunk)?;
        } else if parts.blocks {
            self.save_parts(position, 0, NO_REGION, chunk, Parts::BLOCKS)?;
        }
        Ok(())
    }
    /// How many notes are held to be written by the next [`Chunks::sync`].
    fn noted(&self) -> usize {
        0
    }
    /// A chunk that `dimension`'s generator of `version` made has been handed to a
    /// region, as it was made or to be changed: a file that holds it, or what was made
    /// of it, may be put in place by the next [`Chunks::sync`] (ADR-0021, section 6).
    fn handed(&mut self, _dimension: Dimension, _version: u16) {}
}

/// Keeps chunks in memory: they last as long as the store.
#[derive(Default)]
pub(crate) struct MemoryChunks(BTreeMap<LevelChunk, Chunk>);

impl Chunks for MemoryChunks {
    fn load(&mut self, position: LevelChunk) -> Result<Option<Chunk>, StoreError> {
        Ok(self.0.get(&position).cloned())
    }

    fn note(
        &mut self,
        position: LevelChunk,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError> {
        match self.0.get_mut(&position) {
            None => {
                self.0.insert(position, chunk.clone());
            }
            Some(stored) if parts.blocks => {
                let entities = stored.entities().to_vec();
                *stored = chunk.clone();
                stored.set_entities(entities);
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// As files keep them: a part not named is as it was, and a chunk that is not
    /// stored is stored whole, as a first-timer's files are written whole.
    fn save_parts(
        &mut self,
        position: LevelChunk,
        _tick: u64,
        _epoch: u64,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError> {
        let stored = self.0.get(&position);
        let mut kept = match (parts.blocks, stored) {
            (true, _) | (false, None) => chunk.clone(),
            (false, Some(stored)) => stored.clone(),
        };
        if parts.entities || stored.is_none() {
            kept.set_entities(chunk.entities().to_vec());
        } else {
            kept.set_entities(stored.map_or_else(Vec::new, |stored| stored.entities().to_vec()));
        }
        self.0.insert(position, kept);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Keeps chunks in files of the world, one file a chunk and one for its entities where
/// it has any:
///
/// ```text
/// chunks/<dimension>/<rx>.<rz>/<x>.<z>.chunk      grouped by 32×32 chunks
/// chunks/<dimension>/<rx>.<rz>/<x>.<z>.entities   beside it
/// ```
///
/// A saved chunk is held here and touches no file until [`Chunks::sync`], which writes
/// all that were saved since the last one in one round: the files are written under
/// temporary names and synced at the same time, each is renamed into place, and their
/// directories are synced at the same time. Until then a load is answered with what
/// is held. The entities' file of a chunk whose entities are all gone is removed in the
/// round, after the renames and before the directories are synced.
///
/// A file names nothing but its own chunk and is put in place by renaming a file that
/// is durable, so a chunk on disk is whole, as it was before or as it was saved,
/// wherever the round stops and whichever of its files a crash keeps. It is durably
/// as it was saved once the sync has ended well.
pub(crate) struct FileChunks {
    disk: Arc<dyn Disk>,
    root: PathBuf,
    /// The chunks saved or noted since the last sync, each as it was last saved.
    pending: BTreeMap<LevelChunk, Saved>,
    /// The world's `meta`, where there is one to keep up.
    meta: Option<Meta>,
    /// The highest version of each dimension's generator that made a chunk handed to a
    /// region, in the order of the dimensions' numbers.
    handed: [u16; 3],
}

/// A chunk that was saved or noted and is not in the files yet.
struct Saved {
    /// What a load is answered with.
    chunk: Chunk,
    /// The parts to be written. A later save of the chunk while this one is pending
    /// adds its own, and all are written from the later chunk (the plan, section 11).
    parts: Parts,
    /// The encoded file of its blocks, if they are to be written.
    blocks: Option<Vec<u8>>,
    /// The encoded file of its entities, if they are to be written: none where the
    /// chunk has none, whose file is removed instead.
    entities: Option<Option<Vec<u8>>>,
    /// Noted, not saved: a chunk as the generator made it, which no handle answers for.
    noted: bool,
    /// No blocks file of the chunk is in place: its first one is to be, which a sync
    /// puts in place only after its entities' file (`docs/groundwork/n7l-plan.md`,
    /// sections 2.4 and 9 item 2). Such a save is always of both parts.
    first: bool,
}

impl Saved {
    /// The `parts` of `chunk`, encoded.
    fn new(position: ChunkPos, tick: u64, epoch: u64, chunk: Chunk, parts: Parts) -> Self {
        let blocks = parts.blocks.then(|| {
            ChunkFile {
                position,
                tick,
                epoch,
                chunk: chunk.clone(),
            }
            .encode()
        });
        let entities = parts.entities.then(|| {
            (!chunk.entities().is_empty()).then(|| {
                EntitiesFile {
                    position,
                    tick,
                    epoch,
                    entities: chunk.entities().to_vec(),
                }
                .encode()
            })
        });
        Self {
            chunk,
            parts,
            blocks,
            entities,
            noted: false,
            first: false,
        }
    }
}

/// What a round puts in place, each file with its contents, and what it removes.
type RoundFiles<'a> = (Vec<(PathBuf, &'a [u8])>, Vec<PathBuf>);

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
    /// Puts each of the contents in the file it is paired with, and removes the files
    /// of `removed` once those are in place. They are durably so only if this returns
    /// without an error; `self` says what a failure left.
    fn put(
        &mut self,
        disk: &dyn Disk,
        files: &[(PathBuf, &[u8])],
        removed: &[PathBuf],
    ) -> io::Result<()> {
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
        // A chunk's entities' file is removed where its entities are all gone. A crash
        // that keeps it keeps entities the log has the end of, which a restore replays.
        // Where there is none, there is nothing to make durable, and perhaps no
        // directory to sync.
        for path in removed {
            if disk.exists(path)? {
                disk.remove(path)?;
                directories.insert(parent(path).to_owned());
            }
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
            meta: None,
            handed: [0; 3],
        }
    }

    /// Keeps the `generator-version` of the world's `meta` up with what is stored.
    pub(crate) fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }

    pub(crate) fn chunk_path(&self, chunk: LevelChunk) -> PathBuf {
        let position = chunk.position;
        self.root
            .join("chunks")
            .join(directory(chunk.dimension))
            .join(format!("{}.{}", position.x >> 5, position.z >> 5))
            .join(format!("{}.{}.chunk", position.x, position.z))
    }

    /// Whether a save or note of `position` would put the chunk's first blocks file in
    /// place: none is on disk and none is pending, or the pending one is a first-timer's.
    /// One look at the disk for each new pending position.
    fn first_timer(&self, position: LevelChunk) -> Result<bool, StoreError> {
        if let Some(pending) = self.pending.get(&position) {
            return Ok(pending.first);
        }
        Ok(!self.disk.exists(&self.chunk_path(position))?)
    }

    /// Where the entities of the chunk at `chunk` are kept, beside its file.
    pub(crate) fn entities_path(&self, chunk: LevelChunk) -> PathBuf {
        let position = chunk.position;
        self.chunk_path(chunk)
            .with_file_name(format!("{}.{}.entities", position.x, position.z))
    }
}

impl Chunks for FileChunks {
    /// Without reading the file: a store that starts asks it of the chunks around the
    /// spawn.
    fn has(&mut self, position: LevelChunk) -> Result<bool, StoreError> {
        Ok(self.pending.contains_key(&position) || self.disk.exists(&self.chunk_path(position))?)
    }

    fn load(&mut self, chunk: LevelChunk) -> Result<Option<Chunk>, StoreError> {
        let position = chunk.position;
        // What was saved is what is loaded from then on, also while no file has it.
        if let Some(saved) = self.pending.get(&chunk) {
            return Ok(Some(saved.chunk.clone()));
        }
        let Some(bytes) = self.disk.read(&self.chunk_path(chunk))? else {
            return Ok(None);
        };
        let (mut file, unfit) = ChunkFile::decode(&bytes)?;
        if file.position != position {
            // A file that was moved or copied by hand. Its blocks are another chunk's,
            // however well its checksum fits.
            return Err(FormatError::Corrupt("the file is of another chunk").into());
        }
        // Read only beside the blocks: a chunk that is not stored is made by the
        // generator, and the commits since the last checkpoint, which have every entity
        // change since, are put into it at a restore.
        if let Some(bytes) = self.disk.read(&self.entities_path(chunk))? {
            let entities = EntitiesFile::decode(&bytes)?;
            if entities.position != position {
                return Err(FormatError::Corrupt("the entities' file is of another chunk").into());
            }
            file.chunk.set_entities(entities.entities);
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

    fn save_parts(
        &mut self,
        position: LevelChunk,
        tick: u64,
        epoch: u64,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError> {
        // Only noted. Written by itself a chunk costs two syncs, one after the other,
        // and a checkpoint saves a great many; the next sync writes all of them in a
        // round whose syncs wait together. A later save of the chunk takes the place
        // of this one, with the parts of both, from the later chunk: a pending save of
        // the blocks followed by one of the entities writes both. A pending note is a
        // chunk that is not stored, whose blocks are written whatever is saved of it.
        let first = self.first_timer(position)?;
        let parts = match self.pending.get(&position) {
            Some(pending) => pending.parts.with(parts),
            None => parts,
        };
        if parts.is_none() {
            return Ok(());
        }
        // A blocks file means taken: none is put in place without the entities beside
        // it, so a first-timer is written whole, an empty list removing a stale file.
        let parts = if first { Parts::ALL } else { parts };
        // A save merged into a note drops the note's standing: after a failed sync every
        // handle that saved is lost, and its region restored from the log (the plan,
        // section 9 item 7).
        let mut saved = Saved::new(position.position, tick, epoch, chunk.clone(), parts);
        saved.first = first;
        self.pending.insert(position, saved);
        Ok(())
    }

    fn note(
        &mut self,
        position: LevelChunk,
        chunk: &Chunk,
        parts: Parts,
    ) -> Result<(), StoreError> {
        // A save that is pending is the chunk with a change in it, and a note that is
        // pending is this very chunk, by purity.
        if self.pending.contains_key(&position) {
            return Ok(());
        }
        // A first-timer's note is of both parts: its entities as the generator made
        // them, or none, which removes a file a crash left without its blocks (the
        // plan, section 9 item 6). A remade chunk's is of its blocks: its stored
        // entities' file stays.
        let first = self.first_timer(position)?;
        let parts = if first { Parts::ALL } else { parts };
        let mut saved = Saved::new(position.position, 0, NO_REGION, chunk.clone(), parts);
        saved.noted = true;
        saved.first = first;
        self.pending.insert(position, saved);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        if let Some(meta) = &mut self.meta
            && meta
                .versions
                .iter()
                .zip(&self.handed)
                .any(|(named, handed)| named < handed)
        {
            // Before any file that may hold a chunk of the newer version is put in
            // place, so that no file of the world holds a chunk of a version above
            // the one `meta` names (ADR-0021, section 6). Until then an older server
            // can still open the world.
            let mut raised = meta.versions;
            for (named, handed) in raised.iter_mut().zip(self.handed) {
                *named = (*named).max(handed);
            }
            if let Err(error) = meta.raise(self.disk.as_ref(), raised) {
                // As a round that failed: the saves are made again by whoever saved
                // them, and the notes by the next sync.
                self.pending.retain(|_, saved| saved.noted);
                return Err(error.into());
            }
        }
        // Two rounds where a first blocks file is to be put in place: the first-timers'
        // entities' files, durable with their directories, then everything else, so
        // that a blocks file always has its entities beside it and so means taken
        // (`docs/groundwork/n7l-plan.md`, section 2.4 item 2). An entities' file the
        // first round leaves without its blocks file is never read, and the next note
        // of the chunk writes it again.
        let mut rounds: [RoundFiles<'_>; 2] = Default::default();
        for (position, saved) in &self.pending {
            if let Some(blocks) = &saved.blocks {
                rounds[1]
                    .0
                    .push((self.chunk_path(*position), blocks.as_slice()));
            }
            let round = usize::from(!saved.first);
            match &saved.entities {
                Some(Some(entities)) => {
                    rounds[round]
                        .0
                        .push((self.entities_path(*position), entities.as_slice()));
                }
                Some(None) => rounds[round].1.push(self.entities_path(*position)),
                None => {}
            }
        }
        let mut put = Ok(());
        let mut round = Round::default();
        for (files, removed) in &rounds {
            if files.is_empty() && removed.is_empty() {
                continue;
            }
            round = Round::default();
            put = round.put(self.disk.as_ref(), files, removed);
            if put.is_err() {
                break;
            }
        }
        if let Err(error) = put {
            round.withdraw(self.disk.as_ref());
            // After a failed sync, what it was to make durable may be lost although a
            // later one succeeds: whoever saved these chunks is told, and saves them
            // again. Notes have nobody to save them again, and are kept to be written
            // by the next sync (ADR-0021, S5). The directories are forgotten.
            self.pending.retain(|_, saved| saved.noted);
            // Nothing that was put in place is taken back: such a file is whole and
            // names nothing else, so it is right whether or not a crash keeps it.
            return Err(error.into());
        }
        self.pending.clear();
        Ok(())
    }

    fn pending(&self) -> usize {
        self.pending.values().filter(|saved| !saved.noted).count()
    }

    fn noted(&self) -> usize {
        self.pending.values().filter(|saved| saved.noted).count()
    }

    fn handed(&mut self, dimension: Dimension, version: u16) {
        let handed = &mut self.handed[usize::from(dimension.number())];
        *handed = (*handed).max(version);
    }
}

/// What the thread for chunks is given to do.
pub(crate) enum Job {
    /// Answered with the chunk, or with [`StoreReply::Unreadable`]; one that is not
    /// stored is answered [`StoreReply::Generating`] first.
    Load {
        position: LevelChunk,
        peer: Arc<Peer>,
    },
    /// The chunk is what is loaded from now on. It is written and made durable with
    /// the next checkpoint, return, flush or restoring of whatever handle, or with this
    /// save if it leaves [`PENDING_LIMIT`] chunks to be written.
    Save {
        position: LevelChunk,
        tick: u64,
        /// The epoch the handle opened its region with.
        epoch: u64,
        chunk: Chunk,
        /// The parts of the chunk to write.
        parts: Parts,
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
        chunks: Vec<LevelChunk>,
        peer: Arc<Peer>,
    },
    /// Applies the block changes of the commits a region is restored with to the stored
    /// chunks, makes the saves so far durable, and then hands the opened region to
    /// whoever asked for it; or the error, if either could not be done.
    Restore {
        /// The region's dimension, whose chunks the changes are of.
        dimension: Dimension,
        changes: Vec<Replayed>,
        tick: u64,
        /// The epoch the region is opened with.
        epoch: u64,
        peer: Arc<Peer>,
        opened: Opened,
        restored: Restored,
        answer: Sender<Result<(Opened, Restored), StoreError>>,
    },
    /// Applies block changes, each list of changes to the chunks of its dimension, to
    /// the stored chunks, and makes them durable.
    Fold {
        changes: Vec<(Dimension, Vec<Replayed>)>,
        done: Sender<Result<BTreeSet<LevelChunk>, StoreError>>,
    },
    /// Everything before it is done, of whatever handle, and what there was to say of
    /// it to the commit thread is said: tells that thread so, through `reply_to`,
    /// which answers whoever waits for the store to be at rest. With `sync`, what is
    /// held to be written is made durable first ([`crate::Store::close`]).
    Barrier {
        reply_to: Sender<Message>,
        answer: Sender<Result<(), StoreError>>,
        sync: bool,
    },
    /// Something the thread waits for may have come: an answer of the generator, or
    /// the loss of a handle whose loads are under way. Nothing to do but look.
    Wake,
    /// The base of a border copy (`docs/adr/0025-effects-between-regions.md`,
    /// section 3): the chunk as its holder would load it, with the holder's commits
    /// listed replayed into it in memory, handed back to the commit thread for the
    /// copier's stream with [`Message::Copied`]. Nothing is stored or noted, and no
    /// job of the copier's waits for it.
    Copy {
        position: LevelChunk,
        /// The holder's commits in the log, in its order.
        listed: Vec<Listed>,
        /// The tick the holder holds the chunk from, whose commits after it are
        /// replayed; none where nobody holds it.
        from: Option<u64>,
        sequence: u64,
        last_change: u64,
        peer: Arc<Peer>,
    },
    /// Has the generator make ahead the chunks within `radius` of `around` that are not
    /// stored, nearest first: the spawn's, when the store starts (T8f of
    /// `docs/groundwork/t8-plan.md`). Nobody is answered.
    Prepare { around: LevelChunk, radius: i32 },
    /// The commit thread has ended: nothing will be given any more.
    End,
}

/// Where a commit is in the log, for the base of a copy.
#[derive(Debug, Clone)]
pub(crate) struct Listed {
    pub(crate) path: PathBuf,
    pub(crate) offset: u64,
    pub(crate) length: usize,
}

/// The commit thread's way to the thread for chunks. When it goes, that thread is told
/// that nothing will be given any more: the thread holds a way in of its own, for the
/// generator's answers, so it cannot wait for the last one to go as it once did.
pub(crate) struct Jobs(Sender<Job>);

impl Jobs {
    pub(crate) fn new(jobs: Sender<Job>) -> Self {
        Self(jobs)
    }

    /// Gives the thread `job`. False if it has ended, which it does only once nothing
    /// can give it anything, or if the world could not be read.
    pub(crate) fn send(&self, job: Job) -> bool {
        self.0.send(job).is_ok()
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        // The thread has ended already if the world could not be read.
        let _ = self.0.send(Job::End);
    }
}

/// How many saved chunks wait to be written at most: the save that makes them so many
/// has them written. A region saves a chunk also when its last ticket goes, and
/// nothing else writes it before the next checkpoint, which may be an interval away,
/// or the next return. Without a limit, what an interval changed would be held in
/// memory.
const PENDING_LIMIT: usize = 128;

/// How many notes wait to be written at most, apart from the saves: a first view is
/// 329 chunks, and a join should not make the thread sync in the middle of it
/// (`docs/adr/0021-generation-is-a-function.md`, section 6, S2).
pub(crate) const NOTE_LIMIT: usize = 512;

/// The generator's answers that have come and that the thread has not looked at, by
/// the number of the request. `None` where the generator let go of a request without
/// answering it.
type Arrivals = Arc<Mutex<Arrived>>;

type Arrived = Vec<(u64, Option<Result<Chunk, Failed>>)>;

/// Hands the generator's answer to a request to the thread for chunks, on whatever
/// thread the generator answers, and wakes it. Says so also if the generator lets go
/// of the request without an answer, so that a load never waits for nothing.
struct Answer {
    request: u64,
    arrivals: Arrivals,
    wake: Sender<Job>,
    told: bool,
}

impl Answer {
    fn tell(&mut self, made: Option<Result<Chunk, Failed>>) {
        self.told = true;
        lock(&self.arrivals).push((self.request, made));
        // The thread has gone if the store has ended, and nobody waits for this then.
        let _ = self.wake.send(Job::Wake);
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        if !self.told {
            self.tell(None);
        }
    }
}

fn lock(arrivals: &Arrivals) -> MutexGuard<'_, Arrived> {
    // What is in it is whole whenever the lock is let go of.
    arrivals.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A job the thread was given and has not done.
enum Entry {
    /// Not begun: it waits for what it waits for (ADR-0021, section 7, the table).
    Given(Job),
    /// A load whose chunk the generator is making.
    Loading(Loading),
    /// A restore that waits for the generator to make chunks its changes lie in.
    Restoring(Restoring),
    /// The base of a copy whose chunk the generator is making.
    Copying(Copying),
}

struct Copying {
    position: LevelChunk,
    changes: Vec<Replayed>,
    sequence: u64,
    last_change: u64,
    peer: Arc<Peer>,
    request: u64,
    _wanted: Wanted,
    /// As for a load: the stored chunk, where a newer generator makes it again.
    stored: Option<Chunk>,
}

struct Loading {
    position: LevelChunk,
    peer: Arc<Peer>,
    request: u64,
    /// Dropped, it tells the generator that nobody waits for the chunk any more.
    _wanted: Wanted,
    /// The chunk that is stored, where a newer generator makes it again because the
    /// older one failed in it (ADR-0021, section 6, S8): answered if the newer fails.
    stored: Option<Chunk>,
}

struct Restoring {
    dimension: Dimension,
    changes: Vec<Replayed>,
    tick: u64,
    epoch: u64,
    peer: Arc<Peer>,
    opened: Opened,
    restored: Restored,
    answer: Sender<Result<(Opened, Restored), StoreError>>,
    /// The chunks the changes lie in, and how many changes each has.
    touched: BTreeMap<LevelChunk, usize>,
    /// The chunks the changes are applied to, as they are: stored, or made.
    bases: BTreeMap<LevelChunk, Chunk>,
    /// The chunks that are being made, by the number of their request.
    waiting: BTreeMap<u64, (LevelChunk, Wanted)>,
}

impl Entry {
    /// Whether this is what waits for the answer to `request`.
    fn waits_for(&self, request: u64) -> bool {
        match self {
            Entry::Given(_) => false,
            Entry::Loading(loading) => loading.request == request,
            Entry::Restoring(restoring) => restoring.waiting.contains_key(&request),
            Entry::Copying(copying) => copying.request == request,
        }
    }

    fn under_way(&self) -> bool {
        !matches!(self, Entry::Given(_))
    }
}

/// What the jobs before one in the queue that are not done hold back
/// (ADR-0021, section 7, the table): every job waits for the earlier ones that name a
/// chunk it names; a save, a checkpoint, a flush and a return wait for every earlier
/// job of their handle as well; a restore, a fold and a barrier wait for everything
/// before them; and everything waits for a fold. Nothing waits for a barrier, and a
/// job waits for a restore only for the chunks the restore changes.
#[derive(Default)]
struct Before {
    any: bool,
    fold: bool,
    /// The handles of the jobs before, by their number.
    handles: BTreeSet<u64>,
    /// The chunks the jobs before name.
    named: BTreeSet<LevelChunk>,
}

impl Before {
    fn add(&mut self, entry: &Entry) {
        let (peer, named): (&Peer, Vec<LevelChunk>) = match entry {
            Entry::Given(Job::Barrier { .. } | Job::Wake | Job::End | Job::Prepare { .. }) => {
                return;
            }
            Entry::Given(Job::Fold { .. }) => {
                self.any = true;
                self.fold = true;
                return;
            }
            // A copy holds back what names its chunk, and nothing of its handle: a
            // region's saves and checkpoints do not wait for a base being made.
            Entry::Given(Job::Copy { position, .. }) | Entry::Copying(Copying { position, .. }) => {
                self.any = true;
                self.named.insert(*position);
                return;
            }
            Entry::Given(Job::Load { position, peer } | Job::Save { position, peer, .. }) => {
                (peer, vec![*position])
            }
            Entry::Loading(loading) => (&loading.peer, vec![loading.position]),
            Entry::Given(Job::Checkpoint { peer, .. } | Job::Flush { peer }) => (peer, Vec::new()),
            Entry::Given(Job::Return { chunks, peer, .. }) => (peer, chunks.clone()),
            Entry::Given(Job::Restore {
                dimension,
                changes,
                peer,
                ..
            }) => (peer, touched(of(*dimension, changes)).into_keys().collect()),
            Entry::Restoring(restoring) => {
                (&restoring.peer, restoring.touched.keys().copied().collect())
            }
        };
        self.any = true;
        self.handles.insert(peer.session.number);
        self.named.extend(named);
    }

    fn holds_back(&self, job: &Job) -> bool {
        if self.fold {
            return true;
        }
        let of = |peer: &Peer| self.handles.contains(&peer.session.number);
        match job {
            Job::Load { position, .. } | Job::Copy { position, .. } => {
                self.named.contains(position)
            }
            Job::Save { position, peer, .. } => of(peer) || self.named.contains(position),
            Job::Checkpoint { peer, .. } | Job::Flush { peer } => of(peer),
            Job::Return { chunks, peer, .. } => {
                of(peer) || chunks.iter().any(|chunk| self.named.contains(chunk))
            }
            Job::Restore { .. } | Job::Fold { .. } | Job::Barrier { .. } => self.any,
            Job::Wake | Job::End | Job::Prepare { .. } => false,
        }
    }
}

/// The thread for chunks.
///
/// It keeps what it is given in the order it came, and does a job once the jobs it
/// waits for are done, the oldest first (ADR-0021, section 7). Only the making of a
/// chunk that is not stored happens elsewhere: a load or a restore that waits for the
/// generator does not keep the thread, and holds back only what waits for it.
pub(crate) struct ChunkService {
    chunks: Box<dyn Chunks>,
    disk: Arc<dyn Disk>,
    /// Where state files are written.
    regions: PathBuf,
    /// The handles that saved chunks since they were last made durable. A failure to
    /// make them durable loses all of them. Held weakly, so that a handle that has gone
    /// is not kept from closing: its saves are in the log until a checkpoint of its own,
    /// which would keep it here.
    saved: Vec<Weak<Peer>>,
    /// How many state files have been written, which names them apart.
    states: u64,
    /// What was given and is not done, in the order it was given.
    queue: VecDeque<Entry>,
    /// How many requests the generator has been given, which numbers them.
    requests: u64,
    arrivals: Arrivals,
    /// For the generator's answers to wake the thread with.
    wake: Sender<Job>,
    /// Each dimension's generator's version, in the order of the dimensions' numbers: a
    /// stored chunk that an older one made with a failure is made again (ADR-0021,
    /// section 6, S8).
    versions: [u16; 3],
    /// Last, so that they are dropped last: a test that holds a generator learns from
    /// that that the thread has ended.
    generators: Generators,
}

impl ChunkService {
    pub(crate) fn new(
        chunks: Box<dyn Chunks>,
        generators: Generators,
        disk: Arc<dyn Disk>,
        regions: PathBuf,
        wake: Sender<Job>,
    ) -> Self {
        Self {
            chunks,
            disk,
            regions,
            saved: Vec::new(),
            states: 0,
            queue: VecDeque::new(),
            requests: 0,
            arrivals: Arrivals::default(),
            wake,
            versions: generators.versions(),
            generators,
        }
    }

    /// Runs until nothing will be given any more and nothing waits for the generator.
    /// What waits for it holds its handle, and with that the commit thread, so the
    /// second comes before the first in practice.
    pub(crate) fn run(mut self, jobs: Receiver<Job>) {
        let mut ending = false;
        while !ending || self.queue.iter().any(Entry::under_way) {
            // The thread holds a way in itself, so this does not fail.
            let Ok(job) = jobs.recv() else {
                break;
            };
            match job {
                Job::Wake => {}
                Job::End => ending = true,
                Job::Prepare { around, radius } => self.prepare(around, radius),
                job => self.give(job),
            }
            self.advance();
        }
    }

    /// See [`Job::Prepare`]. A chunk that is stored is read from its file when it is
    /// asked for, so making it would only keep the generator from what is asked; one
    /// whose file cannot be looked at is left to the load that asks for it.
    fn prepare(&mut self, around: LevelChunk, radius: i32) {
        let LevelChunk {
            dimension,
            position: around,
        } = around;
        let mut area: Vec<ChunkPos> = (-radius..=radius)
            .flat_map(|x| (-radius..=radius).map(move |z| (x, z)))
            .map(|(x, z)| ChunkPos::new(around.x + x, around.z + z))
            .collect();
        let distance = |chunk: &ChunkPos| {
            let (x, z) = (i64::from(chunk.x - around.x), i64::from(chunk.z - around.z));
            (x * x + z * z, chunk.x, chunk.z)
        };
        area.sort_by_key(distance);
        area.retain(|position| {
            let chunk = LevelChunk::new(dimension, *position);
            matches!(self.chunks.has(chunk), Ok(false))
        });
        info!(
            dimension = dimension.type_name(),
            x = around.x,
            z = around.z,
            chunks = area.len(),
            "the chunks around the spawn that are not stored are made ahead"
        );
        self.generators.of(dimension).prepare(&area);
    }

    fn give(&mut self, job: Job) {
        if let Job::Return { chunks, peer, .. } = &job {
            self.withdraw(peer, chunks);
        }
        self.queue.push_back(Entry::Given(job));
    }

    /// A region that gives chunks back does not want what it asked for of them and has
    /// not been answered: each such load is taken out and answered
    /// [`StoreReply::Withdrawn`], and what the generator was making for it is let go of
    /// (ADR-0021, section 7, rule 5). Nothing is noted for it. A load that has not
    /// begun is taken out as well, or it would be begun behind the one taken out.
    fn withdraw(&mut self, peer: &Peer, chunks: &[LevelChunk]) {
        let mut withdrawn = Vec::new();
        self.queue.retain(|entry| {
            let (position, of) = match entry {
                Entry::Loading(loading) => (loading.position, &loading.peer),
                Entry::Given(Job::Load { position, peer }) => (*position, peer),
                _ => return true,
            };
            if of.session != peer.session || !chunks.contains(&position) {
                return true;
            }
            withdrawn.push(position);
            false
        });
        for chunk in withdrawn {
            peer.answer(StoreReply::Withdrawn {
                position: chunk.position,
            });
        }
    }

    /// The loads of a handle that is lost are taken out, and what the generator was
    /// making for them let go of: nobody listens to the answer, and nothing is noted
    /// (ADR-0021, section 7, rule 4).
    fn take_out_lost(&mut self) {
        self.queue.retain(|entry| match entry {
            Entry::Loading(loading) => !loading.peer.is_lost(),
            Entry::Copying(copying) => !copying.peer.is_lost(),
            Entry::Given(Job::Load { peer, .. } | Job::Copy { peer, .. }) => !peer.is_lost(),
            _ => true,
        });
    }

    /// Does every job that can be done, the oldest first, and looks at every answer of
    /// the generator that has come.
    fn advance(&mut self) {
        loop {
            self.take_out_lost();
            self.take_arrivals();
            let Some(index) = self.next() else {
                return;
            };
            let Some(Entry::Given(job)) = self.queue.remove(index) else {
                unreachable!("only a job that has not begun is ready to be done");
            };
            self.begin(index, job);
        }
    }

    /// Where the oldest job is that waits for nothing.
    fn next(&self) -> Option<usize> {
        let mut before = Before::default();
        for (index, entry) in self.queue.iter().enumerate() {
            if let Entry::Given(job) = entry
                && !before.holds_back(job)
            {
                return Some(index);
            }
            before.add(entry);
        }
        None
    }

    /// Does `job`, which was at `index`. What waits for the generator is put back
    /// there, so that what was given after it still waits for it.
    fn begin(&mut self, index: usize, job: Job) {
        match job {
            Job::Load { position, peer } => self.load(index, position, peer),
            Job::Copy {
                position,
                listed,
                from,
                sequence,
                last_change,
                peer,
            } => {
                let copying = Copying {
                    position,
                    changes: Vec::new(),
                    sequence,
                    last_change,
                    peer,
                    request: 0,
                    _wanted: Wanted::nothing(),
                    stored: None,
                };
                self.copy(index, copying, &listed, from);
            }
            Job::Restore {
                dimension,
                changes,
                tick,
                epoch,
                peer,
                opened,
                restored,
                answer,
            } => {
                let touched = touched(of(dimension, &changes));
                let mut bases = BTreeMap::new();
                let mut waiting = BTreeMap::new();
                for (position, count) in &touched {
                    match self.chunks.load(*position) {
                        Ok(Some(chunk)) => {
                            bases.insert(*position, chunk);
                        }
                        Ok(None) => {
                            let (request, wanted) = self.request(*position);
                            waiting.insert(request, (*position, wanted));
                        }
                        // It is answered as unreadable when it is loaded, and stays as
                        // it is: not generated, not saved.
                        Err(error) => lost(*position, *count, &error),
                    }
                }
                let restoring = Restoring {
                    dimension,
                    changes,
                    tick,
                    epoch,
                    peer,
                    opened,
                    restored,
                    answer,
                    touched,
                    bases,
                    waiting,
                };
                if restoring.waiting.is_empty() {
                    self.restore(restoring);
                } else {
                    // Without keeping the thread: what does not wait for it is done
                    // meanwhile. An answer that came before the request returned is
                    // looked at before anything else is begun.
                    self.queue.insert(index, Entry::Restoring(restoring));
                }
            }
            job => self.work(job),
        }
    }

    /// Looks a load up (ADR-0021, section 7, rule 2).
    fn load(&mut self, index: usize, level: LevelChunk, peer: Arc<Peer>) {
        // Nobody listens to what a lost handle asked for.
        if peer.is_lost() {
            return;
        }
        let position = level.position;
        let stored = match self.chunks.load(level) {
            Ok(Some(chunk)) if !self.remakes(level.dimension, &chunk) => {
                peer.answer(StoreReply::Loaded { position, chunk });
                return;
            }
            Ok(stored) => stored,
            // Generating the chunk instead would look fine at first and then overwrite
            // what players built once it is saved. The region leaves a hole in the
            // world that someone can look into.
            Err(error) => {
                error!(?position, %error, "a stored chunk cannot be read");
                peer.answer(StoreReply::Unreadable { position });
                return;
            }
        };
        // Before the generator is asked, which may answer before it returns.
        peer.answer(StoreReply::Generating { position });
        let (request, wanted) = self.request(level);
        let loading = Loading {
            position: level,
            peer,
            request,
            _wanted: wanted,
            stored,
        };
        self.queue.insert(index, Entry::Loading(loading));
    }

    /// Makes the base of a copy, as a load of its holder would find the chunk, or has
    /// the generator make the chunk first. A stored chunk that cannot be read, or a
    /// commit that cannot, makes the copy absent.
    fn copy(&mut self, index: usize, mut copying: Copying, listed: &[Listed], from: Option<u64>) {
        if copying.peer.is_lost() {
            return;
        }
        let position = copying.position;
        // The holder's commits are of its dimension, which is the chunk's.
        let changes = match from {
            Some(from) => self.listed_changes(listed, position.position, from),
            None => Ok(Vec::new()),
        };
        copying.changes = match changes {
            Ok(changes) => changes,
            Err(error) => {
                error!(?position, %error, "the commits for the base of a copy cannot be read");
                return copied(&copying, None);
            }
        };
        copying.stored = match self.chunks.load(position) {
            Ok(Some(chunk)) if !self.remakes(position.dimension, &chunk) => {
                return copied(&copying, Some(chunk));
            }
            Ok(stored) => stored,
            Err(error) => {
                error!(?position, %error, "a stored chunk cannot be read for a copy");
                return copied(&copying, None);
            }
        };
        let (request, wanted) = self.request(position);
        copying.request = request;
        copying._wanted = wanted;
        self.queue.insert(index, Entry::Copying(copying));
    }

    /// The changes to `position` of the commits listed, made after the tick `from`. A
    /// segment that is gone was collected after a checkpoint of the holder, whose saves
    /// were durable before it and so are in the stored chunk read after this.
    fn listed_changes(
        &self,
        listed: &[Listed],
        position: ChunkPos,
        from: u64,
    ) -> Result<Vec<Replayed>, StoreError> {
        let mut changes = Vec::new();
        for Listed {
            path,
            offset,
            length,
        } in listed
        {
            let bytes = match self.disk.read_at(path, *offset, *length) {
                Ok(bytes) => bytes,
                Err(_) if !self.disk.exists(path)? => continue,
                Err(error) => return Err(error.into()),
            };
            let (records, _) = read_log(&bytes)?;
            let Some((tick, replayed, _)) = records.into_iter().next().and_then(commit_changes)
            else {
                continue;
            };
            if tick > from {
                changes.extend(
                    replayed
                        .into_iter()
                        .filter(|change| change.chunk() == position),
                );
            }
        }
        Ok(changes)
    }

    /// Whether a stored chunk's blocks count as not stored: an older generator failed in
    /// it, and nobody changed its blocks since, which would have taken the mark (S8).
    /// Its stored entities are kept whatever is made (`docs/groundwork/n7l-plan.md`,
    /// section 2.4 item 4).
    fn remakes(&self, dimension: Dimension, stored: &Chunk) -> bool {
        stored.with_failure() && stored.generated() < self.versions[usize::from(dimension.number())]
    }

    /// Asks the generator of its dimension for the chunk at `chunk`. Its answer comes
    /// among the arrivals, by the number returned.
    fn request(&mut self, chunk: LevelChunk) -> (u64, Wanted) {
        self.requests += 1;
        let request = self.requests;
        let mut answer = Answer {
            request,
            arrivals: Arc::clone(&self.arrivals),
            wake: self.wake.clone(),
            told: false,
        };
        let wanted = self.generators.of(chunk.dimension).request(
            chunk.position,
            Box::new(move |made| answer.tell(Some(made))),
        );
        (request, wanted)
    }

    /// Answers what waited for the generator's answers that have come.
    fn take_arrivals(&mut self) {
        let arrived = std::mem::take(&mut *lock(&self.arrivals));
        for (request, made) in arrived {
            // None, if what waited for it was taken out since.
            let Some(index) = self.queue.iter().position(|entry| entry.waits_for(request)) else {
                continue;
            };
            match self.queue.remove(index) {
                Some(Entry::Loading(loading)) => self.loaded(loading, made),
                Some(Entry::Copying(copying)) => match made {
                    Some(Ok(mut chunk)) => {
                        // A remade chunk's copy shows its stored entities, as its
                        // holder's load would.
                        if let Some(stored) = &copying.stored {
                            chunk.set_entities(stored.entities().to_vec());
                        }
                        let dimension = copying.position.dimension;
                        self.chunks.handed(dimension, chunk.generated());
                        copied(&copying, Some(chunk));
                    }
                    // As a load: a newer generator that fails leaves the older one's.
                    _ => {
                        let stored = copying.stored.clone();
                        copied(&copying, stored);
                    }
                },
                Some(Entry::Restoring(mut restoring)) => {
                    let (position, _wanted) = restoring
                        .waiting
                        .remove(&request)
                        .expect("the restore waits for this request");
                    match made {
                        Some(Ok(chunk)) => {
                            self.chunks.handed(position.dimension, chunk.generated());
                            restoring.bases.insert(position, chunk);
                        }
                        Some(Err(Failed)) => {
                            lost(position, restoring.touched[&position], &"it failed");
                        }
                        None => {
                            let why = "the generator let go of it";
                            lost(position, restoring.touched[&position], &why);
                        }
                    }
                    if restoring.waiting.is_empty() {
                        self.restore(restoring);
                    } else {
                        self.queue.insert(index, Entry::Restoring(restoring));
                    }
                }
                Some(Entry::Given(_)) | None => {
                    unreachable!("only what is under way waits for the generator")
                }
            }
        }
    }

    /// Answers a load that waited for the generator (ADR-0021, section 7, rule 3).
    fn loaded(&mut self, loading: Loading, made: Option<Result<Chunk, Failed>>) {
        let Loading {
            position: level,
            peer,
            stored,
            ..
        } = loading;
        let position = level.position;
        // It was not taken out yet only because the thread had not looked.
        if peer.is_lost() {
            return;
        }
        match made {
            Some(Ok(mut chunk)) => {
                // Made again over a stored chunk (S8): its new blocks with the stored
                // entities, which were taken long ago and are the world's now
                // (`docs/groundwork/n7l-plan.md`, section 2.4 item 4); only its blocks
                // are noted.
                let parts = match &stored {
                    Some(stored) => {
                        chunk.set_entities(stored.entities().to_vec());
                        Parts::BLOCKS
                    }
                    None => Parts::ALL,
                };
                // The answer waits for nothing; the note comes behind it. Nothing that
                // names the chunk was done since it was looked up, so what is stored
                // there is what was then (ADR-0021, section 7, "S1 to S8").
                peer.answer(StoreReply::Loaded {
                    position,
                    chunk: chunk.clone(),
                });
                self.chunks.handed(level.dimension, chunk.generated());
                // Under S8 in the old chunk's place.
                self.note(level, &chunk, parts);
            }
            made => {
                if made.is_none() {
                    error!(
                        ?position,
                        "the generator let go of a chunk without making it"
                    );
                }
                match stored {
                    // A newer generator that fails where an older one failed leaves
                    // the older one's chunk as it was.
                    Some(chunk) => peer.answer(StoreReply::Loaded { position, chunk }),
                    None => peer.answer(StoreReply::Unreadable { position }),
                }
            }
        }
    }

    /// Notes a chunk that a region was answered with, to be written by the next sync
    /// of any handle (ADR-0021, section 6, S2). It is not among any handle's saves: no
    /// handle is lost if it cannot be written, and it is tried again (S5).
    fn note(&mut self, position: LevelChunk, chunk: &Chunk, parts: Parts) {
        if let Err(error) = self.chunks.note(position, chunk, parts) {
            // It is made again when it is needed, and is the same (S3).
            warn!(?position, %error, "a generated chunk could not be noted for writing");
            return;
        }
        if self.chunks.noted() >= NOTE_LIMIT {
            // Whoever saved since the last sync is lost if this fails, as with any.
            let _ = self.sync();
        }
    }

    /// Applies a restore's changes once every chunk they lie in is there, makes them
    /// durable and hands the region over.
    fn restore(&mut self, restoring: Restoring) {
        let Restoring {
            dimension,
            changes,
            tick,
            epoch,
            peer,
            opened,
            restored,
            answer,
            bases,
            ..
        } = restoring;
        let applied = apply(
            self.chunks.as_mut(),
            bases,
            of(dimension, &changes),
            tick,
            epoch,
        )
        // In the files before the region is handed over: a world whose chunk files
        // cannot be written is then not opened, instead of being opened and lost
        // at its first checkpoint, and what an opening applied is not held in
        // memory until one.
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

    /// Applies block changes while a world is made over, and makes them durable. It
    /// keeps the thread until it is done, waiting for the generator on a way of its
    /// own: no region is open then. It answers the chunks it could neither read nor make,
    /// which it left as they were.
    fn fold(
        &mut self,
        changes: &[(Dimension, Vec<Replayed>)],
    ) -> Result<BTreeSet<LevelChunk>, StoreError> {
        let all = || {
            changes
                .iter()
                .flat_map(|(dimension, changes)| of(*dimension, changes))
        };
        let touched = touched(all());
        let mut bases = BTreeMap::new();
        let (made, arriving) = mpsc::channel();
        let mut wanted = BTreeMap::new();
        for (position, count) in &touched {
            match self.chunks.load(*position) {
                Ok(Some(chunk)) => {
                    bases.insert(*position, chunk);
                }
                Ok(None) => {
                    let made = made.clone();
                    let position = *position;
                    let request = self.generators.of(position.dimension).request(
                        position.position,
                        Box::new(move |chunk| {
                            // Nobody waits for it if the fold was given up.
                            let _ = made.send((position, chunk));
                        }),
                    );
                    wanted.insert(position, request);
                }
                Err(error) => lost(*position, *count, &error),
            }
        }
        drop(made);
        while !wanted.is_empty() {
            // Fails once the generator has let go of every request that is left.
            let Ok((position, chunk)) = arriving.recv() else {
                break;
            };
            wanted.remove(&position);
            match chunk {
                Ok(chunk) => {
                    self.chunks.handed(position.dimension, chunk.generated());
                    bases.insert(position, chunk);
                }
                Err(Failed) => lost(position, touched[&position], &"it failed"),
            }
        }
        for position in wanted.into_keys() {
            lost(position, touched[&position], &"the generator let go of it");
        }
        let unmade = touched
            .keys()
            .filter(|position| !bases.contains_key(position))
            .copied()
            .collect();
        // The tick and the epoch of a saved chunk are only for whoever looks at the
        // files, and no region saves these.
        apply(self.chunks.as_mut(), bases, all(), 0, NO_REGION)?;
        self.chunks.sync()?;
        Ok(unmade)
    }

    fn work(&mut self, job: Job) {
        match job {
            Job::Save {
                position,
                tick,
                epoch,
                chunk,
                parts,
                peer,
            } => {
                // Done even if the handle has been lost since: it was asked for before
                // that, and only once the commits before it were durable, so it holds
                // nothing that a restored region does not know of. And it comes before
                // whatever the next owner does with the chunk.
                match self.chunks.save_parts(position, tick, epoch, &chunk, parts) {
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
            Job::Fold { changes, done } => {
                let folded = self.fold(&changes);
                let _ = done.send(folded);
            }
            Job::Barrier {
                reply_to,
                answer,
                sync,
            } => {
                // Nothing is made durable for a flush of the store: saved chunks become
                // that with a checkpoint, a return or a handle's own flush, whose owner
                // is told if they do not. Nothing that passes it looks at files. A
                // close makes durable what is held, notes among it (ADR-0021, S7).
                let chunks = if sync { self.sync() } else { Ok(()) };
                let _ = reply_to.send(Message::Passed { answer, chunks });
            }
            Job::Load { .. }
            | Job::Copy { .. }
            | Job::Restore { .. }
            | Job::Wake
            | Job::End
            | Job::Prepare { .. } => {
                unreachable!("begun or handled elsewhere")
            }
        }
    }

    /// Writes the saves and notes so far and makes them durable. If that fails, every
    /// handle that saved since the last time is lost, and opening its region again
    /// saves its chunks again; the notes are kept to be written by the next sync.
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

/// What the replay of a commit puts into a stored chunk: a block it changed, the
/// chunk's scheduled ticks as the commit's tick left them, whole, that its marks were
/// taken, or its seam entries, whole (`docs/adr/0024-block-behaviour-in-a-region.md`,
/// sections 4 to 6). A commit's records come after its blocks.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entity is held whole while the store reads, files or replays it; boxing it would allocate once more for each"
)]
pub(crate) enum Replayed {
    Block(BlockPos, BlockState),
    /// The data of the block entity at the position, in its place among the blocks
    /// (`docs/adr/0026-redstone.md`, section 6).
    BlockEntity(BlockPos, Vec<u8>),
    Ticks(ChunkPos, Vec<ScheduledTick>),
    /// The chunk's marks for post-processing were taken: it was post-processed.
    MarksTaken(ChunkPos),
    /// The chunk's seam entries, whole.
    Seams(ChunkPos, Vec<Seam>),
    /// The block events the chunk keeps, whole (`docs/adr/0026-redstone.md`, section
    /// 4).
    Events(ChunkPos, Vec<KeptEvent>),
    /// The claims on the chunk's points of interest, whole, after its blocks
    /// (`docs/groundwork/n10-plan.md`, section 3.2).
    Claims(ChunkPos, Vec<Claim>),
    /// The raids the chunk keeps, whole (`docs/groundwork/n10-plan.md`, section 9.1).
    Raids(ChunkPos, Vec<clustine_world::raid::Raid>),
    /// What the commit's tick did to an entity of the chunk, after its blocks
    /// (`docs/adr/0028-entities.md`, section 3).
    Entity(ChunkPos, EntityChange),
}

impl Replayed {
    pub(crate) fn chunk(&self) -> ChunkPos {
        match self {
            Self::Block(position, _) | Self::BlockEntity(position, _) => position.chunk(),
            Self::Ticks(chunk, _)
            | Self::MarksTaken(chunk)
            | Self::Seams(chunk, _)
            | Self::Events(chunk, _)
            | Self::Claims(chunk, _)
            | Self::Raids(chunk, _)
            | Self::Entity(chunk, _) => *chunk,
        }
    }

    /// The part of its chunk it changes.
    fn part(&self) -> Parts {
        match self {
            Self::Entity(..) => Parts::ENTITIES,
            _ => Parts::BLOCKS,
        }
    }
}

/// The changes of a region of `dimension`, each with the chunk it lies in.
fn of(
    dimension: Dimension,
    changes: &[Replayed],
) -> impl Iterator<Item = (LevelChunk, &Replayed)> + Clone {
    changes
        .iter()
        .map(move |change| (LevelChunk::new(dimension, change.chunk()), change))
}

/// The chunks that replayed changes lie in, and how many changes each has.
fn touched<'a>(
    changes: impl Iterator<Item = (LevelChunk, &'a Replayed)>,
) -> BTreeMap<LevelChunk, usize> {
    let mut touched = BTreeMap::new();
    for (chunk, _) in changes {
        *touched.entry(chunk).or_insert(0) += 1;
    }
    touched
}

/// Hands the base of a copy back to the commit thread, with the changes replayed into
/// it, or none where the copy counts as absent.
fn copied(copying: &Copying, chunk: Option<Chunk>) {
    let chunk = chunk.map(|mut chunk| {
        for change in &copying.changes {
            put(&mut chunk, change);
        }
        chunk
    });
    copying.peer.send(Message::Copied {
        session: copying.peer.session,
        position: copying.position,
        sequence: copying.sequence,
        last_change: copying.last_change,
        chunk,
    });
}

/// Says that the changes to a chunk are not applied: the next checkpoint cuts them
/// from the log, and they are gone then (ADR-0021, section 7, "Restore, in full").
fn lost(position: LevelChunk, changes: usize, why: &dyn std::fmt::Display) {
    error!(
        ?position,
        changes,
        %why,
        "block changes to a chunk that cannot be read or made are not applied"
    );
}

/// Applies block changes, in order, to the chunks of `bases` they lie in, and saves
/// those chunks as of `tick`, for the region opened with `epoch`. A change to a chunk
/// that is not among them is passed over. Each chunk's save names the parts the
/// changes replayed into it (the plan, section 11): the other is as stored, and as of
/// the last checkpoint, since no change since was of it.
///
/// A chunk may have been saved with some of them in it already, or with all of them;
/// applying all of them again in order ends in the same state, because each one sets a
/// block to a definite state and the last one for a block wins, and each change of an
/// entity sets what it changes to a definite value.
fn apply<'a>(
    chunks: &mut dyn Chunks,
    bases: BTreeMap<LevelChunk, Chunk>,
    changes: impl Iterator<Item = (LevelChunk, &'a Replayed)>,
    tick: u64,
    epoch: u64,
) -> Result<(), StoreError> {
    let mut bases: BTreeMap<LevelChunk, (Chunk, Parts)> = bases
        .into_iter()
        .map(|(position, chunk)| (position, (chunk, Parts::NONE)))
        .collect();
    for (at, change) in changes {
        if let Some((chunk, parts)) = bases.get_mut(&at) {
            // A chunk not stored yet is written whole by its first save, whatever part
            // changed (`Chunks::save_parts`).
            *parts = parts.with(change.part());
            put(chunk, change);
        }
    }
    for (position, (chunk, parts)) in &bases {
        chunks.save_parts(*position, tick, epoch, chunk, *parts)?;
    }
    Ok(())
}

/// Puts one replayed change into its chunk.
fn put(chunk: &mut Chunk, change: &Replayed) {
    match change {
        Replayed::Block(position, state) => {
            let (x, z) = position.in_chunk();
            chunk.set(x, position.y, z, *state);
        }
        // Applied after the blocks before it, as the tick made it: a block of another
        // kind set later drops it, as it did then. Data the chunk refuses was refused
        // to the region's own chunk too, which kept the block entity without it.
        Replayed::BlockEntity(position, data) => {
            let _ = chunk.apply(&Change::BlockEntity(*position, data.clone()));
        }
        // The last list of a chunk wins, as the last change of a block does. A chunk
        // whose ticks changed is no longer as generated, so that remaking one whose
        // generation failed does not lose them.
        Replayed::Ticks(_, ticks) => {
            chunk.set_ticks(ticks.clone());
            chunk.clear_generated();
        }
        Replayed::MarksTaken(_) => {
            chunk.set_marks(Vec::new());
            chunk.clear_generated();
        }
        Replayed::Seams(_, seams) => {
            chunk.set_seams(seams.clone());
            chunk.clear_generated();
        }
        Replayed::Events(_, events) => {
            chunk.set_events(events.clone());
            chunk.clear_generated();
        }
        // After the commit's blocks, so that a claim on a block placed in the tick holds;
        // one whose block is not of its type is dropped by the chunk. A chunk with claims
        // is no longer as generated, so that remaking one whose generation failed does not
        // lose them (`docs/groundwork/n10-plan.md`, section 3.2).
        Replayed::Claims(_, claims) => {
            let dropped = chunk.set_claims(claims.clone());
            if dropped > 0 {
                warn!(dropped, "claims on blocks of another type were dropped");
            }
            chunk.clear_generated();
        }
        Replayed::Raids(_, raids) => {
            chunk.set_raids(raids.clone());
            chunk.clear_generated();
        }
        // After the commit's blocks. The generated mark is a statement about the blocks
        // (ADR-0022): a remake keeps the stored entities whatever it is
        // (`docs/groundwork/n7l-plan.md`, section 9 item 4).
        Replayed::Entity(_, change) => {
            chunk.apply_entity(change);
        }
    }
}

/// The thread for chunks with a generator that answers when the test says so
/// (`docs/adr/0021-generation-is-a-function.md`, sections 6 and 7). The tests of T10's
/// text are elsewhere; these are the builder's own.
#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Condvar;
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
    use std::time::Duration;

    use clustine_data::blocks;
    use clustine_rpc::{RegionHello, StoreRequest};
    use clustine_world::ChunkGenerator;
    use clustine_world::generation::{Done, GeneratorSettings};
    use clustine_worldgen::FlatGenerator;

    use super::*;
    use crate::disk::{Fault, MemoryDisk, Survival};
    use crate::regions::{claim, gap, give_back};
    use crate::tests::{any_reply, edited, generator, log};
    use crate::{Division, Store, StoreHandle, start};

    const ROOT: &str = "/world";

    /// Chunks of the eastern pinned region of [`gap`], 1, that nothing stores.
    const A: ChunkPos = ChunkPos::new(20, 0);
    const B: ChunkPos = ChunkPos::new(21, 0);
    const C: ChunkPos = ChunkPos::new(22, 0);
    /// A chunk of the western pinned region, 0.
    const WEST: ChunkPos = ChunkPos::new(-3, 0);
    /// A chunk in the gap, which a region holds once it has claimed it.
    const FREE: ChunkPos = ChunkPos::new(5, 5);

    fn made(position: ChunkPos) -> Chunk {
        FlatGenerator::classic().generate(position)
    }

    /// What the test sees of the generator.
    #[derive(Default)]
    struct Seen {
        /// The requests the test has not answered yet.
        waiting: Vec<(ChunkPos, Done)>,
        /// Every chunk asked for, in the order asked.
        entered: Vec<ChunkPos>,
        /// Every chunk whose request was dropped.
        forgotten: Vec<ChunkPos>,
        /// The chunks the generator fails for.
        failing: BTreeSet<ChunkPos>,
    }

    #[derive(Default)]
    struct Shared {
        seen: Mutex<Seen>,
        changed: Condvar,
    }

    impl Shared {
        fn seen(&self) -> MutexGuard<'_, Seen> {
            self.seen.lock().unwrap()
        }

        fn wait_until(&self, done: impl Fn(&Seen) -> bool) {
            let mut seen = self.seen();
            while !done(&seen) {
                seen = self.changed.wait(seen).unwrap();
            }
        }

        /// Waits until the generator has been asked for `position` `times` times.
        fn entered(&self, position: ChunkPos, times: usize) {
            self.wait_until(|seen| {
                seen.entered.iter().filter(|p| **p == position).count() >= times
            });
        }

        fn forgotten(&self, position: ChunkPos) {
            self.wait_until(|seen| seen.forgotten.contains(&position));
        }

        /// Answers the request for `position`, once it has come.
        fn let_go(&self, position: ChunkPos) {
            self.wait_until(|seen| seen.waiting.iter().any(|(p, _)| *p == position));
            let (done, fails) = {
                let mut seen = self.seen();
                let index = seen
                    .waiting
                    .iter()
                    .position(|(p, _)| *p == position)
                    .unwrap();
                let (_, done) = seen.waiting.remove(index);
                (done, seen.failing.contains(&position))
            };
            done(if fails {
                Err(Failed)
            } else {
                Ok(made(position))
            });
        }

        /// Lets go of every request without answering it, as a generator that is
        /// dropped does.
        fn drop_all(&self) {
            let waiting = std::mem::take(&mut self.seen().waiting);
            drop(waiting);
        }
    }

    /// A generator that makes what the flat one makes, when the test lets it.
    struct Holding {
        shared: Arc<Shared>,
        /// Dropped with the generator, for a test to learn that the thread has ended.
        _alive: Sender<()>,
    }

    impl ChunkGenerator for Holding {
        fn generate(&self, position: ChunkPos) -> Chunk {
            made(position)
        }

        fn settings(&self) -> GeneratorSettings {
            generator().settings()
        }

        fn request(&self, position: ChunkPos, done: Done) -> Wanted {
            {
                let mut seen = self.shared.seen();
                seen.entered.push(position);
                seen.waiting.push((position, done));
            }
            self.shared.changed.notify_all();
            let shared = Arc::clone(&self.shared);
            Wanted::new(move || {
                let dropped = {
                    let mut seen = shared.seen();
                    seen.forgotten.push(position);
                    let index = seen.waiting.iter().position(|(p, _)| *p == position);
                    index.map(|index| seen.waiting.remove(index))
                };
                drop(dropped);
                shared.changed.notify_all();
            })
        }
    }

    struct Rig {
        store: Store,
        disk: Arc<MemoryDisk>,
        shared: Arc<Shared>,
        /// Disconnected once the thread for chunks has dropped the generator.
        ended: Receiver<()>,
    }

    /// A store of the world with a gap on a disk in memory, with the generator that
    /// the test holds.
    fn rig() -> Rig {
        let disk = Arc::new(MemoryDisk::default());
        let shared = Arc::new(Shared::default());
        let (alive, ended) = mpsc::channel();
        let generator = Arc::new(Holding {
            shared: Arc::clone(&shared),
            _alive: alive,
        });
        let store = on(&disk, generator, &gap());
        Rig {
            store,
            disk,
            shared,
            ended,
        }
    }

    fn on(
        disk: &Arc<MemoryDisk>,
        generator: Arc<dyn ChunkGenerator>,
        division: &Division,
    ) -> Store {
        let root = Path::new(ROOT);
        let chunks = FileChunks::new(disk.clone(), root);
        start(disk.clone(), root, Box::new(chunks), generator, division).unwrap()
    }

    fn hello(region: u32, epoch: u64) -> RegionHello {
        RegionHello {
            region: clustine_region::RegionId(region),
            epoch,
        }
    }

    fn open(store: &Store, region: u32, epoch: u64) -> StoreHandle {
        store.open_region(hello(region, epoch)).unwrap().0
    }

    fn load(handle: &StoreHandle, position: ChunkPos) {
        handle.request(StoreRequest::Load { position });
    }

    fn generating(position: ChunkPos) -> StoreReply {
        StoreReply::Generating { position }
    }

    fn loaded(position: ChunkPos) -> StoreReply {
        StoreReply::Loaded {
            position,
            chunk: made(position),
        }
    }

    /// The answers in the handle's queue now, but that commits are done.
    fn queued(handle: &StoreHandle) -> Vec<StoreReply> {
        std::iter::from_fn(|| handle.try_reply())
            .filter(|reply| !matches!(reply, StoreReply::Committed { .. }))
            .collect()
    }

    /// Asks for a flush of the handle's own and waits for its answer: the answers that
    /// came before it, but that commits are done.
    fn flushed(handle: &StoreHandle) -> Vec<StoreReply> {
        handle.request(StoreRequest::Flush);
        let mut before = Vec::new();
        loop {
            match any_reply(handle) {
                StoreReply::Flushed => return before,
                StoreReply::Committed { .. } => {}
                reply => before.push(reply),
            }
        }
    }

    /// Whether a crash now would leave a file for the chunk at `position`.
    fn durable(disk: &MemoryDisk, position: ChunkPos) -> bool {
        let left: Arc<dyn Disk> = Arc::new(disk.crashed(Survival::Nothing));
        let mut chunks = FileChunks::new(left, Path::new(ROOT));
        chunks
            .load(crate::tests::overworld(position))
            .unwrap()
            .is_some()
    }

    #[test]
    fn a_load_of_another_chunk_is_answered_while_one_is_being_made() {
        let rig = rig();
        let east = open(&rig.store, 1, 1);
        load(&east, A);
        load(&east, B);
        rig.shared.entered(B, 1);
        rig.shared.let_go(B);
        assert_eq!(any_reply(&east), generating(A));
        assert_eq!(any_reply(&east), generating(B));
        assert_eq!(any_reply(&east), loaded(B));
        rig.shared.let_go(A);
        assert_eq!(any_reply(&east), loaded(A));
    }

    #[test]
    fn what_names_the_chunk_or_is_of_the_handle_waits_for_a_load_and_another_region_does_not() {
        let rig = rig();
        let east = open(&rig.store, 1, 1);
        let west = open(&rig.store, 0, 1);
        load(&east, A);
        rig.shared.entered(A, 1);
        let mut changed = made(C);
        changed.set(1, 100, 1, blocks::GLASS);
        east.request(StoreRequest::Save {
            position: C,
            tick: 1,
            chunk: changed.clone(),
            parts: clustine_rpc::Parts::ALL,
        });
        load(&east, C);
        east.request(StoreRequest::Flush);
        // The other region's save and flush are done meanwhile, and are durable.
        west.request(StoreRequest::Save {
            position: WEST,
            tick: 1,
            chunk: edited(),
            parts: clustine_rpc::Parts::ALL,
        });
        assert_eq!(flushed(&west), []);
        assert!(durable(&rig.disk, WEST));
        // The eastern region's flush was given first, and is not answered.
        assert_eq!(queued(&east), [generating(A)]);
        assert!(!durable(&rig.disk, C));

        rig.shared.let_go(A);
        let changed = StoreReply::Loaded {
            position: C,
            chunk: changed,
        };
        assert_eq!(any_reply(&east), loaded(A));
        assert_eq!(any_reply(&east), changed);
        assert_eq!(any_reply(&east), StoreReply::Flushed);
        // The save was what the load found: nothing was made for it.
        assert_eq!(rig.shared.seen().entered, [A]);
        assert!(durable(&rig.disk, C) && durable(&rig.disk, A));
    }

    #[test]
    fn a_return_withdraws_a_load_whose_chunk_is_being_made() {
        let rig = rig();
        let east = open(&rig.store, 1, 1);
        assert_eq!(claim(&east, &[FREE]).0, [FREE]);
        load(&east, FREE);
        rig.shared.entered(FREE, 1);
        give_back(&east, &[FREE]);
        rig.shared.forgotten(FREE);
        assert_eq!(
            flushed(&east),
            [generating(FREE), StoreReply::Withdrawn { position: FREE }]
        );
        assert!(!durable(&rig.disk, FREE));

        // Claimed and loaded again, it is made again and answered.
        assert_eq!(claim(&east, &[FREE]).0, [FREE]);
        load(&east, FREE);
        rig.shared.let_go(FREE);
        assert_eq!(flushed(&east), [generating(FREE), loaded(FREE)]);
        assert_eq!(rig.shared.seen().entered, [FREE, FREE]);
        assert!(durable(&rig.disk, FREE));
    }

    #[test]
    fn the_load_of_a_lost_handle_is_let_go_of_and_nothing_is_noted() {
        let rig = rig();
        let east = open(&rig.store, 1, 1);
        load(&east, A);
        rig.shared.entered(A, 1);
        let again = open(&rig.store, 1, 2);
        rig.shared.forgotten(A);
        assert!(east.is_lost());
        assert_eq!(flushed(&again), []);
        rig.store.close().unwrap();
        assert!(!durable(&rig.disk, A));
    }

    #[test]
    fn a_chunk_the_generator_fails_for_is_unreadable_and_asked_for_again() {
        let rig = rig();
        rig.shared.seen().failing.insert(A);
        let east = open(&rig.store, 1, 1);
        load(&east, A);
        rig.shared.let_go(A);
        assert_eq!(
            flushed(&east),
            [generating(A), StoreReply::Unreadable { position: A }]
        );
        assert!(!durable(&rig.disk, A));
        load(&east, A);
        rig.shared.entered(A, 2);
        rig.shared.let_go(A);
        assert_eq!(any_reply(&east), generating(A));
        assert_eq!(any_reply(&east), StoreReply::Unreadable { position: A });
    }

    #[test]
    fn a_restore_waits_for_the_chunk_it_changes_and_keeps_nobody_else() {
        let rig = rig();
        let east = open(&rig.store, 1, 1);
        let west = open(&rig.store, 0, 1);
        log(&east, 1, &[(A.x * 16 + 2, 100, 3, blocks::STONE)]);
        assert_eq!(flushed(&east), []);
        std::thread::scope(|scope| {
            let opening = scope.spawn(|| rig.store.open_region(hello(1, 2)).unwrap().0);
            rig.shared.entered(A, 1);
            // The western region is served while the restore waits.
            west.request(StoreRequest::Save {
                position: WEST,
                tick: 1,
                chunk: edited(),
                parts: clustine_rpc::Parts::ALL,
            });
            assert_eq!(flushed(&west), []);
            // The hello is answered only once the restore is through.
            assert!(!opening.is_finished());
            rig.shared.let_go(A);
            let again = opening.join().unwrap();
            load(&again, A);
            let StoreReply::Loaded { chunk, .. } = any_reply(&again) else {
                panic!("the restored chunk is not loaded");
            };
            assert_eq!(chunk.get(2, 100, 3), Some(blocks::STONE));
            assert_eq!(chunk.generated(), 0);
        });
        assert!(durable(&rig.disk, A));
        assert_eq!(rig.shared.seen().entered, [A]);
    }

    #[test]
    fn a_generated_chunk_is_written_by_the_next_sync_of_any_handle_or_by_a_close() {
        let disk = Arc::new(MemoryDisk::default());
        let store = on(&disk, generator(), &gap());
        let east = open(&store, 1, 1);
        let west = open(&store, 0, 1);
        load(&east, A);
        assert_eq!(any_reply(&east), generating(A));
        assert_eq!(any_reply(&east), loaded(A));
        // A flush of the store syncs nothing.
        store.flush().unwrap();
        assert!(!durable(&disk, A));
        // Another region's sync writes it, as the store wrote it itself.
        assert_eq!(flushed(&west), []);
        assert!(durable(&disk, A));
        let left: Arc<dyn Disk> = Arc::new(disk.crashed(Survival::Nothing));
        let file = left
            .read(
                &FileChunks::new(Arc::clone(&left), Path::new(ROOT))
                    .chunk_path(crate::tests::overworld(A)),
            )
            .unwrap()
            .unwrap();
        let (file, _) = ChunkFile::decode(&file).unwrap();
        assert_eq!((file.tick, file.epoch), (0, NO_REGION));
        assert!(file.chunk == made(A));

        load(&east, B);
        assert_eq!(any_reply(&east), generating(B));
        assert_eq!(any_reply(&east), loaded(B));
        store.close().unwrap();
        assert!(durable(&disk, B));
    }

    #[test]
    fn notes_are_written_once_512_of_them_wait() {
        let disk = Arc::new(MemoryDisk::default());
        let store = on(&disk, generator(), &gap());
        let east = open(&store, 1, 1);
        let position = |index: usize| ChunkPos::new(16 + (index % 32) as i32, (index / 32) as i32);
        let answered = |handle: &StoreHandle| loop {
            if let StoreReply::Loaded { .. } = any_reply(handle) {
                return;
            }
        };
        for index in 0..NOTE_LIMIT - 1 {
            load(&east, position(index));
            answered(&east);
        }
        // More than a handle's saves may be pending, and none of them is written yet.
        assert!(!durable(&disk, position(0)));
        load(&east, position(NOTE_LIMIT - 1));
        answered(&east);
        // With the answer comes the note, and with the last of them the sync: the
        // flush is behind it.
        store.flush().unwrap();
        assert!((0..NOTE_LIMIT).all(|index| durable(&disk, position(index))));
    }

    /// A chunk noted for the first time with entities, its sync stopped at every one of
    /// its steps and every kind of crash after: a blocks file is never found without the
    /// entities beside it, so a blocks file means taken (`docs/groundwork/n7l-plan.md`,
    /// sections 2.4 item 2 and 9 item 2).
    #[test]
    fn a_first_blocks_file_is_never_in_place_without_its_entities() {
        let mut first = made(A);
        first.set_entities(vec![crate::entities_tests::item(9, 320.5, 2.5)]);
        let steps = {
            let disk = Arc::new(MemoryDisk::default());
            let mut chunks = FileChunks::new(disk.clone(), Path::new(ROOT));
            chunks
                .note(crate::tests::overworld(A), &first, Parts::ALL)
                .unwrap();
            chunks.sync().unwrap();
            disk.operations()
        };
        assert!(steps > 4, "two rounds of a sync each, {steps}");
        for stop in 1..=steps + 1 {
            for survival in [
                Survival::Nothing,
                Survival::Torn,
                Survival::Everything,
                Survival::Untruncated,
            ] {
                let disk = Arc::new(MemoryDisk::failing(Fault::Stop(stop)));
                let mut chunks = FileChunks::new(disk.clone(), Path::new(ROOT));
                chunks
                    .note(crate::tests::overworld(A), &first, Parts::ALL)
                    .unwrap();
                let _ = chunks.sync();
                let left: Arc<dyn Disk> = Arc::new(disk.crashed(survival));
                let mut after = FileChunks::new(left, Path::new(ROOT));
                match after.load(crate::tests::overworld(A)) {
                    Ok(Some(chunk)) => assert_eq!(
                        chunk.entities(),
                        first.entities(),
                        "stopped at {stop}, {survival:?}"
                    ),
                    Ok(None) => {}
                    Err(error) => panic!("stopped at {stop}, {survival:?}: {error}"),
                }
            }
        }
    }

    /// A first-timer's save names only its entities, as a fold of an arrival into a
    /// chunk never generated does: it is written whole all the same, its blocks beside
    /// its entities; a stored chunk's save of its entities leaves its blocks file.
    #[test]
    fn a_first_save_of_entities_alone_writes_the_blocks_too() {
        let disk = Arc::new(MemoryDisk::default());
        let mut chunks = FileChunks::new(disk.clone(), Path::new(ROOT));
        let mut with = made(A);
        with.set_entities(vec![crate::entities_tests::item(9, 320.5, 2.5)]);
        chunks
            .save_parts(crate::tests::overworld(A), 1, 1, &with, Parts::ENTITIES)
            .unwrap();
        chunks.sync().unwrap();
        assert!(durable(&disk, A));
        let left: Arc<dyn Disk> = Arc::new(disk.crashed(Survival::Nothing));
        let loaded = FileChunks::new(left, Path::new(ROOT))
            .load(crate::tests::overworld(A))
            .unwrap();
        assert_eq!(loaded, Some(with.clone()));

        let before = disk.operations();
        with.set_entities(Vec::new());
        chunks
            .save_parts(crate::tests::overworld(A), 2, 1, &with, Parts::ENTITIES)
            .unwrap();
        chunks.sync().unwrap();
        // One round: the entities' file removed, its directory synced, no blocks file.
        assert!(
            disk.operations() - before <= 2,
            "{}",
            disk.operations() - before
        );
    }

    /// S5: a failed sync drops the saves, whose handles are lost and save them again,
    /// and keeps the notes, which have nobody else to write them. And a note does not
    /// take the place of a save of its chunk.
    #[test]
    fn a_failed_sync_keeps_the_notes_and_a_note_does_not_replace_a_save() {
        let disk = Arc::new(MemoryDisk::failing(Fault::Fail(1)));
        let mut chunks = FileChunks::new(disk.clone(), Path::new(ROOT));
        chunks
            .save(crate::tests::overworld(A), 3, 1, &edited())
            .unwrap();
        chunks
            .note(crate::tests::overworld(A), &made(A), Parts::ALL)
            .unwrap();
        assert!(chunks.load(crate::tests::overworld(A)).unwrap() == Some(edited()));
        chunks
            .note(crate::tests::overworld(B), &made(B), Parts::ALL)
            .unwrap();
        assert_eq!((chunks.pending(), chunks.noted()), (1, 1));

        assert!(chunks.sync().is_err());
        assert_eq!((chunks.pending(), chunks.noted()), (0, 1));
        assert!(chunks.load(crate::tests::overworld(B)).unwrap() == Some(made(B)));
        chunks.sync().unwrap();
        assert!(durable(&disk, B));
        assert!(!durable(&disk, A));
    }

    #[test]
    fn the_thread_for_chunks_ends_once_nothing_is_left_and_the_request_under_way_is_answered() {
        for answered in [true, false] {
            let rig = rig();
            let east = open(&rig.store, 1, 1);
            load(&east, A);
            rig.shared.entered(A, 1);
            let Rig {
                store,
                disk,
                shared,
                ended,
            } = rig;
            drop((east, store));
            let done = disk.operations();
            // The thread holds the generator until it ends, and it waits for the
            // answer.
            assert_eq!(ended.try_recv(), Err(TryRecvError::Empty));
            if answered {
                shared.let_go(A);
            } else {
                shared.drop_all();
            }
            // Bounded only so that a thread that never ends is a failure.
            assert_eq!(
                ended.recv_timeout(Duration::from_secs(60)),
                Err(RecvTimeoutError::Disconnected)
            );
            // The chunk was noted, and nothing writes a note when the store is gone.
            assert_eq!(disk.operations(), done);
        }
    }

    /// What a sync of 512 notes of real terrain costs, the most notes one sync writes
    /// (S2), against ADR-0021's limit of 250 ms. The files go under `TMPDIR`, which is
    /// to be the disk the server runs on:
    ///
    /// ```text
    /// cargo test --release -p clustine-worldstore --lib -- --ignored --nocapture sync_of_512
    /// ```
    #[test]
    #[ignore = "a measurement, for a release build on the disk the server runs on"]
    fn what_a_sync_of_512_notes_of_real_terrain_costs() {
        use std::time::Instant;

        use clustine_world::ChunkGenerator;
        use clustine_worldgen::TerrainGenerator;

        use crate::disk::OsDisk;

        // A view's worth of land at the spawn point and beyond, as a region asks for it.
        let generator = TerrainGenerator::new(13579, clustine_world::Dimension::Overworld);
        let made: Vec<_> = (0..512)
            .map(|i| {
                let position = ChunkPos::new(i % 32 - 16, i / 32 - 8);
                (position, generator.generate(position))
            })
            .collect();
        fn size(path: &Path) -> u64 {
            match std::fs::read_dir(path) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .map(|entry| size(&entry.path()))
                    .sum(),
                Err(_) => std::fs::metadata(path).map_or(0, |metadata| metadata.len()),
            }
        }
        for round in 1..=5 {
            let directory = tempfile::tempdir().unwrap();
            let mut files = FileChunks::new(Arc::new(OsDisk::default()), directory.path());
            let noting = Instant::now();
            for (position, chunk) in &made {
                files
                    .note(crate::tests::overworld(*position), chunk, Parts::ALL)
                    .unwrap();
            }
            let noted = noting.elapsed();
            let syncing = Instant::now();
            files.sync().unwrap();
            let synced = syncing.elapsed();
            let bytes = size(&directory.path().join("chunks"));
            println!(
                "round {round}: 512 notes made ready in {noted:?} (on the store's thread for \
                 chunks), written and synced in {synced:?}; {} KB in the chunks' directory",
                bytes / 1024
            );
        }
    }
}
