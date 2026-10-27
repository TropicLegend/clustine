//! What hurts a mob, and its death (`LivingEntity.hurtServer`, `actuallyHurt`,
//! `CombatRules`, `dealDefaultKnockback`, `knockback`, `die` and `dropAllDeathLoot`; the
//! brief's sections 1.4 to 1.7).
//!
//! A hurt is taken in the mob's tick, of what it meets of itself, or between ticks, of a
//! player's hit; either way on a [`Struck`], which holds the mob as the hurt finds it and
//! the moment it comes at ([`super::moment`]), so that the counters the hurt sets
//! count down from the right tick wherever it came.
//!
//! What the mob drops as it dies is drawn here, from its own stream, in the game's
//! order; laying it down (`spawnAtLocation`, new orbs) is the region's, which reads it
//! from [`super::Death`].

use clustine_blocks::data::entity_in_tag;
use clustine_data::{EntityTypeInfo, TAGS, attributes as ids, synced_registry};
use clustine_world::{EntityId, ItemStack};

use super::{Death, Event, ladder, loot};
use crate::living::Damage;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::tick::MobOutput;
use crate::mobs::types::MobType;
use crate::physics::Vec3;
use crate::region::Random;

/// A damage type, by its name in the game's registry of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DamageType(pub(crate) &'static str);

impl DamageType {
    /// The damage type of the name `name`, where the game has one (its synced
    /// `minecraft:damage_type` registry).
    pub(crate) fn named(name: &str) -> Option<Self> {
        clustine_data::synced_registry("minecraft:damage_type")?
            .entries
            .iter()
            .find(|entry| **entry == name)
            .map(|entry| Self(entry))
    }

    pub(crate) const PLAYER_ATTACK: Self = Self("minecraft:player_attack");
    pub(crate) const IN_FIRE: Self = Self("minecraft:in_fire");
    pub(crate) const CAMPFIRE: Self = Self("minecraft:campfire");
    pub(crate) const ON_FIRE: Self = Self("minecraft:on_fire");
    pub(crate) const LAVA: Self = Self("minecraft:lava");
    pub(crate) const IN_WALL: Self = Self("minecraft:in_wall");
    pub(crate) const CRAMMING: Self = Self("minecraft:cramming");
    pub(crate) const DROWN: Self = Self("minecraft:drown");
    pub(crate) const FALL: Self = Self("minecraft:fall");
    pub(crate) const FREEZE: Self = Self("minecraft:freeze");
    pub(crate) const OUT_OF_WORLD: Self = Self("minecraft:out_of_world");
    pub(crate) const DRY_OUT: Self = Self("minecraft:dry_out");
    pub(crate) const MOB_ATTACK: Self = Self("minecraft:mob_attack");
    pub(crate) const MAGIC: Self = Self("minecraft:magic");
    pub(crate) const INDIRECT_MAGIC: Self = Self("minecraft:indirect_magic");
    pub(crate) const THROWN: Self = Self("minecraft:thrown");
    pub(crate) const THORNS: Self = Self("minecraft:thorns");

    /// Its id in the registry the server sends, which a damage event names it by
    /// (Clustine's sorted order: the brief's finding 18).
    pub(crate) fn id(self) -> Option<i32> {
        synced_registry("minecraft:damage_type")?.id_of(self.0)
    }

    /// The type of the id `id` in that registry.
    pub(crate) fn of_id(id: i32) -> Option<Self> {
        let registry = synced_registry("minecraft:damage_type")?;
        Some(Self(registry.entries.get(usize::try_from(id).ok()?)?))
    }

    /// Whether it is in the damage type tag `tag` (its name without `#`).
    pub(crate) fn is(self, tag: &str) -> bool {
        let Some(id) = self.id() else {
            return false;
        };
        TAGS.iter()
            .find(|tags| tags.registry == "minecraft:damage_type")
            .and_then(|tags| tags.tags.iter().find(|known| known.name == tag))
            .is_some_and(|known| known.entries.contains(&id))
    }
}

impl From<Damage> for DamageType {
    fn from(damage: Damage) -> Self {
        match damage {
            Damage::InFire => Self::IN_FIRE,
            Damage::Campfire => Self::CAMPFIRE,
            Damage::OnFire => Self::ON_FIRE,
            Damage::Lava => Self::LAVA,
            Damage::OutOfWorld => Self::OUT_OF_WORLD,
            Damage::InWall => Self::IN_WALL,
            Damage::Drown => Self::DROWN,
            Damage::Fall => Self::FALL,
            Damage::Freeze => Self::FREEZE,
            Damage::DryOut => Self::DRY_OUT,
        }
    }
}

/// An entity a damage source names: what caused it or what dealt it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Causer {
    pub(crate) key: EntityId,
    /// Its entity type, by id.
    pub(crate) kind: u16,
    /// Its feet.
    pub(crate) position: Vec3,
    /// The player's UUID, where it is a player.
    pub(crate) player: Option<u128>,
    /// Whether it is a `LivingEntity`.
    pub(crate) living: bool,
}

/// A `DamageSource`: its type, what caused it and what dealt it, and the position it
/// names, if any; and where what dealt it is a projectile, the projectile's motion,
/// along which its hurt pushes (`Projectile.calculateHorizontalHurtKnockbackDirection`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DamageSource {
    pub(crate) kind: DamageType,
    pub(crate) cause: Option<Causer>,
    pub(crate) direct: Option<Causer>,
    pub(crate) position: Option<Vec3>,
    pub(crate) projectile_motion: Option<Vec3>,
}

impl DamageSource {
    /// A source of no entity and no position (`damageSources().fall()` and the like).
    pub(crate) fn of(kind: DamageType) -> Self {
        Self {
            kind,
            cause: None,
            direct: None,
            position: None,
            projectile_motion: None,
        }
    }

    /// A source of `kind` that `entity` both caused and dealt (`playerAttack`, a
    /// weapon's own damage type).
    pub(crate) fn by(kind: DamageType, entity: Causer) -> Self {
        Self {
            kind,
            cause: Some(entity.clone()),
            direct: Some(entity),
            position: None,
            projectile_motion: None,
        }
    }

    /// `damageSources().thrown(projectile, owner)`: dealt by the projectile `direct`,
    /// moving by `motion`, and caused by its owner, if it has one.
    pub(crate) fn thrown(direct: Causer, motion: Vec3, owner: Option<Causer>) -> Self {
        Self {
            kind: DamageType::THROWN,
            cause: owner,
            direct: Some(direct),
            position: None,
            projectile_motion: Some(motion),
        }
    }

    /// `getSourcePosition()`: the position it names, else what dealt it's.
    pub(crate) fn source_position(&self) -> Option<Vec3> {
        self.position
            .or_else(|| self.direct.as_ref().map(|direct| direct.position))
    }
}

/// A mob as a hurt finds it, in its tick or between its ticks.
pub(crate) struct Struck<'a> {
    /// Its entity type, by id.
    pub(crate) kind: u16,
    pub(crate) mob: &'a mut Mob,
    pub(crate) mob_type: &'static dyn MobType,
    pub(crate) position: Vec3,
    pub(crate) motion: &'a mut Vec3,
    pub(crate) on_ground: bool,
    /// Whether it burns, which its loot asks.
    pub(crate) on_fire: bool,
    /// The type of what it rides, which its loot asks.
    pub(crate) vehicle: Option<u16>,
    /// Whether anything rides it (`isVehicle`), it is on a lead (`isLeashed`), and it is
    /// in water or lava (`isInLiquid`): what an armadillo asks before it rolls up.
    pub(crate) carries: bool,
    pub(crate) leashed: bool,
    pub(crate) in_liquid: bool,
    /// The moment the hurt comes at ([`super::moment`]).
    pub(crate) moment: u32,
    /// The world's game time (`Level.getGameTime()`), which a camel's pose is stamped
    /// with.
    pub(crate) game_time: i64,
    /// Its own stream, on from what it has drawn (in its tick, or aside from it).
    pub(crate) draws: &'a mut Random,
    pub(crate) out: &'a mut MobOutput,
}

impl Struck<'_> {
    /// `isDeadOrDying()`.
    pub(crate) fn dying(&self) -> bool {
        self.mob.body.vitals.health <= 0.0 || self.mob.body.vitals.died_from.is_some()
    }

    pub(crate) fn attribute(&self, attribute: i32) -> f64 {
        self.mob.body.attributes.value_worn(
            self.kind,
            super::attribute(attribute),
            &self.mob.body.equipment,
        )
    }

    /// `makeSound(sound)`: at its volume and voice's pitch, two draws of its own random
    /// for the pitch, which a silent mob makes too (the brief's finding 21); none for
    /// no sound.
    pub(crate) fn make_sound(&mut self, sound: Option<i32>) {
        let Some(sound) = sound else {
            return;
        };
        let pitch = mob_voice_pitch(self.mob_type, self.mob, self.draws)
            * self.mob_type.voice_pitch_factor();
        self.out.events.push(Event::Sound {
            sound,
            volume: self.mob_type.sound_volume_of(self.mob),
            pitch,
        });
    }
}

/// `getVoicePitch()`: `(nextFloat() − nextFloat()) · 0.2f`, about 1, or 1.5 for a baby.
pub(crate) fn voice_pitch(draws: &mut Random, baby: bool) -> f32 {
    let spread = (draws.next_float() - draws.next_float()) * 0.2;
    spread + if baby { 1.5 } else { 1.0 }
}

/// `getVoicePitch()` of a mob of the type `mob_type`: [`voice_pitch`]'s, about a baby's
/// pitch of its type's own (a baby zombie villager's 2).
pub(crate) fn mob_voice_pitch(mob_type: &dyn MobType, mob: &Mob, draws: &mut Random) -> f32 {
    let spread = (draws.next_float() - draws.next_float()) * 0.2;
    spread
        + if mob_type.is_baby(mob) {
            mob_type.baby_voice_pitch()
        } else {
            1.0
        }
}

/// `isInvulnerableTo(level, source)` of a mob: fire to a type immune to it, a fall to a
/// type of `#fall_damage_immune`. No mob of Clustine is `Invulnerable`, nor has the
/// entity's own invulnerable time a portal gives, nor wears an enchantment that makes it
/// immune.
fn invulnerable_to(struck: &Struck<'_>, source: &DamageSource) -> bool {
    let kind = i32::from(struck.kind);
    (source.kind.is("minecraft:is_fire") && EntityTypeInfo::of(kind).fire_immune)
        || (source.kind.is("minecraft:is_fall")
            && entity_in_tag(kind, "minecraft:fall_damage_immune"))
}

/// `LivingEntity.hurtServer(level, source, amount)` (the brief's section 1.4): whether
/// it took the hurt. Within its cooldown a hurt no harder than the last does nothing,
/// and a harder one only the difference, without its sounds; otherwise the whole, with
/// its damage event, the push away from where the source is and the hurt or death
/// sound; and a mob whose health it took to nothing dies.
pub(crate) fn hurt_server(struck: &mut Struck<'_>, source: &DamageSource, amount: f32) -> bool {
    // The type's own `hurtServer` before it calls its parent's (an axolotl's draws).
    struck.mob_type.before_hurt_server(struck, source, amount);
    // The type's own `hurtServer` first changes the amount (a rolled-up armadillo's).
    let amount = struck.mob_type.hurt_amount(struck.mob, amount);
    let took = living_hurt_server(struck, source, amount);
    if took {
        // The type's own `hurtServer` after `LivingEntity`'s (a horse rearing).
        struck
            .mob_type
            .hurt_taken(struck.mob, struck.draws, struck.moment);
    }
    // What the type's own `hurtServer` does after (a squid's ink).
    struck.mob_type.after_hurt(struck, source, took);
    took
}

/// `LivingEntity.hurtServer`, before what a type adds after it, but for what the type
/// does first once the hurt is not refused for its being invulnerable to it.
fn living_hurt_server(struck: &mut Struck<'_>, source: &DamageSource, amount: f32) -> bool {
    // The type's own refusal before anything (an allay's liked player).
    if struck.mob_type.refuses_hurt(struck.mob, source) {
        return false;
    }
    if invulnerable_to(struck, source) {
        return false;
    }
    // The type's own (a wolf stands up), then `LivingEntity`'s.
    struck.mob_type.before_hurt(struck.mob);
    if struck.dying() {
        return false;
    }
    // Fire resistance: nothing that burns hurts it.
    if source.kind.is("minecraft:is_fire")
        && struck
            .mob
            .body
            .effects
            .has(super::effects::Effect::FIRE_RESISTANCE)
    {
        return false;
    }
    // `noActionTime = 0`, where it counts at all.
    if let Some(since) = &mut struck.mob.body.idle.no_action_since {
        *since = struck.moment;
        struck.mob.body.idle_gained.gained = 0;
    }
    let kind = source.kind;
    let mut amount = if amount < 0.0 { 0.0 } else { amount };
    // No mob holds a shield or blocks otherwise.
    if kind.is("minecraft:is_freezing")
        && entity_in_tag(i32::from(struck.kind), "minecraft:freeze_hurts_extra_types")
    {
        amount *= 5.0;
    }
    if kind.is("minecraft:damages_helmet")
        && struck.mob.body.equipment.slots[usize::from(slots::HEAD)].is_some()
    {
        // `hurtHelmet` wears nothing of a mob's.
        amount *= 0.75;
    }
    if !amount.is_finite() {
        amount = f32::MAX;
    }
    let mut play_sounds = true;
    let cooldown = struck.mob.body.vitals.cooldown(struck.moment);
    if cooldown as f32 > 10.0 && !kind.is("minecraft:bypasses_cooldown") {
        let last = struck.mob.body.vitals.last_hurt;
        if amount <= last {
            return false;
        }
        actually_hurt(struck, source, amount - last);
        struck.mob.body.vitals.last_hurt = amount;
        play_sounds = false;
    } else {
        struck.mob.body.vitals.last_hurt = amount;
        struck.mob.body.vitals.cooldown_from = Some(struck.moment);
        actually_hurt(struck, source, amount);
        struck.mob.body.vitals.hurt_from = Some(struck.moment);
    }
    // `resolveMobResponsibleForDamage` and `resolvePlayerResponsibleForDamage`: a
    // player's hit counts as a player's whatever their game mode.
    if let Some(cause) = &source.cause {
        if cause.living && !kind.is("minecraft:no_anger") {
            struck.mob.body.combat.last_hurt_by = Some((cause.key, struck.moment));
        }
        if let Some(uuid) = cause.player {
            struck.mob.body.combat.last_hurt_by_player = Some((uuid, struck.moment));
        }
    }
    if play_sounds {
        struck.out.events.push(Event::Damaged {
            kind,
            cause: source.cause.as_ref().map(|cause| cause.key),
            direct: source.direct.as_ref().map(|direct| direct.key),
            position: source.position,
        });
        if !kind.is("minecraft:no_impact") {
            // `markHurt`: its motion is sent, to it too.
            struck.out.sync_velocity = true;
        }
        if !kind.is("minecraft:no_knockback") {
            deal_default_knockback(struck, source);
        }
    }
    if struck.mob.body.vitals.health <= 0.0 {
        // No mob of Clustine holds a totem of undying.
        if play_sounds {
            let sound = struck.mob_type.death_sound(struck.mob);
            struck.make_sound(sound);
        }
        die(struck, source);
    } else if play_sounds {
        // `Mob.playHurtSound`: the ambient sound waits its interval again.
        struck.mob.body.idle.ambient_since =
            struck.moment + struck.mob_type.ambient_interval_while(struck.carries);
        let sound = struck.mob_type.hurt_sound(struck.mob, source);
        struck.make_sound(sound);
    }
    // `lastDamageSource` and its stamp, after the death and the sounds; no mob blocks,
    // so every hurt that came this far sets them.
    if let Some(id) = kind.id() {
        let cause = source
            .cause
            .as_ref()
            .filter(|cause| cause.living)
            .map(|cause| cause.key);
        struck.mob.body.last_damage.last = Some((id, cause, struck.game_time));
    }
    true
}

/// `actuallyHurt(level, source, amount)`: the armour's share taken off (a mob's armour
/// takes no wear), then resistance's and the absorption's (`effects.rs`), and what is
/// left off its health, which sculk hears. No mob of Clustine wears an enchantment that
/// protects it.
fn actually_hurt(struck: &mut Struck<'_>, source: &DamageSource, amount: f32) {
    // The type's own first (a camel stands up, an animal's love ends), the living
    // body's, then what the type does after its superclass's (an armadillo rolls up).
    struck.mob_type.before_actually_hurt(struck, source);
    struck.mob_type.actually_hurt(struck.mob, struck.moment);
    living_actually_hurt(struck, source, amount);
    struck.mob_type.after_actually_hurt(struck, source);
}

/// `LivingEntity.actuallyHurt`.
fn living_actually_hurt(struck: &mut Struck<'_>, source: &DamageSource, amount: f32) {
    if invulnerable_to(struck, source) {
        return;
    }
    let mut amount = amount;
    if !source.kind.is("minecraft:bypasses_armor") {
        let armor = struck.attribute(ids::ARMOR).floor() as i32 as f32;
        let toughness = struck.attribute(ids::ARMOR_TOUGHNESS) as f32;
        amount = damage_after_absorb(amount, armor, toughness);
    }
    // Resistance's share, then the type's own (`getDamageAfterMagicAbsorb`), then the
    // absorption's (`docs/groundwork/n9-plan.md`, section 5.2).
    amount = super::effects::magic_absorbed(&struck.mob.body.effects, source, amount);
    amount = struck.mob_type.magic_absorbed(struck.mob, source, amount);
    let most_absorbed = super::effects::max_absorption(struck.mob, struck.kind);
    amount = super::effects::absorbed(&mut struck.mob.body.effects, amount, most_absorbed);
    if amount <= 0.0 {
        return;
    }
    let max = struck.attribute(ids::MAX_HEALTH) as f32;
    let health = struck.mob.body.vitals.health - amount;
    struck.mob.body.vitals.health = clamp(health, 0.0, max);
    super::effects::absorbed_after(&mut struck.mob.body.effects, amount, most_absorbed);
    struck
        .out
        .events
        .push(Event::GameEvent(GameEvent::EntityDamage));
}

/// `Mth.clamp(float, float, float)`.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min { min } else { value.min(max) }
}

/// `CombatRules.getDamageAfterAbsorb(entity, damage, source, armor, toughness)`, with a
/// weapon that has no enchantment to change the armour's worth.
pub(crate) fn damage_after_absorb(damage: f32, armor: f32, toughness: f32) -> f32 {
    let t = 2.0 + toughness / 4.0;
    let effective = clamp(armor - damage / t, armor * 0.2, 20.0);
    let ratio = effective / 25.0;
    damage * (1.0 - ratio)
}

/// `dealDefaultKnockback(source, amount, blocked)`: pushed by `0.4f` along a
/// projectile's motion where a projectile dealt it, else away from where the source is
/// (its position, else what dealt it's).
fn deal_default_knockback(struck: &mut Struck<'_>, source: &DamageSource) {
    let (dx, dz) = match source.projectile_motion {
        Some(motion) => (-motion[0], -motion[2]),
        None => source.source_position().map_or((0.0, 0.0), |at| {
            (at[0] - struck.position[0], at[2] - struck.position[2])
        }),
    };
    knockback(struck, 0.400_000_005_960_464_5, dx, dz);
}

/// `LivingEntity.knockback(strength, x, z)`: its resistance taken off, half its
/// sideways motion kept, pushed away from `(x, z)`, and up to 0.4 from the ground; a
/// direction too short is drawn instead, two `nextDouble` a side.
pub(crate) fn knockback(struck: &mut Struck<'_>, strength: f64, x: f64, z: f64) {
    let strength = strength * (1.0 - struck.attribute(ids::KNOCKBACK_RESISTANCE));
    if strength <= 0.0 {
        return;
    }
    struck.out.needs_sync = true;
    let old = *struck.motion;
    let (mut x, mut z) = (x, z);
    while x * x + z * z < 9.999_999_747_378_752e-6 {
        x = (struck.draws.next_double() - struck.draws.next_double()) * 0.01;
        z = (struck.draws.next_double() - struck.draws.next_double()) * 0.01;
    }
    let push = scale(normalize([x, 0.0, z]), strength);
    *struck.motion = [
        old[0] / 2.0 - push[0],
        if struck.on_ground {
            (old[1] / 2.0 + strength).min(0.4)
        } else {
            old[1]
        },
        old[2] / 2.0 - push[2],
    ];
}

/// `Vec3.normalize()`: of length one, or nothing where it is shorter than `1.0E-5f`.
pub(crate) fn normalize(v: Vec3) -> Vec3 {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < f64::from(1.0e-5_f32) {
        return [0.0; 3];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

fn scale(v: Vec3, by: f64) -> Vec3 {
    [v[0] * by, v[1] * by, v[2] * by]
}

/// The game events a mob's body posts (`ENTITY_DAMAGE`, `ENTITY_DIE`), which sculk
/// sensors hear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GameEvent {
    EntityDamage,
    EntityDie,
    /// `Mob.ate()`'s.
    Eat,
    /// A chicken's egg laid, an armadillo's scute shed.
    EntityPlace,
    /// A camel sitting down or standing up, an armadillo rolling up or out.
    EntityAction,
    /// An armadillo brushed.
    EntityInteract,
}

impl GameEvent {
    /// Its name in the registry.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::EntityDamage => "minecraft:entity_damage",
            Self::EntityDie => "minecraft:entity_die",
            Self::Eat => "minecraft:eat",
            Self::EntityPlace => "minecraft:entity_place",
            Self::EntityAction => "minecraft:entity_action",
            Self::EntityInteract => "minecraft:entity_interact",
        }
    }
}

/// `die(source)`: dead from now; sculk hears it; it drops what it drops; clients are
/// told (entity event 3) and see it lie down (pose `DYING`). Its body stays for twenty
/// ticks (`tickDeath`, [`super::tick_death`]).
pub(crate) fn die(struck: &mut Struck<'_>, source: &DamageSource) {
    if struck.mob.body.vitals.died_from.is_some() || struck.out.removed.is_some() {
        return;
    }
    // A player's kill counts no score, and a named mob logs nothing: Clustine keeps
    // neither scores nor names.
    // The type's own `die` before `LivingEntity`'s (a sniffer idle again).
    struck.mob_type.before_die(struck.mob);
    struck.mob.body.vitals.died_from = Some(struck.moment);
    struck
        .out
        .events
        .push(Event::GameEvent(GameEvent::EntityDie));
    drop_all_death_loot(struck, source);
    struck.out.events.push(Event::Entity(3));
}

/// `dropAllDeathLoot(level, source)`: a mob that is not a baby drops its table's loot
/// and what it wears by their chances; then, where a player hurt it within the memory
/// of it, its experience.
fn drop_all_death_loot(struck: &mut Struck<'_>, source: &DamageSource) {
    let killed_by_player = struck
        .mob
        .body
        .combat
        .player_memory(struck.moment)
        .is_some();
    let baby = struck.mob_type.is_baby(struck.mob);
    // `shouldDropLoot` and `shouldDropExperience`: not a baby's, but a monster's whether
    // it is a baby or not (`Monster`'s own, a baby zombie's).
    let monster = ladder::of(struck.kind) == ladder::Family::Monster;
    let mut drops = Vec::new();
    if !baby || monster {
        let facts = struck.mob_type.loot_facts(struck.mob);
        let this = loot::Subject {
            kind: struck.kind,
            on_fire: struck.on_fire,
            baby,
            vehicle: struck.vehicle,
            facts,
        };
        drops.extend(loot::entity_drops(
            &this,
            source,
            killed_by_player,
            struck.draws,
        ));
        drops.extend(drop_custom_death_loot(struck, killed_by_player));
    }
    // `dropEquipment`: the type's own (a horse's inventory, a donkey's chest), baby or
    // not.
    drops.extend(struck.mob_type.drop_equipment(struck.mob));
    let mut orbs = Vec::new();
    if killed_by_player && (!baby || monster) {
        let reward = struck
            .mob_type
            .experience_reward(struck.kind, struck.mob, struck.draws);
        let mut left = reward;
        while left > 0 {
            let value = experience_value(left);
            left -= value;
            // `tryMergeToExisting`'s draw of the level's random, made at its place in
            // the mob's stream (the plan's 7.2.8).
            let draw = struck.draws.next_int(40);
            orbs.push((value, draw));
        }
    }
    struck.out.died = Some(Death {
        position: struck.position,
        drops,
        orbs,
    });
}

/// `Mob.dropCustomDeathLoot`: each slot in order whose stack drops by its chance, where
/// a player killed it or the slot keeps what it picked up; worn down by two draws where
/// it can wear, which Clustine's stacks do not keep; the slot emptied.
fn drop_custom_death_loot(struck: &mut Struck<'_>, killed_by_player: bool) -> Vec<ItemStack> {
    let mut drops = Vec::new();
    for slot in 0..8 {
        let chance = struck.mob.body.equipment.drop_chance(slot);
        if chance == 0.0 {
            continue;
        }
        let preserved = chance > 1.0;
        let Some(ref stack) = struck.mob.body.equipment.slots[slot] else {
            continue;
        };
        if !(killed_by_player || preserved) || struck.draws.next_float() >= chance {
            continue;
        }
        if !preserved && let Some(max) = clustine_data::ItemComponents::of(stack.item).max_damage {
            let max = i32::from(max);
            let inner = struck.draws.next_int((max - 3).max(1) as u32);
            let _worn_down = max - struck.draws.next_int(1 + inner) as i32;
        }
        drops.push(stack.clone());
        struck.mob.body.equipment.slots[slot] = None;
    }
    drops
}

/// `ExperienceOrb.getExperienceValue(amount)`: the largest orb's worth not above it.
pub(crate) fn experience_value(amount: i32) -> i32 {
    [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3]
        .into_iter()
        .find(|&value| amount >= value)
        .unwrap_or(1)
}

/// The default experience of a mob's death by its class (`getBaseExperienceReward`):
/// an animal's and a water animal's 1 to 3, one draw; any other's its `xpReward` (0
/// but for monsters', which come with N9) and nothing more.
pub(crate) fn base_experience_reward(kind: u16, draws: &mut Random) -> i32 {
    match ladder::of(kind) {
        ladder::Family::Animal | ladder::Family::WaterAnimal => 1 + draws.next_int(3) as i32,
        _ => 0,
    }
}
