//! The commit thread: the lanes of the regions and the log they share.
//!
//! Everything a handle asks for, and every hello, arrives here in one queue, and is
//! taken in the order it arrived. That order is each region's lane: its commits, its
//! checkpoints and the opening of it are done in it. Saves, loads and checkpoints are
//! passed on to the thread for chunks once the commits before them are durable.
//!
//! What is waiting is taken in groups. The commits of a group, of whatever regions, are
//! appended to the log and made durable with one sync, and only then answered. A group
//! ends before a hello is looked at, so that what the previous owner was told is
//! committed is on disk, and read, when the new owner is restored.
//!
//! A group that cannot be written or made durable is cut off the log again, and every
//! region loses its owner, whether it wrote in the group or not. Until what was cut off
//! is durably gone, nobody is served: see `Log::settle`, and section 4.1 of
//! `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! ```text
//! log/<n>.wal          the log, in segments numbered in the order they were begun
//! regions/table        the regions there are and the chunks each holds
//! regions/<r>.region   per region, its highest epoch and its entity ids
//! regions/<r>.state    per region, its state as of its last checkpoint
//! layout               only in a world from before there was a table: the fingerprint
//!                      of the layout its regions were part of
//! ```
//!
//! The table of regions is this thread's alone: it decides who holds a chunk, in the
//! order the messages arrive. See `table.rs`.
//!
//! A segment is removed once nothing in it is needed any more and every segment before
//! it is gone. Records are not removed one by one: a record that a checkpoint covers is
//! passed over because its tick is not above the state file's.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};

use clustine_data::BlockState;
use clustine_format::{
    LogRecord, Logged, MAX_RECORD_LENGTH, RegionFile, StateFile, TableFile, read_log,
    read_log_with_offsets,
};
use clustine_region::RegionId;
use clustine_rpc::{
    Decline, RegionHello, RegionList, Restored, SplitPart, StoreReply, StoreRequest, TickState,
};
use clustine_world::{BlockPos, ChunkPos, EntityId, EntityIds};
use tracing::{error, info, warn};

use crate::chunks::Job;
use crate::disk::{Disk, replace};
use crate::table::{Division, Table};
use crate::{Message, Opened, Peer, Session, StoreError};

/// A segment of the log that has grown beyond this is not appended to any more.
const SEGMENT_LIMIT: u64 = 64 * 1024 * 1024;

/// At most this many messages are taken into one group, so that a store that is never
/// idle still syncs.
const GROUP_LIMIT: usize = 4096;

/// At most this many chunks are granted or returned by one record of the log. A claim
/// or a return of more takes several records, one behind the other in the same group:
/// a record longer than the log's limit would hide itself and what follows it.
const RECORD_CHUNKS: usize = 65_536;

/// The commit thread.
pub(crate) struct Lanes {
    disk: Arc<dyn Disk>,
    root: PathBuf,
    jobs: Sender<Job>,
    log: Log,
    /// The fingerprint a hello has to name, if the world is divided as a layout is.
    layout: Option<u64>,
    /// The regions there are and the chunks each holds.
    table: Table,
    /// A lane for every living region, and for no other.
    regions: BTreeMap<RegionId, Lane>,
    /// The last segment of the log that has a record which changes the table and is not
    /// in the table file, if there is one. The segments from the file's `from` up to it
    /// hold what the file does not have.
    table_last: Option<u64>,
    /// The index of the next block of entity ids: above every block a region file has,
    /// whatever has become of its region, so that none is issued twice.
    next_block: u32,
    /// How many handles have been handed out.
    sessions: u64,
    group: Group,
}

/// What the store knows about a region.
#[derive(Default)]
struct Lane {
    /// What is on disk about it, once it has been opened or made by a split.
    file: Option<RegionFile>,
    /// Whether `file` is not on disk as it is here: the region was made by a split,
    /// and its file could not be written yet. The record of the split has its epoch
    /// meanwhile, so the table file, which would let that record go, waits for it.
    unwritten: bool,
    /// The tick of its state file, if it has one.
    state_tick: Option<u64>,
    /// The record of the merge or the split the region went through last, if that is
    /// its latest whole state: if its tick is above the state file's. The region is
    /// restored from it until a checkpoint puts a later state in place.
    record: Option<Entry>,
    /// Where its commits are in the log that whoever opens it next is restored with:
    /// those with a tick above the state file's that no later opening has passed over.
    live: Vec<Entry>,
    owner: Option<Owner>,
    /// The session that opened the region last, unless it has been lost since. Only its
    /// checkpoints are put in place, also after it gave the region up.
    current: Option<u64>,
    /// The tick of a state file that has been put in place in this group, and is not
    /// durably there until the group ends.
    installing: Option<u64>,
    /// The highest tick of the region the store has in a commit or a whole state: the
    /// tick the region was restored up to when it was opened, raised by every commit
    /// taken since. It is the tick of what the region is granted.
    latest: u64,
    /// The chunks a return of which is on its way through the thread for chunks, each
    /// with the number of that return. A chunk is freed only by the return it is noted
    /// with here, so that one that was called off frees nothing later.
    returning: BTreeMap<ChunkPos, u64>,
    /// How many returns the session that opened the region last has asked for, from
    /// which their numbers come.
    returns: u64,
    /// The highest tick the session that opened the region last has named, in a commit
    /// or in a checkpoint, in place yet or not, or was restored up to. A merge or a
    /// split has to name a tick above it, so that no checkpoint that is still under
    /// way can be put in place over the state it leaves.
    named: u64,
}

impl Lane {
    /// The tick of the region's latest whole state, if it has one.
    fn whole_tick(&self) -> Option<u64> {
        self.record.map(|record| record.tick).max(self.state_tick)
    }

    /// Takes the record at `entry`, of a merge or a split, for the region's latest
    /// whole state if its tick is above the state file's. The commits up to its tick
    /// are covered by it.
    fn whole(&mut self, entry: Entry) {
        if self.state_tick.is_none_or(|state| entry.tick > state) {
            self.record = Some(entry);
        }
        self.live.retain(|live| live.tick > entry.tick);
    }
}

/// Where a commit is in the log, or the record of a merge or a split.
#[derive(Debug, Clone, Copy)]
struct Entry {
    tick: u64,
    segment: u64,
    offset: u64,
    length: usize,
}

/// The store's end of a handle.
struct Owner {
    peer: Arc<Peer>,
    epoch: u64,
    /// The answers to the commits and claims of this group, in the order they were
    /// asked for, which are given once the group is durable.
    unsynced: Vec<StoreReply>,
    /// What the handle asked for after those commits, for the thread for chunks, which
    /// it is given only once they are durable.
    held: Vec<Job>,
}

/// What the current group has done that is answered or finished when it ends.
#[derive(Default)]
struct Group {
    /// The regions that have answers or requests waiting for the group to end.
    regions: BTreeSet<RegionId>,
    /// What the group did to the table, which is taken back, last first, if the group
    /// cannot be made durable.
    changed: Vec<Change>,
    /// The regions whose state file was put in place.
    installs: BTreeSet<RegionId>,
    /// Handles to be told that everything they asked for is done.
    flushes: Vec<Arc<Peer>>,
}

impl Group {
    fn is_empty(&self) -> bool {
        self.regions.is_empty()
            && self.changed.is_empty()
            && self.installs.is_empty()
            && self.flushes.is_empty()
    }
}

/// Something a group did to the table of regions.
enum Change {
    /// The region was granted the chunks.
    Granted {
        region: RegionId,
        chunks: Vec<ChunkPos>,
    },
    /// The region gave the chunks back, which it had held from these ticks.
    Returned {
        region: RegionId,
        grants: Vec<(ChunkPos, u64)>,
    },
}

impl Lanes {
    /// Reads what the world in `root` has, and brings it in line with how `told` says
    /// the world is divided: the regions' files, the table of regions and the log. This
    /// is section 4.2 of ADR-0011. Every step of it is done again, to the same end, by
    /// a store that starts on what a crash in the middle of it left.
    ///
    /// The thread for chunks has to be running: a world that was divided otherwise has
    /// what its regions committed put into the stored chunks.
    pub(crate) fn load(
        disk: Arc<dyn Disk>,
        root: &Path,
        jobs: Sender<Job>,
        told: &Division,
    ) -> Result<Self, StoreError> {
        told.check()?;
        let regions_directory = root.join("regions");
        let log_directory = root.join("log");
        disk.create_dir_all(&regions_directory)?;
        disk.create_dir_all(&log_directory)?;

        let mut regions: BTreeMap<RegionId, Lane> = BTreeMap::new();
        for name in disk.list(&regions_directory)? {
            let path = regions_directory.join(&name);
            if name.ends_with(".tmp") {
                disk.remove(&path)?;
                continue;
            }
            let Some((region, kind)) = name.split_once('.') else {
                continue;
            };
            let Ok(region) = region.parse() else {
                continue;
            };
            let lane = regions.entry(RegionId(region)).or_default();
            let bytes = disk.read(&path)?.unwrap_or_default();
            let damaged = |error| StoreError::Damaged {
                path: path.clone(),
                error,
            };
            match kind {
                "region" => lane.file = Some(RegionFile::decode(&bytes).map_err(damaged)?),
                "state" => lane.state_tick = Some(StateFile::decode(&bytes).map_err(damaged)?.tick),
                _ => {}
            }
        }
        let next_block = regions
            .values()
            .filter_map(|lane| lane.file)
            .filter(|file| !is_empty(file.entity_ids))
            .map(|file| (file.entity_ids.first.0 / EntityIds::BLOCK_SIZE) as u32 + 1)
            .max()
            .unwrap_or(0);

        let table_path = regions_directory.join("table");
        let mut stored = match disk.read(&table_path)? {
            Some(bytes) => {
                let file = TableFile::decode(&bytes).map_err(|error| StoreError::Damaged {
                    path: table_path.clone(),
                    error,
                })?;
                Some(Table::read(file)?)
            }
            None => None,
        };

        let mut table_last = None;
        let mut log = Log {
            disk: Arc::clone(&disk),
            directory: log_directory,
            segments: BTreeSet::new(),
            active: None,
            next: 1,
            unsettled: None,
        };
        let mut segments: Vec<u64> = disk
            .list(&log.directory)?
            .iter()
            .filter_map(|name| name.strip_suffix(".wal")?.parse().ok())
            .collect();
        segments.sort_unstable();
        for segment in segments {
            let path = log.path(segment);
            let bytes = disk.read(&path)?.unwrap_or_default();
            // What follows the records that could be read was being written when the
            // process died, and was never answered. Nothing is appended to this
            // segment any more, so it hides nothing.
            let (logged, _) =
                read_log_with_offsets(&bytes).map_err(|error| StoreError::Damaged {
                    path: path.clone(),
                    error,
                })?;
            for Logged {
                offset,
                length,
                record,
            } in logged
            {
                match record {
                    LogRecord::Commit { region, tick, .. } => {
                        let lane = regions.entry(RegionId(region)).or_default();
                        if lane.state_tick.is_none_or(|state| tick > state) {
                            lane.live.push(Entry {
                                tick,
                                segment,
                                offset: offset as u64,
                                length,
                            });
                        }
                    }
                    LogRecord::Opened {
                        region, restored, ..
                    } => {
                        if let Some(lane) = regions.get_mut(&RegionId(region)) {
                            lane.live.retain(|entry| entry.tick <= restored);
                            lane.record.take_if(|record| record.tick > restored);
                        }
                    }
                    LogRecord::Changes { .. } => {
                        warn!(
                            ?path,
                            "a record of a world from before regions had a state is passed over"
                        );
                    }
                    LogRecord::Granted {
                        region,
                        tick,
                        chunks,
                    } => {
                        if let Some(table) = changed_by(&mut stored, segment)? {
                            table.grant(RegionId(region), tick, &chunks)?;
                            table_last = Some(segment);
                        }
                    }
                    LogRecord::Returned { region, chunks } => {
                        if let Some(table) = changed_by(&mut stored, segment)? {
                            for chunk in chunks {
                                // The record says that the region does not hold the
                                // chunk, which is so.
                                if table.release(RegionId(region), chunk).is_none() {
                                    warn!(
                                        region,
                                        ?chunk,
                                        "the log has a chunk returned that the region was not granted"
                                    );
                                }
                            }
                            table_last = Some(segment);
                        }
                    }
                    // What a merge or a split means for a lane counts wherever the
                    // record is; what it means for the table, only in a segment the
                    // table file does not stand for.
                    LogRecord::Absorbed {
                        region,
                        absorbed,
                        tick,
                        ..
                    } => {
                        let entry = Entry {
                            tick,
                            segment,
                            offset: offset as u64,
                            length,
                        };
                        regions.entry(RegionId(region)).or_default().whole(entry);
                        if let Some(table) = changed_by(&mut stored, segment)? {
                            table.absorb(RegionId(region), RegionId(absorbed), tick)?;
                            table_last = Some(segment);
                        }
                    }
                    LogRecord::Split {
                        region,
                        tick,
                        part,
                        part_epoch,
                        chunks,
                        ..
                    } => {
                        let entry = Entry {
                            tick,
                            segment,
                            offset: offset as u64,
                            length,
                        };
                        regions.entry(RegionId(region)).or_default().whole(entry);
                        let made = regions.entry(RegionId(part)).or_default();
                        made.whole(entry);
                        // The record has the new region's epoch until its file does,
                        // which a store that died right after the split never wrote.
                        if made.file.is_none_or(|file| file.epoch < part_epoch) {
                            let entity_ids =
                                made.file.map_or(NO_ENTITY_IDS, |file| file.entity_ids);
                            made.file = Some(RegionFile {
                                epoch: part_epoch,
                                entity_ids,
                            });
                            made.unwritten = true;
                        }
                        if let Some(table) = changed_by(&mut stored, segment)? {
                            table.split(RegionId(region), RegionId(part), tick, &chunks)?;
                            table_last = Some(segment);
                        }
                    }
                }
            }
            log.segments.insert(segment);
            log.next = segment + 1;
        }
        // Also when no segment is left: a record for the table in a segment below the
        // file's `from` would not be read at the next start.
        if let Some(table) = &stored {
            log.next = log.next.max(table.from);
        }

        // A world from before there was a table says in this file how it was divided:
        // the fingerprint of its layout. A store that is told no fingerprint has
        // nothing to hold against it, and does not read what the file says.
        let layout_path = root.join("layout");
        let before = disk.read(&layout_path)?;
        let as_before = match (&before, told.layout) {
            (Some(bytes), Some(told)) => {
                let layout = parse_layout(bytes).ok_or_else(|| {
                    StoreError::MalformedMeta("the layout file is not a fingerprint".to_owned())
                })?;
                layout == told
            }
            _ => false,
        };
        let remake = match &stored {
            Some(table) => !table.is_of(told),
            // Such a world is kept as it is only if it was last served with the very
            // layout the store is told, so it is made over whenever it is told none.
            None => before.is_some() && !as_before,
        };
        let keep = stored.is_some() && !remake;
        let tabled = stored.is_some();
        let used = stored.as_ref().map_or(0, |table| table.next_region);
        let mut lanes = Self {
            disk,
            root: root.to_owned(),
            jobs,
            log,
            layout: told.layout,
            // Where there is none, this one stands in until the table is written below.
            table: stored.unwrap_or_else(|| Table::made_from(told, 0, 0)),
            regions,
            table_last,
            next_block,
            sessions: 0,
            group: Group::default(),
        };
        if remake {
            // Where there was no table, nobody was granted anything, and every change
            // the regions committed goes into the chunks.
            lanes.make_over(tabled)?;
        }
        if !keep {
            // What changes the table from now on goes to a segment the file names.
            lanes.log.close();
            let from = lanes.log.next;
            lanes.table = Table::made_from(told, used, from);
            lanes.write_table(from)?;
            lanes.table_last = None;
        }
        if remake {
            // Said once the table is durable, by which the world is made over for
            // good, and whether or not a region had anything left to put into the
            // chunks: whoever runs a region of the world as it was finds it gone, or
            // begun anew under its id (ADR-0017, section 2.2).
            info!(
                "the world was divided otherwise before; what its regions had is in the stored chunks now"
            );
            warn!(
                "the regions of this world begin anew: whoever is in it has to join again. Stop the workers and the edges of a cluster before its world store is started with other pins"
            );
        }
        // Only once the table is durable, by which a start after a crash knows that
        // there is nothing left to be made over; and also if a store died right here.
        if lanes.disk.exists(&layout_path)? {
            lanes.disk.remove(&layout_path)?;
            lanes.disk.sync_directory(root)?;
        }
        lanes.align()?;
        Ok(lanes)
    }

    pub(crate) fn run(mut self, messages: Receiver<Message>) {
        while let Ok(first) = messages.recv() {
            self.handle(first);
            for message in messages.try_iter().take(GROUP_LIMIT) {
                self.handle(message);
            }
            self.end_group();
        }
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Open {
                hello,
                reply_to,
                answer,
            } => self.open(hello, reply_to, answer),
            Message::Request { session, request } => self.request(session, request),
            Message::Close { session } => self.close(session),
            Message::Lost { session } => {
                if let Some(lane) = self.regions.get_mut(&session.region)
                    && lane.current == Some(session.number)
                {
                    lose(lane);
                }
            }
            Message::Checkpointed {
                session,
                tick,
                temporary,
            } => self.install(session, tick, &temporary),
            // What a handle that has been lost since asked for is not answered.
            Message::Flushed(peer) => {
                if !peer.is_lost() {
                    self.group.flushes.push(peer);
                }
            }
            Message::Returned {
                session,
                number,
                chunks,
            } => self.returned(session, number, chunks),
            Message::Regions { answer } => {
                // The list has nothing that is not durable, and is not given while what
                // a failed group left in the log could still come back.
                self.end_group();
                let list = match self.log.settle() {
                    Ok(()) => Ok(self.list()),
                    Err(error) => Err(StoreError::Io(error)),
                };
                // Whoever asked may have gone.
                let _ = answer.send(list);
            }
            Message::Barrier { reply_to, answer } => {
                // Everything that was sent before it has been handled. What of it
                // waited for the group to end is answered and passed on now, so that
                // the barrier reaches the thread for chunks behind all of it. A merge
                // and a split were done whole when their requests were handled.
                self.end_group();
                // If that thread has gone, the answer goes with the job, and whoever
                // waits is told so.
                let _ = self.jobs.send(Job::Barrier { reply_to, answer });
            }
            Message::Passed { answer } => {
                // The thread for chunks has done what it was given before the barrier,
                // and what it had to say of it has been handled here: state files are
                // in place and returns are in the log. Ending the group makes that
                // durable, sees to the log and the table file, and answers the handles'
                // own flushes. None of it gives the thread for chunks more to do, which
                // is why one round there and back is enough.
                self.end_group();
                let rested = match self.log.settle() {
                    Ok(()) => Ok(()),
                    Err(error) => Err(StoreError::Io(error)),
                };
                // Whoever asked may have gone.
                let _ = answer.send(rested);
            }
        }
    }

    fn regions_directory(&self) -> PathBuf {
        self.root.join("regions")
    }

    fn region_path(&self, region: RegionId) -> PathBuf {
        self.regions_directory().join(format!("{region}.region"))
    }

    fn state_path(&self, region: RegionId) -> PathBuf {
        self.regions_directory().join(format!("{region}.state"))
    }

    /// The list of regions as the table has it.
    fn list(&self) -> RegionList {
        self.table.list(|region| {
            let file = self.regions.get(&region).and_then(|lane| lane.file);
            file.map_or(0, |file| file.epoch)
        })
    }

    /// Writes the table file, with `from` as the first segment of the log that is not
    /// in it, and makes it durable.
    fn write_table(&mut self, from: u64) -> Result<(), StoreError> {
        let path = self.regions_directory().join("table");
        replace(self.disk.as_ref(), &path, &self.table.file(from).encode())?;
        self.disk.sync_directory(&self.regions_directory())?;
        self.table.from = from;
        Ok(())
    }

    /// The handle of `session` was dropped: the region has no owner, unless another
    /// has it by now.
    fn close(&mut self, session: Session) {
        // What the handle asked for before is done first, as far as it is up to this
        // thread: its commits answered and its saves passed on.
        self.end_group();
        if let Some(lane) = self.regions.get_mut(&session.region) {
            lane.owner
                .take_if(|owner| owner.peer.session.number == session.number);
        }
    }

    /// Does what the owner `session` asks for, if it still owns its region.
    fn request(&mut self, session: Session, request: StoreRequest) {
        // A merge and a split are no part of a group: each ends the group, is made
        // durable by itself, and changes what is known here only then.
        let request = match request {
            StoreRequest::AbsorbCommit {
                absorbed,
                absorbed_epoch,
                tick,
                state,
            } => return self.absorb(session, absorbed, absorbed_epoch, tick, state),
            StoreRequest::SplitCommit {
                tick,
                state,
                part,
                as_epoch,
                region,
            } => return self.split(session, tick, state, part, as_epoch, region),
            request => request,
        };
        let Some(lane) = self.regions.get_mut(&session.region) else {
            return;
        };
        let Some(owner) = lane
            .owner
            .as_mut()
            .filter(|owner| owner.peer.session.number == session.number)
        else {
            // What a handle asks for after its region was taken over, or after it was
            // lost, is not done.
            return;
        };
        let peer = Arc::clone(&owner.peer);
        let job = match request {
            StoreRequest::Commit {
                tick,
                changes,
                state,
            } => {
                let record = LogRecord::Commit {
                    region: session.region.0,
                    tick,
                    epoch: owner.epoch,
                    changes,
                    state,
                }
                .encode();
                self.group.regions.insert(session.region);
                match self.log.append(&record) {
                    Ok((segment, offset)) => {
                        lane.live.push(Entry {
                            tick,
                            segment,
                            offset,
                            length: record.len(),
                        });
                        owner.unsynced.push(StoreReply::Committed { tick });
                        lane.latest = lane.latest.max(tick);
                        lane.named = lane.named.max(tick);
                    }
                    Err(error) => {
                        error!(region = %session.region, %error, "a commit could not be written to the log");
                        self.fail_log();
                    }
                }
                return;
            }
            // Only the holder loads and saves a chunk. Looked at here, where the table
            // is; what the thread for chunks is given has passed, and is done in the
            // order it passed in.
            StoreRequest::Load { position } | StoreRequest::Save { position, .. }
                if self.table.held_from(session.region, position).is_none() =>
            {
                peer.answer(StoreReply::NotHeld {
                    position,
                    holder: self.table.holder(position),
                });
                return;
            }
            // A load need not wait for commits, only for what was asked before it of the
            // same chunk, which may be held.
            StoreRequest::Load { position } => {
                let job = Job::Load { position, peer };
                if owner.held.is_empty() {
                    let _ = self.jobs.send(job);
                } else {
                    owner.held.push(job);
                }
                return;
            }
            StoreRequest::Save {
                position,
                tick,
                chunk,
            } => Job::Save {
                position,
                tick,
                chunk,
                peer,
            },
            StoreRequest::Checkpoint { tick, state } => {
                // Named from now on, whether or not its state is ever put in place.
                lane.named = lane.named.max(tick);
                Job::Checkpoint { tick, state, peer }
            }
            StoreRequest::Flush => Job::Flush { peer },
            StoreRequest::Claim { chunks } => {
                let region = session.region;
                let mut granted = Vec::new();
                let mut foreign = Vec::new();
                let mut new = Vec::new();
                let mut asked = BTreeSet::new();
                for chunk in chunks {
                    // Each once, in the order of the request.
                    if !asked.insert(chunk) {
                        continue;
                    }
                    match self.table.holder(chunk) {
                        // Nothing about it changes, also not its tick. A return of it
                        // that is on its way is called off.
                        Some(holder) if holder == region => {
                            lane.returning.remove(&chunk);
                            granted.push(chunk);
                        }
                        Some(holder) => foreign.push((chunk, holder)),
                        None => {
                            new.push(chunk);
                            granted.push(chunk);
                        }
                    }
                }
                // The tick is the store's, not the region's own, which runs ahead of
                // what it has committed and can be issued again after a restore.
                let tick = lane.latest;
                for chunks in new.chunks(RECORD_CHUNKS) {
                    let record = LogRecord::Granted {
                        region: region.0,
                        tick,
                        chunks: chunks.to_vec(),
                    };
                    match self.log.append(&record.encode()) {
                        Ok((segment, _)) => self.table_last = Some(segment),
                        Err(error) => {
                            error!(%region, %error, "a grant could not be written to the log");
                            self.fail_log();
                            return;
                        }
                    }
                }
                if !new.is_empty() {
                    // At once, so that a later claim in the same group, of whatever
                    // region, is answered by it.
                    self.table
                        .grant(region, tick, &new)
                        .expect("chunks nobody holds are granted to a region that is there");
                    let change = Change::Granted {
                        region,
                        chunks: new,
                    };
                    self.group.changed.push(change);
                }
                // Answered when the group ends, behind the commits asked for before:
                // also a claim that granted nothing anew may rest on a grant of this
                // group.
                owner
                    .unsynced
                    .push(StoreReply::Claimed { granted, foreign });
                self.group.regions.insert(region);
                return;
            }
            StoreRequest::Return { chunks } => {
                let region = session.region;
                let mut returned = Vec::new();
                let mut asked = BTreeSet::new();
                for chunk in chunks {
                    let granted = self.table.granted_from(region, chunk).is_some();
                    if !granted || chunk == self.table.home_chunk {
                        warn!(
                            %region,
                            ?chunk,
                            "a chunk that the region was not granted, or the home chunk, is not returned"
                        );
                    } else if asked.insert(chunk) {
                        returned.push(chunk);
                    }
                }
                if returned.is_empty() {
                    return;
                }
                lane.returns += 1;
                let number = lane.returns;
                for chunk in &returned {
                    lane.returning.insert(*chunk, number);
                }
                // Passed on like a save: the chunks are free once the saves before it
                // are durable, which the thread for chunks sees to.
                Job::Return {
                    number,
                    chunks: returned,
                    peer,
                }
            }
            // Taken above, before the lane was looked up.
            StoreRequest::AbsorbCommit { .. } | StoreRequest::SplitCommit { .. } => return,
        };
        if owner.unsynced.is_empty() && owner.held.is_empty() {
            let _ = self.jobs.send(job);
        } else {
            owner.held.push(job);
        }
    }

    /// Makes what the group wrote durable, and then answers and passes on what waited
    /// for that.
    fn end_group(&mut self) {
        if self.group.is_empty() && !self.log.unsynced() {
            return;
        }
        if let Err(error) = self.log.sync() {
            error!(%error, "the log could not be made durable");
            // Nothing of the group is answered or finished, and nobody is left to be.
            self.fail_log();
            return;
        }
        let group = mem::take(&mut self.group);

        let mut installed = false;
        if !group.installs.is_empty() {
            let synced = self.disk.sync_directory(&self.regions_directory());
            if let Err(error) = &synced {
                error!(%error, "state files could not be made durable");
            }
            for region in &group.installs {
                let lane = self.regions.get_mut(region).expect("installs are of lanes");
                let Some(tick) = lane.installing.take() else {
                    continue;
                };
                if synced.is_ok() {
                    // It is the latest whole state now: a state is only put in place
                    // above the one there was, be that a file or a record.
                    lane.state_tick = Some(tick);
                    lane.record = None;
                    lane.live.retain(|entry| entry.tick > tick);
                    installed = true;
                } else {
                    // A failed sync is not tried again: what it was to make durable may
                    // be lost even if a later one succeeds. The region is opened again,
                    // and read afresh then.
                    lose(lane);
                }
            }
        }
        if installed {
            // What follows goes to a new segment, so that this one can be removed once a
            // later checkpoint covers what is left in it. Before the answers, so that a
            // flush finds the checkpoint done in full.
            self.log.close();
            self.collect();
            self.trim_for_the_table();
        }

        for region in &group.regions {
            let Some(owner) = self
                .regions
                .get_mut(region)
                .and_then(|lane| lane.owner.as_mut())
            else {
                continue;
            };
            for reply in owner.unsynced.drain(..) {
                owner.peer.answer(reply);
            }
            for job in owner.held.drain(..) {
                let _ = self.jobs.send(job);
            }
        }
        for peer in group.flushes {
            peer.answer(StoreReply::Flushed);
        }
    }

    /// After a failed write or sync of the log: what was written in this group is cut
    /// off, and every region loses its owner. Nothing of the group is answered.
    ///
    /// Also a region that wrote nothing in the group: what it asked for in it may have
    /// no answer by which it could learn that it was undone, and what it was answered
    /// may have rested on what another region wrote in it.
    fn fail_log(&mut self) {
        let failed = self.log.fail();
        // What the group did to the table is taken back, last first: its records are
        // cut off the log, and a store that started now would not know of them.
        let group = mem::take(&mut self.group);
        for change in group.changed.into_iter().rev() {
            match change {
                Change::Granted { region, chunks } => {
                    for chunk in chunks {
                        self.table.release(region, chunk);
                    }
                }
                Change::Returned { region, grants } => {
                    for (chunk, tick) in grants {
                        self.table.grant(region, tick, &[chunk]).expect(
                            "a chunk the group freed is free once what it did since is undone",
                        );
                    }
                }
            }
        }
        for lane in self.regions.values_mut() {
            if let Some((segment, durable)) = failed {
                lane.live
                    .retain(|entry| entry.segment != segment || entry.offset < durable);
            }
            lose(lane);
        }
    }

    /// The segments of the log that a lane needs: those with a commit whoever opens
    /// its region next is restored with, or with the record that is its latest whole
    /// state.
    fn needed_by_lanes(&self) -> BTreeSet<u64> {
        self.regions
            .values()
            .flat_map(|lane| lane.live.iter().chain(&lane.record))
            .map(|entry| entry.segment)
            .collect()
    }

    /// Removes the segments at the start of the log that hold nothing needed any more:
    /// no commit a region is restored with, and nothing the table file does not have.
    fn collect(&mut self) {
        let needed = self.needed_by_lanes();
        let from = self.table.from;
        let table = self.table_last.map(|last| from..=last);
        self.log.remove_while(|segment| {
            !needed.contains(&segment)
                && !table.as_ref().is_some_and(|kept| kept.contains(&segment))
        });
    }

    /// Writes the table file anew if that lets the first segment of the log go: if no
    /// lane needs that segment and it is kept only for what it says of the table. The
    /// file then has all of that, and names the next segment as the first that is not
    /// in it.
    ///
    /// For the end of a group, when everything the table has is durable in the log and
    /// no segment is being appended to.
    fn trim_for_the_table(&mut self) {
        let (Some(&first), Some(last)) = (self.log.segments.first(), self.table_last) else {
            return;
        };
        let kept_for_the_table = (self.table.from..=last).contains(&first);
        if !kept_for_the_table
            || self.needed_by_lanes().contains(&first)
            || self.log.active.is_some()
        {
            return;
        }
        // The record of a split has the new region's epoch until the region's file
        // does, and the table file would let that record go.
        if let Err(error) = self.write_region_files() {
            warn!(%error, "the file of a new region cannot be written; the log is kept as it is");
            return;
        }
        // Only once the file is durable are the segments it stands for let go of. If it
        // cannot be written, the file there is stays the one that counts, with every
        // segment from its `from` on.
        match self.write_table(self.log.next) {
            Ok(()) => {
                self.table_last = None;
                self.collect();
            }
            Err(error) => {
                error!(%error, "the table of regions could not be written; the log is kept as it is");
            }
        }
    }

    /// The thread for chunks has made the saves before the return `number` of `session`
    /// durable: the chunks of it that have not been claimed again since, or returned
    /// once more, or taken elsewhere, are free.
    fn returned(&mut self, session: Session, number: u64, chunks: Vec<ChunkPos>) {
        let region = session.region;
        let Some(lane) = self.regions.get_mut(&region) else {
            return;
        };
        if lane.current != Some(session.number) {
            return;
        }
        // Only what is noted with this very return. A chunk that was claimed again and
        // returned once more is noted with the later one, whose saves are not durable
        // yet.
        let freed: Vec<ChunkPos> = chunks
            .into_iter()
            .filter(|chunk| lane.returning.get(chunk) == Some(&number))
            .collect();
        for chunk in &freed {
            lane.returning.remove(chunk);
        }
        for chunks in freed.chunks(RECORD_CHUNKS) {
            let record = LogRecord::Returned {
                region: region.0,
                chunks: chunks.to_vec(),
            };
            match self.log.append(&record.encode()) {
                Ok((segment, _)) => self.table_last = Some(segment),
                Err(error) => {
                    error!(%region, %error, "a return could not be written to the log");
                    self.fail_log();
                    return;
                }
            }
        }
        // At once: the chunks are nobody's, or the pinned region's whose area they are
        // in, for the next claim, which is written behind this record.
        let grants: Vec<(ChunkPos, u64)> = freed
            .into_iter()
            .filter_map(|chunk| Some((chunk, self.table.release(region, chunk)?)))
            .collect();
        if !grants.is_empty() {
            self.group.changed.push(Change::Returned { region, grants });
        }
    }

    /// Puts the state file the thread for chunks has written for `session` in place, if
    /// the session is still the one that opened the region last.
    fn install(&mut self, session: Session, tick: u64, temporary: &Path) {
        let path = self.state_path(session.region);
        let Some(lane) = self.regions.get_mut(&session.region) else {
            let _ = self.disk.remove(temporary);
            return;
        };
        // Only above the latest whole state, be that the state file or the record of a
        // merge or a split: a checkpoint that was under way when such a record was
        // written must not take the place of the state it left.
        let later = lane.whole_tick().is_none_or(|whole| tick > whole)
            && lane.installing.is_none_or(|installing| tick > installing);
        if lane.current != Some(session.number) || !later {
            let _ = self.disk.remove(temporary);
            return;
        }
        match self.disk.rename(temporary, &path) {
            Ok(()) => {
                lane.installing = Some(tick);
                self.group.installs.insert(session.region);
            }
            Err(error) => {
                error!(region = %session.region, %error, "a state file could not be put in place");
                let _ = self.disk.remove(temporary);
                lose(lane);
            }
        }
    }

    /// Opens a region for whoever said `hello`. If commits of the region have to be
    /// put into the stored chunks first, the thread for chunks hands it over once they
    /// are; otherwise it is handed over here and now, without waiting for whatever that
    /// thread is busy with.
    fn open(
        &mut self,
        hello: RegionHello,
        reply_to: Sender<Message>,
        answer: Sender<Result<(Opened, Restored), StoreError>>,
    ) {
        // What the previous owner was told is committed has to be on disk, and in the
        // live entries, before the region is read for the new one.
        self.end_group();
        // What a failed group left in the log has to be durably gone before anyone is
        // told anything again: a crash could bring it back otherwise, beside what was
        // written since.
        if let Err(error) = self.log.settle() {
            warn!(region = %hello.region, %error, "a hello is not answered while the log cannot be cut back");
            let _ = answer.send(Err(StoreError::Io(error)));
            return;
        }
        match self.admit(hello, reply_to) {
            Ok((opened, restored, changes, _, _)) if changes.is_empty() => {
                // Nothing is lost by not going through the thread for chunks: what the
                // owner before had it do is still done before anything the new owner
                // asks, as both go through its one queue.
                let session = opened.session;
                if answer.send(Ok((opened, restored))).is_err() {
                    // Whoever asked has gone; nobody will give the region up for them.
                    self.close(session);
                }
            }
            Ok((opened, restored, changes, tick, peer)) => {
                let _ = self.jobs.send(Job::Restore {
                    changes,
                    tick,
                    peer,
                    opened,
                    restored,
                    answer,
                });
            }
            Err(error) => {
                // Whoever said hello is waiting for the answer.
                let _ = answer.send(Err(error));
            }
        }
    }

    /// Makes whoever said `hello` the owner of the region, if nothing speaks against it,
    /// and reads what the region is restored with.
    #[allow(clippy::type_complexity)]
    fn admit(
        &mut self,
        hello: RegionHello,
        reply_to: Sender<Message>,
    ) -> Result<
        (
            Opened,
            Restored,
            Vec<(BlockPos, BlockState)>,
            u64,
            Arc<Peer>,
        ),
        StoreError,
    > {
        let RegionHello {
            region,
            epoch,
            layout,
        } = hello;
        if let Some(expected) = self.layout
            && expected != layout
        {
            return Err(StoreError::LayoutMismatch {
                expected,
                offered: layout,
            });
        }
        // Before anything is written: the regions are those the table has, and a hello
        // makes none.
        if !self.table.has(region) {
            return Err(match self.table.absorbed_into(region) {
                Some(into) => StoreError::Absorbed { region, into },
                None => StoreError::UnknownRegion { region },
            });
        }

        // An owner gives way to one with the same epoch, which is the same owner come
        // back before the store noticed that it had gone, and to a later one.
        let file = self.regions.get(&region).and_then(|lane| lane.file);
        if let Some(file) = file
            && epoch < file.epoch
        {
            return Err(StoreError::EpochRefused {
                region,
                offered: epoch,
                seen: file.epoch,
            });
        }
        // A region that is pinned or home gets a block of entity ids when it is first
        // opened, and keeps it. A region that was split off another has none, unless
        // its id has become that of a pinned or home region since.
        let issued = file
            .map(|file| file.entity_ids)
            .filter(|ids| !is_empty(*ids));
        let entity_ids = match issued {
            Some(entity_ids) => entity_ids,
            None if !self.table.pinned(region).is_empty() || region == self.table.home_region => {
                let block = EntityIds::block(self.next_block).ok_or(StoreError::OutOfEntityIds)?;
                // Before it is written: if writing fails half way, the block may be on
                // disk all the same, and must not be another region's as well.
                self.next_block += 1;
                block
            }
            None => NO_ENTITY_IDS,
        };
        let updated = RegionFile { epoch, entity_ids };
        let unwritten = self.regions.get(&region).is_some_and(|lane| lane.unwritten);
        if file != Some(updated) || unwritten {
            // On disk before anyone is told that the region is theirs, so that an owner
            // with a lower epoch is refused after a restart too.
            replace(
                self.disk.as_ref(),
                &self.region_path(region),
                &updated.encode(),
            )?;
            self.disk.sync_directory(&self.regions_directory())?;
            let lane = self.regions.entry(region).or_default();
            lane.file = Some(updated);
            lane.unwritten = false;
        }
        let state_path = self.state_path(region);
        let lane = self.regions.get_mut(&region).expect("made above");
        let entity_ids = lane.file.expect("written above").entity_ids;

        // From here on the owner that was there, if any, gets nothing done and hears
        // nothing: its handle is lost, and its end of the replies is closed once the
        // thread for chunks is through with what it had asked for.
        if let Some(replaced) = lane.owner.take() {
            replaced.peer.mark_lost();
            info!(%region, epoch, "the owner of a region was replaced");
        }
        lane.current = None;

        let state = match self.disk.read(&state_path)? {
            Some(bytes) => {
                Some(
                    StateFile::decode(&bytes).map_err(|error| StoreError::Damaged {
                        path: state_path.clone(),
                        error,
                    })?,
                )
            }
            None => None,
        };
        // The latest whole state: the state file, or the record of the merge or the
        // split the region went through since, if its tick is higher.
        let state = match lane.record {
            Some(entry) if state.as_ref().is_none_or(|file| entry.tick > file.tick) => {
                Some(self.log.whole_state(&entry, region)?)
            }
            _ => state,
        };
        let state_tick = state.as_ref().map(|state| state.tick);
        let mut chosen = BTreeMap::new();
        for entry in &lane.live {
            if let Some((tick, changes, delta)) = self.log.read(entry)? {
                chosen.insert(tick, (changes, delta));
            }
        }
        chosen.retain(|tick, _| state_tick.is_none_or(|state| *tick > state));
        let restored_tick = chosen
            .last_key_value()
            .map(|(tick, _)| *tick)
            .or(state_tick)
            .unwrap_or(0);

        // Whatever of the region is in the log beyond this, because a write that was
        // never answered turned out to have reached the disk after all, is not part of
        // what the new owner goes on from.
        let opened = LogRecord::Opened {
            region: region.0,
            epoch,
            restored: restored_tick,
        };
        if let Err(error) = self.log.append(&opened.encode()) {
            self.fail_log();
            return Err(error.into());
        }

        self.sessions += 1;
        let session = Session {
            region,
            number: self.sessions,
        };
        let (sender, replies) = mpsc::channel();
        let lost = Arc::new(AtomicBool::new(false));
        let peer = Arc::new(Peer::new(session, sender, Arc::clone(&lost), reply_to));
        let lane = self.regions.get_mut(&region).expect("made above");
        lane.owner = Some(Owner {
            peer: Arc::clone(&peer),
            epoch,
            unsynced: Vec::new(),
            held: Vec::new(),
        });
        lane.current = Some(session.number);
        lane.latest = restored_tick;
        lane.named = restored_tick;
        // A return the owner before had asked for frees nothing any more.
        lane.returning.clear();
        lane.returns = 0;

        let mut changes = Vec::new();
        let mut deltas = Vec::new();
        for (tick, (changed, state)) in chosen {
            changes.extend(replayed(&self.table, region, tick, changed));
            deltas.push(TickState { tick, state });
        }
        let restored = Restored {
            entity_ids,
            state: state.map(|StateFile { tick, state }| TickState { tick, state }),
            deltas,
            held: self.table.grants(region),
            pinned: self.table.pinned(region).to_vec(),
        };
        let opened = Opened {
            session,
            replies,
            lost,
        };
        Ok((opened, restored, changes, restored_tick, peer))
    }

    /// The merge of section 3.6 of ADR-0011: the region of `session`, which survives,
    /// takes in the region `absorbed`. It is one record of the log, and has happened
    /// when that record is durable; what is known here is changed only then.
    fn absorb(
        &mut self,
        session: Session,
        absorbed: RegionId,
        absorbed_epoch: u64,
        tick: u64,
        state: Vec<u8>,
    ) {
        // Everything either region asked for before is durable and answered.
        self.end_group();
        let region = session.region;
        let Some((peer, epoch)) = self.owner_of(session) else {
            return;
        };
        let record = LogRecord::Absorbed {
            region: region.0,
            epoch,
            absorbed: absorbed.0,
            tick,
            state,
        }
        .encode();
        if let Err(reason) = self.may_absorb(region, absorbed, absorbed_epoch, tick, &record) {
            info!(%region, %absorbed, ?reason, "a merge is declined");
            peer.answer(StoreReply::Declined { reason });
            return;
        }
        let Some(entry) = self.write_alone(tick, &record) else {
            return;
        };

        // Read before the table makes them the survivor's, which then cannot tell them
        // from its own: the survivor is not opened anew, and nothing else tells it
        // the areas it is pinned to from now on.
        let pinned = self.table.pinned(absorbed).to_vec();
        let chunks = self
            .table
            .absorb(region, absorbed, tick)
            .expect("two living regions, as was looked at above");
        self.table_last = Some(entry.segment);
        if let Some(mut gone) = self.regions.remove(&absorbed) {
            // Its owner gets nothing done any more; what it had the thread for chunks
            // do is done before anything the survivor asks from now on.
            lose(&mut gone);
            let mut removed = Vec::new();
            if gone.state_tick.is_some() {
                removed.push(self.state_path(absorbed));
            }
            if gone.file.is_some_and(|file| is_empty(file.entity_ids)) {
                removed.push(self.region_path(absorbed));
            }
            // If this fails, the files are removed when the store starts.
            let done = removed
                .iter()
                .try_for_each(|path| self.disk.remove(path))
                .and_then(|()| match removed.is_empty() {
                    true => Ok(()),
                    false => self.disk.sync_directory(&self.regions_directory()),
                });
            if let Err(error) = done {
                warn!(%absorbed, %error, "the files of an absorbed region could not be removed yet");
            }
        }
        let lane = self.regions.get_mut(&region).expect("the survivor's lane");
        lane.record = Some(entry);
        lane.latest = tick;
        lane.named = tick;
        info!(%region, %absorbed, tick, "a region has absorbed another");
        peer.answer(StoreReply::Absorbed {
            absorbed,
            chunks,
            pinned,
        });
    }

    /// Whether the region `region` may absorb `absorbed` now, with the record made for
    /// it. Nothing is changed.
    fn may_absorb(
        &self,
        region: RegionId,
        absorbed: RegionId,
        absorbed_epoch: u64,
        tick: u64,
        record: &[u8],
    ) -> Result<(), Decline> {
        let (Some(survivor), Some(other)) =
            (self.regions.get(&region), self.regions.get(&absorbed))
        else {
            return Err(Decline::NoSuchRegion);
        };
        if absorbed == region {
            return Err(Decline::NoSuchRegion);
        }
        if absorbed == self.table.home_region {
            return Err(Decline::Home);
        }
        // Whoever asks is the one that was told to absorb the region only if it has
        // the region open with the epoch it was told: the store does not know workers.
        let opened = other.owner.as_ref().map(|owner| owner.epoch);
        if opened != Some(absorbed_epoch) {
            return Err(Decline::NotOpened { epoch: opened });
        }
        // The chunks and areas that come to the survivor count as held from the tick of
        // the merge or from 0, which is only right if no commit of either region from
        // before is left to be replayed.
        for (id, lane) in [(region, survivor), (absorbed, other)] {
            if !lane.live.is_empty() {
                return Err(Decline::Uncheckpointed { region: id });
            }
        }
        if tick <= survivor.named {
            return Err(Decline::Tick {
                named: survivor.named,
            });
        }
        if record.len() > MAX_RECORD_LENGTH {
            return Err(Decline::TooLarge);
        }
        Ok(())
    }

    /// The split of section 3.7 of ADR-0011: the chunks of `part` leave the region of
    /// `session` for the new region `new`, which is made with `as_epoch` as the
    /// highest epoch it was opened with. It is one record of the log, as a merge is.
    ///
    /// Whoever asks names the new region, as the state it hands in names it to edges;
    /// the store only holds it to the next id (ADR-0014, section 9).
    fn split(
        &mut self,
        session: Session,
        tick: u64,
        state: Vec<u8>,
        part: SplitPart,
        as_epoch: u64,
        new: RegionId,
    ) {
        self.end_group();
        let region = session.region;
        let Some((peer, epoch)) = self.owner_of(session) else {
            return;
        };
        let SplitPart {
            chunks,
            state: part_state,
        } = part;
        // Each chunk once.
        let chunks: Vec<ChunkPos> = chunks
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let record = LogRecord::Split {
            region: region.0,
            epoch,
            tick,
            state,
            part: new.0,
            part_epoch: as_epoch,
            chunks: chunks.clone(),
            part_state,
        }
        .encode();
        let next = RegionId(self.table.next_region);
        let declined = match self.may_split(region, tick, &chunks, as_epoch) {
            Ok(()) if record.len() > MAX_RECORD_LENGTH => Err(Decline::TooLarge),
            // Looked at last, so that whoever is told the next id knows that nothing
            // else stands in the way of the same split with it.
            Ok(()) if new != next => Err(Decline::NotNext { next }),
            other => other,
        };
        if let Err(reason) = declined {
            info!(%region, ?reason, "a split is declined");
            peer.answer(StoreReply::Declined { reason });
            return;
        }
        let Some(entry) = self.write_alone(tick, &record) else {
            return;
        };

        self.table
            .split(region, new, tick, &chunks)
            .expect("the next id and chunks the region holds, as was looked at above");
        self.table_last = Some(entry.segment);
        let lane = self
            .regions
            .get_mut(&region)
            .expect("the lane of the region");
        // A chunk whose return was on its way is the new region's: the message that
        // comes later finds no number for it, and frees nothing.
        for chunk in &chunks {
            lane.returning.remove(chunk);
        }
        lane.record = Some(entry);
        lane.latest = tick;
        lane.named = tick;
        let file = RegionFile {
            epoch: as_epoch,
            entity_ids: NO_ENTITY_IDS,
        };
        let made = Lane {
            file: Some(file),
            // Until it is on disk; the record has the epoch meanwhile, and its segment
            // stays for as long.
            unwritten: true,
            record: Some(entry),
            latest: tick,
            named: tick,
            ..Lane::default()
        };
        self.regions.insert(new, made);
        if let Err(error) = self.write_region_files() {
            warn!(region = %new, %error, "the file of a new region could not be written yet");
        }
        info!(%region, part = %new, tick, "a region was split");
        peer.answer(StoreReply::Split { region: new });
    }

    /// Whether the region `region` may be split now, apart from how long the record of
    /// it would be and from the id the new region is to have. Nothing is changed.
    fn may_split(
        &self,
        region: RegionId,
        tick: u64,
        chunks: &[ChunkPos],
        as_epoch: u64,
    ) -> Result<(), Decline> {
        let lane = self.regions.get(&region).ok_or(Decline::NoSuchRegion)?;
        if !lane.live.is_empty() {
            return Err(Decline::Uncheckpointed { region });
        }
        if tick <= lane.named {
            return Err(Decline::Tick { named: lane.named });
        }
        if chunks.is_empty() || as_epoch == 0 {
            return Err(Decline::Malformed);
        }
        if let Some(chunk) = chunks
            .iter()
            .find(|chunk| self.table.held_from(region, **chunk).is_none())
        {
            return Err(Decline::NotHeld { chunk: *chunk });
        }
        if chunks.contains(&self.table.home_chunk) {
            return Err(Decline::Home);
        }
        Ok(())
    }

    /// The handle of `session` and the epoch it opened its region with, if it still
    /// owns the region.
    fn owner_of(&self, session: Session) -> Option<(Arc<Peer>, u64)> {
        let owner = self.regions.get(&session.region)?.owner.as_ref()?;
        (owner.peer.session.number == session.number)
            .then(|| (Arc::clone(&owner.peer), owner.epoch))
    }

    /// Appends the record of a merge or a split and makes it durable by itself, apart
    /// from any group: what such a record changes could not be undone simply, so
    /// nothing is changed before it is durable. Returns where the record is, or `None`
    /// if it could not be written, after which every region has lost its owner and
    /// nothing is answered.
    fn write_alone(&mut self, tick: u64, record: &[u8]) -> Option<Entry> {
        let written = self
            .log
            .append(record)
            .and_then(|at| self.log.sync().map(|()| at));
        match written {
            Ok((segment, offset)) => Some(Entry {
                tick,
                segment,
                offset,
                length: record.len(),
            }),
            Err(error) => {
                error!(%error, "a merge or a split could not be written to the log");
                self.fail_log();
                None
            }
        }
    }

    /// Writes the region file of every region whose file is not on disk as it is known
    /// here, which is that of a region a split has made, and makes them durable.
    fn write_region_files(&mut self) -> io::Result<()> {
        let unwritten: Vec<(RegionId, RegionFile)> = self
            .regions
            .iter()
            .filter(|(_, lane)| lane.unwritten)
            .filter_map(|(region, lane)| Some((*region, lane.file?)))
            .collect();
        if unwritten.is_empty() {
            return Ok(());
        }
        for (region, file) in &unwritten {
            replace(
                self.disk.as_ref(),
                &self.region_path(*region),
                &file.encode(),
            )?;
        }
        self.disk.sync_directory(&self.regions_directory())?;
        for (region, _) in unwritten {
            if let Some(lane) = self.regions.get_mut(&region) {
                lane.unwritten = false;
            }
        }
        Ok(())
    }

    /// Makes the world over for another division than it had: puts the block changes
    /// of every region's commits into the stored chunks, and then lets go of the
    /// regions' commits and states. The commits of those regions mean nothing to the
    /// regions of the new division, but the blocks they changed belong to the world.
    ///
    /// In an order that makes doing it again harmless: until the table of the new
    /// division is durable, which is the caller's next step, a store that starts finds
    /// the old one, or the old layout file, and does all of this again.
    ///
    /// `held` says whether the world has a table by which to tell what a region held:
    /// if so, the changes are chosen as for an opening, and otherwise all are taken.
    fn make_over(&mut self, held: bool) -> Result<(), StoreError> {
        let left: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, lane)| {
                !lane.live.is_empty() || lane.state_tick.is_some() || lane.record.is_some()
            })
            .map(|(region, _)| *region)
            .collect();
        if left.is_empty() {
            return Ok(());
        }
        let mut changes = Vec::new();
        for (region, lane) in &self.regions {
            let mut chosen = BTreeMap::new();
            for entry in &lane.live {
                if let Some((tick, changed, _)) = self.log.read(entry)? {
                    chosen.insert(tick, changed);
                }
            }
            for (tick, changed) in chosen {
                if held {
                    changes.extend(replayed(&self.table, *region, tick, changed));
                } else {
                    changes.extend(changed);
                }
            }
        }
        let (done, finished) = mpsc::channel();
        let _ = self.jobs.send(Job::Fold { changes, done });
        // No region is open before the store has started, so waiting here keeps no
        // commit waiting.
        finished
            .recv()
            .map_err(|_| io::Error::other("the thread for chunks has gone"))??;

        // The commits are passed over from now on, and the states go. What is known
        // here is changed only once that is durable.
        for region in &left {
            let opened = LogRecord::Opened {
                region: region.0,
                epoch: self.regions[region].file.map_or(0, |file| file.epoch),
                // Ticks are numbered from 1.
                restored: 0,
            };
            if let Err(error) = self.log.append(&opened.encode()) {
                self.fail_log();
                return Err(error.into());
            }
        }
        if let Err(error) = self.log.sync() {
            self.fail_log();
            return Err(error.into());
        }
        for region in &left {
            let lane = self.regions.get_mut(region).expect("lanes that are left");
            lane.live.clear();
            lane.record = None;
            if lane.state_tick.is_some() {
                self.disk.remove(&self.state_path(*region))?;
            }
        }
        self.disk.sync_directory(&self.regions_directory())?;
        for region in &left {
            let lane = self.regions.get_mut(region).expect("lanes that are left");
            lane.state_tick = None;
        }
        self.log.close();
        self.collect();
        Ok(())
    }

    /// Brings the files of the regions in line with the table, and the lanes: a region
    /// the table does not have has no lane and no state file, and no region file
    /// either unless that has a block of entity ids, which has to stay known so that it
    /// is not issued again. Every living region has a lane.
    fn align(&mut self) -> Result<(), StoreError> {
        let gone: Vec<RegionId> = self
            .regions
            .keys()
            .filter(|region| !self.table.has(**region))
            .copied()
            .collect();
        let mut removed = false;
        for region in gone {
            let lane = self.regions.remove(&region).expect("a lane that is there");
            if lane.state_tick.is_some() {
                self.disk.remove(&self.state_path(region))?;
                removed = true;
            }
            if lane.file.is_some_and(|file| is_empty(file.entity_ids)) {
                self.disk.remove(&self.region_path(region))?;
                removed = true;
            }
        }
        if removed {
            self.disk.sync_directory(&self.regions_directory())?;
        }
        for region in self.table.regions() {
            self.regions.entry(region).or_default();
        }
        // The file of a region that a split made, if the store died before it was
        // written: the record of the split had its epoch.
        self.write_region_files()?;
        Ok(())
    }
}

/// The table the records of `segment` change: the one the world has, if the segment is
/// not one its file stands for already. A record for the table in a world without one
/// is a log that does not fit.
fn changed_by(stored: &mut Option<Table>, segment: u64) -> Result<Option<&mut Table>, StoreError> {
    match stored {
        Some(table) if segment >= table.from => Ok(Some(table)),
        Some(_) => Ok(None),
        None => Err(StoreError::Table(
            "the log has a record of regions and there is no table of them".to_owned(),
        )),
    }
}

/// Of the block changes a commit of `region` with `tick` made, those that are put into
/// the stored chunks when the commit is replayed: those in chunks the region holds
/// now, made after the tick it holds the chunk from. What it did to a chunk before
/// that is in the chunk as it gave it away, and has perhaps been built over since.
fn replayed(
    table: &Table,
    region: RegionId,
    tick: u64,
    changes: Vec<(BlockPos, BlockState)>,
) -> impl Iterator<Item = (BlockPos, BlockState)> + '_ {
    changes.into_iter().filter(move |(position, _)| {
        table
            .held_from(region, position.chunk())
            .is_some_and(|from| tick > from)
    })
}

/// The block of entity ids of a region that has none.
const NO_ENTITY_IDS: EntityIds = EntityIds {
    first: EntityId(0),
    end: EntityId(0),
};

fn is_empty(entity_ids: EntityIds) -> bool {
    entity_ids.first.0 >= entity_ids.end.0
}

/// The region's owner, if any, gets nothing done any more, and the region is read
/// afresh when it is opened next.
fn lose(lane: &mut Lane) {
    if let Some(owner) = lane.owner.take() {
        owner.peer.mark_lost();
    }
    lane.current = None;
    lane.installing = None;
}

fn parse_layout(bytes: &[u8]) -> Option<u64> {
    u64::from_str_radix(std::str::from_utf8(bytes).ok()?.trim(), 16).ok()
}

/// The log, which all regions share so that one sync makes the commits of all of them
/// durable.
struct Log {
    disk: Arc<dyn Disk>,
    directory: PathBuf,
    /// The segments there are, the one appended to among them.
    segments: BTreeSet<u64>,
    /// The segment appended to, once something has been since the last was closed.
    active: Option<Active>,
    /// The number of the next segment.
    next: u64,
    /// The segment a write or a sync of which failed, and how much of it is good, for as
    /// long as cutting it back to that is not durable.
    unsettled: Option<(u64, u64)>,
}

struct Active {
    number: u64,
    /// How much of it is durable.
    durable: u64,
    /// How much of it has been written.
    written: u64,
    /// Whether its directory has been synced since it was begun, which is what makes
    /// the file itself durable.
    announced: bool,
}

impl Log {
    fn path(&self, segment: u64) -> PathBuf {
        segment_path(&self.directory, segment)
    }

    /// Appends a framed record. Returns the segment and the offset it went to.
    fn append(&mut self, record: &[u8]) -> io::Result<(u64, u64)> {
        if self.unsettled.is_some() {
            // Nobody is served meanwhile, so nothing gets here; were it written, a
            // crash could leave it beside what was cut off before it.
            return Err(io::Error::other(
                "a segment of the log is not durably cut back",
            ));
        }
        let active = match &mut self.active {
            Some(active) => active,
            None => {
                let number = self.next;
                self.next += 1;
                self.segments.insert(number);
                self.active.insert(Active {
                    number,
                    durable: 0,
                    written: 0,
                    announced: false,
                })
            }
        };
        let offset = active.written;
        self.disk
            .append(&segment_path(&self.directory, active.number), record)?;
        active.written += record.len() as u64;
        Ok((active.number, offset))
    }

    /// Whether something has been written that is not durable yet.
    fn unsynced(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.written > active.durable || !active.announced)
    }

    fn sync(&mut self) -> io::Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        let path = segment_path(&self.directory, active.number);
        if active.written > active.durable {
            self.disk.sync(&path)?;
        }
        if !active.announced {
            self.disk.sync_directory(&self.directory)?;
            active.announced = true;
        }
        active.durable = active.written;
        if active.written >= SEGMENT_LIMIT {
            self.active = None;
        }
        Ok(())
    }

    /// After a failed append or sync: cuts the segment back to what was durable, and
    /// appends nothing to it any more, so that if cutting it fails as well, what is left
    /// of a record hides no record that follows. Returns the segment and how much of it
    /// is good.
    fn fail(&mut self) -> Option<(u64, u64)> {
        let active = self.active.take()?;
        self.unsettled = Some((active.number, active.durable));
        if let Err(error) = self.settle() {
            warn!(%error, "a segment of the log could not be cut back; nobody is served until it is");
        }
        Some((active.number, active.durable))
    }

    /// Cuts the segment a write or a sync of which failed back to what was durable, and
    /// makes that durable, if it is not yet. Until it is, a crash can leave what was cut
    /// off, which was never answered and must not count beside what is written later.
    ///
    /// The sync here does not try again what failed: nothing is done to make the group
    /// durable after all. It is of the file's new length.
    fn settle(&mut self) -> io::Result<()> {
        let Some((segment, durable)) = self.unsettled else {
            return Ok(());
        };
        let path = self.path(segment);
        // A segment whose first append failed before there was a file has nothing to
        // cut back.
        if self.disk.exists(&path)? {
            self.disk.truncate(&path, durable)?;
            self.disk.sync(&path)?;
        }
        self.unsettled = None;
        Ok(())
    }

    /// Appends go to a new segment from now on. Only for a segment that is durable.
    fn close(&mut self) {
        if !self.unsynced() {
            self.active = None;
        }
    }

    /// Reads the record at `entry`, if there is a whole one.
    fn record(&self, entry: &Entry) -> Result<Option<LogRecord>, StoreError> {
        let path = self.path(entry.segment);
        let bytes = self.disk.read_at(&path, entry.offset, entry.length)?;
        let (records, _) = read_log(&bytes).map_err(|error| StoreError::Damaged {
            path: path.clone(),
            error,
        })?;
        Ok(records.into_iter().next())
    }

    /// Reads the commit at `entry`: its tick, block changes and state.
    #[allow(clippy::type_complexity)]
    fn read(
        &self,
        entry: &Entry,
    ) -> Result<Option<(u64, Vec<(BlockPos, BlockState)>, Vec<u8>)>, StoreError> {
        Ok(match self.record(entry)? {
            Some(LogRecord::Commit {
                tick,
                changes,
                state,
                ..
            }) => Some((tick, changes, state)),
            _ => None,
        })
    }

    /// Reads the whole state of `region` that the record of a merge or a split at
    /// `entry` has: of a split, that of the region that was split or that of the part.
    fn whole_state(&self, entry: &Entry, region: RegionId) -> Result<StateFile, StoreError> {
        let state = match self.record(entry)? {
            Some(LogRecord::Absorbed {
                region: survivor,
                tick,
                state,
                ..
            }) if survivor == region.0 => Some(StateFile { tick, state }),
            Some(LogRecord::Split {
                region: old,
                tick,
                state,
                part,
                part_state,
                ..
            }) => {
                if old == region.0 {
                    Some(StateFile { tick, state })
                } else if part == region.0 {
                    let state = part_state;
                    Some(StateFile { tick, state })
                } else {
                    None
                }
            }
            _ => None,
        };
        state.ok_or_else(|| {
            StoreError::Table(format!(
                "the log has no state of region {region} where the merge or split it went through was written"
            ))
        })
    }

    /// Removes segments from the start of the log for as long as `unneeded` says so of
    /// them. The one appended to stays.
    fn remove_while(&mut self, unneeded: impl Fn(u64) -> bool) {
        let active = self.active.as_ref().map(|active| active.number);
        while let Some(&segment) = self.segments.first() {
            if Some(segment) == active || !unneeded(segment) {
                break;
            }
            // Should the removal not survive a crash, the segment is read again and its
            // records passed over for their ticks.
            if let Err(error) = self.disk.remove(&self.path(segment)) {
                warn!(%error, segment, "a segment of the log could not be removed");
                break;
            }
            self.segments.remove(&segment);
        }
    }
}

fn segment_path(directory: &Path, segment: u64) -> PathBuf {
    directory.join(format!("{segment:020}.wal"))
}
