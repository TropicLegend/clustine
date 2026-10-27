//! What the world keeps of N9's projectiles beside what every entity keeps
//! (`docs/groundwork/n9-plan.md`, section 4.1): arrows and tridents, the llama's spit,
//! firework rockets, the area effect cloud and the evoker's fangs. The thrown items keep
//! [`crate::EntityData::Thrown`] as before.
//!
//! The game's counters that only count ticks are kept as the `lived` they began at, as
//! A28 section 3 has derived counters, so that a projectile lying still commits nothing:
//! an arrow's time in the ground (its despawn at 1200 and the tipped arrow's 600), a
//! cloud's age, the fangs' warmup and life.

use serde::{Deserialize, Serialize};

use crate::components::PotionContents;
use crate::{Double, EntityId, Float, ItemStack};

/// `AbstractArrow.Pickup`, in the game's order of ids (n9 AbstractArrow$Pickup 81–96).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pickup {
    #[default]
    Disallowed,
    Allowed,
    CreativeOnly,
}

/// Where an arrow is stuck: the block state it hit (`lastState`), by whose change it
/// knows to fall, and the `lived` it landed at, from which its time in the ground is
/// counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InGround {
    pub state: u16,
    pub since: u32,
}

/// An arrow, a spectral arrow or a trident (`AbstractArrow`, `Arrow`, `SpectralArrow`,
/// `ThrownTrident`; D9 1.1). The kind is its entity type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrow {
    pub pickup: Pickup,
    /// `pickupItemStack`: what a player picks up, and where a tipped arrow's potion and
    /// a trident's enchantments are read from.
    pub pickup_stack: ItemStack,
    /// `firedFromWeapon`: the bow or crossbow it was shot from (none for a trident).
    pub weapon: Option<ItemStack>,
    /// `owner`: who shot it, by key.
    pub owner: Option<EntityId>,
    /// `leftOwner`: it has been clear of its owner's unit once.
    pub left_owner: bool,
    /// `hasBeenShot`: its first tick has posted `projectile_shoot`.
    pub has_been_shot: bool,
    /// `baseDamage`, 2 unless set.
    pub base_damage: Double,
    /// `ID_FLAGS` bit 1: a critical arrow.
    pub crit: bool,
    /// `PIERCE_LEVEL`.
    pub pierce_level: u8,
    /// `piercingIgnoreEntityIds`: the entities it has pierced, at most `pierce_level + 1`,
    /// cleared when it hits a block. In the order pierced.
    pub pierced: Vec<EntityId>,
    /// `soundEvent`: what it plays when it hits an entity, by name (`arrow_hit` unless
    /// set, the crossbow's `crossbow_hit`).
    pub hit_sound: String,
    /// `inGround`: where it is stuck, if it is.
    pub in_ground: Option<InGround>,
    /// The `lived` at which its shake ends (`shakeTime`, 7 at a block hit).
    pub shake_until: u32,
    /// What its kind keeps beside.
    pub of_kind: ArrowKind,
}

/// What a kind of arrow keeps beside every arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArrowKind {
    /// `Arrow`, plain or tipped: its potion is the pickup stack's.
    Arrow,
    /// `SpectralArrow`: the glowing it gives (`duration`, 200).
    Spectral { glowing_duration: i32 },
    /// `ThrownTrident`: whether it has dealt its damage, and the ticks it has been
    /// returning to its owner for (loyalty).
    Trident { dealt_damage: bool, returning: i32 },
}

/// The llama's spit (`LlamaSpit`, M9 3.8): `Projectile`'s state alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spit {
    pub owner: Option<EntityId>,
    pub left_owner: bool,
    pub has_been_shot: bool,
}

/// A firework rocket (`FireworkRocketEntity`): the stack whose `fireworks` it flies and
/// explodes by, its life and lifetime, and whether it was shot at an angle (a crossbow's
/// or a dispenser's, which flies straight). It is never attached to an entity: nobody
/// glides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Firework {
    pub stack: ItemStack,
    pub owner: Option<EntityId>,
    pub left_owner: bool,
    pub has_been_shot: bool,
    /// `life`: the ticks it has flown.
    pub life: i32,
    /// `lifetime`: the life at which it explodes.
    pub lifetime: i32,
    /// `DATA_SHOT_AT_ANGLE`.
    pub shot_at_angle: bool,
}

/// An area effect cloud (`AreaEffectCloud`, D9 2.5). Its age is `lived`. Its radius
/// shrinks by `radius_per_tick` every tick and by `radius_on_use` at every application;
/// the record holds it as of `radius_at`, the `lived` of the last application or of
/// its making, so that a cloud commits only when it applies or ends (finding 53).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cloud {
    pub duration: i32,
    pub wait_time: i32,
    pub reapplication_delay: i32,
    pub radius: Float,
    pub radius_at: u32,
    pub radius_on_use: Float,
    pub radius_per_tick: Float,
    pub duration_on_use: i32,
    pub potion: PotionContents,
    pub duration_scale: Float,
    pub owner: Option<EntityId>,
    /// `victims`: each entity affected and the `lived` from which it may be affected
    /// again, ascending by key.
    pub victims: Vec<(EntityId, u32)>,
}

/// The evoker's fangs (`EvokerFangs`, D9 1.1): who cast them, the warmup they were made
/// with (counted down from `lived` 0: the game's `warmupDelayTicks`); their yaw is the entity's own.
/// Their life, 22 ticks after the warmup ran out, is counted from `lived` too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fangs {
    pub owner: Option<EntityId>,
    pub warmup: i32,
}
