//! A mob's record is parts, each kept by the code it belongs to: a type of its own, an
//! id fixed here for good, and the version of its encoding (`docs/groundwork/n7-plan.md`,
//! section 11.3, items 1 to 3).
//!
//! A part is encoded with postcard, which writes the same bytes for the same value
//! wherever it runs. A build reads every earlier version of a part it knows and drops a
//! part whose id it does not know, so that a later step adds a part, or a goal, without
//! a change to the store's format. No version but the first exists yet.

use clustine_world::{EntityField, Entry};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The ids of the body's parts. Never reused, never renumbered.
pub(crate) mod body_ids {
    /// Health, hurt and death times, invulnerability, air and freezing.
    pub(crate) const VITALS: u16 = 1;
    /// The attribute instances that exist, with their base values and modifiers.
    pub(crate) const ATTRIBUTES: u16 = 2;
    /// What it wears and holds, and the chances it drops them.
    pub(crate) const EQUIPMENT: u16 = 3;
    /// The head's and the body's yaw, and how long the head has been still.
    pub(crate) const HEAD: u16 = 4;
    /// The movement inputs its controls set: `xxa`, `yya`, `zza`, jumping, speed.
    pub(crate) const INPUTS: u16 = 5;
    /// The move, look and jump controls' wanted states.
    pub(crate) const CONTROLS: u16 = 6;
    /// The path it follows, whole as it was made.
    pub(crate) const PATH: u16 = 7;
    /// How far along its path it is, and the navigation's timers.
    pub(crate) const PROGRESS: u16 = 8;
    /// Its target, whether it is aggressive, and who last hurt it and when.
    pub(crate) const COMBAT: u16 = 9;
    /// Persistence, no AI, the left hand, picking up loot.
    pub(crate) const FLAGS: u16 = 10;
    /// Where it is kept to (a leash's or a home's restriction).
    pub(crate) const HOME: u16 = 11;
    /// The stamps of the counters that count every tick: the ambient sound's, the
    /// idle time's.
    pub(crate) const IDLE: u16 = 12;
    /// The navigation's timeout timer and its last check, which move every tick a mob
    /// follows a path: apart, so that the rest of its progress is not written with them.
    pub(crate) const TIMEOUT: u16 = 13;
    /// What last hurt it, and the game time of it (`lastDamageSource`).
    pub(crate) const LAST_DAMAGE: u16 = 14;
    /// What a monster's idle time gained in the light.
    pub(crate) const IDLE_GAINED: u16 = 15;
    /// The effects it has, each with its amplifier and its end.
    pub(crate) const EFFECTS: u16 = 16;
    /// Whether it swims (`Entity`'s shared flag 4), which of the mobs only a drowned
    /// sets.
    pub(crate) const SWIMMING: u16 = 17;
}

/// The ids of the mind's parts below [`mind_ids::FIRST_GOAL`]; a goal's part is its
/// slot's index among its type's goals above it.
pub(crate) mod mind_ids {
    /// The goal selector's running goals and the flags they hold.
    pub(crate) const GOAL_SELECTOR: u16 = 1;
    /// The target selector's.
    pub(crate) const TARGET_SELECTOR: u16 = 2;
    /// The first goal slot's part; a type's goals are numbered in the order it adds
    /// them, its target goals after its goals.
    pub(crate) const FIRST_GOAL: u16 = 64;
}

/// A part of a mob's record.
pub(crate) trait Part: Serialize + DeserializeOwned + Default + PartialEq {
    /// Its id in the body or the mind.
    const ID: u16;
    /// The version of its encoding that this build writes.
    const VERSION: u8;
}

/// Why a mob's record could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PartError {
    /// The record is no mob's.
    NotAMob,
    /// A part's bytes are not its encoding, or of a version this build does not know.
    Unreadable { id: u16, version: u8 },
}

/// The entry of `part`.
pub(crate) fn encode<P: Part>(part: &P) -> Entry {
    Entry {
        id: P::ID,
        version: P::VERSION,
        bytes: postcard::to_allocvec(part).expect("a part is encoded into memory"),
    }
}

/// The part `P` of `entries`, or `P` as it is fresh where they have none.
pub(crate) fn decode<P: Part>(entries: &[Entry]) -> Result<P, PartError> {
    let Ok(at) = entries.binary_search_by_key(&P::ID, |entry| entry.id) else {
        return Ok(P::default());
    };
    let entry = &entries[at];
    if entry.version != P::VERSION {
        return Err(PartError::Unreadable {
            id: entry.id,
            version: entry.version,
        });
    }
    postcard::from_bytes(&entry.bytes).map_err(|_| PartError::Unreadable {
        id: entry.id,
        version: entry.version,
    })
}

/// The part `P` of `entries` as [`decode`] reads it, or its entry's bytes as `Old` where
/// they are of the version `old`, the one before, which the caller makes the part of.
pub(crate) fn decode_or_older<P: Part, Old: serde::de::DeserializeOwned>(
    entries: &[Entry],
    old: u8,
) -> Result<Result<P, Old>, PartError> {
    let Ok(at) = entries.binary_search_by_key(&P::ID, |entry| entry.id) else {
        return Ok(Ok(P::default()));
    };
    let entry = &entries[at];
    let unreadable = || PartError::Unreadable {
        id: entry.id,
        version: entry.version,
    };
    if entry.version == old && old != P::VERSION {
        return postcard::from_bytes(&entry.bytes)
            .map(Err)
            .map_err(|_| unreadable());
    }
    decode(entries).map(Ok)
}

/// Puts the entry of `part` into `entries`, ascending by id, unless the part is as a
/// fresh one is, which the record leaves out.
pub(crate) fn put<P: Part>(entries: &mut Vec<Entry>, part: &P) {
    if *part == P::default() {
        return;
    }
    let entry = encode(part);
    match entries.binary_search_by_key(&entry.id, |held| held.id) {
        Ok(at) => entries[at] = entry,
        Err(at) => entries.insert(at, entry),
    }
}

/// The fields that make `before` into `after`, both ascending by id: `set` of each entry
/// new or changed, `gone` of each id no longer there.
pub(crate) fn diff(
    before: &[Entry],
    after: &[Entry],
    fields: &mut Vec<EntityField>,
    set: fn(Entry) -> EntityField,
    gone: fn(u16) -> EntityField,
) {
    let (mut old, mut new) = (before.iter().peekable(), after.iter().peekable());
    loop {
        match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(was), None) => {
                fields.push(gone(was.id));
                old.next();
            }
            (None, Some(is)) => {
                fields.push(set((*is).clone()));
                new.next();
            }
            (Some(was), Some(is)) => match was.id.cmp(&is.id) {
                std::cmp::Ordering::Less => {
                    fields.push(gone(was.id));
                    old.next();
                }
                std::cmp::Ordering::Greater => {
                    fields.push(set((*is).clone()));
                    new.next();
                }
                std::cmp::Ordering::Equal => {
                    if was != is {
                        fields.push(set((*is).clone()));
                    }
                    old.next();
                    new.next();
                }
            },
        }
    }
}
