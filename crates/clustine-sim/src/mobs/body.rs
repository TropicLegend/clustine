//! A mob's body: the parts every mob keeps, and the hooks of its tick that are the body's
//! (`LivingEntity` and `Mob` without the mind; the brief's section 1).
//!
//! Counters the game counts by one every tick are kept as the moment they count from
//! (`docs/groundwork/n7-plan.md`, section 11.2), so that a mob at rest commits nothing
//! for them. A moment is a `lived` as the counters see it: the game counts its hurt time,
//! its cooldown and a player's memory down in `baseTick`, after what `baseTick` meets
//! (fire, the void, a wall, the water) and before `aiStep`; what happens before that
//! count (in `baseTick`, or between ticks, as a player's hit) comes at the tick's
//! `lived`, and what happens after it (in `aiStep`) at the next ([`moment`]). A counter
//! set to `n` at a moment `m` is `n - (now - m)` at the moment `now`.
//!
//! The submodules hold its hurts and death ([`hurt`]), its attributes ([`attributes`]),
//! its loot ([`loot`]), its effects ([`effects`]) and the defaults its class of the
//! game's ladder gives ([`ladder`]).
//!
//! Named differences: the pushes of minecarts and boats by a mob, and of the entities in
//! another region's chunk, are left out (they still count for cramming); the oxygen
//! bonus draws nothing, as no body has one; the level's draws of a hurt between ticks
//! come from the mob's stream aside, and a lava hurt's burn sound draws after the other
//! effects of blocks.

pub(crate) mod attributes;
pub(crate) mod burning;
#[cfg(test)]
mod burning_tests;
pub(crate) mod effects;
#[cfg(test)]
mod effects_tests;
pub(crate) mod hurt;
pub(crate) mod ladder;
pub(crate) mod loot;

use clustine_blocks::data::{entity_in_tag, item_in_tag};
use clustine_data::{attributes as ids, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::part::{self, Part, PartError, body_ids};
use super::tick::{Local, MobTick};
use super::view::Seen;
use crate::living::{Damage, Kind, slots};
use crate::physics::{self, Vec3};
use clustine_world::Entry;
use hurt::{DamageSource, DamageType, Struck};

/// Health, hurt and death, the cooldown of hurts, air and freezing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Vitals {
    pub(crate) health: f32,
    /// The moment its hurt time counts from (`hurtTime`, 10 then).
    pub(crate) hurt_from: Option<u32>,
    /// `hurtDir`, 0 for a mob (a player's own).
    pub(crate) hurt_direction: f32,
    /// The moment it died: its `deathTime` is 1 at the first `tickDeath` at or after it.
    pub(crate) died_from: Option<u32>,
    /// The moment its cooldown of hurts counts from (`damageCooldownTime`, 20 then; a
    /// hit while it is above 10 does only what it adds to the last).
    pub(crate) cooldown_from: Option<u32>,
    /// `lastHurt`: the damage of the hit the cooldown counts from.
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

impl Vitals {
    /// `damageCooldownTime` at the moment `now`.
    pub(crate) fn cooldown(&self, now: u32) -> i32 {
        counter(20, self.cooldown_from, now)
    }

    /// `isDeadOrDying()`.
    pub(crate) fn dying(&self) -> bool {
        self.health <= 0.0 || self.died_from.is_some()
    }
}

/// A counter set to `start` at the moment `from`, at the moment `now`: down by one a
/// moment, not below 0.
fn counter(start: i32, from: Option<u32>, now: u32) -> i32 {
    from.map_or(0, |from| {
        let gone = i64::from(now) - i64::from(from);
        (i64::from(start) - gone).clamp(0, i64::from(start)) as i32
    })
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

/// The id of the attribute `attribute` of `clustine_data::attributes`, as instances
/// keep it.
pub(crate) fn attribute(attribute: i32) -> u16 {
    u16::try_from(attribute).expect("an attribute's id fits 16 bits")
}

/// What it wears and holds, by `EquipmentSlot` ordinal, and the chance each drops.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Equipment {
    pub(crate) slots: [Option<ItemStack>; 8],
    /// Each slot's chance, where it is not `DropChances.DEFAULT`'s.
    pub(crate) drop_chances: Option<[f32; 8]>,
}

impl Part for Equipment {
    const ID: u16 = body_ids::EQUIPMENT;
    const VERSION: u8 = 1;
}

impl Equipment {
    /// `DropChances.byEquipment(slot)`: `0.085f` by default.
    pub(crate) fn drop_chance(&self, slot: usize) -> f32 {
        self.drop_chances.map_or(0.085, |chances| chances[slot])
    }
}

/// The head's and the body's yaw (`yHeadRot`, `yBodyRot`), the `lived` from which its
/// head has been still (`BodyRotationControl.headStableTime`), and the head's yaw it was
/// last still at (`lastStableYHeadRot`), which is set every tick the mob moves, as the
/// two yaws are: kept with them, so that a moving mob writes one part for its turning
/// (version 1 had it among the controls, `ai::Controls`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Head {
    pub(crate) head_yaw: f32,
    pub(crate) body_yaw: f32,
    pub(crate) still_since: u32,
    pub(crate) last_stable_yaw: f32,
}

impl Part for Head {
    const ID: u16 = body_ids::HEAD;
    const VERSION: u8 = 2;
}

/// [`Head`] as version 1 wrote it.
#[derive(Deserialize)]
struct HeadV1 {
    head_yaw: f32,
    body_yaw: f32,
    still_since: u32,
}

/// The movement inputs its controls set and `travel` reads: `xxa`, `yya`, `zza`,
/// `jumping` and `speed`; and the `lived` of its last jump from the ground, which its
/// `noJumpDelay` of 10 counts from while it keeps jumping.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Inputs {
    pub(crate) xxa: f32,
    pub(crate) yya: f32,
    pub(crate) zza: f32,
    pub(crate) jumping: bool,
    pub(crate) speed: f32,
    pub(crate) jumped_at: Option<u32>,
}

impl Part for Inputs {
    const ID: u16 = body_ids::INPUTS;
    const VERSION: u8 = 1;
}

impl Inputs {
    /// `noJumpDelay` after `aiStep` counted it down in the tick that began at `lived`.
    pub(crate) fn no_jump_delay(&self, lived: u32) -> i32 {
        self.jumped_at.map_or(0, |at| {
            (10 - (i64::from(lived) - i64::from(at))).clamp(0, 10) as i32
        })
    }
}

/// Its target, whether it is aggressive, and who last hurt it, each with the moment it
/// was set at.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Combat {
    pub(crate) target: Option<EntityId>,
    pub(crate) aggressive: bool,
    /// `lastHurtByMob` and the moment of its hurt (`lastHurtByMobTimestamp`).
    pub(crate) last_hurt_by: Option<(EntityId, u32)>,
    /// `lastHurtByPlayer`, by the player's UUID, and the moment its memory of 100
    /// counts from: a player's kill counts while it lasts.
    pub(crate) last_hurt_by_player: Option<(u128, u32)>,
}

impl Part for Combat {
    const ID: u16 = body_ids::COMBAT;
    const VERSION: u8 = 1;
}

impl Combat {
    /// The player who hurt it, while `lastHurtByPlayerMemoryTime` is above 0 at the
    /// moment `now`.
    pub(crate) fn player_memory(&self, now: u32) -> Option<u128> {
        self.last_hurt_by_player
            .filter(|&(_, from)| counter(100, Some(from), now) > 0)
            .map(|(player, _)| player)
    }
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

/// Where it is kept to (`Mob.homePosition` and `homeRadius`, a leash's or a type's).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Home {
    pub(crate) centre: Option<BlockPos>,
    pub(crate) radius: i32,
}

impl Part for Home {
    const ID: u16 = body_ids::HOME;
    const VERSION: u8 = 1;
}

/// The stamps of what counts every tick: `ambientSoundTime` is `lived - ambient_since`
/// as its tick compares it; `noActionTime` is `lived - no_action_since` as
/// `checkDespawn` reads it, and `None` while a player is within 32 blocks, which writes
/// it only as the nearest player comes and goes (the plan's 7.2.6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Idle {
    pub(crate) ambient_since: u32,
    pub(crate) no_action_since: Option<u32>,
}

impl Part for Idle {
    const ID: u16 = body_ids::IDLE;
    const VERSION: u8 = 1;
}

/// What `noActionTime` has gained beyond the ticks since it was last reset: a monster's
/// two more in the light (`Monster.updateNoActionTime`, `burning.rs`). A part of its own,
/// so that a record written before it reads as none gained.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct IdleGained {
    pub(crate) gained: u32,
}

impl Part for IdleGained {
    const ID: u16 = body_ids::IDLE_GAINED;
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
    pub(crate) timeout: super::ai::Timeout,
    pub(crate) combat: Combat,
    pub(crate) flags: Flags,
    pub(crate) home: Home,
    pub(crate) idle: Idle,
    pub(crate) idle_gained: IdleGained,
    pub(crate) last_damage: LastDamage,
    pub(crate) effects: effects::Effects,
}

/// `lastDamageSource` and `lastDamageStamp`: what last hurt it, by its damage type's
/// registry id and its cause where that is living (all a brain's `HurtBySensor` reads of
/// the source), and the game time of the hurt. The game keeps the field for good and
/// gives it for 40 ticks after (`getLastDamageSource()`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LastDamage {
    pub(crate) last: Option<(i32, Option<EntityId>, i64)>,
}

impl Part for LastDamage {
    const ID: u16 = body_ids::LAST_DAMAGE;
    const VERSION: u8 = 1;
}

impl LastDamage {
    /// `getLastDamageSource()` at the game time `now`: the type and the living cause,
    /// within 40 ticks of the hurt.
    pub(crate) fn source(&self, now: i64) -> Option<(i32, Option<EntityId>)> {
        let (kind, cause, stamp) = self.last?;
        (now - stamp <= 40).then_some((kind, cause))
    }
}

impl Body {
    pub(crate) fn decode(entries: &[Entry]) -> Result<Self, PartError> {
        // Version 1 of the controls held the head's last stable yaw, and version 1 of the
        // progress the timeout's timer and check: they are taken into their parts now.
        let (controls, last_stable_yaw) = match part::decode_or_older::<
            super::ai::Controls,
            super::ai::ControlsV1,
        >(entries, 1)?
        {
            Ok(controls) => (controls, None),
            Err(old) => old.split(),
        };
        let (progress, old_timeout) = match part::decode_or_older::<
            super::ai::Progress,
            super::ai::ProgressV1,
        >(entries, 1)?
        {
            Ok(progress) => (progress, None),
            Err(old) => old.split(),
        };
        let mut head = match part::decode_or_older::<Head, HeadV1>(entries, 1)? {
            Ok(head) => head,
            Err(old) => Head {
                head_yaw: old.head_yaw,
                body_yaw: old.body_yaw,
                still_since: old.still_since,
                last_stable_yaw: 0.0,
            },
        };
        if let Some(yaw) = last_stable_yaw {
            head.last_stable_yaw = yaw;
        }
        let timeout = match old_timeout {
            Some(timeout) => timeout,
            None => part::decode(entries)?,
        };
        Ok(Self {
            vitals: part::decode(entries)?,
            attributes: part::decode(entries)?,
            equipment: part::decode(entries)?,
            head,
            inputs: part::decode(entries)?,
            controls,
            path: part::decode(entries)?,
            progress,
            timeout,
            combat: part::decode(entries)?,
            flags: part::decode(entries)?,
            home: part::decode(entries)?,
            idle: part::decode(entries)?,
            idle_gained: part::decode(entries)?,
            last_damage: part::decode(entries)?,
            effects: part::decode(entries)?,
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
        part::put(&mut entries, &self.timeout);
        part::put(&mut entries, &self.last_damage);
        part::put(&mut entries, &self.idle_gained);
        part::put(&mut entries, &self.effects);
        entries
    }

    /// `noActionTime` as `serverAiStep` leaves it in the tick that began at `lived`
    /// (counted up by one at its head; what `RandomStrollGoal` reads).
    pub(crate) fn no_action_time(&self, lived: u32) -> i64 {
        self.idle.no_action_since.map_or(1, |since| {
            i64::from(lived) - i64::from(since) + 1 + i64::from(self.idle_gained.gained)
        })
    }
}

/// What clients are shown of a mob's body, in neutral form, in the order made.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    /// `playSound` at the mob, from its sound source.
    Sound { sound: i32, volume: f32, pitch: f32 },
    /// `broadcastEntityEvent(mob, event)`: an entity event, by its byte.
    Entity(u8),
    /// `broadcastDamageEvent(mob, source)`: the damage type, what caused and what dealt
    /// it, and the position the source names.
    Damaged {
        kind: DamageType,
        cause: Option<EntityId>,
        direct: Option<EntityId>,
        position: Option<Vec3>,
    },
    /// A game event at the mob, which sculk hears.
    GameEvent(hurt::GameEvent),
}

/// What a mob drops as it dies, drawn as it died: the stacks, each laid at its feet
/// (`spawnAtLocation`), and the orbs of its experience, each with the level's draw of
/// `tryMergeToExisting` (`nextInt(40)`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Death {
    pub(crate) position: Vec3,
    pub(crate) drops: Vec<ItemStack>,
    pub(crate) orbs: Vec<(i32, u32)>,
}

/// How a mob left the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removed {
    /// `discard()`: despawned, dropping nothing.
    Discarded,
    /// `remove(KILLED)` at the end of its death.
    Killed,
}

/// The moment of what the tick does now ([`self`]'s documentation).
pub(crate) fn moment(tick: &MobTick<'_>) -> u32 {
    tick.lived.saturating_add(u32::from(tick.counted))
}

/// The mob of `tick` as a hurt finds it now.
fn struck<'a>(tick: &'a mut MobTick<'_>) -> Struck<'a> {
    let moment = moment(tick);
    let in_liquid = tick.living.body.in_water || tick.living.in_lava();
    Struck {
        kind: tick.kind,
        mob: &mut tick.mob,
        mob_type: tick.mob_type,
        position: tick.living.body.position,
        motion: &mut tick.living.body.motion,
        on_ground: tick.living.body.on_ground,
        on_fire: tick.living.fire_ticks > 0,
        vehicle: tick.vehicle,
        carries: tick.carries,
        leashed: tick.leashed,
        in_liquid,
        moment,
        game_time: tick.world.game_time(),
        draws: &mut *tick.draws,
        out: &mut tick.out,
    }
}

/// `Mob.checkDespawn()` (the brief's section 1.12), which the level runs before the
/// mob's tick: a mob kept by nothing goes at once beyond its category's despawn
/// distance from the nearest player, and by one chance in 800 a tick beyond 32 blocks
/// once idle for 600 ticks (drawn only then, an animal's too); within 32 its idle time
/// starts again. Where there is no player it only idles. Read here from the players the
/// mob's view gives (the region's own; others' with N7a2's faces).
pub(crate) fn check_despawn(tick: &mut MobTick<'_>) {
    let keep = tick.mob.body.flags.persistent
        || tick.vehicle.is_some()
        || tick.leashed
        || tick.mob_type.requires_custom_persistence(&tick.mob);
    let at = tick.living.body.position;
    let nearest = tick
        .world
        .players()
        .into_iter()
        .filter(|player| !player.spectator)
        .map(|player| {
            let d = [
                player.position[0] - at[0],
                player.position[1] - at[1],
                player.position[2] - at[2],
            ];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
        })
        .fold(None, |nearest: Option<f64>, d| match nearest {
            Some(least) if least <= d => Some(least),
            _ => Some(d),
        });
    let Some(distance) = nearest else {
        idles(tick);
        return;
    };
    let idle = tick.mob.body.idle.no_action_since.map_or(0, |since| {
        i64::from(tick.lived) - i64::from(since) + i64::from(tick.mob.body.idle_gained.gained)
    });
    let far = ladder::despawn_distance(tick.kind);
    let far = f64::from(far * far);
    let near = 32.0 * 32.0;
    let remove = |tick: &MobTick<'_>| {
        tick.mob_type
            .remove_when_far_away(tick.kind, &tick.mob, tick.tick_count, distance)
    };
    if !keep && distance > far && remove(tick) {
        tick.out.removed = Some(Removed::Discarded);
    }
    if !keep && idle > 600 && tick.draws.next_int(800) == 0 && distance > near && remove(tick) {
        tick.out.removed = Some(Removed::Discarded);
    } else if distance < near {
        tick.mob.body.idle.no_action_since = None;
        tick.mob.body.idle_gained.gained = 0;
        return;
    }
    idles(tick);
}

/// Its idle time counts on from here (a mob without AI never counts it).
fn idles(tick: &mut MobTick<'_>) {
    let idle = &mut tick.mob.body.idle;
    if idle.no_action_since.is_none() && !tick.mob.body.flags.no_ai {
        // Reset a tick ago, and counted once since.
        idle.no_action_since = Some(tick.lived.saturating_sub(1));
    }
}

/// `Mob.baseTick` up to the counters: what the body reads of its attributes and its
/// type for the tick, then `Entity.baseTick` and `LivingEntity.baseTick`'s wall and air
/// ([`Living::base_tick`]), whose hurts the tick takes next.
pub(crate) fn base_tick(tick: &mut MobTick<'_>) {
    prepare(tick);
    tick.living.base_tick(tick.world);
    if std::mem::take(&mut tick.living.drowned) {
        // `broadcastEntityEvent(this, 67)`, the bubbles, before the drowning hit.
        tick.out.events.push(Event::Entity(67));
    }
}

/// What a living body of the type `kind` reads of its attributes `values`: the physics'
/// values, and its step height. Its tick's, and a ridden mob's moves' and landing's.
pub(crate) fn living_attributes(
    kind: u16,
    values: &Attributes,
) -> (crate::living::Attributes, f32) {
    let value = |id: i32| values.value(kind, attribute(id));
    let living = crate::living::Attributes {
        gravity: value(ids::GRAVITY),
        safe_fall_distance: value(ids::SAFE_FALL_DISTANCE),
        fall_damage_multiplier: value(ids::FALL_DAMAGE_MULTIPLIER),
        friction_modifier: value(ids::FRICTION_MODIFIER),
        air_drag_modifier: value(ids::AIR_DRAG_MODIFIER),
        water_movement_efficiency: value(ids::WATER_MOVEMENT_EFFICIENCY),
        bounciness: value(ids::BOUNCINESS),
        scale: value(ids::SCALE) as f32,
    };
    (living, value(ids::STEP_HEIGHT) as f32)
}

/// Whose `causeFallDamage` a fall of the mob `mob` of the type `kind` takes: a llama's,
/// the rest of the horse family's, or any living body's.
pub(crate) fn landing(kind: u16, mob: &crate::mobs::Mob) -> crate::living::Landing {
    use crate::mobs::types::{animal, equine};
    if let Some(own) = crate::mobs::types::of(kind).landing(mob) {
        return own;
    }
    if equine::is_llama(kind) {
        crate::living::Landing::Llama
    } else if equine::is_equine(kind) {
        crate::living::Landing::Horse {
            baby: animal::is_baby(mob),
        }
    } else if i32::from(kind) == clustine_data::entity_types::FROG {
        crate::living::Landing::Lowered { by: 5 }
    } else {
        crate::living::Landing::Living
    }
}

/// What the body's tick reads of its attributes and its type: the physics' values, its
/// step, and which hurts its type is spared.
fn prepare(tick: &mut MobTick<'_>) {
    let kind = tick.kind;
    let (mut attributes, step_height) = living_attributes(kind, &tick.mob.body.attributes);
    // `getEffectiveGravity`: none for a body without gravity.
    if tick.mob_type.no_gravity(&tick.mob) {
        attributes.gravity = 0.0;
    }
    tick.living.attributes = attributes;
    tick.living.step_height = step_height;
    tick.living.omnidirectional_air = tick.mob_type.omnidirectional_air();
    tick.living.water_slow_down = tick.mob_type.water_slow_down();
    tick.living.yaw = tick.yaw;
    let id = i32::from(kind);
    tick.living.fall_damage_immune = entity_in_tag(id, "minecraft:fall_damage_immune");
    tick.living.landing = landing(kind, &tick.mob);
    tick.living.breathes_under_water = entity_in_tag(id, "minecraft:can_breathe_under_water")
        || tick.mob_type.breathes_under_water(&tick.mob);
    tick.living.water_breathing = tick.mob.body.effects.water_breathing();
    tick.living.effects_refill_air = tick.mob.body.effects.refill_air();
    // `canFreeze`: not of the types that do not freeze, and wearing nothing that keeps
    // the cold off.
    let worn = &tick.mob.body.equipment.slots;
    let armour = [slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD];
    tick.living.can_freeze = tick.mob_type.can_freeze()
        && !entity_in_tag(id, "minecraft:freeze_immune_entity_types")
        && !armour
            .into_iter()
            .filter_map(|slot| worn[usize::from(slot)])
            .any(|stack| item_in_tag(stack.item, "minecraft:freeze_immune_wearables"));
    tick.living.walks_on_powder_snow = entity_in_tag(id, "minecraft:powder_snow_walkable_mobs")
        || worn[usize::from(slots::FEET)].is_some_and(|stack| stack.item == items::LEATHER_BOOTS);
    tick.living.pushed_by_fluid = tick.mob_type.pushed_by_fluid();
    tick.living.climbs = tick.mob_type.on_climbable(&tick.mob);
    tick.living.stuck_in_webs = tick.mob_type.stuck_in_webs();
    tick.living.max_air = tick.mob_type.max_air();
    tick.living.refills_at_once = tick.mob_type.refills_air_at_once();
    // A dying mob is not `isAlive()`: no wall, no air, no effects of blocks.
    if tick.mob.body.vitals.dying() {
        tick.living.end = crate::living::End::Killed;
    }
}

/// The rest of `LivingEntity.baseTick` and `Mob.baseTick`, after its hurts: the counters
/// count down (from here what happens comes at the next moment), a dying mob's death
/// goes on (`tickDeath`), the memories of who hurt it lapse, and a living mob draws its
/// ambient sound's chance (`nextInt(1000)`, every tick: the brief's finding 1).
pub(crate) fn after_base_tick(tick: &mut MobTick<'_>) {
    tick.counted = true;
    tick_death(tick);
    if tick.out.removed.is_some() {
        return;
    }
    let lived = tick.lived;
    let combat = &mut tick.mob.body.combat;
    // `lastHurtByPlayerMemoryTime` at 0 lets the player go.
    if combat
        .last_hurt_by_player
        .is_some_and(|(_, from)| i64::from(lived) - i64::from(from) >= 100)
    {
        combat.last_hurt_by_player = None;
    }
    if combat
        .last_hurt_by
        .is_some_and(|(_, at)| i64::from(lived) - i64::from(at) > 100)
    {
        combat.last_hurt_by = None;
    }
    // `tickEffects`, a dying mob's too.
    effects::tick_effects(tick);
    if tick.mob.body.vitals.dying() {
        return;
    }
    // `random.nextInt(1000) < ambientSoundTime++`.
    let time = i64::from(lived) - i64::from(tick.mob.body.idle.ambient_since);
    if i64::from(tick.draws.next_int(1000)) < time {
        tick.mob.body.idle.ambient_since =
            lived + 1 + tick.mob_type.ambient_interval_while(tick.carries);
        let mob_type = tick.mob_type;
        mob_type.play_ambient_sound(tick);
    }
}

/// `Mob.playAmbientSound()`: the type's sound as its tick asks it, at its volume and
/// voice (what a type plays unless it plays its own, a fox's screech).
pub(crate) fn play_ambient_sound(tick: &mut MobTick<'_>) {
    let lived = tick.lived;
    let body = &tick.living.body;
    let (in_water, on_ground) = (body.in_water, body.on_ground);
    let now = tick.world.game_time();
    let place = Some((in_water, on_ground));
    let sound = tick
        .mob_type
        .ambient_sound_drawn(&tick.mob, tick.draws, now, lived, place);
    struck(tick).make_sound(sound);
}

/// `Mob.makeSound(sound)` at `volume`: its voice's pitch drawn of its own random.
pub(crate) fn make_sound(tick: &mut MobTick<'_>, sound: Option<i32>, volume: f32) {
    let Some(sound) = sound else {
        return;
    };
    let pitch = hurt::voice_pitch(tick.draws, tick.mob_type.is_baby(&tick.mob))
        * tick.mob_type.voice_pitch_factor();
    tick.out.events.push(Event::Sound {
        sound,
        volume,
        pitch,
    });
}

/// `tickDeath()`: a dying mob's death goes on; at its twentieth tick clients see it
/// vanish in smoke (entity event 60) and it is removed.
fn tick_death(tick: &mut MobTick<'_>) {
    let Some(died) = tick.mob.body.vitals.died_from else {
        return;
    };
    let death_time = i64::from(tick.lived) + 1 - i64::from(died);
    if death_time >= 20 {
        tick.out.events.push(Event::Entity(60));
        tick.out.removed = Some(Removed::Killed);
    }
}

/// `isImmobile()`: a dying mob's, unless its type says otherwise (a horse's).
pub(crate) fn immobile(tick: &MobTick<'_>) -> bool {
    tick.mob_type.immobile(tick)
}

/// The start of `aiStep`: the small motion zeroed, and the inputs' decay (`xxa *=
/// 0.98`, `zza *= 0.98`); an immobile mob's inputs stop (`isImmobile`). Its `noJumpDelay`
/// counts itself down ([`Inputs::no_jump_delay`]).
pub(crate) fn start_ai_step(tick: &mut MobTick<'_>) {
    tick.living.zero_small_motion();
    let still = immobile(tick);
    let inputs = &mut tick.mob.body.inputs;
    inputs.xxa *= 0.98;
    inputs.zza *= 0.98;
    if still {
        inputs.jumping = false;
        inputs.xxa = 0.0;
        inputs.zza = 0.0;
    }
}

/// `aiStep`'s jump, where it is jumping: up through water and lava, which it is in
/// deeper than its threshold or off the ground in; else from the ground (or the
/// shallows) once its delay of 10 is over. Not jumping, the delay is over at once.
pub(crate) fn jump(tick: &mut MobTick<'_>) {
    if !tick.mob.body.inputs.jumping {
        tick.mob.body.inputs.jumped_at = None;
        return;
    }
    let living = &tick.living;
    let in_lava = living.in_lava();
    let height = if in_lava {
        living.fluids.lava_height
    } else {
        living.fluids.water_height
    };
    let in_water = living.body.in_water && height > 0.0;
    let threshold = if living.eye_height < 0.4 { 0.0 } else { 0.4 };
    let on_ground = living.body.on_ground;
    if (in_water && (!on_ground || height > threshold))
        || (in_lava && (!on_ground || living.fluids.lava_height > threshold))
    {
        jump_in_liquid(tick);
    } else if (on_ground || in_water && height <= threshold)
        && tick.mob.body.inputs.no_jump_delay(tick.lived) == 0
    {
        jump_from_ground(tick);
        tick.mob.body.inputs.jumped_at = Some(tick.lived);
    }
}

/// `Mob.jumpInLiquid(fluid)`: up by `0.04f` where its navigation can float, by 0.3
/// where it cannot (N7c's `FloatGoal` makes most able; until then none is).
fn jump_in_liquid(tick: &mut MobTick<'_>) {
    let up = if can_float(tick) {
        0.039_999_999_105_930_33
    } else {
        0.3
    };
    tick.living.body.motion[1] += up;
}

/// `navigation.canFloat()`.
fn can_float(tick: &MobTick<'_>) -> bool {
    super::ai::can_float(tick)
}

/// `jumpFromGround()`: its jump strength times the jump factor of what it is in or
/// stands on (honey's half), as its upward motion where that is more; its move is sent
/// at the next update.
fn jump_from_ground(tick: &mut MobTick<'_>) {
    let strength = tick
        .mob
        .body
        .attributes
        .value(tick.kind, attribute(ids::JUMP_STRENGTH)) as f32;
    let factor = block_jump_factor(tick);
    let power = tick
        .mob_type
        .jump_power(tick, factor)
        .unwrap_or(strength * 1.0 * factor);
    if power > 1.0e-5 {
        let motion = &mut tick.living.body.motion;
        motion[1] = f64::from(power).max(motion[1]);
        tick.out.needs_sync = true;
    }
    // The type's own part of the jump (a rabbit's push on).
    tick.mob_type.jumped_from_ground(tick);
}

/// `getBlockJumpFactor()`: the block it is in's, unless that is 1, then the block's
/// below that moves it.
fn block_jump_factor(tick: &MobTick<'_>) -> f32 {
    let world = tick.world;
    let body = &tick.living.body;
    let factor = |pos: BlockPos| {
        crate::physics::body::physics_of(world.state(pos))
            .map_or(1.0, |physics| physics.jump_factor)
    };
    let own = factor(body.block_position());
    if own == 1.0 {
        factor(body.below_for_movement(world))
    } else {
        own
    }
}

/// `aiStep`'s `travel` with the inputs (a mob without AI does not travel at all), the
/// effects of the blocks walked through, powder snow's freezing and its slowing
/// (`removeFrost`, `tryAddFrost`), and every 40th tick the hurt of being frozen through.
pub(crate) fn travel(tick: &mut MobTick<'_>) {
    let inputs = &tick.mob.body.inputs;
    tick.living.input = [
        f64::from(inputs.xxa),
        f64::from(inputs.yya),
        f64::from(inputs.zza),
    ];
    tick.living.speed = inputs.speed;
    tick.living.yaw = tick.yaw;
    tick.living.travel_kind = tick.mob_type.travel(tick);
    if tick.living.travels {
        // The type's own `travel` first (a sitting camel's), then the living body's.
        tick.mob_type.before_travel(tick);
        tick.living.travel(tick.world);
    }
    tick.living.after_travel(tick.world, &mut *tick.draws);
    sound_steps(tick);
    frost(tick);
    // The game's `tickCount` in the tick, counted at its start.
    let tick_count = tick.tick_count.wrapping_add(1);
    if tick_count.is_multiple_of(40)
        && tick.living.frozen >= crate::living::TICKS_TO_FREEZE
        && tick.living.can_freeze
    {
        tick.living.hurt(Damage::Freeze, 1.0);
    }
}

/// The steps its moves took (`vibrationAndSoundEffectsFromBlock`), in order: each its
/// sound (`playStepSound`: the type's own at 0.15 and 1, or the block's step at 0.15 of
/// the block's volume and at its pitch), and its `STEP` game event with the block
/// stepped on, which wool dampens; a swim its `SWIM`.
fn sound_steps(tick: &mut MobTick<'_>) {
    let Some(steps) = tick.living.body.steps.as_mut() else {
        return;
    };
    for step in std::mem::take(&mut steps.taken) {
        match step {
            physics::body::Step::Block {
                state,
                at,
                sound,
                event,
                ..
            } => {
                if sound && tick.mob_type.plays_step_sounds() {
                    let played = match tick.mob_type.step_sound_on(&tick.mob, state) {
                        Some(own) => Some(own),
                        None => state
                            .sound_type()
                            .map(|kind| (kind.step, kind.volume * 0.15, kind.pitch)),
                    };
                    if let Some((sound, volume, pitch)) = played {
                        tick.out.events.push(Event::Sound {
                            sound,
                            volume,
                            pitch,
                        });
                    }
                }
                if event {
                    tick.out.local.push(Local::GameEventAt {
                        name: "minecraft:step",
                        at,
                        affected: Some(state),
                    });
                }
            }
            physics::body::Step::Swim { at } => tick.out.local.push(Local::GameEventAt {
                name: "minecraft:swim",
                at,
                affected: None,
            }),
        }
    }
}

/// `removeFrost()` and `tryAddFrost()`: powder snow's slowing taken off, and put back as
/// much as it is frozen while it stands on something.
fn frost(tick: &mut MobTick<'_>) {
    let speed = attribute(ids::MOVEMENT_SPEED);
    let attributes = &mut tick.mob.body.attributes;
    if let Some(made) = attributes.made(speed)
        && made.has_modifier(attributes::POWDER_SNOW)
        && let Some(instance) = attributes.instance(tick.kind, speed)
    {
        instance.remove_modifier(attributes::POWDER_SNOW);
    }
    let frozen = tick.living.frozen;
    if frozen <= 0 {
        return;
    }
    let below = tick.living.body.on_pos_legacy(tick.world);
    if tick.world.state(below).props().is_air() {
        return;
    }
    let percent =
        frozen.min(crate::living::TICKS_TO_FREEZE) as f32 / crate::living::TICKS_TO_FREEZE as f32;
    if let Some(instance) = attributes.instance(tick.kind, speed) {
        instance.add_modifier(Modifier {
            id: attributes::POWDER_SNOW.to_owned(),
            amount: f64::from(-0.05_f32 * percent),
            operation: attributes::ADD_VALUE,
        });
    }
}

/// `LivingEntity.pushEntities()` (the brief's section 1.13): what is pushable in its
/// box, and where more than 23 of what rides nothing are, one chance in four of
/// cramming's 6; then each pushes it away and is pushed away (`doPush`), the mob's own
/// share at once and the other's as a write to it.
pub(crate) fn push_entities(tick: &mut MobTick<'_>) {
    // A type that is not pushable pushes nothing either (a bat's `pushEntities`).
    if !tick.mob_type.pushable() {
        return;
    }
    let bounds = tick.living.body.bounds();
    let mut pushable: Vec<Pushable> = tick
        .world
        .entities_in(&bounds)
        .into_iter()
        .filter(|seen| seen.entity.key != tick.key)
        .filter_map(|seen| pushable(&seen, tick))
        .collect();
    for player in tick.world.players() {
        let feet = player.position;
        let box_of = crate::physics::aabb::Aabb::of_entity(feet, 0.6, 1.8);
        if !player.spectator && box_of.intersects(bounds) {
            pushable.push(Pushable {
                key: None,
                position: feet,
                passenger: player.vehicle.is_some(),
            });
        }
    }
    if pushable.is_empty() {
        return;
    }
    let max = 24_usize;
    if pushable.len() > max - 1 && tick.draws.next_int(4) == 0 {
        let free = pushable.iter().filter(|other| !other.passenger).count();
        if free > max - 1 {
            let mut struck = struck(tick);
            hurt::hurt_server(&mut struck, &DamageSource::of(DamageType::CRAMMING), 6.0);
        }
    }
    let mine_pushable = !tick.mob.body.vitals.dying() && !tick.carries && !on_climbable(tick);
    for other in pushable {
        let at = tick.living.body.position;
        let mut dx = at[0] - other.position[0];
        let mut dz = at[2] - other.position[2];
        let mut d = if dx.abs() >= dz.abs() {
            dx.abs()
        } else {
            dz.abs()
        };
        if d < 0.009_999_999_776_482_582 {
            continue;
        }
        d = d.sqrt();
        dx /= d;
        dz /= d;
        let k = (1.0 / d).min(1.0);
        dx *= k;
        dz *= k;
        dx *= 0.050_000_000_745_058_06;
        dz *= 0.050_000_000_745_058_06;
        if let Some(key) = other.key {
            tick.out.local.push(Local::Push {
                key,
                by: [-dx, 0.0, -dz],
            });
        }
        if mine_pushable && dx.is_finite() && dz.is_finite() {
            tick.living.body.motion[0] += dx;
            tick.living.body.motion[2] += dz;
            tick.out.needs_sync = true;
        }
    }
}

/// An entity a mob pushes and is pushed by: the region's own mob, by key, or what only
/// pushes back (a player, a copy's mob, a cart or a boat).
struct Pushable {
    key: Option<EntityId>,
    position: Vec3,
    passenger: bool,
}

/// What of `seen` the mob pushes (`getPushableEntities`): a mob that lives and does not
/// climb, pushed where it is the region's own; a cart or a boat, which counts but is
/// not pushed here (named).
fn pushable(seen: &Seen<'_>, tick: &MobTick<'_>) -> Option<Pushable> {
    let entity = seen.entity;
    let passenger = entity.riding.vehicle.is_some();
    match &entity.data {
        clustine_world::EntityData::Mob { body, state, .. } => {
            if !super::types::of(entity.kind).pushable() {
                return None;
            }
            let vitals: Vitals = part::decode(body).ok()?;
            if vitals.dying() {
                return None;
            }
            // `isPushable()`: not while it climbs, by its type's own answer (a spider's
            // flag) or the block it is in.
            let module = super::types::of(entity.kind);
            let climbs = module
                .on_climbable(&super::Mob {
                    body: Default::default(),
                    mind: Default::default(),
                    state: state.clone(),
                })
                .unwrap_or_else(|| {
                    let block = physics::containing(entity.position);
                    clustine_blocks::data::in_tag(tick.world.state(block), "minecraft:climbable")
                });
            if climbs {
                return None;
            }
            // `!e.isVehicle() && e.isPushable()` for its share.
            let pushed = !seen.copied && entity.riding.passengers.is_empty();
            Some(Pushable {
                key: pushed.then_some(entity.key),
                position: entity.position,
                passenger,
            })
        }
        _ if crate::physics::entities::is_pushable(entity.kind) => Some(Pushable {
            key: None,
            position: entity.position,
            passenger,
        }),
        _ => None,
    }
}

/// `onClimbable()` of the mob.
fn on_climbable(tick: &MobTick<'_>) -> bool {
    tick.living.on_climbable(tick.world)
}

/// `hurtServer(source, amount)` of the mob in its tick by a source of its type's own
/// rules (a bee's sting's death); a dead mob takes no more of its effects.
pub(crate) fn hurt_by(tick: &mut MobTick<'_>, source: &DamageSource, amount: f32) -> bool {
    let took = {
        let mut struck = struck(tick);
        hurt::hurt_server(&mut struck, source, amount)
    };
    if tick.mob.body.vitals.dying()
        && let Kind::Mob { .. } = tick.living.kind
    {
        tick.living.end = crate::living::End::Killed;
    }
    took
}

/// `hurtServer(source, amount)` of what the mob met of itself in its tick, in the order
/// met; lava's burn sound where the hurt took (`lavaHurt`, one draw of its own random).
pub(crate) fn hurt_server(tick: &mut MobTick<'_>, damage: Damage, amount: f32) -> bool {
    let source = DamageSource::of(DamageType::from(damage));
    let took = {
        let mut struck = struck(tick);
        hurt::hurt_server(&mut struck, &source, amount)
    };
    if took && damage == Damage::Lava {
        let pitch = 2.0 + tick.draws.next_float() * 0.4;
        tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_GENERIC_BURN,
            volume: 0.4,
            pitch,
        });
    }
    // Its health as the living body reads it: a dead mob takes no more of its effects.
    if tick.mob.body.vitals.dying()
        && let Kind::Mob { .. } = tick.living.kind
    {
        tick.living.end = crate::living::End::Killed;
    }
    took
}

#[cfg(test)]
mod tests;
