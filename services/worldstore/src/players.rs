//! What the store keeps of every player who ever joined: the latest stay the home
//! region gave them, how often that stay was handed on, and their place. See
//! `docs/adr/0020-one-stay-per-player.md`, sections 2, 3 and 8.
//!
//! The records are the commit thread's alone. They are changed by the stay notes of
//! the commits it takes, in the order it takes them, and by nothing else; a start
//! makes them again from the players' file and the notes in the log behind it.

use std::collections::BTreeMap;

use clustine_format::{LoggedPlace, LoggedStay, PlayerRecord, PlayersFile};
use clustine_region::RegionId;
use clustine_rpc::{ItemStack, Place, Pose, StayNote};
use clustine_world::{ChunkPos, Vec3};
use tracing::error;

/// The records, and what is kept beside them.
#[derive(Debug, Default)]
pub(crate) struct Players {
    /// One record per player, by the number of the player's id, which is the order
    /// the file has them in.
    records: BTreeMap<u128, Record>,
    /// The highest entity id an `Entering` note ever named that raised a floor; 0 if
    /// none did.
    issued: i32,
    /// The first segment of the log whose notes are not in the players' file; 0 in a
    /// world without the file.
    pub(crate) from: u64,
    /// The last segment of the log that has a commit with notes which are not in the
    /// file, if there is one. The segments from `from` up to it are kept for them.
    pub(crate) last: Option<u64>,
}

/// What is kept of one player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    /// The floor: the highest stay the store was told the home region gave the
    /// player. Never lowered.
    stay: i32,
    /// The highest hand-over count the store was told of that stay.
    hops: u32,
    place: Option<LoggedPlace>,
}

/// A player the store was told nothing of yet. No block of entity ids has id 0.
const UNKNOWN: Record = Record {
    stay: 0,
    hops: 0,
    place: None,
};

/// What a group of commits changed of the records, by which it is taken back if the
/// group cannot be made durable.
#[derive(Debug, Default)]
pub(crate) struct Undo {
    /// For each player the group touched, the record as it was before the group first
    /// touched it, or that there was none.
    records: BTreeMap<u128, Option<Record>>,
    /// `issued` as it was before the group first raised it.
    issued: Option<i32>,
    /// `last` as it was before the group first wrote a commit with notes.
    last: Option<Option<u64>>,
}

/// What the store made of a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Taken {
    /// An `Entering` note raised the player's floor: the stay may enter, with this
    /// place, and every stay of the player below it is dead.
    Raised { place: Option<LoggedPlace> },
    /// An `Entering` note named the stay the record has: it may enter, as was said
    /// before.
    Again { place: Option<LoggedPlace> },
    /// The note named a stay that is dead: one below `(stay, hops)`, which is what
    /// the record has. `why` says whether that is in order.
    Dead { stay: i32, hops: u32, why: Why },
    /// A `Has` note of the living stay: the record has its hand-overs and its place.
    Written,
    /// A `Has` note of a stay above the record's, which cannot be. Nothing is changed
    /// and nothing is answered, so that a mistake here costs a place and not a player.
    Ahead,
}

/// Why a note is answered with `Dead`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// A later stay of the player has been given out. This is in order.
    Replaced,
    /// The stay is the living one, with fewer hand-overs than the store was told of:
    /// a copy, which should not exist.
    Copy,
    /// An `Entering` note that did not come from the home region, which alone gives
    /// stays out.
    NotHome,
}

impl Players {
    /// The records as the file has them.
    pub(crate) fn read(file: PlayersFile) -> Self {
        let records = file.records.into_iter().map(|record| {
            let kept = Record {
                stay: record.stay,
                hops: record.hops,
                place: record.place,
            };
            (record.player, kept)
        });
        Self {
            records: records.collect(),
            issued: file.issued,
            from: file.from,
            last: None,
        }
    }

    /// The file of the records as they are, with `from` as the first segment of the
    /// log whose notes are not in it.
    pub(crate) fn file(&self, from: u64) -> PlayersFile {
        PlayersFile {
            from,
            issued: self.issued,
            records: self.list(),
        }
    }

    /// The records, in ascending order of the players.
    pub(crate) fn list(&self) -> Vec<PlayerRecord> {
        let listed = self.records.iter().map(|(player, record)| PlayerRecord {
            player: *player,
            stay: record.stay,
            hops: record.hops,
            place: record.place,
        });
        listed.collect()
    }

    /// Whether there is no record: no stay was ever given, as far as the store knows.
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The highest entity id a stay was ever given; 0 if none was.
    pub(crate) fn issued(&self) -> i32 {
        self.issued
    }

    /// Notes that `segment` of the log has a commit with stay notes.
    pub(crate) fn noted(&mut self, segment: u64, undo: Option<&mut Undo>) {
        if let Some(undo) = undo {
            undo.last.get_or_insert(self.last);
        }
        self.last = Some(segment);
    }

    /// Does what the table of section 3 of ADR-0020 says for `note`, of a commit that
    /// was taken. `home` says whether the commit is the home region's. What is changed
    /// is noted in `undo`, if there is a group it could be taken back with.
    ///
    /// It says nothing in the log: [`Players::take`] does, for a store that runs. A
    /// start, which goes through the notes of the log again, has said it before.
    pub(crate) fn apply(
        &mut self,
        note: &LoggedStay,
        home: bool,
        undo: Option<&mut Undo>,
    ) -> Taken {
        match *note {
            LoggedStay::Entering { player, entity } => {
                let record = self.records.get(&player).copied().unwrap_or(UNKNOWN);
                if !home {
                    return Taken::Dead {
                        stay: record.stay,
                        hops: record.hops,
                        why: Why::NotHome,
                    };
                }
                if entity < record.stay {
                    return Taken::Dead {
                        stay: record.stay,
                        hops: record.hops,
                        why: Why::Replaced,
                    };
                }
                if entity == record.stay {
                    return Taken::Again {
                        place: record.place,
                    };
                }
                if let Some(undo) = undo {
                    let before = self.records.get(&player).copied();
                    undo.records.entry(player).or_insert(before);
                    undo.issued.get_or_insert(self.issued);
                }
                // The place stays: it is where the player was last, whatever stay
                // they were there with.
                let raised = Record {
                    stay: entity,
                    hops: 0,
                    place: record.place,
                };
                self.records.insert(player, raised);
                self.issued = self.issued.max(entity);
                Taken::Raised {
                    place: record.place,
                }
            }
            LoggedStay::Has {
                player,
                entity,
                hops,
                place,
            } => {
                let record = self.records.get(&player).copied().unwrap_or(UNKNOWN);
                if entity > record.stay {
                    return Taken::Ahead;
                }
                if entity < record.stay || hops < record.hops {
                    let why = match entity < record.stay {
                        true => Why::Replaced,
                        false => Why::Copy,
                    };
                    return Taken::Dead {
                        stay: record.stay,
                        hops: record.hops,
                        why,
                    };
                }
                if let Some(undo) = undo {
                    let before = self.records.get(&player).copied();
                    undo.records.entry(player).or_insert(before);
                }
                let written = Record {
                    stay: entity,
                    hops,
                    place: Some(place),
                };
                self.records.insert(player, written);
                Taken::Written
            }
        }
    }

    /// As [`Players::apply`], for a commit of `region` that a running store takes:
    /// what the record calls an error is said in the log.
    pub(crate) fn take(
        &mut self,
        region: RegionId,
        home: bool,
        note: &LoggedStay,
        undo: &mut Undo,
    ) -> Taken {
        let taken = self.apply(note, home, Some(undo));
        let (LoggedStay::Entering { player, entity } | LoggedStay::Has { player, entity, .. }) =
            *note;
        match taken {
            Taken::Dead {
                why: Why::NotHome, ..
            } => error!(
                %region,
                player,
                entity,
                "a region that is not the home region named a stay as entering; the note is dropped"
            ),
            Taken::Dead {
                hops,
                why: Why::Copy,
                ..
            } => error!(
                %region,
                player,
                entity,
                hops,
                "a region named the living stay of a player with fewer hand-overs than the store was told of: a copy of it, which is answered as dead"
            ),
            Taken::Ahead => error!(
                %region,
                player,
                entity,
                "a region named a stay the home region never said it gave out; the note is dropped"
            ),
            _ => {}
        }
        taken
    }

    /// Takes back what a group changed.
    pub(crate) fn undo(&mut self, undo: Undo) {
        for (player, before) in undo.records {
            match before {
                Some(record) => self.records.insert(player, record),
                // A record the group made is gone again, and not left with stay 0.
                None => self.records.remove(&player),
            };
        }
        if let Some(issued) = undo.issued {
            self.issued = issued;
        }
        if let Some(last) = undo.last {
            self.last = last;
        }
    }
}

/// The note as the log keeps it.
pub(crate) fn logged(note: &StayNote) -> LoggedStay {
    match note {
        StayNote::Entering { player, entity } => LoggedStay::Entering {
            player: player.0.as_u128(),
            entity: entity.0,
        },
        StayNote::Has {
            player,
            entity,
            hops,
            place,
        } => LoggedStay::Has {
            player: player.0.as_u128(),
            entity: entity.0,
            hops: *hops,
            place: logged_place(place),
        },
    }
}

/// The place as the log and the players' file keep it.
pub(crate) fn logged_place(place: &Place) -> LoggedPlace {
    let Vec3 { x, y, z } = place.pose.position;
    LoggedPlace {
        position: [x, y, z],
        yaw: place.pose.yaw,
        pitch: place.pose.pitch,
        on_ground: place.pose.on_ground,
        flying: place.flying,
        selected_slot: place.selected_slot,
        hotbar: place
            .hotbar
            .map(|slot| slot.map(|stack| (stack.item, stack.count))),
    }
}

/// The place as a region is told it.
pub(crate) fn place(logged: &LoggedPlace) -> Place {
    let [x, y, z] = logged.position;
    Place {
        pose: Pose {
            position: Vec3::new(x, y, z),
            yaw: logged.yaw,
            pitch: logged.pitch,
            on_ground: logged.on_ground,
        },
        flying: logged.flying,
        hotbar: logged
            .hotbar
            .map(|slot| slot.map(|(item, count)| ItemStack { item, count })),
        selected_slot: logged.selected_slot,
    }
}

/// The chunk the feet of someone at `place` are in.
pub(crate) fn chunk_of(place: &LoggedPlace) -> ChunkPos {
    ChunkPos::containing(place.position[0], place.position[2])
}
