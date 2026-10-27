//! A raid as the world keeps it (`Raid`; `docs/groundwork/n10-plan.md`, section 9.1): a
//! record kept in a chunk, at its making its centre's, ticked by the region that ticks
//! that chunk, and moved with the chunk through merges, splits, lends and hand-overs.
//!
//! The game's codec keeps the raid's progress and its heroes (`Raid.MAP_CODEC`,
//! `docs/groundwork/n10-dumps.md`, part C); Clustine keeps besides what the game makes
//! again at a load and a raid that moves between regions must not lose: its bar's UUID (or
//! a client would see a second bar), its members by wave with their health, its leaders,
//! and the place its next wave comes from.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BlockPos, EntityId, Float};

/// Where a raid stands (`Raid$RaidStatus`), in the game's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaidStatus {
    Ongoing,
    Victory,
    Loss,
    Stopped,
}

impl RaidStatus {
    pub fn id(self) -> u8 {
        match self {
            Self::Ongoing => 0,
            Self::Victory => 1,
            Self::Loss => 2,
            Self::Stopped => 3,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Ongoing,
            1 => Self::Victory,
            2 => Self::Loss,
            3 => Self::Stopped,
            _ => return None,
        })
    }
}

/// What a raid's bar says (`docs/groundwork/n10-dumps.md`, part C, section 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaidBarName {
    /// `event.minecraft.raid`.
    Raid,
    /// `event.minecraft.raid`, `" - "` and `event.minecraft.raid.raiders_remaining` of
    /// this many: shown for one or two left.
    RaidersRemaining(i32),
    /// `event.minecraft.raid.victory.full`.
    Victory,
    /// `event.minecraft.raid.defeat.full`.
    Defeat,
}

/// A raider of the raid: its wave, its key, and its health as the raid last counted it
/// (`Raid.updateBossbar`'s sum reads each member's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaidMember {
    pub wave: i32,
    pub key: EntityId,
    pub health: Float,
}

/// A raid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Raid {
    /// Unique in the world: an entity key of the region that made it.
    pub id: EntityId,
    pub centre: BlockPos,
    pub started: bool,
    /// `active`: its centre's chunk is loaded (in Clustine: the record's chunk ticked and
    /// the centre's in its region's land or copies).
    pub active: bool,
    pub ticks_active: u64,
    pub omen_level: i32,
    pub groups_spawned: i32,
    pub group_count: i32,
    pub cooldown_ticks: i32,
    pub post_raid_ticks: i32,
    pub celebration_ticks: i32,
    pub total_health: Float,
    pub status: RaidStatus,
    pub spawn_pos: Option<BlockPos>,
    /// The bar's UUID, which stays the raid's wherever it is held, whether it is shown
    /// (`setVisible`), its progress and its name.
    pub bar: Uuid,
    pub bar_shown: bool,
    pub bar_progress: Float,
    pub bar_name: RaidBarName,
    /// The heroes, by UUID, ascending.
    pub heroes: Vec<Uuid>,
    /// The members, ascending by wave and then key.
    pub members: Vec<RaidMember>,
    /// Each wave's leader, ascending by wave.
    pub leaders: Vec<(i32, EntityId)>,
}

impl Raid {
    /// `isOver`: won or lost.
    pub fn is_over(&self) -> bool {
        matches!(self.status, RaidStatus::Victory | RaidStatus::Loss)
    }

    /// `isStopped`.
    pub fn is_stopped(&self) -> bool {
        self.status == RaidStatus::Stopped
    }

    /// `isBetweenWaves`: started, not over, and none of its raiders alive.
    pub fn is_between_waves(&self) -> bool {
        self.has_first_wave_spawned() && self.members.is_empty() && self.cooldown_ticks > 0
    }

    /// `hasFirstWaveSpawned`.
    pub fn has_first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }
}
