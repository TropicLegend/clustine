//! Activities (`schedule.Activity`) and the order the brain visits them in at one
//! priority (the brief's 7.1.1 finding 6): the game keeps them in a `HashMap` keyed by
//! the activity, whose hash is its name's `String.hashCode()`, so that at one priority the
//! activities are visited by their buckets in a table of sixteen, and in one bucket in the
//! order they were added.

use serde::{Deserialize, Serialize};

/// An activity, by a number of Clustine's own taken once from 26.3's `activity`
/// registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub(crate) enum Activity {
    Core = 0,
    Idle = 1,
    Work = 2,
    Play = 3,
    Rest = 4,
    Meet = 5,
    Panic = 6,
    Raid = 7,
    PreRaid = 8,
    Hide = 9,
    Fight = 10,
    Celebrate = 11,
    AdmireItem = 12,
    Avoid = 13,
    Ride = 14,
    PlayDead = 15,
    LongJump = 16,
    Ram = 17,
    Tongue = 18,
    Swim = 19,
    LaySpawn = 20,
    Sniff = 21,
    Investigate = 22,
    Roar = 23,
    Emerge = 24,
    Dig = 25,
}

impl Activity {
    /// Its name, as the game's `Activity` has it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Idle => "idle",
            Self::Work => "work",
            Self::Play => "play",
            Self::Rest => "rest",
            Self::Meet => "meet",
            Self::Panic => "panic",
            Self::Raid => "raid",
            Self::PreRaid => "pre_raid",
            Self::Hide => "hide",
            Self::Fight => "fight",
            Self::Celebrate => "celebrate",
            Self::AdmireItem => "admire_item",
            Self::Avoid => "avoid",
            Self::Ride => "ride",
            Self::PlayDead => "play_dead",
            Self::LongJump => "long_jump",
            Self::Ram => "ram",
            Self::Tongue => "tongue",
            Self::Swim => "swim",
            Self::LaySpawn => "lay_spawn",
            Self::Sniff => "sniff",
            Self::Investigate => "investigate",
            Self::Roar => "roar",
            Self::Emerge => "emerge",
            Self::Dig => "dig",
        }
    }

    /// Its bucket in a `HashMap` of capacity 16: `(h ^ (h >>> 16)) & 15` of its name's
    /// `String.hashCode()`, which `Activity.hashCode` returns.
    pub(crate) fn bucket(self) -> u8 {
        let hash = java_string_hash(self.name());
        let spread = hash ^ ((hash as u32) >> 16) as i32;
        (spread & 15) as u8
    }
}

/// `String.hashCode()`: `s[0]·31^(n−1) + … + s[n−1]`, in wrapping 32-bit arithmetic, over
/// the UTF-16 units, which for these names are their bytes.
pub(crate) fn java_string_hash(name: &str) -> i32 {
    name.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}
