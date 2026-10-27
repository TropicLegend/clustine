//! The brain's memories (`ai.memory`, the brief's 7.1.2 and `docs/groundwork/n7j-plan.md`
//! section 2.2): a fixed table of the kinds, a slot each with the moment it expires, and
//! the typed view a brain's code reads and writes them by, with the game's rules for
//! registration, empty collections and expiry, and for the cooldowns that a behaviour
//! lowers a tick at a time, which are kept as the value they were set to and the moment
//! their lowering began, so that one at rest is written once.

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

/// A memory type (`MemoryModuleType`), by a number of Clustine's own that never changes:
/// taken once from 26.3's `memory_module_type` registry, whose names the table keeps and
/// whose order a later version may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub(crate) enum MemoryKind {
    Dummy = 0,
    Home = 1,
    JobSite = 2,
    PotentialJobSite = 3,
    MeetingPoint = 4,
    SecondaryJobSite = 5,
    Mobs = 6,
    VisibleMobs = 7,
    VisibleVillagerBabies = 8,
    NearestPlayers = 9,
    NearestVisiblePlayer = 10,
    NearestVisibleTargetablePlayer = 11,
    NearestVisibleTargetablePlayers = 12,
    WalkTarget = 13,
    LookTarget = 14,
    AttackTarget = 15,
    AttackCoolingDown = 16,
    InteractionTarget = 17,
    BreedTarget = 18,
    RideTarget = 19,
    Path = 20,
    DoorsToClose = 21,
    NearestBed = 22,
    HurtBy = 23,
    HurtByEntity = 24,
    AvoidTarget = 25,
    NearestHostile = 26,
    NearestAttackable = 27,
    HidingPlace = 28,
    HeardBellTime = 29,
    CantReachWalkTargetSince = 30,
    GolemDetectedRecently = 31,
    DangerDetectedRecently = 32,
    LastSlept = 33,
    LastWoken = 34,
    LastWorkedAtPoi = 35,
    NearestVisibleAdult = 36,
    NearestVisibleWantedItem = 37,
    NearestVisibleNemesis = 38,
    PlayDeadTicks = 39,
    TemptingPlayer = 40,
    TemptationCooldownTicks = 41,
    GazeCooldownTicks = 42,
    LongJumpCoolingDown = 43,
    LongJumpMidJump = 44,
    HasHuntingCooldown = 45,
    RamCooldownTicks = 46,
    RamTarget = 47,
    IsInWater = 48,
    IsPregnant = 49,
    IsPanicking = 50,
    UnreachableTongueTargets = 51,
    VisitedBlockPositions = 52,
    UnreachableTransportBlockPositions = 53,
    TransportItemsCooldownTicks = 54,
    ChargeCooldownTicks = 55,
    AttackTargetCooldown = 56,
    SpearFleeingTime = 57,
    SpearFleeingPosition = 58,
    SpearChargePosition = 59,
    SpearEngageTime = 60,
    SpearStatus = 61,
    AngryAt = 62,
    UniversalAnger = 63,
    AdmiringItem = 64,
    TimeTryingToReachAdmireItem = 65,
    DisableWalkToAdmireItem = 66,
    AdmiringDisabled = 67,
    HuntedRecently = 68,
    CelebrateLocation = 69,
    Dancing = 70,
    NearestVisibleHuntableHoglin = 71,
    NearestVisibleBabyHoglin = 72,
    NearestTargetablePlayerNotWearingGold = 73,
    NearbyAdultPiglins = 74,
    NearestVisibleAdultPiglins = 75,
    NearestVisibleAdultHoglins = 76,
    NearestVisibleAdultPiglin = 77,
    NearestVisibleZombified = 78,
    VisibleAdultPiglinCount = 79,
    VisibleAdultHoglinCount = 80,
    NearestPlayerHoldingWantedItem = 81,
    AteRecently = 82,
    NearestRepellent = 83,
    Pacified = 84,
    RoarTarget = 85,
    DisturbanceLocation = 86,
    RecentProjectile = 87,
    IsSniffing = 88,
    IsEmerging = 89,
    RoarSoundDelay = 90,
    DigCooldown = 91,
    RoarSoundCooldown = 92,
    SniffCooldown = 93,
    TouchCooldown = 94,
    VibrationCooldown = 95,
    SonicBoomCooldown = 96,
    SonicBoomSoundCooldown = 97,
    SonicBoomSoundDelay = 98,
    LikedPlayer = 99,
    LikedNoteblock = 100,
    LikedNoteblockCooldownTicks = 101,
    ItemPickupCooldownTicks = 102,
    SnifferExploredPositions = 103,
    SnifferSniffingTarget = 104,
    SnifferDigging = 105,
    SnifferHappy = 106,
    BreezeJumpCooldown = 107,
    BreezeShoot = 108,
    BreezeShootCharging = 109,
    BreezeShootRecover = 110,
    BreezeShootCooldown = 111,
    BreezeJumpInhaling = 112,
    BreezeJumpTarget = 113,
    BreezeLeavingWater = 114,
}

impl MemoryKind {
    /// Every kind, in the order of their numbers.
    pub(crate) const ALL: [MemoryKind; 115] = [
        Self::Dummy,
        Self::Home,
        Self::JobSite,
        Self::PotentialJobSite,
        Self::MeetingPoint,
        Self::SecondaryJobSite,
        Self::Mobs,
        Self::VisibleMobs,
        Self::VisibleVillagerBabies,
        Self::NearestPlayers,
        Self::NearestVisiblePlayer,
        Self::NearestVisibleTargetablePlayer,
        Self::NearestVisibleTargetablePlayers,
        Self::WalkTarget,
        Self::LookTarget,
        Self::AttackTarget,
        Self::AttackCoolingDown,
        Self::InteractionTarget,
        Self::BreedTarget,
        Self::RideTarget,
        Self::Path,
        Self::DoorsToClose,
        Self::NearestBed,
        Self::HurtBy,
        Self::HurtByEntity,
        Self::AvoidTarget,
        Self::NearestHostile,
        Self::NearestAttackable,
        Self::HidingPlace,
        Self::HeardBellTime,
        Self::CantReachWalkTargetSince,
        Self::GolemDetectedRecently,
        Self::DangerDetectedRecently,
        Self::LastSlept,
        Self::LastWoken,
        Self::LastWorkedAtPoi,
        Self::NearestVisibleAdult,
        Self::NearestVisibleWantedItem,
        Self::NearestVisibleNemesis,
        Self::PlayDeadTicks,
        Self::TemptingPlayer,
        Self::TemptationCooldownTicks,
        Self::GazeCooldownTicks,
        Self::LongJumpCoolingDown,
        Self::LongJumpMidJump,
        Self::HasHuntingCooldown,
        Self::RamCooldownTicks,
        Self::RamTarget,
        Self::IsInWater,
        Self::IsPregnant,
        Self::IsPanicking,
        Self::UnreachableTongueTargets,
        Self::VisitedBlockPositions,
        Self::UnreachableTransportBlockPositions,
        Self::TransportItemsCooldownTicks,
        Self::ChargeCooldownTicks,
        Self::AttackTargetCooldown,
        Self::SpearFleeingTime,
        Self::SpearFleeingPosition,
        Self::SpearChargePosition,
        Self::SpearEngageTime,
        Self::SpearStatus,
        Self::AngryAt,
        Self::UniversalAnger,
        Self::AdmiringItem,
        Self::TimeTryingToReachAdmireItem,
        Self::DisableWalkToAdmireItem,
        Self::AdmiringDisabled,
        Self::HuntedRecently,
        Self::CelebrateLocation,
        Self::Dancing,
        Self::NearestVisibleHuntableHoglin,
        Self::NearestVisibleBabyHoglin,
        Self::NearestTargetablePlayerNotWearingGold,
        Self::NearbyAdultPiglins,
        Self::NearestVisibleAdultPiglins,
        Self::NearestVisibleAdultHoglins,
        Self::NearestVisibleAdultPiglin,
        Self::NearestVisibleZombified,
        Self::VisibleAdultPiglinCount,
        Self::VisibleAdultHoglinCount,
        Self::NearestPlayerHoldingWantedItem,
        Self::AteRecently,
        Self::NearestRepellent,
        Self::Pacified,
        Self::RoarTarget,
        Self::DisturbanceLocation,
        Self::RecentProjectile,
        Self::IsSniffing,
        Self::IsEmerging,
        Self::RoarSoundDelay,
        Self::DigCooldown,
        Self::RoarSoundCooldown,
        Self::SniffCooldown,
        Self::TouchCooldown,
        Self::VibrationCooldown,
        Self::SonicBoomCooldown,
        Self::SonicBoomSoundCooldown,
        Self::SonicBoomSoundDelay,
        Self::LikedPlayer,
        Self::LikedNoteblock,
        Self::LikedNoteblockCooldownTicks,
        Self::ItemPickupCooldownTicks,
        Self::SnifferExploredPositions,
        Self::SnifferSniffingTarget,
        Self::SnifferDigging,
        Self::SnifferHappy,
        Self::BreezeJumpCooldown,
        Self::BreezeShoot,
        Self::BreezeShootCharging,
        Self::BreezeShootRecover,
        Self::BreezeShootCooldown,
        Self::BreezeJumpInhaling,
        Self::BreezeJumpTarget,
        Self::BreezeLeavingWater,
    ];

    /// Its number, which the record keys it by.
    pub(crate) fn id(self) -> u16 {
        self as u16
    }

    /// The kind numbered `id`, if one is.
    pub(crate) fn of_id(id: u16) -> Option<Self> {
        Self::ALL.get(usize::from(id)).copied()
    }

    /// Its name in the game's registry.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dummy => "dummy",
            Self::Home => "home",
            Self::JobSite => "job_site",
            Self::PotentialJobSite => "potential_job_site",
            Self::MeetingPoint => "meeting_point",
            Self::SecondaryJobSite => "secondary_job_site",
            Self::Mobs => "mobs",
            Self::VisibleMobs => "visible_mobs",
            Self::VisibleVillagerBabies => "visible_villager_babies",
            Self::NearestPlayers => "nearest_players",
            Self::NearestVisiblePlayer => "nearest_visible_player",
            Self::NearestVisibleTargetablePlayer => "nearest_visible_targetable_player",
            Self::NearestVisibleTargetablePlayers => "nearest_visible_targetable_players",
            Self::WalkTarget => "walk_target",
            Self::LookTarget => "look_target",
            Self::AttackTarget => "attack_target",
            Self::AttackCoolingDown => "attack_cooling_down",
            Self::InteractionTarget => "interaction_target",
            Self::BreedTarget => "breed_target",
            Self::RideTarget => "ride_target",
            Self::Path => "path",
            Self::DoorsToClose => "doors_to_close",
            Self::NearestBed => "nearest_bed",
            Self::HurtBy => "hurt_by",
            Self::HurtByEntity => "hurt_by_entity",
            Self::AvoidTarget => "avoid_target",
            Self::NearestHostile => "nearest_hostile",
            Self::NearestAttackable => "nearest_attackable",
            Self::HidingPlace => "hiding_place",
            Self::HeardBellTime => "heard_bell_time",
            Self::CantReachWalkTargetSince => "cant_reach_walk_target_since",
            Self::GolemDetectedRecently => "golem_detected_recently",
            Self::DangerDetectedRecently => "danger_detected_recently",
            Self::LastSlept => "last_slept",
            Self::LastWoken => "last_woken",
            Self::LastWorkedAtPoi => "last_worked_at_poi",
            Self::NearestVisibleAdult => "nearest_visible_adult",
            Self::NearestVisibleWantedItem => "nearest_visible_wanted_item",
            Self::NearestVisibleNemesis => "nearest_visible_nemesis",
            Self::PlayDeadTicks => "play_dead_ticks",
            Self::TemptingPlayer => "tempting_player",
            Self::TemptationCooldownTicks => "temptation_cooldown_ticks",
            Self::GazeCooldownTicks => "gaze_cooldown_ticks",
            Self::LongJumpCoolingDown => "long_jump_cooling_down",
            Self::LongJumpMidJump => "long_jump_mid_jump",
            Self::HasHuntingCooldown => "has_hunting_cooldown",
            Self::RamCooldownTicks => "ram_cooldown_ticks",
            Self::RamTarget => "ram_target",
            Self::IsInWater => "is_in_water",
            Self::IsPregnant => "is_pregnant",
            Self::IsPanicking => "is_panicking",
            Self::UnreachableTongueTargets => "unreachable_tongue_targets",
            Self::VisitedBlockPositions => "visited_block_positions",
            Self::UnreachableTransportBlockPositions => "unreachable_transport_block_positions",
            Self::TransportItemsCooldownTicks => "transport_items_cooldown_ticks",
            Self::ChargeCooldownTicks => "charge_cooldown_ticks",
            Self::AttackTargetCooldown => "attack_target_cooldown",
            Self::SpearFleeingTime => "spear_fleeing_time",
            Self::SpearFleeingPosition => "spear_fleeing_position",
            Self::SpearChargePosition => "spear_charge_position",
            Self::SpearEngageTime => "spear_engage_time",
            Self::SpearStatus => "spear_status",
            Self::AngryAt => "angry_at",
            Self::UniversalAnger => "universal_anger",
            Self::AdmiringItem => "admiring_item",
            Self::TimeTryingToReachAdmireItem => "time_trying_to_reach_admire_item",
            Self::DisableWalkToAdmireItem => "disable_walk_to_admire_item",
            Self::AdmiringDisabled => "admiring_disabled",
            Self::HuntedRecently => "hunted_recently",
            Self::CelebrateLocation => "celebrate_location",
            Self::Dancing => "dancing",
            Self::NearestVisibleHuntableHoglin => "nearest_visible_huntable_hoglin",
            Self::NearestVisibleBabyHoglin => "nearest_visible_baby_hoglin",
            Self::NearestTargetablePlayerNotWearingGold => {
                "nearest_targetable_player_not_wearing_gold"
            }
            Self::NearbyAdultPiglins => "nearby_adult_piglins",
            Self::NearestVisibleAdultPiglins => "nearest_visible_adult_piglins",
            Self::NearestVisibleAdultHoglins => "nearest_visible_adult_hoglins",
            Self::NearestVisibleAdultPiglin => "nearest_visible_adult_piglin",
            Self::NearestVisibleZombified => "nearest_visible_zombified",
            Self::VisibleAdultPiglinCount => "visible_adult_piglin_count",
            Self::VisibleAdultHoglinCount => "visible_adult_hoglin_count",
            Self::NearestPlayerHoldingWantedItem => "nearest_player_holding_wanted_item",
            Self::AteRecently => "ate_recently",
            Self::NearestRepellent => "nearest_repellent",
            Self::Pacified => "pacified",
            Self::RoarTarget => "roar_target",
            Self::DisturbanceLocation => "disturbance_location",
            Self::RecentProjectile => "recent_projectile",
            Self::IsSniffing => "is_sniffing",
            Self::IsEmerging => "is_emerging",
            Self::RoarSoundDelay => "roar_sound_delay",
            Self::DigCooldown => "dig_cooldown",
            Self::RoarSoundCooldown => "roar_sound_cooldown",
            Self::SniffCooldown => "sniff_cooldown",
            Self::TouchCooldown => "touch_cooldown",
            Self::VibrationCooldown => "vibration_cooldown",
            Self::SonicBoomCooldown => "sonic_boom_cooldown",
            Self::SonicBoomSoundCooldown => "sonic_boom_sound_cooldown",
            Self::SonicBoomSoundDelay => "sonic_boom_sound_delay",
            Self::LikedPlayer => "liked_player",
            Self::LikedNoteblock => "liked_noteblock",
            Self::LikedNoteblockCooldownTicks => "liked_noteblock_cooldown_ticks",
            Self::ItemPickupCooldownTicks => "item_pickup_cooldown_ticks",
            Self::SnifferExploredPositions => "sniffer_explored_positions",
            Self::SnifferSniffingTarget => "sniffer_sniffing_target",
            Self::SnifferDigging => "sniffer_digging",
            Self::SnifferHappy => "sniffer_happy",
            Self::BreezeJumpCooldown => "breeze_jump_cooldown",
            Self::BreezeShoot => "breeze_shoot",
            Self::BreezeShootCharging => "breeze_shoot_charging",
            Self::BreezeShootRecover => "breeze_shoot_recover",
            Self::BreezeShootCooldown => "breeze_shoot_cooldown",
            Self::BreezeJumpInhaling => "breeze_jump_inhaling",
            Self::BreezeJumpTarget => "breeze_jump_target",
            Self::BreezeLeavingWater => "breeze_leaving_water",
        }
    }

    /// Whether a sensor's scan rewrites it whole from the world, so that the region keeps
    /// it in its memory of the entity and not in the record, and senses it again after a
    /// take-in (the plan, section 2.2 item 7).
    pub(crate) fn sensed(self) -> bool {
        matches!(
            self,
            Self::Mobs
                | Self::VisibleMobs
                | Self::NearestPlayers
                | Self::NearestVisiblePlayer
                | Self::NearestVisibleTargetablePlayer
                | Self::NearestVisibleTargetablePlayers
                | Self::HurtBy
                | Self::NearestHostile
                | Self::NearestAttackable
                | Self::NearestVisibleAdult
                | Self::NearestVisibleWantedItem
                | Self::TemptingPlayer
                | Self::IsInWater
        )
    }

    /// Whether a behaviour lowers it by one a tick (`CountDownCooldownTicks`,
    /// `ValidatePlayDead`), so that it is kept as a [`Countdown`] (section 2.2 item 3).
    pub(crate) fn counts_down(self) -> bool {
        matches!(
            self,
            Self::PlayDeadTicks
                | Self::TemptationCooldownTicks
                | Self::GazeCooldownTicks
                | Self::LongJumpCoolingDown
                | Self::RamCooldownTicks
                | Self::TransportItemsCooldownTicks
                | Self::ChargeCooldownTicks
                | Self::AttackTargetCooldown
                | Self::LikedNoteblockCooldownTicks
                | Self::ItemPickupCooldownTicks
        )
    }

    /// Whether the game saves it (its type has a codec, `MemoryModuleType.register(name,
    /// codec)`): what a fresh load keeps (section 2.10).
    pub(crate) fn saved_by_game(self) -> bool {
        matches!(
            self,
            Self::Home
                | Self::JobSite
                | Self::PotentialJobSite
                | Self::MeetingPoint
                | Self::GolemDetectedRecently
                | Self::DangerDetectedRecently
                | Self::LastSlept
                | Self::LastWoken
                | Self::LastWorkedAtPoi
                | Self::PlayDeadTicks
                | Self::TemptationCooldownTicks
                | Self::GazeCooldownTicks
                | Self::LongJumpCoolingDown
                | Self::HasHuntingCooldown
                | Self::RamCooldownTicks
                | Self::IsInWater
                | Self::IsPregnant
                | Self::IsPanicking
                | Self::VisitedBlockPositions
                | Self::UnreachableTransportBlockPositions
                | Self::ChargeCooldownTicks
                | Self::AttackTargetCooldown
                | Self::AngryAt
                | Self::UniversalAnger
                | Self::AdmiringItem
                | Self::AdmiringDisabled
                | Self::HuntedRecently
                | Self::RecentProjectile
                | Self::IsSniffing
                | Self::IsEmerging
                | Self::RoarSoundDelay
                | Self::DigCooldown
                | Self::RoarSoundCooldown
                | Self::SniffCooldown
                | Self::TouchCooldown
                | Self::VibrationCooldown
                | Self::SonicBoomCooldown
                | Self::SonicBoomSoundCooldown
                | Self::SonicBoomSoundDelay
                | Self::LikedPlayer
                | Self::LikedNoteblock
                | Self::LikedNoteblockCooldownTicks
                | Self::ItemPickupCooldownTicks
                | Self::SnifferExploredPositions
                | Self::BreezeJumpCooldown
                | Self::BreezeShoot
                | Self::BreezeShootCharging
                | Self::BreezeShootRecover
                | Self::BreezeShootCooldown
                | Self::BreezeJumpInhaling
                | Self::BreezeJumpTarget
                | Self::BreezeLeavingWater
        )
    }
}

/// What a memory holds: the game's value types, with entities by their keys (a player by
/// their stay's entity), resolved through the read view whenever they are read, never
/// kept as a position (the plan, section 2.2 item 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum Value {
    /// `Unit`, a flag that is there or not.
    Unit,
    Bool(bool),
    Int(i32),
    Long(i64),
    /// A UUID (`liked_player`, `angry_at`).
    Uuid(u128),
    Entity(EntityId),
    /// A list of entities, nearest first, as a sensor sorts them.
    Entities(Vec<EntityId>),
    Block(BlockPos),
    /// A list or a set of blocks (a set sorted, its order never read but in saving).
    Blocks(Vec<BlockPos>),
    /// `PositionTracker`: an entity's (`EntityTracker`) or a block's or point's
    /// (`BlockPosTracker`).
    Tracker(Tracker),
    /// `WalkTarget`: where to, at what speed, and how close is close enough.
    Walk {
        to: Tracker,
        speed: f32,
        close_enough: i32,
    },
    /// The `path` memory: a mark that says "the navigation's path", which the body keeps
    /// (section 2.2 item 5).
    Path,
}

impl Value {
    /// Whether it is an empty collection, which `setMemory` takes for none.
    fn is_empty_collection(&self) -> bool {
        match self {
            Value::Entities(entities) => entities.is_empty(),
            Value::Blocks(blocks) => blocks.is_empty(),
            _ => false,
        }
    }
}

/// `PositionTracker` (`EntityTracker`, `BlockPosTracker`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) enum Tracker {
    /// An entity, its position its eyes' where `eyes`, its block its eyes' where
    /// `eyes_block`.
    Entity {
        key: EntityId,
        eyes: bool,
        eyes_block: bool,
    },
    /// A block, at its centre.
    Block(BlockPos),
    /// A point, with the block it is in.
    Point([f64; 3]),
}

/// A memory's slot (`MemorySlot`): its value and the moment it expires, the last `lived`
/// it is there for, where it expires at all; or, for a kind that [`MemoryKind::counts_down`],
/// its count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum Slot {
    Kept { value: Value, expires: Option<u32> },
    Count(Countdown),
}

/// A cooldown that a behaviour lowers by one each tick it runs: the value it was set to,
/// and the tick its lowering began, if it has, after which every tick takes one off, in
/// that tick's running pass, at the lowering behaviour's place in it (the brief's 7.3.12;
/// the plan, section 2.2 item 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Countdown {
    pub(crate) value: i32,
    pub(crate) since: Option<u32>,
}

impl Countdown {
    /// Its value in the tick `now`, before the lowering of that tick where `lowered` is
    /// false and after it where it is true.
    pub(crate) fn at(self, now: u32, lowered: bool) -> i32 {
        let Some(since) = self.since else {
            return self.value;
        };
        let steps = i64::from(now.saturating_sub(since)) + i64::from(lowered && now >= since);
        let left = i64::from(self.value) - steps;
        i32::try_from(left).unwrap_or(i32::MIN)
    }
}

/// `MemoryStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Status {
    Registered,
    Present,
    Absent,
}

/// A brain's memories as its code reads and writes them, in a tick or outside one
/// (`Brain`'s memory methods): the slots of the registered kinds, the moment `now` reads
/// are made at (the mob's `lived`), and, while the brain ticks, which countdowns have
/// been lowered in this tick so far.
#[derive(Debug, Clone, Default)]
pub(crate) struct Memories {
    pub(crate) registered: BTreeSet<MemoryKind>,
    pub(crate) slots: BTreeMap<MemoryKind, Slot>,
    pub(crate) now: u32,
    /// Inside the brain's tick: the countdowns lowered in it so far. Outside it (`None`)
    /// every countdown reads as after its lowering of the mob's last tick, which is
    /// what the game reads between ticks and before the mob's own tick in a server tick.
    pub(crate) lowered: Option<BTreeSet<MemoryKind>>,
    /// The kinds written since the record was read, which the record writes again.
    pub(crate) changed: BTreeSet<MemoryKind>,
    /// Inside the brain's tick: the activities active as the behaviour now asked was
    /// reached (`Brain.isActive`), which a behaviour that changes them reads (a
    /// villager's panic). Kept here, beside the memories every behaviour is handed, so
    /// that the behaviours' contract needs nothing new.
    pub(crate) active: BTreeSet<super::Activity>,
    /// The activities a behaviour asked to be made active in this tick
    /// (`setActiveActivityIfPossible`), in the order asked, which the brain makes so as
    /// soon as that behaviour returns, before it asks the next: a villager's panic and
    /// its schedule, which change what the rest of the tick's passes visit.
    pub(crate) requested: Vec<super::Activity>,
    /// Inside the brain's tick: the brain a behaviour asked to be made again
    /// (`refreshBrain`, a villager's profession taken or lost), its sensors' delays drawn
    /// as it asked, which the brain becomes once its tick is done.
    pub(crate) refresh: Option<super::BrainState>,
}

impl Memories {
    /// Whether `kind` is registered.
    pub(crate) fn registered(&self, kind: MemoryKind) -> bool {
        self.registered.contains(&kind)
    }

    /// `checkMemory(kind, status)`: false for a kind not registered.
    pub(crate) fn check(&self, kind: MemoryKind, status: Status) -> bool {
        if !self.registered(kind) {
            return false;
        }
        match status {
            Status::Registered => true,
            Status::Present => self.get(kind).is_some(),
            Status::Absent => self.get(kind).is_none(),
        }
    }

    /// `getMemory(kind)`: its value, none where it has none or is not registered (where
    /// the game throws, which a test of each type's registration keeps from happening).
    /// A countdown reads as its count at this place in the tick.
    pub(crate) fn get(&self, kind: MemoryKind) -> Option<Value> {
        if !self.registered(kind) {
            return None;
        }
        match self.slots.get(&kind)? {
            Slot::Kept { value, .. } => Some(value.clone()),
            Slot::Count(count) => Some(Value::Int(count.at(self.now, self.lowered_now(kind)))),
        }
    }

    /// Its value as an int, if it is one.
    pub(crate) fn int(&self, kind: MemoryKind) -> Option<i32> {
        match self.get(kind)? {
            Value::Int(value) => Some(value),
            _ => None,
        }
    }

    /// Whether it holds `value` (`isMemoryValue`).
    pub(crate) fn is(&self, kind: MemoryKind, value: &Value) -> bool {
        self.get(kind).as_ref() == Some(value)
    }

    /// Whether this tick's lowering of `kind` has happened at this place.
    fn lowered_now(&self, kind: MemoryKind) -> bool {
        self.lowered
            .as_ref()
            .is_none_or(|lowered| lowered.contains(&kind))
    }

    /// `setMemory(kind, value)`: set with no expiry, replacing any it had; an empty
    /// collection erases; nothing for a kind not registered. A countdown set while its
    /// lowering runs goes on being lowered from its new value, from this tick where this
    /// tick's lowering has not come yet and from the next where it has.
    pub(crate) fn set(&mut self, kind: MemoryKind, value: Value) {
        if !self.registered(kind) {
            return;
        }
        if value.is_empty_collection() {
            self.erase(kind);
            return;
        }
        let slot = match (kind.counts_down(), &value) {
            (true, Value::Int(count)) => {
                let running = matches!(
                    self.slots.get(&kind),
                    Some(Slot::Count(Countdown { since: Some(_), .. }))
                );
                let since = running.then(|| self.now + u32::from(self.lowered_now(kind)));
                Slot::Count(Countdown {
                    value: *count,
                    since,
                })
            }
            _ => Slot::Kept {
                value,
                expires: None,
            },
        };
        self.slots.insert(kind, slot);
        self.changed.insert(kind);
    }

    /// `setMemoryWithExpiry(kind, value, ttl)`: there for the behaviours of this tick and
    /// the next `ttl`, gone in the forget pass after them.
    pub(crate) fn set_with_expiry(&mut self, kind: MemoryKind, value: Value, ttl: i64) {
        if !self.registered(kind) {
            return;
        }
        if value.is_empty_collection() {
            self.erase(kind);
            return;
        }
        let expires = u32::try_from(i64::from(self.now).saturating_add(ttl.max(0))).ok();
        self.slots.insert(kind, Slot::Kept { value, expires });
        self.changed.insert(kind);
    }

    /// `eraseMemory(kind)`.
    pub(crate) fn erase(&mut self, kind: MemoryKind) {
        if self.slots.remove(&kind).is_some() {
            self.changed.insert(kind);
        }
    }

    /// `getTimeUntilExpiry(kind)`: the ticks it is there for still, read after this
    /// tick's forget pass as the game's is; `i64::MAX` for one with no expiry or none
    /// (`Long.MAX_VALUE`).
    pub(crate) fn expires_in(&self, kind: MemoryKind) -> i64 {
        match self.slots.get(&kind) {
            Some(Slot::Kept {
                expires: Some(expires),
                ..
            }) => i64::from(*expires) - i64::from(self.now),
            _ => i64::MAX,
        }
    }

    /// The forget pass of the brain's tick at `now` (`forgetOutdatedMemories`): every
    /// memory whose last tick has passed goes, and every countdown is as before this
    /// tick's lowering.
    pub(crate) fn forget(&mut self, now: u32) {
        self.now = now;
        self.lowered = Some(BTreeSet::new());
        let gone: Vec<MemoryKind> = self
            .slots
            .iter()
            .filter(|(_, slot)| {
                matches!(slot, Slot::Kept { expires: Some(expires), .. } if *expires < now)
            })
            .map(|(kind, _)| *kind)
            .collect();
        for kind in gone {
            self.erase(kind);
        }
    }

    /// What the countdown `kind`'s lowering behaviour does as it starts: the lowering
    /// begins in this tick, where it had not begun.
    pub(crate) fn begin_lowering(&mut self, kind: MemoryKind) {
        let now = self.now;
        if let Some(Slot::Count(count)) = self.slots.get_mut(&kind)
            && count.since.is_none()
        {
            count.since = Some(now);
            self.changed.insert(kind);
        }
    }

    /// What the countdown `kind`'s lowering behaviour does as it ticks: this tick's
    /// lowering is done; readers after it read one less. Nothing is written.
    pub(crate) fn lower(&mut self, kind: MemoryKind) {
        if let Some(lowered) = self.lowered.as_mut() {
            lowered.insert(kind);
        }
    }

    /// The end of the brain's tick: reads after it are outside it.
    pub(crate) fn end_tick(&mut self) {
        self.lowered = None;
    }
}
