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
//! regions/players      what is kept of every player who ever joined
//! regions/<r>.region   per region, its highest epoch and its entity ids
//! regions/<r>.state    per region, its state as of its last checkpoint
//! layout               only in a world from before there was a table; that it is
//!                      there says so, and what it says is not read
//! ```
//!
//! The table of regions is this thread's alone: it decides who holds a chunk, in the
//! order the messages arrive. See `table.rs`.
//!
//! So are the records of the players: a commit says what its region knows of stays,
//! and the store keeps of each player the latest stay and its place. See `players.rs`
//! and `docs/adr/0020-one-stay-per-player.md`.
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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use clustine_format::{
    BlocksFile, CouplingsFile, LogRecord, Logged, LoggedPlace, LoggedStay, MAX_RECORD_LENGTH,
    PlayersFile, RegionFile, StateFile, TableFile, read_log, read_log_with_offsets,
};
use clustine_region::RegionId;
use clustine_rpc::{
    CopyUse, Decline, Lends as LendsOf, RegionHello, RegionList, Restored, SplitPart, StayNote,
    StoreReply, StoreRequest, TickState,
};
use clustine_world::{
    BlockPos, Change as BlockChange, ChunkPos, Due, Effect, EffectId, EntityId, EntityIds,
    FiledEffect, PlayerId,
};
use tracing::{error, info, warn};

use crate::chunks::{Job, Jobs, Listed, Replayed};
use crate::copies::Copies;
use crate::couplings::{Couplings, Said};
use crate::disk::{Disk, replace};
use crate::effects::{self, Filed, Kept, Uses, sequence};
use crate::lends::{self, Lend, Lends};
use crate::players::{self, Players, Taken, Undo};
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

/// At most this many effects are dropped or written forward by one record of the log,
/// for the same reason.
const RECORD_EFFECTS: usize = 65_536;

/// How often the commit thread looks at the time while regions are coupled or in a
/// handshake, when nothing else wakes it.
const COUPLING_LOOK: Duration = Duration::from_millis(10);

/// The commit thread's clock: the time since the store started. Tests hand in one they
/// move themselves.
pub(crate) type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

fn real_clock() -> Clock {
    let start = Instant::now();
    Arc::new(move || start.elapsed())
}

/// The commit thread.
pub(crate) struct Lanes {
    disk: Arc<dyn Disk>,
    root: PathBuf,
    jobs: Jobs,
    log: Log,
    /// The regions there are and the chunks each holds.
    table: Table,
    /// The world's spawn ([`Restored::spawn`]). The table's home chunk is its chunk.
    spawn: BlockPos,
    /// The world's number ([`Restored::world`]).
    world: u64,
    /// A lane for every living region, and for no other.
    regions: BTreeMap<RegionId, Lane>,
    /// The last segment of the log that has a record which changes the table and is not
    /// in the table file, if there is one. The segments from the file's `from` up to it
    /// hold what the file does not have.
    table_last: Option<u64>,
    /// What is kept of every player who ever joined.
    players: Players,
    /// The index of the next block of entity keys: above every block a region was ever
    /// given, so that none is given twice (`docs/adr/0028-entities.md`, section 1).
    next_block: u32,
    /// The first block not reserved in the blocks' file: blocks from `next_block` up to
    /// it may be given without writing it.
    reserved: u32,
    /// How many handles have been handed out.
    sessions: u64,
    /// Which region uses which chunk (`docs/adr/0025-effects-between-regions.md`,
    /// section 5). Not in the log: after a start every chunk a region holds by the
    /// table is in use by it, which is more than was and so loses nothing.
    uses: Uses,
    /// The effects filed for chunks in use and not taken yet.
    filed: Filed,
    /// Who copies which chunk (the same record, section 3). Not in the log: a region
    /// asks for its copies again after every hello.
    copies: Copies,
    /// Lockstep between regions (the same record, section 8), kept in
    /// `regions/couplings`.
    couplings: Couplings,
    /// The lends under way (`docs/adr/0027-lending-at-a-line.md`, section 3).
    lends: Lends,
    clock: Clock,
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
    /// The returns of that session that give chunks back for a lend, each with the
    /// lend's number (`docs/adr/0027-lending-at-a-line.md`, section 3, step 3).
    lending: BTreeMap<u64, u64>,
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
    /// What the group did to the records of the players, which is taken back if the
    /// group cannot be made durable.
    undo: Undo,
    /// The floors the group raised: of each player named, every stay below this one
    /// is dead, which every region that has an owner is told once the group is
    /// durable (ADR-0020, section 5).
    dead: Vec<(PlayerId, EntityId, u32)>,
    /// What the group did to the effects filed, which is taken back if the group
    /// cannot be made durable.
    effects: Vec<effects::Undo>,
    /// What goes on each region's stream once the group is durable, in order.
    stream: BTreeMap<RegionId, Vec<Streamed>>,
    /// The copies asked for in the group, which are registered and based once it is
    /// durable, so that a base is current through a group.
    copies_wanted: Vec<(RegionId, ChunkPos)>,
    /// The highest tick each region committed in the group, whose progress its
    /// partners are told once the group is durable.
    committed: BTreeMap<RegionId, u64>,
    /// Whether the time is to be looked at, for the couplings.
    looking: bool,
}

impl Group {
    fn is_empty(&self) -> bool {
        self.regions.is_empty()
            && self.changed.is_empty()
            && self.installs.is_empty()
            && self.flushes.is_empty()
            && self.stream.is_empty()
            && self.copies_wanted.is_empty()
            && self.committed.is_empty()
            && !self.looking
    }

    /// Puts what the couplings said on the streams.
    fn said(&mut self, said: Said) {
        for (region, reply) in said.0 {
            self.stream(region, Streamed::Reply(Box::new(reply)));
        }
    }

    fn stream(&mut self, region: RegionId, streamed: Streamed) {
        self.stream.entry(region).or_default().push(streamed);
    }
}

/// Something for a region's stream.
enum Streamed {
    /// An effect filed, sent unless it was taken or dropped within the group.
    Effect(EffectId),
    Reply(Box<StoreReply>),
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
    /// The lanes of a world whose number is `world` ([`Restored::world`]).
    pub(crate) fn of_world(mut self, world: u64) -> Self {
        self.world = world;
        self
    }

    pub(crate) fn load(
        disk: Arc<dyn Disk>,
        root: &Path,
        jobs: Jobs,
        told: &Division,
        spawn: BlockPos,
    ) -> Result<Self, StoreError> {
        told.check()?;
        debug_assert_eq!(told.home, spawn.chunk(), "the home chunk is the spawn's");
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
        // Above every block reserved, and, against a damaged or missing file of blocks,
        // above every block a region file names. What was reserved and not given before
        // the store stopped is never given.
        let blocks_path = regions_directory.join("blocks");
        let reserved = match disk.read(&blocks_path)? {
            Some(bytes) => {
                BlocksFile::decode(&bytes)
                    .map_err(|error| StoreError::Damaged {
                        path: blocks_path.clone(),
                        error,
                    })?
                    .end
            }
            None => 0,
        };
        let next_block = regions
            .values()
            .filter_map(|lane| lane.file)
            .filter(|file| !is_empty(file.entity_ids))
            .map(|file| file.entity_ids.index().saturating_add(1))
            .chain([reserved])
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

        // What is kept of the players, as of the segment the file names. A world
        // without the file has no records, and every note of its log counts.
        let players_path = regions_directory.join("players");
        let mut players = match disk.read(&players_path)? {
            Some(bytes) => {
                let file = PlayersFile::decode(&bytes).map_err(|error| StoreError::Damaged {
                    path: players_path.clone(),
                    error,
                })?;
                Players::read(file)
            }
            None => Players::default(),
        };
        // The notes of the commits in the segments the file does not stand for, each
        // with its region and its tick, in the order of the log.
        let mut noted: Vec<(u32, u64, Vec<LoggedStay>)> = Vec::new();

        let mut table_last = None;
        let mut log = Log {
            disk: Arc::clone(&disk),
            directory: log_directory,
            segments: BTreeSet::new(),
            active: None,
            next: 1,
            floor: 0,
            unsettled: None,
            last: 0,
        };
        // The effects not taken yet, as the log has them.
        let mut filed = Filed::default();
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
                // A mark after a start says no less than what the log has.
                log.last = sequence(segment, offset as u64);
                match record {
                    LogRecord::Commit {
                        region,
                        tick,
                        stays,
                        effects,
                        taken,
                        ..
                    } => {
                        // Whether or not the commit is part of its region's history
                        // any more: its effects were filed when it was appended. Only a
                        // make-over passes over commits that were answered, and it
                        // writes their effects forward first; a commit that was never
                        // answered and turned up after all is counted as one that was.
                        let at = sequence(segment, offset as u64);
                        for (index, (effect, due)) in effects.into_iter().enumerate() {
                            let id = EffectId {
                                sequence: at,
                                index: index as u32,
                            };
                            filed.file(
                                id,
                                Kept {
                                    effect,
                                    due,
                                    segment,
                                },
                            );
                        }
                        for id in taken {
                            filed.remove(id);
                        }
                        let lane = regions.entry(RegionId(region)).or_default();
                        if lane.state_tick.is_none_or(|state| tick > state) {
                            lane.live.push(Entry {
                                tick,
                                segment,
                                offset: offset as u64,
                                length,
                            });
                        }
                        // Whether or not a checkpoint covers the commit: its notes
                        // are in no state file.
                        if segment >= players.from && !stays.is_empty() {
                            players.noted(segment, None);
                            noted.push((region, tick, stays));
                        }
                    }
                    LogRecord::Opened {
                        region, restored, ..
                    } => {
                        if let Some(lane) = regions.get_mut(&RegionId(region)) {
                            lane.live.retain(|entry| entry.tick <= restored);
                            lane.record.take_if(|record| record.tick > restored);
                        }
                        // A commit that is no part of the region's history has said
                        // nothing of stays either.
                        noted.retain(|(noting, tick, _)| *noting != region || *tick <= restored);
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
                        // Who used the chunks before is not known here.
                        filed.all_count_from_taking();
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
                        // Who used the chunks before is not known here.
                        filed.all_count_from_taking();
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
                    LogRecord::EffectsDropped { ids } => {
                        for id in ids {
                            filed.remove(id);
                        }
                    }
                    LogRecord::EffectsForward { effects } => {
                        for FiledEffect { id, effect, due } in effects {
                            filed.file(
                                id,
                                Kept {
                                    effect,
                                    due,
                                    segment,
                                },
                            );
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
        // And for the players' file alike, whose `from` can be far above the table's:
        // the notes of a segment numbered below it would be passed over by the start
        // after this one (ADR-0020, section 8, rule P).
        log.next = log.next.max(players.from);
        log.floor = players.from;

        // A world from before there was a table has this file, which said how it was
        // divided. The store has nothing to hold against that any more, and does not
        // read what the file says.
        let layout_path = root.join("layout");
        let remake = match &stored {
            Some(table) => !table.is_of(told),
            // Such a world is made over whatever it was divided into (ADR-0017,
            // section 2.2).
            None => disk.exists(&layout_path)?,
        };
        let keep = stored.is_some() && !remake;
        let tabled = stored.is_some();
        let used = stored.as_ref().map_or(0, |table| table.next_region);
        let mut lanes = Self {
            disk,
            root: root.to_owned(),
            jobs,
            log,
            // Where there is none, this one stands in until the table is written below.
            table: stored.unwrap_or_else(|| Table::made_from(told, 0, 0)),
            spawn,
            world: 0,
            regions,
            table_last,
            players,
            next_block,
            reserved: next_block,
            sessions: 0,
            uses: Uses::default(),
            filed,
            copies: Copies::default(),
            couplings: Couplings::default(),
            clock: real_clock(),
            lends: Lends::default(),
            group: Group::default(),
        };
        // What is left of the notes, in the order of the log and by the same rules as
        // when they were taken, without answering anybody: every handle was lost with
        // the store that took them, and a region names its stays when it is opened.
        // The table is the one the notes were written under: a world that was made
        // over has had its file written first, as below.
        let home = lanes.table.home_region;
        for (region, _, stays) in noted {
            for note in &stays {
                lanes.players.apply(note, RegionId(region) == home, None);
            }
        }
        if remake {
            // Making the world over writes, for every region that had anything, that
            // it was opened and restored with nothing, and the start after this one
            // would forget every note since the file there is. So the file is written
            // first, and stands for the whole log: no segment is being appended to
            // while a store starts. If it cannot be written, the start fails
            // (ADR-0020, section 8, step 5). A world in which no stay was ever given
            // has nothing that could be forgotten, and gets no file for it.
            if !lanes.players.is_empty() {
                let from = lanes.log.next;
                lanes.write_players(from)?;
            }

            // Where there was no table, nobody was granted anything, and every change
            // the regions committed goes into the chunks.
            lanes.make_over(tabled)?;
            // Said when what the regions had is in the chunks, before the table of
            // the new division is put in the old one's place, and whether or not a
            // region had anything left: whoever runs a region of the world as it was
            // finds it gone, or begun anew under its id (ADR-0017, section 2.2). Not
            // later: a start that fails between putting the table in place and
            // making sure of the directory leaves the world made over, and the start
            // after it finds nothing to say. A start that fails here says it, and so
            // does the next if it has to do it again.
            info!(
                "the world was divided otherwise before; what its regions had is in the stored chunks now"
            );
            warn!(
                "the regions of this world begin anew: whoever is in it has to join again. Stop the workers and the edges of a cluster before its world store is started with other pins"
            );
        }
        if !keep {
            // What changes the table from now on goes to a segment the file names.
            lanes.log.close();
            let from = lanes.log.next;
            lanes.table = Table::made_from(told, used, from);
            lanes.write_table(from)?;
            lanes.table_last = None;
        }
        // Only once the table is durable, by which a start after a crash knows that
        // there is nothing left to be made over; and also if a store died right here.
        if lanes.disk.exists(&layout_path)? {
            lanes.disk.remove(&layout_path)?;
            lanes.disk.sync_directory(root)?;
        }
        lanes.align()?;
        lanes.begin_uses()?;
        lanes.read_couplings()?;
        Ok(lanes)
    }

    /// Reads `regions/couplings`, if there is one: a restart of the store keeps
    /// couplings (`docs/adr/0025-effects-between-regions.md`, section 8). Each region's
    /// progress is its highest tick in the log.
    fn read_couplings(&mut self) -> Result<(), StoreError> {
        let path = self.regions_directory().join("couplings");
        let Some(bytes) = self.disk.read(&path)? else {
            return Ok(());
        };
        let mut file = CouplingsFile::decode(&bytes).map_err(|error| StoreError::Damaged {
            path: path.clone(),
            error,
        })?;
        let ticks = self
            .regions
            .iter()
            .map(|(region, lane)| {
                let live = lane.live.iter().map(|entry| entry.tick).max();
                (*region, live.max(lane.whole_tick()).unwrap_or(0))
            })
            .collect();
        // Regions the table no longer has are not coupled.
        let table = &self.table;
        file.regions
            .retain(|region| table.has(RegionId(region.region)));
        file.couplings
            .retain(|(a, b)| table.has(RegionId(*a)) && table.has(RegionId(*b)));
        file.handshakes
            .retain(|shake| table.has(RegionId(shake.low)) && table.has(RegionId(shake.high)));
        self.couplings = Couplings::from_file(file, ticks, (self.clock)());
        Ok(())
    }

    /// Writes `regions/couplings` as the couplings are now. A failure is said and
    /// nothing more: a store that starts on the file before has couplings that were,
    /// which end by their grace or their patience.
    fn write_couplings(&mut self) {
        let path = self.regions_directory().join("couplings");
        let written = replace(self.disk.as_ref(), &path, &self.couplings.file().encode())
            .and_then(|()| self.disk.sync_directory(&self.regions_directory()));
        if let Err(error) = written {
            error!(%error, "the file of the couplings could not be written");
        }
    }

    /// With `clock` for the commit thread's: tests move theirs themselves.
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// After a start, every chunk a region holds by the table is in use by it, and so is
    /// the chunk of every effect filed that a region holds: it was in use when the
    /// effect was filed, and the region's runner says [`StoreRequest::Dropped`] of it
    /// if it does not load it again. An effect whose chunk nobody holds is dropped, as
    /// a record: the world may have been made over for another division.
    fn begin_uses(&mut self) -> Result<(), StoreError> {
        let regions: Vec<RegionId> = self.table.regions().collect();
        for region in regions {
            for (chunk, _) in self.table.grants(region) {
                self.uses.begin(region, chunk);
            }
        }
        let mut dropped = Vec::new();
        for filed in self.filed.all() {
            let chunk = filed.effect.at.chunk();
            match self.table.holder(chunk) {
                Some(holder) if self.uses.user(chunk).is_none() => {
                    self.uses.begin(holder, chunk);
                }
                Some(_) => {}
                None => dropped.push(filed.id),
            }
        }
        if dropped.is_empty() {
            return Ok(());
        }
        for id in &dropped {
            self.filed.remove(*id);
        }
        let written = dropped.chunks(RECORD_EFFECTS).try_for_each(|ids| {
            let record = LogRecord::EffectsDropped { ids: ids.to_vec() };
            self.log.append(&record.encode()).map(|_| ())
        });
        if let Err(error) = written.and_then(|()| self.log.sync()) {
            self.fail_log();
            return Err(error.into());
        }
        Ok(())
    }

    pub(crate) fn run(mut self, messages: Receiver<Message>) {
        loop {
            // While regions are coupled or in a handshake, the time is looked at even
            // when nobody asks for anything: a region that stopped asks for nothing.
            let first = if self.couplings.has_timers() {
                match messages.recv_timeout(COUPLING_LOOK) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match messages.recv() {
                    Ok(message) => Some(message),
                    Err(_) => return,
                }
            };
            if let Some(first) = first {
                self.handle(first);
                for message in messages.try_iter().take(GROUP_LIMIT) {
                    self.handle(message);
                }
            }
            self.group.looking |= self.couplings.has_timers();
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
                    lose(lane, &self.jobs);
                    self.owner_lost(session.region);
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
            Message::Copied {
                session,
                position,
                sequence,
                last_change,
                chunk,
            } => {
                // On the stream as of now, with who uses the chunk now: what the store
                // put on it since the base's group is before it, and the runner applies
                // what is above the base's sequence.
                let current = self
                    .regions
                    .get(&session.region)
                    .is_some_and(|lane| lane.current == Some(session.number));
                if current && self.copies.copies(session.region, position) {
                    let base = StoreReply::CopyBase {
                        position,
                        sequence,
                        last_change,
                        chunk,
                        used: copy_use(&self.uses, &self.couplings, session.region, position),
                    };
                    self.group
                        .stream(session.region, Streamed::Reply(Box::new(base)));
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
            Message::Barrier {
                reply_to,
                answer,
                sync,
            } => {
                // Everything that was sent before it has been handled. What of it
                // waited for the group to end is answered and passed on now, so that
                // the barrier reaches the thread for chunks behind all of it. A merge
                // and a split were done whole when their requests were handled.
                self.end_group();
                // If that thread has gone, the answer goes with the job, and whoever
                // waits is told so.
                let _ = self.jobs.send(Job::Barrier {
                    reply_to,
                    answer,
                    sync,
                });
            }
            #[cfg(test)]
            Message::Players { answer } => {
                // As the list of regions: nothing that is not durable.
                self.end_group();
                let _ = answer.send((self.players.issued(), self.players.list()));
            }
            Message::Passed { answer, chunks } => {
                // The thread for chunks has done what it was given before the barrier,
                // and what it had to say of it has been handled here: state files are
                // in place and returns are in the log. Ending the group makes that
                // durable, sees to the log and the table file, and answers the handles'
                // own flushes. None of it gives the thread for chunks more to do, which
                // is why one round there and back is enough.
                self.end_group();
                let rested = match self.log.settle() {
                    Ok(()) => chunks,
                    Err(error) => Err(StoreError::Io(error)),
                };
                // Whoever asked may have gone.
                let _ = answer.send(rested);
            }
        }
    }

    /// The next block of entity keys, reserved in the blocks' file first: sixteen at a
    /// time, so that most hellos sync only their region's file.
    fn fresh_block(&mut self) -> Result<EntityIds, StoreError> {
        let block = EntityIds::block(self.next_block).ok_or(StoreError::OutOfEntityIds)?;
        if self.next_block >= self.reserved {
            let end = self.next_block.saturating_add(BLOCKS_RESERVED);
            replace(
                self.disk.as_ref(),
                &self.regions_directory().join("blocks"),
                &BlocksFile { end }.encode(),
            )?;
            self.disk.sync_directory(&self.regions_directory())?;
            self.reserved = end;
        }
        self.next_block += 1;
        Ok(block)
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
        let epoch = |region| {
            let file = self.regions.get(&region).and_then(|lane| lane.file);
            file.map_or(0, |file: RegionFile| file.epoch)
        };
        self.table.list(self.spawn, epoch)
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
        if let Some(lane) = self.regions.get_mut(&session.region)
            && lane
                .owner
                .take_if(|owner| owner.peer.session.number == session.number)
                .is_some()
        {
            self.owner_lost(session.region);
        }
    }

    /// The region has lost its owner: its couplings end at once, and its partners go on
    /// without it (`docs/adr/0025-effects-between-regions.md`, section 8).
    fn owner_lost(&mut self, region: RegionId) {
        let mut said = Said::default();
        let why = clustine_rpc::Uncoupling::OwnerLost { region };
        self.couplings.end_all(region, why, &mut said);
        self.group.said(said);
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
                stays,
                ticks,
                marks_taken,
                seams,
                effects,
                taken,
                events,
                // Carried into the log by step N2b.
                entity_changes: _,
            } => {
                let region = session.region;
                // An effect is filed if its chunk is in use, and left out otherwise:
                // dropped as the game drops an update into a chunk that is not loaded
                // (`docs/adr/0025-effects-between-regions.md`, section 5). Its use is
                // that of now, which is that of its record in the log.
                // When each is due is converted by the offsets in force now (section 8).
                let effects: Vec<(Effect, Due, RegionId)> = effects
                    .into_iter()
                    .filter_map(|effect| {
                        let user = self.uses.user(effect.at.chunk())?;
                        let due = self.couplings.due(region, user, tick, &effect);
                        Some((effect, due, user))
                    })
                    .collect();
                // Only what is filed for a chunk the region uses: a number named twice,
                // or of an effect dropped or moved to another region's chunk since, takes
                // nothing.
                let mut taken: Vec<EffectId> = taken
                    .into_iter()
                    .filter(|id| {
                        self.filed.get(*id).is_some_and(|kept| {
                            self.uses.user(kept.effect.at.chunk()) == Some(region)
                        })
                    })
                    .collect();
                taken.sort_unstable();
                taken.dedup();
                // The changes of chunks that others copy, by chunk and in their order:
                // those the region holds from before the tick, as a replay puts in
                // (section 3), block entities' data among them.
                let mut updates: BTreeMap<ChunkPos, Vec<BlockChange>> = BTreeMap::new();
                for change in &changes {
                    let chunk = change.position().chunk();
                    let held = self
                        .table
                        .held_from(region, chunk)
                        .is_some_and(|from| tick > from);
                    if held && !self.copies.copiers(chunk).is_empty() {
                        updates.entry(chunk).or_default().push(change.clone());
                    }
                }
                // The notes are in the commit's own record, so that the two are on
                // disk together or not at all.
                let notes: Vec<LoggedStay> = stays.iter().map(players::logged).collect();
                let record = LogRecord::Commit {
                    region: region.0,
                    tick,
                    epoch: owner.epoch,
                    changes,
                    state,
                    stays: notes.clone(),
                    ticks,
                    marks_taken,
                    seams,
                    effects: effects
                        .iter()
                        .map(|(effect, due, _)| (*effect, *due))
                        .collect(),
                    taken: taken.clone(),
                    events,
                    entity_changes: Vec::new(),
                }
                .encode();
                self.group.regions.insert(region);
                match self.log.append(&record) {
                    Ok((segment, offset)) => {
                        let at = sequence(segment, offset);
                        for (index, (effect, due, user)) in effects.into_iter().enumerate() {
                            let id = EffectId {
                                sequence: at,
                                index: index as u32,
                            };
                            let kept = Kept {
                                effect,
                                due,
                                segment,
                            };
                            self.filed.file(id, kept);
                            self.group.effects.push(effects::Undo::Filed(id));
                            self.group.stream(user, Streamed::Effect(id));
                        }
                        for id in taken {
                            if let Some(kept) = self.filed.remove(id) {
                                self.group.effects.push(effects::Undo::Removed(id, kept));
                            }
                        }
                        for (chunk, changes) in updates {
                            let previous = self
                                .copies
                                .changed(chunk, at)
                                .expect("a chunk somebody copies has a chain");
                            for copier in self.copies.copiers(chunk) {
                                let update = StoreReply::CopyUpdate {
                                    position: chunk,
                                    sequence: at,
                                    previous,
                                    changes: changes.clone(),
                                    entity_changes: Vec::new(),
                                };
                                self.group.stream(copier, Streamed::Reply(Box::new(update)));
                            }
                        }
                        lane.live.push(Entry {
                            tick,
                            segment,
                            offset,
                            length: record.len(),
                        });
                        owner.unsynced.push(StoreReply::Committed { tick });
                        let highest = self.group.committed.entry(region).or_insert(tick);
                        *highest = (*highest).max(tick);
                        lane.latest = lane.latest.max(tick);
                        lane.named = lane.named.max(tick);
                        if notes.is_empty() {
                            return;
                        }
                        // Only now that the commit is taken: the notes of an owner
                        // that was replaced went with its commit above, and a note
                        // whose record is not in the log would raise a floor for a
                        // stay that is in no state (ADR-0020, section 3). What the
                        // notes are answered with goes behind the commit's own
                        // answer, and out when the group is durable.
                        self.players.noted(segment, Some(&mut self.group.undo));
                        let home = region == self.table.home_region;
                        let mut dead = Vec::new();
                        for (note, logged) in stays.iter().zip(&notes) {
                            let (StayNote::Entering { player, entity }
                            | StayNote::Has { player, entity, .. }) = *note;
                            // Who holds the chunk of the place as the table is now.
                            // It is advice: a claim later in this group can take the
                            // chunk, and its holder can give it back a moment later.
                            let enter = |place: Option<LoggedPlace>| StoreReply::Enter {
                                player,
                                entity,
                                place: place.as_ref().map(players::place),
                                holder: place
                                    .and_then(|place| self.table.holder(players::chunk_of(&place))),
                            };
                            match self
                                .players
                                .take(region, home, logged, &mut self.group.undo)
                            {
                                Taken::Raised { place } => {
                                    self.group.dead.push((player, entity, 0));
                                    owner.unsynced.push(enter(place));
                                }
                                Taken::Again { place } => owner.unsynced.push(enter(place)),
                                Taken::Dead { stay, hops, .. } => {
                                    dead.push((player, EntityId(stay), hops));
                                }
                                Taken::Written | Taken::Ahead => {}
                            }
                        }
                        if !dead.is_empty() {
                            owner.unsynced.push(StoreReply::Dead { stays: dead });
                        }
                    }
                    Err(error) => {
                        error!(%region, %error, "a commit could not be written to the log");
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
                // Its use begins as the load is passed on, so that what is filed while
                // it is loaded waits for it (`docs/adr/0025-effects-between-regions.md`,
                // section 5).
                if self.uses.begin(session.region, position) {
                    tell_copiers(
                        &self.uses,
                        &self.couplings,
                        &self.copies,
                        &mut self.group,
                        position,
                    );
                }
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
                // Written by parts from step N2b on; until then every save writes both.
                parts: _,
            } => Job::Save {
                position,
                tick,
                epoch: owner.epoch,
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
                    // A lent chunk on its way back is its lender's: a claim takes nothing
                    // of another's pinned land, and does not call the return off
                    // (`docs/adr/0027-lending-at-a-line.md`, section 4).
                    let lender = self
                        .table
                        .pinned_holder(chunk)
                        .filter(|lender| *lender != region);
                    if let Some(lender) = lender
                        && lane.returning.contains_key(&chunk)
                    {
                        foreign.push((chunk, lender));
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
                // Their use begins, also of a chunk whose return this calls off.
                for chunk in &granted {
                    if self.uses.begin(region, *chunk) {
                        tell_copiers(
                            &self.uses,
                            &self.couplings,
                            &self.copies,
                            &mut self.group,
                            *chunk,
                        );
                    }
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
                // The region lets go of every chunk it names, also of one it holds by
                // being pinned and was never granted, which no return frees: its use
                // ends all the same (`docs/adr/0025-effects-between-regions.md`,
                // section 5, and "Found while building", E6).
                let mut let_go: Vec<ChunkPos> = chunks
                    .iter()
                    .copied()
                    .filter(|chunk| *chunk != self.table.home_chunk)
                    .collect();
                let_go.sort_unstable();
                let_go.dedup();
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
                // A chunk being returned is not in use: what is filed for it is dropped.
                let ended = end_uses(
                    &mut self.uses,
                    &mut self.filed,
                    &self.couplings,
                    &self.copies,
                    &mut self.log,
                    &mut self.group,
                    region,
                    &let_go,
                );
                if let Err(error) = ended {
                    error!(%region, %error, "effects dropped could not be written to the log");
                    self.fail_log();
                    return;
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
            StoreRequest::Dropped { chunks } => {
                let region = session.region;
                let ended = end_uses(
                    &mut self.uses,
                    &mut self.filed,
                    &self.couplings,
                    &self.copies,
                    &mut self.log,
                    &mut self.group,
                    region,
                    &chunks,
                );
                if let Err(error) = ended {
                    error!(%region, %error, "effects dropped could not be written to the log");
                    self.fail_log();
                }
                return;
            }
            StoreRequest::Copies { wanted, gone } => {
                let region = session.region;
                for chunk in gone {
                    self.copies.unregister(region, chunk);
                }
                // A copy wanted again, after a gap in its updates, is based anew.
                for chunk in wanted {
                    self.group.copies_wanted.push((region, chunk));
                }
                return;
            }
            StoreRequest::Beside { begins, ends } => {
                self.couplings.beside(session.region, &begins, &ends);
                // Looked at when the group ends, with the use as it is then.
                self.group.looking = true;
                return;
            }
            StoreRequest::JoinsAt { partner, tick } => {
                self.couplings.joins_at(session.region, partner, tick);
                self.group.looking = true;
                return;
            }
            StoreRequest::EndCouplings => {
                let mut said = Said::default();
                self.couplings.reshaping(session.region, &mut said);
                self.group.said(said);
                return;
            }
            StoreRequest::Lend { chunks } => return self.lend(session.region, &peer, chunks),
            StoreRequest::LendRefused {
                lend,
                chunks,
                again,
            } => return self.lend_refused(session.region, lend, &chunks, again),
            StoreRequest::Lending { lend, chunks } => {
                return self.lending(session.region, peer, lend, &chunks);
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
                    lose(lane, &self.jobs);
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
            // After the table, not before: a first segment that is kept for the
            // table and for the players alike is freed of the table there, and a look
            // taken before would find it kept for more than the players, do nothing,
            // and not come again until the next state file. At a clean stop there is
            // none, and segments would stay.
            self.trim_for_the_players();
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
        // What the couplings make of the group: partners' progress behind the effects
        // of the same commits, couplings, handshakes, their ends and regions given up.
        // The file is durable before any of it goes out.
        let mut group = group;
        let mut said = Said::default();
        let now = (self.clock)();
        let regions = &self.regions;
        let has_owner = |region| {
            regions
                .get(&region)
                .is_some_and(|lane: &Lane| lane.owner.is_some())
        };
        self.couplings
            .turn(now, &group.committed, &self.uses, has_owner, &mut said);
        group.said(said);
        if self.couplings.take_changed() {
            self.write_couplings();
        }
        // What the group filed goes on the streams, behind the answers, with a mark
        // through the group's last record (`docs/adr/0025-effects-between-regions.md`,
        // section 2). Not what was taken or dropped since within the group.
        let through = self.log.last;
        for (region, streamed) in group.stream {
            let Some(owner) = self
                .regions
                .get(&region)
                .and_then(|lane| lane.owner.as_ref())
            else {
                continue;
            };
            let mut told = false;
            let mut effects = Vec::new();
            for item in streamed {
                match item {
                    Streamed::Effect(id) => {
                        effects.extend(self.filed.filed(id).filter(|filed| {
                            self.uses.user(filed.effect.at.chunk()) == Some(region)
                        }))
                    }
                    Streamed::Reply(reply) => {
                        if !effects.is_empty() {
                            let effects = mem::take(&mut effects);
                            owner.peer.answer(StoreReply::Effects { effects });
                        }
                        owner.peer.answer(*reply);
                        told = true;
                    }
                }
            }
            if !effects.is_empty() {
                owner.peer.answer(StoreReply::Effects { effects });
                told = true;
            }
            if told {
                owner.peer.answer(StoreReply::Mark { through });
            }
        }
        // The copies asked for in the group are registered as of its end, and their
        // bases made on the thread for chunks with the holder's commits up to it.
        for (region, chunk) in group.copies_wanted {
            let Some(peer) = self
                .regions
                .get(&region)
                .and_then(|lane| lane.owner.as_ref())
                .map(|owner| Arc::clone(&owner.peer))
            else {
                continue;
            };
            let last_change = self.copies.register(region, chunk, through);
            let holder = self.table.holder(chunk);
            let from = holder.and_then(|holder| self.table.held_from(holder, chunk));
            let listed = holder
                .and_then(|holder| self.regions.get(&holder))
                .map_or_else(Vec::new, |lane| {
                    lane.live
                        .iter()
                        .map(|entry| Listed {
                            path: self.log.path(entry.segment),
                            offset: entry.offset,
                            length: entry.length,
                        })
                        .collect()
                });
            let _ = self.jobs.send(Job::Copy {
                position: chunk,
                listed,
                from,
                sequence: through,
                last_change,
                peer,
            });
        }
        // The floors of the group are durable: every region that has an owner is
        // told of the stays that are dead by them, behind the group's own answers.
        // Whoever opens a region from now on does so after the floor is durable, and
        // names its stays (ADR-0020, section 5).
        if !group.dead.is_empty() {
            for owner in self.regions.values().filter_map(|lane| lane.owner.as_ref()) {
                owner.peer.answer(StoreReply::Dead {
                    stays: group.dead.clone(),
                });
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
        // Nobody is left to be told of a lend called off: every region is opened again.
        self.lends.clear();
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
        // What the notes of the group's commits did goes with the commits: each record
        // is as it was before the group, a record the group made is gone, and so is
        // what it added to the ids that were given out. Nobody was told of any of it.
        self.players.undo(group.undo);
        // So do the effects it filed, took and dropped. Use is not in the log, and stays.
        self.filed.undo(group.effects);
        for lane in self.regions.values_mut() {
            if let Some((segment, durable)) = failed {
                lane.live
                    .retain(|entry| entry.segment != segment || entry.offset < durable);
            }
            lose(lane, &self.jobs);
        }
        let regions: Vec<RegionId> = self.regions.keys().copied().collect();
        for region in regions {
            self.owner_lost(region);
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
    /// no commit a region is restored with, nothing the table file does not have, and
    /// no stay note the players' file does not have. The effects not taken yet that
    /// live in them are written forward first (`docs/adr/0025-effects-between-regions.md`,
    /// section 5).
    fn collect(&mut self) {
        let needed = self.needed_by_lanes();
        let from = self.table.from;
        let table = self.table_last.map(|last| from..=last);
        let from = self.players.from;
        let players = self.players.last.map(|last| from..=last);
        let unneeded = |segment: u64| {
            !needed.contains(&segment)
                && !table.as_ref().is_some_and(|kept| kept.contains(&segment))
                && !players.as_ref().is_some_and(|kept| kept.contains(&segment))
        };
        let active = self.log.active.as_ref().map(|active| active.number);
        let going = self
            .log
            .segments
            .iter()
            .copied()
            .take_while(|segment| Some(*segment) != active && unneeded(*segment))
            .last();
        if let Some(last) = going {
            let carried = self.filed.living_in(..=last);
            if !carried.is_empty() && self.write_forward(carried).is_err() {
                return;
            }
        }
        let filed = &self.filed;
        self.log
            .remove_while(|segment| unneeded(segment) && !filed.lives_in(segment));
    }

    /// Writes the effects forward, as records of a group of their own, and makes them
    /// durable: they live in the segment of those records from then on. A failure cuts
    /// the group off like any other, after which every region has lost its owner.
    fn write_forward(&mut self, carried: Vec<FiledEffect>) -> Result<(), ()> {
        let mut segment = None;
        let written = carried.chunks(RECORD_EFFECTS).try_for_each(|effects| {
            let record = LogRecord::EffectsForward {
                effects: effects.to_vec(),
            };
            let (at, _) = self.log.append(&record.encode())?;
            segment = Some(at);
            Ok(())
        });
        if let Err(error) = written.and_then(|()| self.log.sync()) {
            error!(%error, "effects could not be written forward in the log");
            self.fail_log();
            return Err(());
        }
        // A segment of its own, so that what keeps it is the effects alone.
        self.log.close();
        let segment = segment.expect("effects were written");
        for FiledEffect { id, effect, due } in carried {
            self.filed.file(
                id,
                Kept {
                    effect,
                    due,
                    segment,
                },
            );
        }
        Ok(())
    }

    /// Writes the players' file, with `from` as the first segment of the log whose
    /// notes are not in it, and makes it durable. Only when the records have nothing
    /// that is not durable in the log, and no segment is being appended to: no
    /// segment is numbered below `from` from then on.
    fn write_players(&mut self, from: u64) -> Result<(), StoreError> {
        let path = self.regions_directory().join("players");
        let file = self.players.file(from).encode();
        replace(self.disk.as_ref(), &path, &file)?;
        self.disk.sync_directory(&self.regions_directory())?;
        self.players.from = from;
        self.players.last = None;
        self.log.floor = from;
        Ok(())
    }

    /// Writes the players' file anew if that lets the first segment of the log go: if
    /// that segment is kept only for the stay notes in it, which no lane and not the
    /// table need. The file then has what all notes so far made of the records, and
    /// names the next segment as the first that is not in it.
    ///
    /// For the end of a group, as [`Lanes::trim_for_the_table`], and after it.
    fn trim_for_the_players(&mut self) {
        let (Some(&first), Some(last)) = (self.log.segments.first(), self.players.last) else {
            return;
        };
        let kept_for_the_players = (self.players.from..=last).contains(&first);
        let kept_for_the_table = self
            .table_last
            .is_some_and(|last| (self.table.from..=last).contains(&first));
        if !kept_for_the_players
            || kept_for_the_table
            || self.needed_by_lanes().contains(&first)
            || self.log.active.is_some()
        {
            return;
        }
        // Only once the file is durable are the segments it stands for let go of. If it
        // cannot be written, the file there is stays the one that counts, with every
        // segment from its `from` on.
        match self.write_players(self.log.next) {
            Ok(()) => self.collect(),
            Err(error) => {
                error!(%error, "the file of the players could not be written; the log is kept as it is");
            }
        }
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
    /// What `region` holds by a lend and has lent away, as the table has it now.
    fn lends_of(&self, region: RegionId) -> LendsOf {
        let (lent, lent_away) = self.table.lends(region);
        LendsOf { lent, lent_away }
    }

    /// Answers the owner of `region`, if it has one, once the group is durable.
    fn answer_in_group(&mut self, region: RegionId, reply: StoreReply) {
        let Some(owner) = self
            .regions
            .get_mut(&region)
            .and_then(|lane| lane.owner.as_mut())
        else {
            return;
        };
        owner.unsynced.push(reply);
        self.group.regions.insert(region);
    }

    /// Answers the owner of `region` now, if it has one.
    fn answer_now(&self, region: RegionId, reply: StoreReply) {
        if let Some(owner) = self
            .regions
            .get(&region)
            .and_then(|lane| lane.owner.as_ref())
        {
            owner.peer.answer(reply);
        }
    }

    /// Calls off every lend `region` lends or borrows in, and tells both regions of
    /// each: the borrower keeps its ask, and the lender takes the chunks up again
    /// (`docs/adr/0027-lending-at-a-line.md`, section 3).
    fn call_off(&mut self, region: RegionId) {
        for (lend, of) in self.lends.call_off(region) {
            let chunks: Vec<ChunkPos> = of.chunks.keys().copied().collect();
            for party in [of.lender, of.borrower] {
                let chunks = chunks.clone();
                self.answer_now(party, StoreReply::LendCalledOff { lend, chunks });
            }
        }
    }

    /// Tells the pinned regions whose chunks among `chunks` `from` held by a grant
    /// that `to` holds them now, after a merge or a split.
    fn tell_lenders(&self, from: RegionId, to: RegionId, chunks: &[ChunkPos]) {
        let mut lenders: BTreeMap<RegionId, Vec<ChunkPos>> = BTreeMap::new();
        for chunk in chunks {
            if let Some(lender) = self.table.pinned_holder(*chunk)
                && lender != from
                && lender != to
            {
                lenders.entry(lender).or_default().push(*chunk);
            }
        }
        for (lender, chunks) in lenders {
            self.answer_now(lender, StoreReply::Lent { chunks, to });
        }
    }

    /// `borrower` asks for a lend of `chunks` (`docs/adr/0027-lending-at-a-line.md`,
    /// section 2): each is asked of its lender, or answered at once.
    fn lend(&mut self, borrower: RegionId, peer: &Arc<Peer>, chunks: Vec<ChunkPos>) {
        let pinned = self.table.pinned(borrower).to_vec();
        let beside = |chunk: ChunkPos| {
            [-1, 1].into_iter().any(|dx| {
                let next = ChunkPos::new(chunk.x + dx, chunk.z);
                pinned.iter().any(|area| area.contains(next))
            })
        };
        let mut not_lendable = Vec::new();
        let mut wait = Vec::new();
        let mut held = Vec::new();
        let mut asked: BTreeMap<RegionId, Vec<ChunkPos>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for chunk in chunks {
            if !seen.insert(chunk) {
                continue;
            }
            // Its own ask under way is passed over; another's waits for that lend,
            // which may yet be refused.
            if let Some(lend) = self.lends.of_chunk(chunk) {
                if self
                    .lends
                    .get(lend)
                    .is_some_and(|lend| lend.borrower != borrower)
                {
                    wait.push(chunk);
                }
                continue;
            }
            // Its own pinned land and the home chunk are no lend's, whoever holds them.
            let pinned_holder = self.table.pinned_holder(chunk);
            if pinned_holder == Some(borrower) || chunk == self.table.home_chunk {
                not_lendable.push(chunk);
                continue;
            }
            let holder = self.table.holder(chunk);
            if holder == Some(borrower) {
                held.push(chunk);
                continue;
            }
            // Held by its pin, not by a grant to anyone.
            let lender = pinned_holder.filter(|lender| holder == Some(*lender));
            let Some(lender) = lender else {
                not_lendable.push(chunk);
                continue;
            };
            if pinned.is_empty() || !beside(chunk) {
                not_lendable.push(chunk);
                continue;
            }
            // One way at a time in a stretch of the line: the table's lends fix it, and
            // a lend under way the other way may yet be refused (section 1).
            let stretch = lends::stretch(chunk);
            let other_way = self.table.grants(lender).iter().any(|(granted, _)| {
                lends::stretch(*granted) == stretch
                    && self.table.pinned_holder(*granted) == Some(borrower)
            });
            if other_way {
                not_lendable.push(chunk);
                continue;
            }
            let lender_owned = self
                .regions
                .get(&lender)
                .is_some_and(|lane| lane.owner.is_some());
            if self.lends.under_way_in(borrower, lender, stretch) || !lender_owned {
                wait.push(chunk);
                continue;
            }
            asked.entry(lender).or_default().push(chunk);
        }
        if !held.is_empty() {
            self.answer_in_group(
                borrower,
                StoreReply::Claimed {
                    granted: held,
                    foreign: Vec::new(),
                },
            );
        }
        for (chunks, again) in [(not_lendable, false), (wait, true)] {
            if !chunks.is_empty() {
                peer.answer(StoreReply::NotLent { chunks, again });
            }
        }
        for (lender, chunks) in asked {
            let lend = self.lends.begin(lender, borrower, &chunks);
            self.answer_now(
                lender,
                StoreReply::LendAsked {
                    lend,
                    chunks,
                    to: borrower,
                },
            );
        }
    }

    /// The lender does not let go of `chunks` for `lend`: the borrower is told, and
    /// they are no longer under lend.
    fn lend_refused(&mut self, lender: RegionId, lend: u64, chunks: &[ChunkPos], again: bool) {
        let Some(of) = self.lends.get(lend).filter(|of| of.lender == lender) else {
            return;
        };
        let borrower = of.borrower;
        // Not one it has given back for the lend already.
        let refused: Vec<ChunkPos> = chunks
            .iter()
            .copied()
            .filter(|chunk| of.chunks.get(chunk) == Some(&None))
            .collect();
        let refused = self.lends.take(lend, &refused);
        if !refused.is_empty() {
            self.answer_now(
                borrower,
                StoreReply::NotLent {
                    chunks: refused,
                    again,
                },
            );
        }
    }

    /// The lender gives `chunks` back for `lend`: passed on like a return, behind the
    /// saves before it, without its use ending (the same record, section 3, step 3). A
    /// chunk not under the lend any more is answered as called off at once.
    fn lending(&mut self, lender: RegionId, peer: Arc<Peer>, lend: u64, chunks: &[ChunkPos]) {
        let of = self.lends.get(lend).filter(|of| of.lender == lender);
        let mut given = Vec::new();
        let mut off = Vec::new();
        for chunk in chunks.iter().copied().collect::<BTreeSet<_>>() {
            match of {
                Some(of) if of.chunks.get(&chunk) == Some(&None) => given.push(chunk),
                _ => off.push(chunk),
            }
        }
        if !off.is_empty() {
            peer.answer(StoreReply::LendCalledOff { lend, chunks: off });
        }
        if given.is_empty() {
            return;
        }
        let Some(lane) = self.regions.get_mut(&lender) else {
            return;
        };
        lane.returns += 1;
        let number = lane.returns;
        lane.lending.insert(number, lend);
        for chunk in &given {
            self.lends.returning(lend, *chunk, number);
        }
        let Some(owner) = lane.owner.as_mut() else {
            return;
        };
        let job = Job::Return {
            number,
            chunks: given,
            peer,
        };
        if owner.unsynced.is_empty() && owner.held.is_empty() {
            let _ = self.jobs.send(job);
        } else {
            owner.held.push(job);
        }
    }

    /// The lender's saves before its return `number` for `lend` are durable: the chunks
    /// of the lend still under it are granted to the borrower, with their use, the
    /// effects filed for them and their copies (the same record, section 3, step 4).
    /// The others are answered as called off.
    fn lent(&mut self, lender: RegionId, lend: u64, number: u64, chunks: Vec<ChunkPos>) {
        let of: Option<Lend> = self.lends.get(lend).cloned();
        let borrower = of.as_ref().map(|of| of.borrower);
        let (good, bad): (Vec<ChunkPos>, Vec<ChunkPos>) = chunks.into_iter().partition(|chunk| {
            of.as_ref().is_some_and(|of| {
                of.lender == lender
                    && of.chunks.get(chunk) == Some(&Some(number))
                    && self.table.holder(*chunk) == Some(lender)
                    && self.table.pinned_holder(*chunk) == Some(lender)
                    && self.table.has(of.borrower)
            })
        });
        if !bad.is_empty() {
            let taken = self.lends.take(lend, &bad);
            if let Some(borrower) = borrower
                && !taken.is_empty()
            {
                let chunks = taken;
                self.answer_now(borrower, StoreReply::LendCalledOff { lend, chunks });
            }
            self.answer_now(lender, StoreReply::LendCalledOff { lend, chunks: bad });
        }
        let Some(borrower) = borrower else {
            return;
        };
        let granted = self.lends.take(lend, &good);
        if granted.is_empty() {
            return;
        }
        let tick = self.regions.get(&borrower).map_or(0, |lane| lane.latest);
        for chunks in granted.chunks(RECORD_CHUNKS) {
            let record = LogRecord::Granted {
                region: borrower.0,
                tick,
                chunks: chunks.to_vec(),
            };
            match self.log.append(&record.encode()) {
                Ok((segment, _)) => self.table_last = Some(segment),
                Err(error) => {
                    error!(%borrower, %error, "a lend could not be written to the log");
                    self.fail_log();
                    return;
                }
            }
        }
        self.table
            .grant(borrower, tick, &granted)
            .expect("chunks held by a pin are granted to a living region");
        self.group.changed.push(Change::Granted {
            region: borrower,
            chunks: granted.clone(),
        });
        // The borrower uses them from now on and copies them no longer; the lender
        // copies them instead, from a base made once the grant is durable. What was
        // filed for them counts its delays from its taking, the borrower's ticks not
        // being the lender's.
        for chunk in &granted {
            self.copies.unregister(borrower, *chunk);
            self.uses.begin(borrower, *chunk);
        }
        self.filed.count_from_taking(&granted);
        for chunk in &granted {
            tell_copiers(
                &self.uses,
                &self.couplings,
                &self.copies,
                &mut self.group,
                *chunk,
            );
            self.group.copies_wanted.push((lender, *chunk));
        }
        self.answer_in_group(
            borrower,
            StoreReply::Claimed {
                granted: granted.clone(),
                foreign: Vec::new(),
            },
        );
        // What the lender was sent and had not taken goes to the borrower, behind a mark.
        let resent = self.filed.for_chunks(&granted);
        if !resent.is_empty() {
            let effects = StoreReply::Effects { effects: resent };
            self.group
                .stream(borrower, Streamed::Reply(Box::new(effects)));
        }
        info!(%lender, %borrower, chunks = granted.len(), "chunks were lent");
        self.answer_in_group(
            lender,
            StoreReply::Lent {
                chunks: granted,
                to: borrower,
            },
        );
    }

    fn returned(&mut self, session: Session, number: u64, chunks: Vec<ChunkPos>) {
        let region = session.region;
        let Some(lane) = self.regions.get_mut(&region) else {
            return;
        };
        if lane.current != Some(session.number) {
            return;
        }
        if let Some(lend) = lane.lending.remove(&number) {
            return self.lent(region, lend, number, chunks);
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
        // A chunk of another's pinned area is that region's again, which it is told
        // once the release is durable, whoever held it by a grant
        // (`docs/adr/0027-lending-at-a-line.md`, sections 1 and 4).
        let mut back: BTreeMap<RegionId, Vec<ChunkPos>> = BTreeMap::new();
        for (chunk, _) in &grants {
            if let Some(pinned) = self.table.pinned_holder(*chunk)
                && pinned != region
            {
                back.entry(pinned).or_default().push(*chunk);
            }
        }
        for (pinned, chunks) in back {
            self.answer_in_group(pinned, StoreReply::PinnedBack { chunks });
        }
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
                lose(lane, &self.jobs);
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
                    epoch: hello.epoch,
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
    ) -> Result<(Opened, Restored, Vec<Replayed>, u64, Arc<Peer>), StoreError> {
        let RegionHello { region, epoch } = hello;
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
        // Every hello is given a fresh block, so that keys a lost tick gave out are never
        // given again to another entity, and blocks only rise, so that the home region's
        // stays come in the order of their keys (`docs/adr/0028-entities.md`, section
        // 1). Before the region file is written: if writing fails half way, the block may
        // be on disk all the same, and must not be another region's as well.
        let entity_ids = self.fresh_block()?;
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
        let replaced = lane.owner.take();
        if let Some(replaced) = &replaced {
            replaced.peer.mark_lost();
            // Its loads that are being generated are let go of at once.
            let _ = self.jobs.send(Job::Wake);
            info!(%region, epoch, "the owner of a region was replaced");
        }
        lane.current = None;
        // Couplings end with the owner replaced; those a region has without an owner
        // come from the file, after a restart of the store, and are kept (section 8).
        if replaced.is_some() {
            self.owner_lost(region);
        }
        let now = (self.clock)();
        self.couplings.opened(region, now);
        let of_region = self.couplings.of_region(region);
        let lane = self.regions.get_mut(&region).expect("made above");

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
        let through = self.log.last;
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
        // Whoever runs the region asks for its copies again (section 3).
        self.copies.forget(region);
        // And for its lends (`docs/adr/0027-lending-at-a-line.md`, section 3).
        self.call_off(region);
        // The effects of the chunks the region uses go on its stream again: the runner
        // drops those it has taken (`docs/adr/0025-effects-between-regions.md`,
        // section 5). Everything the store has written before the record of the
        // opening is durable.
        let in_use = self.uses.of(region);
        let resent = self.filed.for_chunks(&in_use);
        if !resent.is_empty() {
            peer.answer(StoreReply::Effects { effects: resent });
            peer.answer(StoreReply::Mark { through });
        }
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
        lane.lending.clear();

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
            issued: EntityId(self.players.issued()),
            spawn: self.spawn,
            world: self.world,
            in_use,
            offset: of_region.offset,
            couplings: of_region.couplings,
            handshakes: of_region.handshakes,
            held_tick: of_region.held_tick,
            given_up: of_region.given_up,
            lends: self.lends_of(region),
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
        self.call_off(region);
        self.call_off(absorbed);
        let chunks = self
            .table
            .absorb(region, absorbed, tick)
            .expect("two living regions, as was looked at above");
        self.table_last = Some(entry.segment);
        self.tell_lenders(absorbed, region, &chunks);
        // The use moves with the holding, and the effects not taken with it; a tick
        // counted in the absorbed region's ticks counts from its taking now.
        let in_use = self.uses.moved(absorbed, region, None);
        self.filed.count_from_taking(&in_use);
        for chunk in &in_use {
            tell_copiers(
                &self.uses,
                &self.couplings,
                &self.copies,
                &mut self.group,
                *chunk,
            );
        }
        self.copies.forget(absorbed);
        let mut said = Said::default();
        self.couplings.gone(absorbed, &mut said);
        self.couplings.reshaped(region);
        self.group.said(said);
        if let Some(mut gone) = self.regions.remove(&absorbed) {
            // Its owner gets nothing done any more; what it had the thread for chunks
            // do is done before anything the survivor asks from now on.
            lose(&mut gone, &self.jobs);
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
            in_use,
            lends: self.lends_of(region),
        });
        // The survivor begins anew, and has its effects on its stream again.
        let resent = self.filed.for_chunks(&self.uses.of(region));
        if !resent.is_empty() {
            peer.answer(StoreReply::Effects { effects: resent });
            peer.answer(StoreReply::Mark {
                through: self.log.last,
            });
        }
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

        self.call_off(region);
        self.table
            .split(region, new, tick, &chunks)
            .expect("the next id and chunks the region holds, as was looked at above");
        self.table_last = Some(entry.segment);
        // A chunk lent to the region that goes to the part is the part's by a grant, and
        // its lender is told who holds it now (`docs/adr/0027-lending-at-a-line.md`,
        // section 4).
        self.tell_lenders(region, new, &chunks);
        // The part's hello lists the chunks whose use moved to it, and resends their
        // effects.
        let moved = self.uses.moved(region, new, Some(&chunks));
        self.filed.count_from_taking(&moved);
        self.couplings.reshaped(region);
        for chunk in &moved {
            tell_copiers(
                &self.uses,
                &self.couplings,
                &self.copies,
                &mut self.group,
                *chunk,
            );
        }
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
    changes: Vec<Replayed>,
) -> impl Iterator<Item = Replayed> + '_ {
    changes.into_iter().filter(move |change| {
        table
            .held_from(region, change.chunk())
            .is_some_and(|from| tick > from)
    })
}

/// Ends the use by `region` of those of `chunks` it uses, and drops what is filed for
/// them, as a record of the group (`docs/adr/0025-effects-between-regions.md`,
/// section 5). The caller fails the log if that cannot be written.
#[allow(clippy::too_many_arguments)]
fn end_uses(
    uses: &mut Uses,
    filed: &mut Filed,
    couplings: &Couplings,
    copies: &Copies,
    log: &mut Log,
    group: &mut Group,
    region: RegionId,
    chunks: &[ChunkPos],
) -> io::Result<()> {
    let mut dropped = Vec::new();
    for chunk in chunks {
        if !uses.end(region, *chunk) {
            continue;
        }
        tell_copiers(uses, couplings, copies, group, *chunk);
        for id in filed.ids_of(*chunk) {
            if let Some(kept) = filed.remove(id) {
                group.effects.push(effects::Undo::Removed(id, kept));
                dropped.push(id);
            }
        }
    }
    dropped.sort_unstable();
    for ids in dropped.chunks(RECORD_EFFECTS) {
        let record = LogRecord::EffectsDropped { ids: ids.to_vec() };
        log.append(&record.encode())?;
    }
    Ok(())
}

/// Who uses a chunk, as a region that copies it is told
/// (`docs/adr/0025-effects-between-regions.md`, section 8).
fn copy_use(uses: &Uses, couplings: &Couplings, copier: RegionId, chunk: ChunkPos) -> CopyUse {
    couplings.copy_use(copier, uses.user(chunk))
}

/// Puts on the streams of the regions that copy `chunk` who uses it now.
fn tell_copiers(
    uses: &Uses,
    couplings: &Couplings,
    copies: &Copies,
    group: &mut Group,
    chunk: ChunkPos,
) {
    for copier in copies.copiers(chunk) {
        let used = copy_use(uses, couplings, copier, chunk);
        let reply = StoreReply::CopyUsed {
            position: chunk,
            used,
        };
        group.stream(copier, Streamed::Reply(Box::new(reply)));
    }
}

/// How many blocks of entity keys the blocks' file is raised by at a time.
const BLOCKS_RESERVED: u32 = 16;

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
fn lose(lane: &mut Lane, jobs: &Jobs) {
    if let Some(owner) = lane.owner.take() {
        owner.peer.mark_lost();
        // What the generator is making for its loads is let go of now, and not when
        // the thread for chunks is next given something (ADR-0021, section 7, rule 4).
        let _ = jobs.send(Job::Wake);
    }
    lane.current = None;
    lane.installing = None;
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
    /// The players' file's `from`: no segment is numbered below it.
    floor: u64,
    /// The segment a write or a sync of which failed, and how much of it is good, for as
    /// long as cutting it back to that is not durable.
    unsettled: Option<(u64, u64)>,
    /// The sequence of the last record appended: once the log is synced, everything
    /// up to it is durable, which a mark on a stream says.
    last: u64,
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
                // The notes of a segment below the players' file's `from` would be
                // passed over in silence by the next start, which cannot tell such a
                // segment from one that is rightly there. So it is held to here, where
                // segments are numbered (ADR-0020, section 8, rule P).
                number
                    .checked_sub(self.floor)
                    .expect("no segment of the log is numbered below the players' file's `from`");
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
        self.last = sequence(active.number, offset);
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

    /// Reads the commit at `entry`: its tick, what it changed in chunks (its blocks,
    /// then the ticks of its chunks, then whose marks it took, then their seams) and its
    /// state.
    #[allow(clippy::type_complexity)]
    fn read(&self, entry: &Entry) -> Result<Option<(u64, Vec<Replayed>, Vec<u8>)>, StoreError> {
        Ok(self.record(entry)?.and_then(commit_changes))
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

/// Of a commit's record: its tick, what it changed in chunks (its blocks and their block
/// entities' data in the order the tick changed them, then the
/// ticks of its chunks, then whose marks it took, then their seams) and its state.
#[allow(clippy::type_complexity)]
pub(crate) fn commit_changes(record: LogRecord) -> Option<(u64, Vec<Replayed>, Vec<u8>)> {
    let LogRecord::Commit {
        tick,
        changes,
        state,
        ticks,
        marks_taken,
        seams,
        events,
        ..
    } = record
    else {
        return None;
    };
    let blocks = changes.into_iter().map(|change| match change {
        BlockChange::Block(position, block) => Replayed::Block(position, block),
        BlockChange::BlockEntity(position, data) => Replayed::BlockEntity(position, data),
    });
    let ticks = ticks
        .into_iter()
        .map(|(chunk, ticks)| Replayed::Ticks(chunk, ticks));
    let marks = marks_taken.into_iter().map(Replayed::MarksTaken);
    let seams = seams
        .into_iter()
        .map(|(chunk, seams)| Replayed::Seams(chunk, seams));
    let events = events
        .into_iter()
        .map(|(chunk, events)| Replayed::Events(chunk, events));
    let records = ticks.chain(marks).chain(seams).chain(events);
    Some((tick, blocks.chain(records).collect(), state))
}

fn segment_path(directory: &Path, segment: u64) -> PathBuf {
    directory.join(format!("{segment:020}.wal"))
}
