//! A mob's body: the parts every mob keeps, and the hooks of its tick that are the body's
//! (`LivingEntity` and `Mob` without the mind; the brief's section 1). Part N7b of the
//! plan fills the hooks and may add to a part's fields while no world keeps them.
//!
//! Counters the game counts by one every tick are kept as the `lived` at which they
//! were set (`docs/groundwork/n7-plan.md`, section 11.2), so that a mob at rest commits
//! nothing for them.

use clustine_world::{BlockPos, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::part::{self, Part, PartError, body_ids};
use super::tick::MobTick;
use crate::living::Damage;
use clustine_world::Entry;

/// Health, hurt and death, invulnerability, air and freezing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Vitals {
    pub(crate) health: f32,
    /// The `lived` of its last hurt that set its hurt time (`hurtTime` is 10 from it).
    pub(crate) hurt_at: Option<u32>,
    /// `hurtDir`.
    pub(crate) hurt_direction: f32,
    /// The `lived` it died at (`deathTime` counts from it).
    pub(crate) died_at: Option<u32>,
    /// The `lived` until which a hit does only what it adds to the last
    /// (`invulnerableTime`, 20 from a hurt, a hit within it counted only above 10).
    pub(crate) invulnerable_until: u32,
    /// `lastHurt`: the damage of the hit the window counts from.
    pub(crate) last_hurt: f32,
    /// `DATA_AIR_SUPPLY_ID`.
    pub(crate) air: i32,
    /// `DATA_TICKS_FROZEN`.
    pub(crate) frozen: i32,
}

impl Part for Vitals {
    const ID: u16 = body_ids::VITALS;
    const VERSION: u8 = 1;
}

/// An attribute modifier a mob has (`AttributeModifier`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Modifier {
    pub(crate) id: String,
    pub(crate) amount: f64,
    /// `AttributeModifier.Operation`'s ordinal.
    pub(crate) operation: u8,
}

/// An attribute instance a mob has made (`AttributeInstance`): which ones exist is a
/// history, since the game makes them as they are first asked of (the brief's 1.0.17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Instance {
    /// By its id in `clustine_data::ATTRIBUTES`.
    pub(crate) attribute: u16,
    pub(crate) base: f64,
    /// Ascending by id.
    pub(crate) modifiers: Vec<Modifier>,
}

/// The instances a mob has made, ascending by attribute.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Attributes {
    pub(crate) instances: Vec<Instance>,
}

impl Part for Attributes {
    const ID: u16 = body_ids::ATTRIBUTES;
    const VERSION: u8 = 1;
}

/// What it wears and holds, by `EquipmentSlot` ordinal, and the chance each drops.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Equipment {
    pub(crate) slots: [Option<ItemStack>; 8],
    pub(crate) drop_chances: [f32; 8],
}

impl Part for Equipment {
    const ID: u16 = body_ids::EQUIPMENT;
    const VERSION: u8 = 1;
}

/// The head's and the body's yaw (`yHeadRot`, `yBodyRot`), and the `lived` from which
/// its head has been still (`BodyRotationControl.headStableTime`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Head {
    pub(crate) head_yaw: f32,
    pub(crate) body_yaw: f32,
    pub(crate) still_since: u32,
}

impl Part for Head {
    const ID: u16 = body_ids::HEAD;
    const VERSION: u8 = 1;
}

/// The movement inputs its controls set and `travel` reads: `xxa`, `yya`, `zza`,
/// `jumping` and `speed`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Inputs {
    pub(crate) xxa: f32,
    pub(crate) yya: f32,
    pub(crate) zza: f32,
    pub(crate) jumping: bool,
    pub(crate) speed: f32,
}

impl Part for Inputs {
    const ID: u16 = body_ids::INPUTS;
    const VERSION: u8 = 1;
}

/// Its target, whether it is aggressive, and who last hurt it, each with the `lived`
/// it was set at.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Combat {
    pub(crate) target: Option<EntityId>,
    pub(crate) aggressive: bool,
    /// `lastHurtByMob` and its time.
    pub(crate) last_hurt_by: Option<(EntityId, u32)>,
    /// `lastHurtByPlayer`, by the player's UUID, and its time: a player's kill counts
    /// for 100 ticks after their hit.
    pub(crate) last_hurt_by_player: Option<(u128, u32)>,
}

impl Part for Combat {
    const ID: u16 = body_ids::COMBAT;
    const VERSION: u8 = 1;
}

/// `PersistenceRequired`, `NoAI`, `LeftHanded`, `CanPickUpLoot`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Flags {
    pub(crate) persistent: bool,
    pub(crate) no_ai: bool,
    pub(crate) left_handed: bool,
    pub(crate) picks_up_loot: bool,
}

impl Part for Flags {
    const ID: u16 = body_ids::FLAGS;
    const VERSION: u8 = 1;
}

/// Where it is kept to (`PathfinderMob.restrictTo`): a centre and a radius.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Home {
    pub(crate) centre: Option<BlockPos>,
    pub(crate) radius: i32,
}

impl Part for Home {
    const ID: u16 = body_ids::HOME;
    const VERSION: u8 = 1;
}

/// The stamps of what counts every tick: `ambientSoundTime` from `ambient_since`,
/// `noActionTime` from `no_action_since` (0 while a player is within 32 blocks, which
/// writes it only as the nearest player comes and goes: the plan's 7.2.6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Idle {
    pub(crate) ambient_since: u32,
    pub(crate) no_action_since: u32,
}

impl Part for Idle {
    const ID: u16 = body_ids::IDLE;
    const VERSION: u8 = 1;
}

/// A mob's body as its tick has it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Body {
    pub(crate) vitals: Vitals,
    pub(crate) attributes: Attributes,
    pub(crate) equipment: Equipment,
    pub(crate) head: Head,
    pub(crate) inputs: Inputs,
    pub(crate) controls: super::ai::Controls,
    pub(crate) path: super::ai::Path,
    pub(crate) progress: super::ai::Progress,
    pub(crate) combat: Combat,
    pub(crate) flags: Flags,
    pub(crate) home: Home,
    pub(crate) idle: Idle,
}

impl Body {
    pub(crate) fn decode(entries: &[Entry]) -> Result<Self, PartError> {
        Ok(Self {
            vitals: part::decode(entries)?,
            attributes: part::decode(entries)?,
            equipment: part::decode(entries)?,
            head: part::decode(entries)?,
            inputs: part::decode(entries)?,
            controls: part::decode(entries)?,
            path: part::decode(entries)?,
            progress: part::decode(entries)?,
            combat: part::decode(entries)?,
            flags: part::decode(entries)?,
            home: part::decode(entries)?,
            idle: part::decode(entries)?,
        })
    }

    /// The body's entries, ascending by id, those as a fresh body has them left out.
    pub(crate) fn encode(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        part::put(&mut entries, &self.vitals);
        part::put(&mut entries, &self.attributes);
        part::put(&mut entries, &self.equipment);
        part::put(&mut entries, &self.head);
        part::put(&mut entries, &self.inputs);
        part::put(&mut entries, &self.controls);
        part::put(&mut entries, &self.path);
        part::put(&mut entries, &self.progress);
        part::put(&mut entries, &self.combat);
        part::put(&mut entries, &self.flags);
        part::put(&mut entries, &self.home);
        part::put(&mut entries, &self.idle);
        entries
    }
}

/// `Mob.baseTick` and `LivingEntity.baseTick`: the body's own tick before its AI (its
/// fluids, fire, air; the ambient sound's draw). N7b's: for now the reduced body's.
pub(crate) fn base_tick(tick: &mut MobTick<'_>) {
    tick.living.base_tick(tick.world);
}

/// The start of `aiStep`: the jump's delay, the small motion zeroed, and the inputs'
/// decay (`xxa *= 0.98`, `zza *= 0.98`). N7b's.
pub(crate) fn start_ai_step(tick: &mut MobTick<'_>) {
    tick.living.zero_small_motion();
}

/// `aiStep`'s jump, from `jumping`, in fluids and on the ground. N7b's.
pub(crate) fn jump(tick: &mut MobTick<'_>) {
    let _ = tick;
}

/// `aiStep`'s `travel` with the inputs, and what follows it: the effects of the blocks
/// walked through and the freezing. N7b's: for now the reduced body's, without inputs.
pub(crate) fn travel(tick: &mut MobTick<'_>) {
    if tick.living.travels {
        tick.living.travel(tick.world);
    }
    tick.living.after_travel(tick.world, &mut *tick.draws);
}

/// `pushEntities`. N7b's.
pub(crate) fn push_entities(tick: &mut MobTick<'_>) {
    let _ = tick;
}

/// What a dead mob does while its death lasts (`tickDeath`: 20 ticks, then its
/// removal with entity event 60). N7b's.
pub(crate) fn tick_death(tick: &mut MobTick<'_>) {
    let _ = tick;
}

/// `hurtServer(source, amount)` for what the mob meets of itself in its tick (fire,
/// lava, the void), as the reduced body notes it: whether it took the hurt. N7b's: for
/// now it takes none.
pub(crate) fn hurt_server(tick: &mut MobTick<'_>, damage: Damage, amount: f32) -> bool {
    let _ = (tick, damage, amount);
    false
}
