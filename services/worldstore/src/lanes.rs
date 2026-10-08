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
//! ```text
//! log/<n>.wal          the log, in segments numbered in the order they were begun
//! regions/<r>.region   per region, its highest epoch and its entity ids
//! regions/<r>.state    per region, its state as of its last checkpoint
//! layout               the fingerprint of the layout the regions are part of
//! ```
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
    FormatError, LogRecord, Logged, RegionFile, StateFile, read_log, read_log_with_offsets,
};
use clustine_region::RegionId;
use clustine_rpc::{RegionHello, Restored, StoreReply, StoreRequest, TickState};
use clustine_world::{BlockPos, EntityIds};
use tracing::{error, info, warn};

use crate::chunks::Job;
use crate::disk::{Disk, replace};
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
    /// The fingerprint of the layout the regions are part of, known from the first hello.
    layout: Option<u64>,
    /// Whether the layout file may have been written without being made durable.
    layout_unsure: bool,
    regions: BTreeMap<RegionId, Lane>,
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
    /// Reads what the world in `root` has: the regions' files and the log. Nothing is
    /// changed, except that temporary files of writes that never finished are removed.
    pub(crate) fn load(
        disk: Arc<dyn Disk>,
        root: &Path,
        jobs: Sender<Job>,
    ) -> Result<Self, StoreError> {
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

        let mut log = Log {
            disk: Arc::clone(&disk),
            directory: log_directory,
            segments: BTreeSet::new(),
            active: None,
            next: 1,
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

        Ok(Self {
            disk,
            root: root.to_owned(),
            jobs,
            log,
            layout: None,
            layout_unsure: false,
            regions,
            sessions: 0,
            group: Group::default(),
        })
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
            Message::Flushed(peer) => self.group.flushes.push(peer),
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
            self.fail_log();
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
    /// off, and every region that wrote to it loses its owner, which is not answered.
    fn fail_log(&mut self) {
        let Some((segment, durable)) = self.log.fail() else {
            return;
        };
        for region in mem::take(&mut self.group.regions) {
            let lane = self.regions.get_mut(&region).expect("a group is of lanes");
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
        match self.layout {
            Some(expected) if expected != layout => {
                return Err(StoreError::LayoutMismatch {
                    expected,
                    offered: layout,
                });
            }
            Some(_) => {}
            None => self.decide_layout(layout)?,
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
        if file.map(|file| file.epoch) != Some(epoch) {
            let entity_ids = match file {
                Some(file) => file.entity_ids,
                None => self.allocate()?,
            };
            let updated = RegionFile { epoch, entity_ids };
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
        };
        let opened = Opened {
            session,
            replies,
            lost,
        };
        Ok((opened, restored, changes, restored_tick, peer))
    }

    /// The entity ids for a region opened for the first time: a block no region has had.
    fn allocate(&self) -> Result<EntityIds, StoreError> {
        let next = self
            .regions
            .values()
            .filter_map(|lane| lane.file)
            .map(|file| (file.entity_ids.first.0 / EntityIds::BLOCK_SIZE) as u32 + 1)
            .max()
            .unwrap_or(0);
        EntityIds::block(next).ok_or(StoreError::OutOfEntityIds)
    }

    /// Takes the layout of the first hello as the one the regions are part of. If the
    /// world was last run with another, what its regions have that is not in the stored
    /// chunks is put there first: the commits of those regions mean nothing to the
    /// regions of this layout, but the blocks they changed belong to the world.
    fn decide_layout(&mut self, offered: u64) -> Result<(), StoreError> {
        let path = self.root.join("layout");
        let stored = match self.disk.read(&path)? {
            Some(bytes) => Some(parse_layout(&bytes).ok_or_else(|| {
                StoreError::MalformedMeta("the layout file is not a fingerprint".to_owned())
            })?),
            None => None,
        };
        // A file this store wrote and failed to make durable is there to be read, and is
        // written again all the same.
        if stored != Some(offered) || self.layout_unsure {
            let left = self
                .regions
                .values()
                .any(|lane| !lane.live.is_empty() || lane.state_tick.is_some());
            if left {
                self.fold()?;
            }
            self.layout_unsure = true;
            replace(
                self.disk.as_ref(),
                &path,
                format!("{offered:016x}\n").as_bytes(),
            )?;
            self.disk.sync_directory(&self.root)?;
            self.layout_unsure = false;
        }
        self.layout = Some(offered);
        Ok(())
    }

    /// Puts the block changes of every region's commits into the stored chunks, and
    /// then lets go of the regions' commits and states.
    fn fold(&mut self) -> Result<(), StoreError> {
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
        // No region is open before the layout is known, so waiting here keeps no commit
        // waiting.
        finished
            .recv()
            .map_err(|_| io::Error::other("the thread for chunks has gone"))??;
        info!(
            "the world was last run with another layout; what its regions had is in the stored chunks now"
        );

        // The commits are passed over from now on, and the states go. What is known
        // here is changed only once that is durable: until then, a hello that comes
        // after a failure does all of this again, as does a store started after a crash,
        // because the layout on disk is still the old one.
        let folded: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, lane)| !lane.live.is_empty() || lane.state_tick.is_some())
            .map(|(region, _)| *region)
            .collect();
        for region in &folded {
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
        for region in &folded {
            let lane = self
                .regions
                .get_mut(region)
                .expect("folded lanes are lanes");
            lane.live.clear();
            if lane.state_tick.is_some() {
                self.disk.remove(&self.state_path(*region))?;
            }
        }
        self.disk.sync_directory(&self.regions_directory())?;
        for region in &folded {
            let lane = self
                .regions
                .get_mut(region)
                .expect("folded lanes are lanes");
            lane.state_tick = None;
        }
        self.log.close();
        self.collect();
        Ok(())
    }
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
        let path = self.path(active.number);
        if let Err(error) = self.disk.truncate(&path, active.durable) {
            warn!(%error, "a segment of the log could not be cut back");
        }
        Some((active.number, active.durable))
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
