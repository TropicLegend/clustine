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
    FormatError, LogRecord, Logged, RegionFile, StateFile, TableFile, read_log,
    read_log_with_offsets,
};
use clustine_region::RegionId;
use clustine_rpc::{RegionHello, RegionList, Restored, StoreReply, StoreRequest, TickState};
use clustine_world::{BlockPos, EntityId, EntityIds};
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
    /// What is on disk about it, once it has been opened.
    file: Option<RegionFile>,
    /// The tick of its state file, if it has one.
    state_tick: Option<u64>,
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
}

/// Where a commit is in the log.
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
    /// The ticks of the commits appended in this group.
    unsynced: Vec<u64>,
    /// What the handle asked for after those commits, for the thread for chunks, which
    /// it is given only once they are durable.
    held: Vec<Job>,
}

/// What the current group has done that is answered or finished when it ends.
#[derive(Default)]
struct Group {
    /// The regions that appended commits.
    regions: BTreeSet<RegionId>,
    /// The regions whose state file was put in place.
    installs: BTreeSet<RegionId>,
    /// Handles to be told that everything they asked for is done.
    flushes: Vec<Arc<Peer>>,
}

impl Group {
    fn is_empty(&self) -> bool {
        self.regions.is_empty() && self.installs.is_empty() && self.flushes.is_empty()
    }
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
        let stored = match disk.read(&table_path)? {
            Some(bytes) => {
                let file = TableFile::decode(&bytes).map_err(|error| StoreError::Damaged {
                    path: table_path.clone(),
                    error,
                })?;
                Some(Table::read(file)?)
            }
            None => None,
        };

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
                        }
                    }
                    LogRecord::Changes { .. } => {
                        warn!(
                            ?path,
                            "a record of a world from before regions had a state is passed over"
                        );
                    }
                    // Of regions that hold chunks and merge and split (ADR-0011), which
                    // the store does not keep yet: to it they are what they were before
                    // the format had them, records of no kind it knows.
                    LogRecord::Granted { .. }
                    | LogRecord::Returned { .. }
                    | LogRecord::Absorbed { .. }
                    | LogRecord::Split { .. } => {
                        return Err(StoreError::Damaged {
                            path,
                            error: FormatError::Corrupt("record kind"),
                        });
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

        // A world from before there was a table says in this file how it was divided.
        let layout_path = root.join("layout");
        let before = match disk.read(&layout_path)? {
            Some(bytes) => Some(parse_layout(&bytes).ok_or_else(|| {
                StoreError::MalformedMeta("the layout file is not a fingerprint".to_owned())
            })?),
            None => None,
        };
        let remake = match &stored {
            Some(table) => !table.is_of(told),
            None => before.is_some_and(|layout| Some(layout) != told.layout),
        };
        let keep = stored.is_some() && !remake;
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
            next_block,
            sessions: 0,
            group: Group::default(),
        };
        if remake {
            lanes.make_over()?;
        }
        if !keep {
            // What changes the table from now on goes to a segment the file names.
            lanes.log.close();
            let from = lanes.log.next;
            lanes.table = Table::made_from(told, used, from);
            lanes.write_table(from)?;
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
            Message::Close { session } => {
                // What the handle asked for before is done first, as far as it is up to
                // this thread: its commits answered and its saves passed on.
                self.end_group();
                if let Some(lane) = self.regions.get_mut(&session.region) {
                    lane.owner
                        .take_if(|owner| owner.peer.session.number == session.number);
                }
            }
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

    /// Does what the owner `session` asks for, if it still owns its region.
    fn request(&mut self, session: Session, request: StoreRequest) {
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
                        owner.unsynced.push(tick);
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
            StoreRequest::Checkpoint { tick, state } => Job::Checkpoint { tick, state, peer },
            StoreRequest::Flush => Job::Flush { peer },
            // Of regions that hold chunks and merge and split (ADR-0010), which the
            // store does not keep yet. No worker asks for these.
            request @ (StoreRequest::Claim { .. }
            | StoreRequest::Return { .. }
            | StoreRequest::AbsorbCommit { .. }
            | StoreRequest::SplitCommit { .. }) => {
                error!(region = %session.region, ?request, "asked for what the store does not do yet");
                return;
            }
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
                    lane.state_tick = Some(tick);
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
        }

        for region in &group.regions {
            let Some(owner) = self
                .regions
                .get_mut(region)
                .and_then(|lane| lane.owner.as_mut())
            else {
                continue;
            };
            for tick in owner.unsynced.drain(..) {
                owner.peer.answer(StoreReply::Committed { tick });
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
        let Some((segment, durable)) = self.log.fail() else {
            return;
        };
        self.group = Group::default();
        for lane in self.regions.values_mut() {
            lane.live
                .retain(|entry| entry.segment != segment || entry.offset < durable);
            lose(lane);
        }
    }

    /// Removes the segments at the start of the log that hold nothing needed any more.
    fn collect(&mut self) {
        let needed: BTreeSet<u64> = self
            .regions
            .values()
            .flat_map(|lane| lane.live.iter().map(|entry| entry.segment))
            .collect();
        self.log.remove_while(|segment| !needed.contains(&segment));
    }

    /// Puts the state file the thread for chunks has written for `session` in place, if
    /// the session is still the one that opened the region last.
    fn install(&mut self, session: Session, tick: u64, temporary: &Path) {
        let path = self.state_path(session.region);
        let Some(lane) = self.regions.get_mut(&session.region) else {
            let _ = self.disk.remove(temporary);
            return;
        };
        let later = lane.state_tick.is_none_or(|state| tick > state)
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

    /// Opens a region for whoever said `hello`, and has the thread for chunks hand it
    /// over once the region's commits are in the stored chunks.
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
            return Err(StoreError::UnknownRegion { region });
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
        if file != Some(updated) {
            // On disk before anyone is told that the region is theirs, so that an owner
            // with a lower epoch is refused after a restart too.
            replace(
                self.disk.as_ref(),
                &self.region_path(region),
                &updated.encode(),
            )?;
            self.disk.sync_directory(&self.regions_directory())?;
            self.regions.entry(region).or_default().file = Some(updated);
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

        let mut changes = Vec::new();
        let mut deltas = Vec::new();
        for (tick, (changed, state)) in chosen {
            changes.extend(changed);
            deltas.push(TickState { tick, state });
        }
        let restored = Restored {
            entity_ids,
            state: state.map(|StateFile { tick, state }| TickState { tick, state }),
            deltas,
            held: Vec::new(),
            pinned: self.table.pinned(region).to_vec(),
        };
        let opened = Opened {
            session,
            replies,
            lost,
        };
        Ok((opened, restored, changes, restored_tick, peer))
    }

    /// Makes the world over for another division than it had: puts the block changes
    /// of every region's commits into the stored chunks, and then lets go of the
    /// regions' commits and states. The commits of those regions mean nothing to the
    /// regions of the new division, but the blocks they changed belong to the world.
    ///
    /// In an order that makes doing it again harmless: until the table of the new
    /// division is durable, which is the caller's next step, a store that starts finds
    /// the old one, or the old layout file, and does all of this again.
    fn make_over(&mut self) -> Result<(), StoreError> {
        let left: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, lane)| !lane.live.is_empty() || lane.state_tick.is_some())
            .map(|(region, _)| *region)
            .collect();
        if left.is_empty() {
            return Ok(());
        }
        let mut changes = Vec::new();
        for lane in self.regions.values() {
            let mut chosen = BTreeMap::new();
            for entry in &lane.live {
                if let Some((tick, changed, _)) = self.log.read(entry)? {
                    chosen.insert(tick, changed);
                }
            }
            changes.extend(chosen.into_values().flatten());
        }
        let (done, finished) = mpsc::channel();
        let _ = self.jobs.send(Job::Fold { changes, done });
        // No region is open before the store has started, so waiting here keeps no
        // commit waiting.
        finished
            .recv()
            .map_err(|_| io::Error::other("the thread for chunks has gone"))??;
        info!(
            "the world was divided otherwise before; what its regions had is in the stored chunks now"
        );

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
        Ok(())
    }
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

    /// Reads the commit at `entry`: its tick, block changes and state.
    #[allow(clippy::type_complexity)]
    fn read(
        &self,
        entry: &Entry,
    ) -> Result<Option<(u64, Vec<(BlockPos, BlockState)>, Vec<u8>)>, StoreError> {
        let path = self.path(entry.segment);
        let bytes = self.disk.read_at(&path, entry.offset, entry.length)?;
        let (records, _) = read_log(&bytes).map_err(|error| StoreError::Damaged {
            path: path.clone(),
            error,
        })?;
        Ok(records.into_iter().find_map(|record| match record {
            LogRecord::Commit {
                tick,
                changes,
                state,
                ..
            } => Some((tick, changes, state)),
            _ => None,
        }))
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
