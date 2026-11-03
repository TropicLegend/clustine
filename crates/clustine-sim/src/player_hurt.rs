//! A hurt to a player, as a pure function (V4's part B, `v4-design.md` sections 2 B, 7
//! and 8): the port of `ServerPlayer.hurtServer`, `Player.hurtServer`,
//! `LivingEntity.hurtServer` and `Player.actuallyHurt` over a [`PlayerBody`], a view of
//! the player that the region fills, returning a [`PlayerHurt`] that the region applies:
//! the health and absorption, the armour's and the shield's wear, the knockback, the
//! packets and sounds, the totem and the death. Nothing here reads or writes the region;
//! the only state it touches is the player's own stream of draws, which a knockback with
//! no direction draws from as the game's `random` does.
//!
//! The game's classes, cited by method and bytecode offset: `n5:ServerPlayer` (the
//! `n5-javap` dump), `n7:Player`, `n7:LivingEntity`, `n7:Entity`, `n7:DamageSource`,
//! `n7:CombatRules`, `n7:EnchantmentHelper` and `v4:BlocksAttacks` with its records.
//!
//! The order of a hurt, as the game runs it:
//!
//! 1. `ServerPlayer.hurtServer @0-10`: refused where the player is invulnerable to the
//!    source (`ServerPlayer.isInvulnerableTo`: `Player.isInvulnerableTo`'s four damage
//!    rules, changing level unless the source is an ender pearl's, the client not yet
//!    loaded); `@11-42` refused where a player caused it and `pvp` is off.
//! 2. `Player.hurtServer @11-32`: refused where the abilities make the player
//!    invulnerable, unless the type bypasses invulnerability (the void); `@38-46` refused
//!    where dead; `@51-107` scaled by the difficulty; `@108-115` refused where nothing is
//!    left.
//! 3. `LivingEntity.hurtServer @0-41`: refused where dying or a fire with fire
//!    resistance; `@42-50` the sleeper wakes; `@75-101` the shield
//!    (`applyItemBlocking`); `@103-128` freezing ×5 for the types it hurts extra; `@129-163`
//!    the helmet ×0.75 and its wear for a type that damages helmets; `@184-271` the
//!    cooldown's two halves; `@272-282` who is blamed; `@283-390` the damage event (or the
//!    shield's block), `markHurt` and the knockback, for a full hit only; `@393-446` the
//!    totem, the death or the hurt sound.
//! 4. `Player.actuallyHurt @10-23`: the armour (and its wear) and the magic (resistance,
//!    then the protections of the enchantments worn); `@24-48` the absorption; `@89-95`
//!    **nothing more where nothing is left**; `@96-101` the type's exhaustion; `@113-119`
//!    the health.
//!
//! The outcome follows the hurt's branches (the review's M7), as the bytecode has them:
//!
//! | Case | Damage event | Hurt sound | `markHurt` | Knockback | Health |
//! |---|---|---|---|---|---|
//! | second half of the cooldown (the excess only) | no | no | no | no | yes |
//! | full hit | yes | yes | unless `no_impact` | unless `no_knockback` | yes |
//! | blocked, nothing left | no (`onBlocked`) | **yes** (see below) | no | no | — |
//!
//! One cell differs from the review's table: `LivingEntity.hurtServer @434-446` plays the
//! hurt sound whenever the hit was a full one (`flag8`) and the player is not dying,
//! whether or not a shield took all of it; nothing at `@283-390` tests the block before
//! it. So a hit blocked whole, outside the cooldown, still plays the type's hurt sound to
//! everyone but the player. A hit saved by a totem plays none (`@405 ifne 449` jumps
//! past both sounds).
//!
//! Named differences, where they live: the entity's own `Invulnerable` flag and
//! `invulnerableTime` (`Entity.isInvulnerableToBase @7-28`) are not kept for players;
//! enchantments' `damage_immunity` (`EnchantmentHelper.isImmuneToDamage`) is not read, as
//! no enchantment of the game has one; `removeEntitiesOnShoulder` (`Player.hurtServer @47`)
//! is not built, as no parrot rides a player; teams (`Player.canHarmPlayer`) are none, so
//! only `pvp` refuses a player's hurt; the causer that is itself an arrow
//! (`ServerPlayer.hurtServer @43-90`) is not told apart from other causers; a weapon's
//! `armor_effectiveness` (Breach, `CombatRules.getDamageAfterAbsorb @33-83`) is not read;
//! the statistics and advancement triggers are not kept; `MobEffectInstance.onMobHurt`
//! (`@488-530`) is the region's, after [`PlayerHurt::Hurt`] with `took`.

use clustine_blocks::data::entity_in_tag;
use clustine_data::enchanting::enchantment;
use clustine_data::{
    BlocksAttacks, ConsumeEffect, DamageEffects, DamageScaling, DamageTypeData, DamageTypes,
    DeathMessageType, ItemComponents, entity_types, items,
};
use clustine_world::components::{Component, Kind};
use clustine_world::{Abilities, Difficulty, EntityId, GameRules, ItemStack};

use crate::living::slots;
use crate::mobs::ai::goals::view_vector;
use crate::mobs::ai::maths::{DEGREES, atan2};
use crate::mobs::body::effects::{self, Effect, Effects};
use crate::mobs::body::hurt::{DamageSource, DamageType, damage_after_absorb, normalize};
use crate::physics::Vec3;
use crate::region::Random;

/// `Float.MAX_VALUE`, which an amount that is not finite becomes (`hurtServer @164-180`).
const FLOAT_MAX: f32 = f32::MAX;

/// A player as a hurt finds them: what the region knows of them at the moment of the
/// hurt.
#[derive(Debug, Clone)]
pub(crate) struct PlayerBody<'a> {
    pub(crate) health: f32,
    /// `getMaxHealth()`, which `setHealth` clamps to (20 but for Health Boost).
    pub(crate) max_health: f32,
    /// The effects, whose `absorption` is the player's absorption amount.
    pub(crate) effects: &'a Effects,
    /// `getMaxAbsorption()`, which `setAbsorptionAmount` clamps to.
    pub(crate) max_absorption: f32,
    /// What the player holds and wears, by `EquipmentSlot` ordinal: the main hand, the
    /// off hand, the feet, the legs, the chest and the head ([`slots`]).
    pub(crate) equipment: [Option<&'a ItemStack>; 6],
    /// The `death_protection` of what each hand holds, the main hand first (the totem's
    /// effects), as `ItemComponents` has it.
    pub(crate) death_protection: [Option<&'static [ConsumeEffect]>; 2],
    /// The attributes `armor`, `armor_toughness` and `knockback_resistance`, as the
    /// region computes them (`region/worn.rs`).
    pub(crate) armor: f64,
    pub(crate) toughness: f64,
    pub(crate) knockback_resistance: f64,
    /// `damageCooldownTime`: the ticks left of the hurt cooldown, and `lastHurt`, the
    /// amount of the hurt that began it.
    pub(crate) damage_cooldown: i32,
    pub(crate) last_hurt: f32,
    /// The feet, the server-side motion (`getDeltaMovement()`, which knockback starts
    /// from: the review's M2) and whether on the ground.
    pub(crate) position: Vec3,
    pub(crate) motion: Vec3,
    pub(crate) on_ground: bool,
    /// `getYRot()`, which the hurt's direction is told against (`indicateDamage`), and
    /// `getYHeadRot()`, which the shield's angle is measured from (`applyItemBlocking
    /// @113-118`).
    pub(crate) yaw: f32,
    pub(crate) head_yaw: f32,
    /// The item in use, where one is.
    pub(crate) using: Option<ItemUse>,
    pub(crate) abilities: Abilities,
    /// `hasClientLoaded()`, and `isChangingDimension()`.
    pub(crate) client_loaded: bool,
    pub(crate) changing_level: bool,
    pub(crate) sleeping: bool,
    pub(crate) rules: GameRules,
    pub(crate) difficulty: Difficulty,
}

/// The item a player is using (`isUsingItem()`), as `getItemBlockingWith` reads it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ItemUse {
    /// The hand, as its `EquipmentSlot` ordinal (0 the main hand, 1 the off hand).
    pub(crate) hand: u8,
    /// How long it has been in use: `getUseDuration − useItemRemaining`.
    pub(crate) ticks: i32,
    /// Its `blocks_attacks`, as `ItemComponents` has it.
    pub(crate) blocks_attacks: Option<BlocksAttacks>,
}

/// What a hurt comes with besides its source and amount.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Hurting<'a> {
    /// The weapon of the causer (`HurtBy.weapon`, `EntityUse::Attack.weapon`), whose
    /// `weapon` component's `disable_blocking_for_seconds` disables a shield
    /// (`LivingEntity.getSecondsToDisableBlocking`).
    pub(crate) weapon: Option<&'a ItemStack>,
    /// Where what dealt it is an arrow, its pierce level, which takes it past a shield
    /// (`applyItemBlocking @71-101`).
    pub(crate) pierce_level: u8,
}

/// What a hurt did, for the region to apply.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlayerHurt {
    /// Refused before anything changed: the player is invulnerable to it, a player
    /// caused it with `pvp` off, the player is dead or immune to fire, or nothing was
    /// left after the difficulty.
    Refused,
    /// Boxed, as what it carries is large beside a refusal.
    Hurt(Box<Hurt>),
}

impl PlayerHurt {
    /// `hurtServer`'s answer: whether the hurt took.
    pub(crate) fn took(&self) -> bool {
        match self {
            Self::Refused => false,
            Self::Hurt(hurt) => hurt.took,
        }
    }
}

/// Which of the cooldown's branches a hurt came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Branch {
    /// Within the cooldown and no harder than the hurt that began it: nothing more
    /// (`@206-216`), though the shield and the helmet were worn and the sleeper woke.
    Ignored,
    /// Within the cooldown and harder: only the excess, and no event, sound, `markHurt`
    /// or knockback (`@217-237`).
    Excess,
    /// A full hit, which begins the cooldown (`@240-269`).
    Full,
}

/// A hurt that came past the refusals.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hurt {
    /// `hurtServer`'s answer: false where ignored, or blocked with nothing left
    /// (`@449-465`).
    pub(crate) took: bool,
    pub(crate) branch: Branch,
    /// The sleeper woke (`stopSleeping`, `@42-50`).
    pub(crate) woke: bool,
    /// What a shield did, where it blocked any of it or was worn.
    pub(crate) shield: Option<ShieldBlock>,
    /// The health and the absorption after the hurt, and what each lost.
    pub(crate) health: f32,
    pub(crate) absorption: f32,
    pub(crate) from_health: f32,
    pub(crate) from_absorption: f32,
    /// What reached the health before `setHealth` clamped it, as the combat tracker
    /// records it (`Player.actuallyHurt @104-110`); 0 where nothing did.
    pub(crate) damage: f32,
    /// The cooldown and `lastHurt` from now.
    pub(crate) damage_cooldown: i32,
    pub(crate) last_hurt: f32,
    /// `broadcastDamageEvent` (`@321-324`): to the player and those who see them.
    pub(crate) damage_event: bool,
    /// The type's hurt sound (`Player.getHurtSound`: `DamageEffects.sound`), to everyone
    /// but the player (`Player.playSound`; the review's M6), whose client plays its own
    /// from the damage event.
    pub(crate) hurt_sound: Option<&'static str>,
    /// `playSecondaryHurtSound`: thorns' hit, to everyone, as the player is hurt or dies.
    pub(crate) thorns_sound: bool,
    /// The death sound (`entity.player.death`), to everyone but the player.
    pub(crate) death_sound: bool,
    /// `markHurt`: the player's motion is sent to them.
    pub(crate) mark_hurt: bool,
    /// The server-side motion after the hurt's pushes (a shield's and the knockback's);
    /// the body's own where nothing pushed.
    pub(crate) motion: Vec3,
    /// `indicateDamage`'s `hurtDir`, which `ClientboundHurtAnimationPacket` tells the
    /// player (`n5:ServerPlayer.indicateDamage @0-32`), where the knockback was dealt
    /// unblocked.
    pub(crate) hurt_direction: Option<f32>,
    /// The armour's wear, a piece at a time in the game's order (the helmet's first where
    /// the type damages helmets, then the feet to the head): the slot by `EquipmentSlot`
    /// ordinal and the wear `hurtAndBreak` is called with.
    pub(crate) wear: Vec<(u8, i32)>,
    /// The food exhaustion the hurt costs (`causeFoodExhaustion`), 0 where none.
    pub(crate) exhaustion: f32,
    /// Who is blamed: `setLastHurtByMob` (a living causer, unless `no_anger`) and
    /// `setLastHurtByPlayer` (a player causer, for 100 ticks).
    pub(crate) blame_mob: Option<EntityId>,
    pub(crate) blame_player: Option<u128>,
    /// `ENTITY_DAMAGE` posted (sculk hears it): something reached the health.
    pub(crate) game_event: bool,
    /// The totem that saved the player, where one did: its health is then 1.
    pub(crate) totem: Option<Totem>,
    /// The player died of it (`die(source)`, the region's).
    pub(crate) died: bool,
}

/// What a shield did (`applyItemBlocking`, `blockUsingItem`), where it blocked some of
/// the hurt or was worn by it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShieldBlock {
    /// The hand that holds it, as its `EquipmentSlot` ordinal.
    pub(crate) hand: u8,
    /// What it took away of the hurt.
    pub(crate) blocked: f32,
    /// The wear `hurtBlockingItem` gives it, 0 below the threshold.
    pub(crate) wear: i32,
    /// `onBlocked`'s sound, at the player to everyone, where the block was of a full hit
    /// (`@301-318`).
    pub(crate) block_sound: Option<&'static str>,
    /// Where the attacker's weapon disables it: the cooldown the item takes, in ticks
    /// (`BlocksAttacks.disable`: the use stops, the disable sound plays to everyone).
    pub(crate) disabled_for: Option<i32>,
    pub(crate) disable_sound: Option<&'static str>,
}

/// A totem's save (`checkTotemDeathProtection`): one of the hand's stack used up, the
/// health set to 1, the item's effects applied (by the region, in their order) and
/// entity event 35 told.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Totem {
    /// The hand, as its `EquipmentSlot` ordinal.
    pub(crate) hand: u8,
    pub(crate) effects: &'static [ConsumeEffect],
}

/// The data of `kind` from the jar, or the game's defaults for a type it has none of: no
/// exhaustion, no scaling, the plain hurt sound.
pub(crate) fn type_data(kind: DamageType) -> DamageTypeData {
    clustine_data::damage_type(kind.0)
        .copied()
        .unwrap_or(DamageTypeData {
            name: kind.0,
            message_id: "generic",
            scaling: DamageScaling::Never,
            exhaustion: 0.0,
            effects: DamageEffects::Hurt,
            death_message_type: DeathMessageType::Default,
        })
}

/// `DamageEffects.sound()` for a player (`n7:DamageEffects.<clinit> @4-99`): thorns' is
/// the plain hurt's.
pub(crate) fn hurt_sound(effects: DamageEffects) -> &'static str {
    match effects {
        DamageEffects::Hurt | DamageEffects::Thorns => "minecraft:entity.player.hurt",
        DamageEffects::Drowning => "minecraft:entity.player.hurt_drown",
        DamageEffects::Burning => "minecraft:entity.player.hurt_on_fire",
        DamageEffects::Poking => "minecraft:entity.player.hurt_sweet_berry_bush",
        DamageEffects::Freezing => "minecraft:entity.player.hurt_freeze",
    }
}

/// The hurt of `amount` from `source`, of the type whose data is `data`, to the player
/// `body`, with what `hurting` adds; the knockback's draws come from `draws`, the
/// player's stream.
pub(crate) fn hurt_player(
    body: &PlayerBody<'_>,
    source: &DamageSource,
    data: &DamageTypeData,
    amount: f32,
    hurting: Hurting<'_>,
    draws: &mut Random,
) -> PlayerHurt {
    let kind = source.kind;
    // `ServerPlayer.hurtServer @0-10`, `Player.hurtServer @0-10` and `LivingEntity.
    // hurtServer @0-10` each ask the same; asked once.
    if invulnerable_to(body, kind) {
        return PlayerHurt::Refused;
    }
    // `ServerPlayer.hurtServer @11-42`: `canHarmPlayer` is `isPvpAllowed` and the teams,
    // of which there are none.
    if source
        .cause
        .as_ref()
        .is_some_and(|cause| cause.player.is_some())
        && !body.rules.pvp
    {
        return PlayerHurt::Refused;
    }
    // `Player.hurtServer @11-32`.
    if body.abilities.invulnerable && !kind.is("minecraft:bypasses_invulnerability") {
        return PlayerHurt::Refused;
    }
    // `@38-46`, and `LivingEntity.hurtServer @11-19`.
    if dying(body) {
        return PlayerHurt::Refused;
    }
    let amount = scaled(amount, data.scaling, source, body.difficulty);
    // `@108-115`: `fcmpl ifne`, so only an amount of exactly nothing is refused.
    if amount == 0.0 {
        return PlayerHurt::Refused;
    }
    // `LivingEntity.hurtServer @20-41`.
    if kind.is("minecraft:is_fire") && body.effects.has(Effect::FIRE_RESISTANCE) {
        return PlayerHurt::Refused;
    }
    living_hurt_server(body, source, data, amount, hurting, draws)
}

/// `ServerPlayer.isInvulnerableTo` (`n5 @0-41`): `Player.isInvulnerableTo`, or changing
/// level unless an ender pearl hurts, or the client not yet loaded.
fn invulnerable_to(body: &PlayerBody<'_>, kind: DamageType) -> bool {
    player_invulnerable_to(body, kind)
        || (body.changing_level && kind.0 != "minecraft:ender_pearl")
        || !body.client_loaded
}

/// `Player.isInvulnerableTo @0-152`: the living body's (nothing a player is immune to:
/// not fire-immune, not of `#fall_damage_immune`), then the first of the four damage
/// rules whose tag the type is in decides.
fn player_invulnerable_to(body: &PlayerBody<'_>, kind: DamageType) -> bool {
    if kind.is("minecraft:is_drowning") {
        !body.rules.drowning_damage
    } else if kind.is("minecraft:is_fall") {
        !body.rules.fall_damage
    } else if kind.is("minecraft:is_fire") {
        !body.rules.fire_damage
    } else if kind.is("minecraft:is_freezing") {
        !body.rules.freeze_damage
    } else {
        false
    }
}

/// `isDeadOrDying()`.
fn dying(body: &PlayerBody<'_>) -> bool {
    dying_at(body.health)
}

/// `isDeadOrDying()` at the health `health`.
fn dying_at(health: f32) -> bool {
    health <= 0.0
}

/// `DamageSource.scalesWithDifficulty` (`n7 @0-83`): never, always, or where the causer
/// is a living entity that is not a player.
fn scales(scaling: DamageScaling, source: &DamageSource) -> bool {
    match scaling {
        DamageScaling::Never => false,
        DamageScaling::Always => true,
        DamageScaling::WhenCausedByLivingNonPlayer => source
            .cause
            .as_ref()
            .is_some_and(|cause| cause.living && cause.player.is_none()),
    }
}

/// `Player.hurtServer @51-107`: nothing in peaceful, `min(d/2 + 1, d)` in easy, `d·3/2`
/// in hard, where the source scales.
fn scaled(
    amount: f32,
    scaling: DamageScaling,
    source: &DamageSource,
    difficulty: Difficulty,
) -> f32 {
    if !scales(scaling, source) {
        return amount;
    }
    match difficulty {
        Difficulty::Peaceful => 0.0,
        Difficulty::Easy => (amount / 2.0 + 1.0).min(amount),
        Difficulty::Normal => amount,
        Difficulty::Hard => amount * 3.0 / 2.0,
    }
}

/// `LivingEntity.hurtServer @42-446` for a player, past the refusals.
fn living_hurt_server(
    body: &PlayerBody<'_>,
    source: &DamageSource,
    data: &DamageTypeData,
    amount: f32,
    hurting: Hurting<'_>,
    draws: &mut Random,
) -> PlayerHurt {
    let kind = source.kind;
    let mut motion = body.motion;
    let mut wear = Vec::new();
    // `@58-65`.
    let mut amount = if amount < 0.0 { 0.0 } else { amount };
    // `@75-101`: the shield, which may push the player too.
    let shield = apply_item_blocking(body, source, amount, hurting, &mut motion, draws);
    let blocked_amount = shield.as_ref().map_or(0.0, |shield| shield.blocked);
    amount -= blocked_amount;
    let blocked = blocked_amount > 0.0;
    // `@103-128`.
    amount *= freeze_multiplier(entity_types::PLAYER, kind);
    // `@129-163`: the helmet is worn by the amount before it is lowered.
    if kind.is("minecraft:damages_helmet") && body.equipment[usize::from(slots::HEAD)].is_some() {
        hurt_equipment(body, kind, amount, &[slots::HEAD], &mut wear);
        amount *= 0.75;
    }
    // `@164-180`.
    if !amount.is_finite() {
        amount = FLOAT_MAX;
    }
    let mut health = body.health;
    let mut absorption = body.effects.absorption;
    let in_cooldown = body.damage_cooldown as f32 > 10.0 && !kind.is("minecraft:bypasses_cooldown");
    if in_cooldown && amount <= body.last_hurt {
        // `@206-216`: nothing more, but what the shield and the helmet took stays.
        return PlayerHurt::Hurt(Box::new(Hurt {
            took: false,
            branch: Branch::Ignored,
            woke: body.sleeping,
            shield,
            health,
            absorption,
            from_health: 0.0,
            from_absorption: 0.0,
            damage: 0.0,
            damage_cooldown: body.damage_cooldown,
            last_hurt: body.last_hurt,
            damage_event: false,
            hurt_sound: None,
            thorns_sound: false,
            death_sound: false,
            mark_hurt: false,
            motion,
            hurt_direction: None,
            wear,
            exhaustion: 0.0,
            blame_mob: None,
            blame_player: None,
            game_event: false,
            totem: None,
            died: false,
        }));
    }
    let (branch, damage_cooldown, hurt_by) = if in_cooldown {
        // `@217-237`: the excess over the hurt that began the cooldown.
        (
            Branch::Excess,
            body.damage_cooldown,
            amount - body.last_hurt,
        )
    } else {
        // `@240-269`: the cooldown begins at 20 before the health is touched.
        (Branch::Full, 20, amount)
    };
    let taken = actually_hurt(
        body,
        source,
        data,
        hurt_by,
        &mut health,
        &mut absorption,
        &mut wear,
    );
    let full = branch == Branch::Full;
    // `@272-282`: `resolveMobResponsibleForDamage` and `resolvePlayerResponsibleForDamage`.
    // A wind charge's hurt angers every player (no player is of
    // `#no_anger_from_wind_charge`), and a tamed wolf's owner is not known here.
    let blame_mob = source
        .cause
        .as_ref()
        .filter(|cause| cause.living && !kind.is("minecraft:no_anger"))
        .map(|cause| cause.key);
    let blame_player = source.cause.as_ref().and_then(|cause| cause.player);
    let mut damage_event = false;
    let mut mark_hurt = false;
    let mut hurt_direction = None;
    // `@288-324`: a block's sound in place of the damage event, for a full hit only.
    let shield = shield.map(|shield| ShieldBlock {
        block_sound: shield.block_sound.filter(|_| full && blocked),
        ..shield
    });
    if full {
        damage_event = !blocked;
        // `@327-349`.
        mark_hurt = !kind.is("minecraft:no_impact") && (!blocked || amount > 0.0);
        // `@352-390`.
        let blocked_whole = blocked && amount <= 0.0;
        if !kind.is("minecraft:no_knockback") && !blocked_whole {
            hurt_direction = deal_default_knockback(body, source, blocked, &mut motion, draws);
        }
    }
    // `@393-446`.
    let after_hurt = health;
    let mut totem = None;
    let mut died = false;
    let mut death_sound = false;
    let mut sound = None;
    let mut thorns_sound = false;
    if dying_at(health) {
        totem = totem_death_protection(body, kind);
        if totem.is_some() {
            // `@142-144`: `setHealth(1.0f)`.
            health = 1.0;
        } else {
            death_sound = full;
            thorns_sound = full && kind == DamageType::THORNS;
            died = true;
        }
    } else if full {
        sound = Some(hurt_sound(data.effects));
        thorns_sound = kind == DamageType::THORNS;
    }
    PlayerHurt::Hurt(Box::new(Hurt {
        // `@449-465`.
        took: !blocked || amount > 0.0,
        branch,
        woke: body.sleeping,
        shield,
        health,
        absorption,
        from_health: body.health - after_hurt,
        from_absorption: body.effects.absorption - absorption,
        damage: taken.damage,
        damage_cooldown,
        last_hurt: amount,
        damage_event,
        hurt_sound: sound,
        thorns_sound,
        death_sound,
        mark_hurt,
        motion,
        hurt_direction,
        wear,
        exhaustion: taken.exhaustion,
        blame_mob,
        blame_player,
        game_event: taken.damage != 0.0,
        totem,
        died,
    }))
}

/// `EntityType.is(#freeze_hurts_extra_types)` for a freezing type
/// (`LivingEntity.hurtServer @103-128`): ×5 for the strider, the blaze and the magma
/// cube, ×1 for anything else, a player among them.
fn freeze_multiplier(entity_type: i32, kind: DamageType) -> f32 {
    if kind.is("minecraft:is_freezing")
        && entity_in_tag(entity_type, "minecraft:freeze_hurts_extra_types")
    {
        5.0
    } else {
        1.0
    }
}

/// What `actuallyHurt` took.
#[derive(Debug, Clone, Copy, Default)]
struct Taken {
    damage: f32,
    exhaustion: f32,
}

/// `Player.actuallyHurt @10-153`: the armour's share (and its wear) and the magic's
/// share, then the absorption; where nothing is left, nothing more (`@89-95`), not even
/// exhaustion; else the type's exhaustion, then the health. The second
/// `setAbsorptionAmount` that `LivingEntity.actuallyHurt @137-144` makes after the health
/// is not the player's: `Player.actuallyHurt` has none.
fn actually_hurt(
    body: &PlayerBody<'_>,
    source: &DamageSource,
    data: &DamageTypeData,
    amount: f32,
    health: &mut f32,
    absorption: &mut f32,
    wear: &mut Vec<(u8, i32)>,
) -> Taken {
    // `@0-9` asks `isInvulnerableTo` again, which [`hurt_player`] has answered.
    let kind = source.kind;
    let mut amount = amount;
    // `LivingEntity.getDamageAfterArmorAbsorb @0-37`: the wear first, by the amount
    // before the armour; `getArmorValue()` is the attribute floored.
    if !kind.is("minecraft:bypasses_armor") {
        hurt_equipment(
            body,
            kind,
            amount,
            &[slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD],
            wear,
        );
        let armor = body.armor.floor() as i32 as f32;
        amount = damage_after_absorb(amount, armor, body.toughness as f32);
    }
    amount = magic_absorb(body, source, amount);
    // `@24-48`, by the shared `absorbed` on a copy of the absorption alone.
    let mut held = Effects {
        active: Vec::new(),
        absorption: *absorption,
    };
    amount = effects::absorbed(&mut held, amount, body.max_absorption);
    *absorption = held.absorption;
    // `@89-95`.
    if amount == 0.0 {
        return Taken::default();
    }
    // `@96-101`: `causeFoodExhaustion`, nothing while invulnerable (`Player.
    // causeFoodExhaustion @0-10`).
    let exhaustion = if body.abilities.invulnerable {
        0.0
    } else {
        data.exhaustion
    };
    // `@113-119`: `setHealth` clamps within 0 and the most.
    *health = clamp(*health - amount, 0.0, body.max_health);
    Taken {
        damage: amount,
        exhaustion,
    }
}

/// `LivingEntity.getDamageAfterMagicAbsorb @0-233`: the effects' share by the shared
/// `magic_absorbed` (nothing taken where the type bypasses effects, resistance's fifth a
/// level, nothing where nothing is left), then, unless the type bypasses enchantments,
/// the protections worn (`CombatRules.getDamageAfterMagicAbsorb`: at most 20, a
/// twenty-fifth a point).
fn magic_absorb(body: &PlayerBody<'_>, source: &DamageSource, amount: f32) -> f32 {
    let amount = effects::magic_absorbed(body.effects, source, amount);
    // `@0-11` returns the amount before the enchantments are asked; `@166-173` returns
    // nothing.
    if source.kind.is("minecraft:bypasses_effects") || amount <= 0.0 {
        return amount;
    }
    if source.kind.is("minecraft:bypasses_enchantments") {
        return amount;
    }
    let protection = damage_protection(body, source.kind);
    if protection > 0.0 {
        let protection = clamp(protection, 0.0, 20.0);
        amount * (1.0 - protection / 25.0)
    } else {
        amount
    }
}

/// `EnchantmentHelper.getDamageProtection` (`n7 @0-26`): over every slot
/// (`runIterationOnEquipment`), each enchantment of the stack there that works from the
/// slot (`matchingSlot`, from the enchantments' data), the sum of its
/// `minecraft:damage_protection` effects whose requirements the type meets. The effects
/// are written here from the jar's files (`data/minecraft/enchantment/*.json`), as
/// `items/enchantments.rs` writes the others: each an `add` of a linear value, required
/// not to bypass invulnerability:
///
/// - `protection.json`: base 1, per level 1, any type;
/// - `fire_protection.json`: base 2, per level 2, `#is_fire`;
/// - `blast_protection.json`: base 2, per level 2, `#is_explosion`;
/// - `projectile_protection.json`: base 2, per level 2, `#is_projectile`;
/// - `feather_falling.json`: base 3, per level 3, `#is_fall`.
///
/// No other enchantment of the jar has one.
fn damage_protection(body: &PlayerBody<'_>, kind: DamageType) -> f32 {
    if kind.is("minecraft:bypasses_invulnerability") {
        return 0.0;
    }
    let mut protection = 0.0;
    for (slot, stack) in body.equipment.iter().enumerate() {
        let Some(stack) = stack else {
            continue;
        };
        let Some(Component::Enchantments(levels)) = stack.components.get(Kind::Enchantments) else {
            continue;
        };
        let slot = u8::try_from(slot).expect("six slots");
        for (name, level) in levels {
            let Some(per_level) = protection_per_level(name, kind) else {
                continue;
            };
            if !enchantment(name).is_some_and(|known| known.matching_slot(slot)) {
                continue;
            }
            // `LevelBasedValue.Linear` with base and step equal.
            protection += per_level + per_level * (*level - 1) as f32;
        }
    }
    protection
}

/// The step of the protection enchantment `name` against `kind`, where it protects
/// against it.
fn protection_per_level(name: &str, kind: DamageType) -> Option<f32> {
    let (per_level, tag) = match name {
        "minecraft:protection" => (1.0, None),
        "minecraft:fire_protection" => (2.0, Some("minecraft:is_fire")),
        "minecraft:blast_protection" => (2.0, Some("minecraft:is_explosion")),
        "minecraft:projectile_protection" => (2.0, Some("minecraft:is_projectile")),
        "minecraft:feather_falling" => (3.0, Some("minecraft:is_fall")),
        _ => return None,
    };
    tag.is_none_or(|tag| kind.is(tag)).then_some(per_level)
}

/// `LivingEntity.doHurtEquipment @0-111`: where `amount` is above nothing, each of
/// `slots` that holds an equippable item that is hurt on a hurt, can wear and is not
/// resistant to the type is worn by `max(1, amount/4)` (as an `int`).
///
/// The data does not carry `equippable`'s `damage_on_hurt`; of the items a player wears
/// in the four armour slots only the elytra has it false (the jar's item reports), so it
/// is told apart by its id.
fn hurt_equipment(
    body: &PlayerBody<'_>,
    kind: DamageType,
    amount: f32,
    worn_slots: &[u8],
    wear: &mut Vec<(u8, i32)>,
) {
    if amount <= 0.0 {
        return;
    }
    let by = 1.0_f32.max(amount / 4.0) as i32;
    for &slot in worn_slots {
        let Some(stack) = body.equipment[usize::from(slot)] else {
            continue;
        };
        let components = ItemComponents::of(stack.item);
        let damage_on_hurt = components.equippable.is_some() && stack.item != items::ELYTRA;
        // `ItemStack.isDamageableItem`: a most it can wear to (no stack is unbreakable).
        let damageable = components.max_damage.is_some();
        // `ItemStack.canBeHurtBy`: not resistant to the type (`damage_resistant`).
        let hurt_by = !components
            .damage_resistant
            .is_some_and(|tag| kind.is(tag.trim_start_matches('#')));
        if damage_on_hurt && damageable && hurt_by {
            wear.push((slot, by));
        }
    }
}

/// `LivingEntity.applyItemBlocking @0-267` with `Player.blockUsingItem @0-68`: what the
/// item in use blocks of `amount`, where it blocks. Nothing for nothing, for an item that
/// does not block or has not been raised long enough (`getItemBlockingWith`: its
/// `blockDelayTicks`), for a type that bypasses it, or for a piercing arrow; else the
/// reductions by the angle between where the player's head faces and where the source
/// is (π where it names no place), the item worn by what it blocked, and, for a melee
/// hit by a living entity, the player pushed (`blockedByItem`, where not all was
/// blocked) and the item disabled by the weapon's seconds.
fn apply_item_blocking(
    body: &PlayerBody<'_>,
    source: &DamageSource,
    amount: f32,
    hurting: Hurting<'_>,
    motion: &mut Vec3,
    draws: &mut Random,
) -> Option<ShieldBlock> {
    if amount <= 0.0 {
        return None;
    }
    let using = body.using?;
    let blocks = using.blocks_attacks?;
    // `getItemBlockingWith @0-62`.
    if using.ticks < java_round(blocks.block_delay_seconds * 20.0) {
        return None;
    }
    let kind = source.kind;
    // `@34-70`.
    if blocks
        .bypassed_by
        .is_some_and(|tag| kind.is(tag.trim_start_matches('#')))
    {
        return None;
    }
    // `@71-101`.
    if hurting.pierce_level > 0 {
        return None;
    }
    // `@102-175`.
    let angle = match source.source_position() {
        Some(at) => {
            let view = view_vector(0.0, body.head_yaw);
            let towards = normalize([at[0] - body.position[0], 0.0, at[2] - body.position[2]]);
            (towards[0] * view[0] + towards[1] * view[1] + towards[2] * view[2]).acos()
        }
        None => f64::from(std::f32::consts::PI),
    };
    let blocked = resolve_blocked_damage(&blocks, kind, amount, angle);
    // `@188-203`: `hurtBlockingItem`.
    let wear = item_damage(&blocks, blocked);
    let mut disabled_for = None;
    // `@206-262`: `blockUsingItem`, for a melee hit by a living entity.
    let melee = !kind.is("minecraft:is_projectile")
        && source.direct.as_ref().is_some_and(|direct| direct.living);
    if blocked > 0.0 && melee {
        let attacker = source
            .direct
            .as_ref()
            .expect("a melee hit has a direct entity");
        // `LivingEntity.blockedByItem @0-32`, which the attacker runs on the player:
        // where not all was blocked, the player is pushed by 0.5 along
        // `(player − attacker)`, which `knockback` takes as the direction away from which
        // to push.
        if blocked < amount {
            knockback(
                body,
                0.5,
                body.position[0] - attacker.position[0],
                body.position[2] - attacker.position[2],
                motion,
                draws,
            );
        }
        // `Player.blockUsingItem @39-65`, `BlocksAttacks.disable @0-55`.
        let seconds = hurting
            .weapon
            .and_then(|weapon| ItemComponents::of(weapon.item).weapon)
            .map_or(0.0, |weapon| weapon.disable_blocking_for_seconds);
        if seconds > 0.0 {
            let ticks = disable_blocking_for_ticks(&blocks, seconds);
            if ticks > 0 {
                disabled_for = Some(ticks);
            }
        }
    }
    // `hurtBlockingItem` wears it even where it blocked nothing, which only an item
    // whose threshold is not above nothing can.
    if blocked <= 0.0 && wear <= 0 {
        return None;
    }
    Some(ShieldBlock {
        hand: using.hand,
        blocked,
        wear,
        // Kept by the caller only where the block was of a full hit.
        block_sound: blocks.block_sound,
        disable_sound: disabled_for.and(blocks.disable_sound),
        disabled_for,
    })
}

/// `BlocksAttacks.resolveBlockedDamage @0-59`: the sum of the reductions, within
/// nothing and the amount.
fn resolve_blocked_damage(
    blocks: &BlocksAttacks,
    kind: DamageType,
    amount: f32,
    angle: f64,
) -> f32 {
    let mut sum = 0.0_f32;
    for reduction in blocks.damage_reductions {
        // `DamageReduction.resolve @0-65`: the angle against the reduction's, in radians
        // as a float.
        if angle > f64::from(0.017_453_292_f32 * reduction.horizontal_blocking_angle) {
            continue;
        }
        let of_kind = match reduction.kinds {
            None => true,
            Some(DamageTypes::Tag(tag)) => kind.is(tag.trim_start_matches('#')),
            Some(DamageTypes::Listed(names)) => names.contains(&kind.0),
        };
        if !of_kind {
            continue;
        }
        sum += clamp(reduction.base + reduction.factor * amount, 0.0, amount);
    }
    clamp(sum, 0.0, amount)
}

/// `BlocksAttacks$ItemDamageFunction.apply @0-25`: nothing below the threshold, else
/// `floor(base + factor · blocked)`.
fn item_damage(blocks: &BlocksAttacks, blocked: f32) -> i32 {
    let function = blocks.item_damage;
    if blocked < function.threshold {
        return 0;
    }
    (function.base + function.factor * blocked).floor() as i32
}

/// `BlocksAttacks.disableBlockingForTicks @0-22`: `round(seconds · scale · 20)` where
/// above nothing.
fn disable_blocking_for_ticks(blocks: &BlocksAttacks, seconds: f32) -> i32 {
    let scaled = seconds * blocks.disable_cooldown_scale;
    if scaled > 0.0 {
        java_round(scaled * 20.0)
    } else {
        0
    }
}

/// Java's `Math.round(float)`: the nearest `int`, halves up.
fn java_round(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}

/// `LivingEntity.dealDefaultKnockback @0-115`: pushed by `0.4f` along a projectile's
/// motion where a projectile dealt it, else away from where the source is; then, unless
/// a shield blocked some of it, `indicateDamage`, whose direction is returned.
fn deal_default_knockback(
    body: &PlayerBody<'_>,
    source: &DamageSource,
    blocked: bool,
    motion: &mut Vec3,
    draws: &mut Random,
) -> Option<f32> {
    let (dx, dz) = match source.projectile_motion {
        Some(projectile) => (-projectile[0], -projectile[2]),
        None => source.source_position().map_or((0.0, 0.0), |at| {
            (at[0] - body.position[0], at[2] - body.position[2])
        }),
    };
    knockback(body, 0.400_000_005_960_464_5, dx, dz, motion, draws);
    if blocked {
        return None;
    }
    // `n5:ServerPlayer.indicateDamage @0-17`.
    Some((atan2(dz, dx) * DEGREES - f64::from(body.yaw)) as f32)
}

/// `LivingEntity.knockback @0-184` on the motion `motion`: the knockback resistance
/// taken off; half the old sideways motion kept less the push away from `(x, z)`; up by
/// at most 0.4 from the ground, else the old vertical motion; a direction too short is
/// drawn instead, two `nextDouble` a side.
fn knockback(
    body: &PlayerBody<'_>,
    strength: f64,
    x: f64,
    z: f64,
    motion: &mut Vec3,
    draws: &mut Random,
) {
    let strength = strength * (1.0 - body.knockback_resistance);
    if strength <= 0.0 {
        return;
    }
    let old = *motion;
    let (mut x, mut z) = (x, z);
    while x * x + z * z < 9.999_999_747_378_752e-6 {
        x = (draws.next_double() - draws.next_double()) * 0.01;
        z = (draws.next_double() - draws.next_double()) * 0.01;
    }
    let push = normalize([x, 0.0, z]);
    let push = [push[0] * strength, 0.0, push[2] * strength];
    *motion = [
        old[0] / 2.0 - push[0],
        if body.on_ground {
            (old[1] / 2.0 + strength).min(0.4)
        } else {
            old[1]
        },
        old[2] / 2.0 - push[2],
    ];
}

/// `LivingEntity.checkTotemDeathProtection @0-172`: none against a type that bypasses
/// invulnerability; else the first hand, the main hand before the off hand
/// (`InteractionHand.values()`), whose item has `death_protection`.
fn totem_death_protection(body: &PlayerBody<'_>, kind: DamageType) -> Option<Totem> {
    if kind.is("minecraft:bypasses_invulnerability") {
        return None;
    }
    [slots::MAIN_HAND, slots::OFF_HAND]
        .into_iter()
        .find_map(|hand| {
            body.death_protection[usize::from(hand)].map(|effects| Totem { hand, effects })
        })
}

/// `Mth.clamp(float, float, float)`.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min { min } else { value.min(max) }
}

#[cfg(test)]
mod tests {
    //! From the game's numbers. The damage types' data, the shield's `blocks_attacks`
    //! and the totem's `death_protection` are written here by hand from the jar's
    //! values (`data/minecraft/damage_type/*.json` and the item components report of
    //! `shield` and `totem_of_undying`), so that the tests do not hang on the generated
    //! tables.

    use clustine_data::{DamageReduction, EffectInstance, ItemDamageFunction};
    use clustine_world::components::Component;

    use super::*;
    use crate::mobs::body::effects::Layer;
    use crate::mobs::body::hurt::Causer;

    /// A damage type's data as its file in the jar has it.
    const fn kind_data(
        name: &'static str,
        message_id: &'static str,
        exhaustion: f32,
        scaling: DamageScaling,
        effects: DamageEffects,
    ) -> DamageTypeData {
        DamageTypeData {
            name,
            message_id,
            scaling,
            exhaustion,
            effects,
            death_message_type: DeathMessageType::Default,
        }
    }

    const LIVING: DamageScaling = DamageScaling::WhenCausedByLivingNonPlayer;
    const MOB_ATTACK: DamageTypeData = kind_data(
        "minecraft:mob_attack",
        "mob",
        0.1,
        LIVING,
        DamageEffects::Hurt,
    );
    const GENERIC: DamageTypeData = kind_data(
        "minecraft:generic",
        "generic",
        0.0,
        LIVING,
        DamageEffects::Hurt,
    );
    const OUT_OF_WORLD: DamageTypeData = kind_data(
        "minecraft:out_of_world",
        "outOfWorld",
        0.0,
        LIVING,
        DamageEffects::Hurt,
    );
    const DROWN: DamageTypeData = kind_data(
        "minecraft:drown",
        "drown",
        0.0,
        LIVING,
        DamageEffects::Drowning,
    );
    const FALLING_ANVIL: DamageTypeData = kind_data(
        "minecraft:falling_anvil",
        "anvil",
        0.1,
        LIVING,
        DamageEffects::Hurt,
    );
    const FREEZE: DamageTypeData = kind_data(
        "minecraft:freeze",
        "freeze",
        0.0,
        LIVING,
        DamageEffects::Freezing,
    );
    const ON_FIRE: DamageTypeData = kind_data(
        "minecraft:on_fire",
        "onFire",
        0.0,
        LIVING,
        DamageEffects::Burning,
    );
    const EXPLOSION: DamageTypeData = kind_data(
        "minecraft:explosion",
        "explosion",
        0.1,
        DamageScaling::Always,
        DamageEffects::Hurt,
    );
    const ARROW: DamageTypeData =
        kind_data("minecraft:arrow", "arrow", 0.1, LIVING, DamageEffects::Hurt);
    const FALL: DamageTypeData = DamageTypeData {
        death_message_type: DeathMessageType::FallVariants,
        ..kind_data("minecraft:fall", "fall", 0.0, LIVING, DamageEffects::Hurt)
    };
    const ENDER_PEARL: DamageTypeData = DamageTypeData {
        death_message_type: DeathMessageType::FallVariants,
        ..kind_data(
            "minecraft:ender_pearl",
            "fall",
            0.0,
            LIVING,
            DamageEffects::Hurt,
        )
    };
    const PLAYER_ATTACK: DamageTypeData = kind_data(
        "minecraft:player_attack",
        "player",
        0.1,
        LIVING,
        DamageEffects::Hurt,
    );
    const CACTUS: DamageTypeData = kind_data(
        "minecraft:cactus",
        "cactus",
        0.1,
        LIVING,
        DamageEffects::Hurt,
    );
    const THORNS: DamageTypeData = kind_data(
        "minecraft:thorns",
        "thorns",
        0.1,
        LIVING,
        DamageEffects::Thorns,
    );

    /// The shield's `blocks_attacks`: the jar's `block_delay_seconds` 0.25, `bypassed_by`
    /// `#minecraft:bypasses_shield`, `item_damage` threshold 3, base 1, factor 1, and its
    /// two sounds; and the codec's defaults for what the item leaves out
    /// (`disable_cooldown_scale` 1; one reduction of 90° for every type, base 0, factor
    /// 1: `BlocksAttacks.lambda$static$0 @6-69`).
    const SHIELD: BlocksAttacks = BlocksAttacks {
        block_delay_seconds: 0.25,
        disable_cooldown_scale: 1.0,
        damage_reductions: &[DamageReduction {
            horizontal_blocking_angle: 90.0,
            kinds: None,
            base: 0.0,
            factor: 1.0,
        }],
        item_damage: ItemDamageFunction {
            threshold: 3.0,
            base: 1.0,
            factor: 1.0,
        },
        bypassed_by: Some("#minecraft:bypasses_shield"),
        block_sound: Some("minecraft:item.shield.block"),
        disable_sound: Some("minecraft:item.shield.break"),
    };

    /// The totem's `death_protection`: every effect cleared, then Regeneration II for
    /// 900 ticks, Absorption II for 100 and Fire Resistance for 800.
    static TOTEM: &[ConsumeEffect] = &[
        ConsumeEffect::ClearAllEffects,
        ConsumeEffect::ApplyEffects {
            effects: &[
                totem_effect("minecraft:regeneration", 900, 1),
                totem_effect("minecraft:absorption", 100, 1),
                totem_effect("minecraft:fire_resistance", 800, 0),
            ],
            probability: 1.0,
        },
    ];

    const fn totem_effect(effect: &'static str, duration: i32, amplifier: i32) -> EffectInstance {
        EffectInstance {
            effect,
            duration,
            amplifier,
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }

    const STRENGTH: f64 = 0.400_000_005_960_464_5;

    fn kind(name: &str) -> DamageType {
        DamageType::named(name).expect("the game has the type")
    }

    /// A survival player at the origin facing south (yaw 0, toward +z), at full health,
    /// on the ground and still, wearing and holding nothing, in normal difficulty, which
    /// scales nothing.
    fn body(effects: &Effects) -> PlayerBody<'_> {
        PlayerBody {
            health: 20.0,
            max_health: 20.0,
            effects,
            max_absorption: 0.0,
            equipment: [None; 6],
            death_protection: [None; 2],
            armor: 0.0,
            toughness: 0.0,
            knockback_resistance: 0.0,
            damage_cooldown: 0,
            last_hurt: 0.0,
            position: [0.0; 3],
            motion: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            head_yaw: 0.0,
            using: None,
            abilities: Abilities {
                may_fly: false,
                instabuild: false,
                invulnerable: false,
                may_build: true,
            },
            client_loaded: true,
            changing_level: false,
            sleeping: false,
            rules: GameRules::WORLD,
            difficulty: Difficulty::Normal,
        }
    }

    fn zombie_at(position: Vec3) -> Causer {
        Causer {
            name: None,
            key: EntityId(7),
            kind: entity_types::ZOMBIE as u16,
            position,
            player: None,
            living: true,
        }
    }

    fn player_at(position: Vec3) -> Causer {
        Causer {
            name: None,
            key: EntityId(8),
            kind: entity_types::PLAYER as u16,
            position,
            player: Some(0x1234),
            living: true,
        }
    }

    fn draws() -> Random {
        Random::of_entity(0, EntityId(1), 0)
    }

    fn outcome(
        body: &PlayerBody<'_>,
        source: &DamageSource,
        data: &DamageTypeData,
        amount: f32,
    ) -> PlayerHurt {
        hurt_player(body, source, data, amount, Hurting::default(), &mut draws())
    }

    fn hurt(
        body: &PlayerBody<'_>,
        source: &DamageSource,
        data: &DamageTypeData,
        amount: f32,
    ) -> Hurt {
        hurt_with(body, source, data, amount, Hurting::default())
    }

    fn hurt_with(
        body: &PlayerBody<'_>,
        source: &DamageSource,
        data: &DamageTypeData,
        amount: f32,
        hurting: Hurting<'_>,
    ) -> Hurt {
        match hurt_player(body, source, data, amount, hurting, &mut draws()) {
            PlayerHurt::Hurt(hurt) => *hurt,
            PlayerHurt::Refused => panic!("the hurt was refused"),
        }
    }

    fn enchanted(item: i32, enchantments: &[(&str, i32)]) -> ItemStack {
        let mut stack = ItemStack::new(item, 1);
        stack.components.set(Component::Enchantments(
            enchantments
                .iter()
                .map(|(name, level)| ((*name).to_owned(), *level))
                .collect(),
        ));
        stack
    }

    fn shielded(effects: &Effects, ticks: i32) -> PlayerBody<'_> {
        let mut player = body(effects);
        player.using = Some(ItemUse {
            hand: slots::OFF_HAND,
            ticks,
            blocks_attacks: Some(SHIELD),
        });
        player
    }

    #[test]
    fn the_hand_written_data_names_types_the_game_has() {
        for data in [
            MOB_ATTACK,
            GENERIC,
            OUT_OF_WORLD,
            DROWN,
            FALLING_ANVIL,
            FREEZE,
            ON_FIRE,
            EXPLOSION,
            ARROW,
            FALL,
            ENDER_PEARL,
            PLAYER_ATTACK,
            CACTUS,
            THORNS,
        ] {
            assert_eq!(kind(data.name).0, data.name);
            assert!(!data.message_id.is_empty());
        }
    }

    #[test]
    fn easy_scales_a_zombie_s_three_down_to_two_and_a_half() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.difficulty = Difficulty::Easy;
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([1.0, 0.0, 0.0]));
        assert_eq!(hurt(&player, &source, &MOB_ATTACK, 3.0).from_health, 2.5);
        // `min(d/2 + 1, d)`: a hurt of 1 stays 1.
        assert_eq!(hurt(&player, &source, &MOB_ATTACK, 1.0).from_health, 1.0);
        player.difficulty = Difficulty::Hard;
        assert_eq!(hurt(&player, &source, &MOB_ATTACK, 3.0).from_health, 4.5);
        player.difficulty = Difficulty::Peaceful;
        assert_eq!(
            outcome(&player, &source, &MOB_ATTACK, 3.0),
            PlayerHurt::Refused
        );
    }

    #[test]
    fn a_player_s_hit_and_a_hit_of_no_one_are_not_scaled_but_an_explosion_s_always_is() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.difficulty = Difficulty::Easy;
        let by_player = DamageSource::by(DamageType::PLAYER_ATTACK, player_at([1.0, 0.0, 0.0]));
        assert_eq!(
            hurt(&player, &by_player, &PLAYER_ATTACK, 3.0).from_health,
            3.0
        );
        let by_no_one = DamageSource::of(DamageType::MOB_ATTACK);
        assert_eq!(hurt(&player, &by_no_one, &MOB_ATTACK, 3.0).from_health, 3.0);
        let explosion = DamageSource::of(kind("minecraft:explosion"));
        assert_eq!(hurt(&player, &explosion, &EXPLOSION, 3.0).from_health, 2.5);
    }

    #[test]
    fn armour_of_twenty_with_toughness_eight_takes_a_hit_of_ten_down_to_three() {
        let effects = Effects::default();
        let pieces = [
            ItemStack::new(items::DIAMOND_BOOTS, 1),
            ItemStack::new(items::DIAMOND_LEGGINGS, 1),
            ItemStack::new(items::DIAMOND_CHESTPLATE, 1),
            ItemStack::new(items::DIAMOND_HELMET, 1),
        ];
        let mut player = body(&effects);
        player.equipment = [
            None,
            None,
            Some(&pieces[0]),
            Some(&pieces[1]),
            Some(&pieces[2]),
            Some(&pieces[3]),
        ];
        player.armor = 20.0;
        player.toughness = 8.0;
        let source = DamageSource::of(DamageType::MOB_ATTACK);
        let hurt = hurt(&player, &source, &MOB_ATTACK, 10.0);
        // `2 + 8/4 = 4`; `clamp(20 − 10/4, 4, 20) = 17.5`; `10 · (1 − 17.5/25)`.
        assert_eq!(hurt.damage, 10.0 * (1.0 - 17.5_f32 / 25.0));
        assert!((hurt.from_health - 3.0).abs() < 1.0e-5);
        // Each piece worn by `max(1, 10/4)` as an `int`, the feet first.
        assert_eq!(
            hurt.wear,
            vec![
                (slots::FEET, 2),
                (slots::LEGS, 2),
                (slots::CHEST, 2),
                (slots::HEAD, 2)
            ]
        );
    }

    #[test]
    fn an_elytra_is_not_worn_by_a_hurt_and_a_type_that_bypasses_armour_wears_nothing() {
        let effects = Effects::default();
        let elytra = ItemStack::new(items::ELYTRA, 1);
        let helmet = ItemStack::new(items::DIAMOND_HELMET, 1);
        let mut player = body(&effects);
        player.equipment[usize::from(slots::CHEST)] = Some(&elytra);
        player.equipment[usize::from(slots::HEAD)] = Some(&helmet);
        let source = DamageSource::of(DamageType::MOB_ATTACK);
        assert_eq!(
            hurt(&player, &source, &MOB_ATTACK, 2.0).wear,
            vec![(slots::HEAD, 1)]
        );
        let fall = DamageSource::of(DamageType::FALL);
        assert!(hurt(&player, &fall, &FALL, 6.0).wear.is_empty());
    }

    #[test]
    fn each_protection_takes_its_points_off_the_types_it_protects_from() {
        let effects = Effects::default();
        // The enchantment at level IV on the chestplate, the type, and the points.
        let cases = [
            (
                "minecraft:protection",
                DamageType::MOB_ATTACK,
                MOB_ATTACK,
                4.0,
            ),
            (
                "minecraft:fire_protection",
                DamageType::ON_FIRE,
                ON_FIRE,
                8.0,
            ),
            (
                "minecraft:fire_protection",
                DamageType::MOB_ATTACK,
                MOB_ATTACK,
                0.0,
            ),
            (
                "minecraft:blast_protection",
                kind("minecraft:explosion"),
                EXPLOSION,
                8.0,
            ),
            (
                "minecraft:projectile_protection",
                kind("minecraft:arrow"),
                ARROW,
                8.0,
            ),
            ("minecraft:feather_falling", DamageType::FALL, FALL, 12.0),
            (
                "minecraft:feather_falling",
                DamageType::MOB_ATTACK,
                MOB_ATTACK,
                0.0,
            ),
        ];
        for (name, kind, data, points) in cases {
            let chest = enchanted(items::DIAMOND_CHESTPLATE, &[(name, 4)]);
            let mut player = body(&effects);
            player.equipment[usize::from(slots::CHEST)] = Some(&chest);
            let hurt = hurt(&player, &DamageSource::of(kind), &data, 10.0);
            assert_eq!(
                hurt.damage,
                10.0 * (1.0 - points / 25.0),
                "{name} against {}",
                kind.0
            );
        }
    }

    #[test]
    fn a_protection_held_in_the_hand_protects_nothing() {
        let effects = Effects::default();
        let sword = enchanted(items::IRON_SWORD, &[("minecraft:protection", 4)]);
        let mut player = body(&effects);
        player.equipment[usize::from(slots::MAIN_HAND)] = Some(&sword);
        let source = DamageSource::of(DamageType::MOB_ATTACK);
        assert_eq!(hurt(&player, &source, &MOB_ATTACK, 10.0).from_health, 10.0);
    }

    #[test]
    fn protections_together_stop_at_twenty_points() {
        let effects = Effects::default();
        let pieces = [
            enchanted(
                items::DIAMOND_BOOTS,
                &[
                    ("minecraft:protection", 4),
                    ("minecraft:feather_falling", 4),
                ],
            ),
            enchanted(items::DIAMOND_LEGGINGS, &[("minecraft:protection", 4)]),
            enchanted(items::DIAMOND_CHESTPLATE, &[("minecraft:protection", 4)]),
            enchanted(items::DIAMOND_HELMET, &[("minecraft:protection", 4)]),
        ];
        let mut player = body(&effects);
        for (slot, piece) in [slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD]
            .into_iter()
            .zip(&pieces)
        {
            player.equipment[usize::from(slot)] = Some(piece);
        }
        // 16 of protection and 12 of feather falling: 28, of which 20 count.
        let fall = DamageSource::of(DamageType::FALL);
        // What reached the health, before the health's own rounding.
        assert_eq!(
            hurt(&player, &fall, &FALL, 10.0).damage,
            10.0 * (1.0 - 20.0_f32 / 25.0)
        );
        // The 16 of protection alone against another type.
        let generic = DamageSource::of(kind("minecraft:generic"));
        assert_eq!(
            hurt(&player, &generic, &GENERIC, 10.0).damage,
            10.0 * (1.0 - 16.0_f32 / 25.0)
        );
    }

    #[test]
    fn resistance_two_takes_two_fifths_off() {
        let mut effects = Effects::default();
        effects.add(Effect::RESISTANCE, Layer::new(1, 200, 0), 0);
        let player = body(&effects);
        let generic = DamageSource::of(kind("minecraft:generic"));
        // `max(10 · (25 − 10) / 25, 0)`.
        assert_eq!(hurt(&player, &generic, &GENERIC, 10.0).from_health, 6.0);
        // The void bypasses resistance.
        let void = DamageSource::of(DamageType::OUT_OF_WORLD);
        assert_eq!(hurt(&player, &void, &OUT_OF_WORLD, 10.0).from_health, 10.0);
    }

    #[test]
    fn absorption_is_taken_before_the_health() {
        let effects = Effects {
            absorption: 4.0,
            ..Effects::default()
        };
        let mut player = body(&effects);
        player.max_absorption = 4.0;
        let generic = DamageSource::of(kind("minecraft:generic"));
        let hurt = hurt(&player, &generic, &GENERIC, 6.0);
        assert_eq!(
            (
                hurt.from_absorption,
                hurt.absorption,
                hurt.from_health,
                hurt.health
            ),
            (4.0, 0.0, 2.0, 18.0)
        );
        assert_eq!(hurt.damage, 2.0);
        assert!(hurt.game_event);
    }

    #[test]
    fn exhaustion_is_not_added_where_absorption_took_all_of_the_hurt() {
        let effects = Effects {
            absorption: 4.0,
            ..Effects::default()
        };
        let mut player = body(&effects);
        player.max_absorption = 4.0;
        let source = DamageSource::of(DamageType::MOB_ATTACK);
        let all_absorbed = hurt(&player, &source, &MOB_ATTACK, 3.0);
        assert_eq!(
            (
                all_absorbed.exhaustion,
                all_absorbed.from_health,
                all_absorbed.absorption
            ),
            (0.0, 0.0, 1.0)
        );
        assert!(all_absorbed.took);
        assert!(!all_absorbed.game_event);
        // The full hit is still told.
        assert!(all_absorbed.damage_event);
        let none_held = Effects::default();
        let player = body(&none_held);
        assert_eq!(hurt(&player, &source, &MOB_ATTACK, 3.0).exhaustion, 0.1);
    }

    #[test]
    fn within_the_cooldown_a_hit_takes_only_what_it_exceeds_the_last_by() {
        let effects = Effects::default();
        let generic = DamageSource::of(kind("minecraft:generic"));
        let first = hurt(&body(&effects), &generic, &GENERIC, 6.0);
        assert_eq!(first.branch, Branch::Full);
        assert_eq!((first.damage_cooldown, first.last_hurt), (20, 6.0));
        assert_eq!(first.health, 14.0);
        let mut player = body(&effects);
        player.health = first.health;
        player.damage_cooldown = 19;
        player.last_hurt = first.last_hurt;
        // A hit of 4 after one of 6 takes nothing, and does not take.
        let softer = hurt(&player, &generic, &GENERIC, 4.0);
        assert_eq!(softer.branch, Branch::Ignored);
        assert!(!softer.took);
        assert_eq!((softer.health, softer.last_hurt), (14.0, 6.0));
        // One of 8 takes 2, with no event, sound, `markHurt` or push.
        let harder = hurt(&player, &generic, &GENERIC, 8.0);
        assert_eq!(harder.branch, Branch::Excess);
        assert!(harder.took);
        assert_eq!(
            (harder.from_health, harder.health, harder.last_hurt),
            (2.0, 12.0, 8.0)
        );
        assert_eq!(harder.damage_cooldown, 19);
        assert!(!harder.damage_event);
        assert_eq!(harder.hurt_sound, None);
        assert!(!harder.mark_hurt);
        assert_eq!(harder.hurt_direction, None);
        // At 10 ticks left the cooldown no longer holds (`> 10`).
        player.damage_cooldown = 10;
        let again = hurt(&player, &generic, &GENERIC, 4.0);
        assert_eq!(again.branch, Branch::Full);
        assert_eq!(again.from_health, 4.0);
    }

    #[test]
    fn a_full_hit_by_a_zombie_is_told_sounded_marked_and_pushes() {
        let effects = Effects::default();
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([1.0, 0.0, 0.0]));
        let hurt = hurt(&body(&effects), &source, &MOB_ATTACK, 3.0);
        assert!(hurt.took && hurt.damage_event && hurt.mark_hurt);
        assert_eq!(hurt.hurt_sound, Some("minecraft:entity.player.hurt"));
        assert!(!hurt.death_sound && !hurt.thorns_sound && !hurt.died);
        assert_eq!(hurt.blame_mob, Some(EntityId(7)));
        assert_eq!(hurt.blame_player, None);
        // Pushed away from the zombie, along −x; the hurt told from the east, which is
        // `atan2(0, 1)` against a yaw of 0.
        assert_eq!(hurt.motion, [-STRENGTH, 0.4, 0.0]);
        assert_eq!(hurt.hurt_direction, Some(0.0));
    }

    #[test]
    fn knockback_starts_from_the_motion_an_earlier_hit_left() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.motion = [0.2, -0.0784, 0.0];
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([1.0, 0.0, 0.0]));
        let on_ground = hurt(&player, &source, &MOB_ATTACK, 3.0);
        // `old/2 − dir · 0.4f` sideways; on the ground `min(0.4, old.y/2 + 0.4f)`.
        assert_eq!(
            on_ground.motion,
            [
                0.2 / 2.0 - STRENGTH,
                (-0.0784 / 2.0 + STRENGTH).min(0.4),
                0.0
            ]
        );
        // In the air the vertical motion is kept.
        player.on_ground = false;
        assert_eq!(hurt(&player, &source, &MOB_ATTACK, 3.0).motion[1], -0.0784);
        // A knockback resistance of 1 pushes nothing, but the motion is still sent.
        player.knockback_resistance = 1.0;
        let resisted = hurt(&player, &source, &MOB_ATTACK, 3.0);
        assert_eq!(resisted.motion, player.motion);
        assert!(resisted.mark_hurt);
    }

    #[test]
    fn drowning_marks_nothing_and_pushes_nothing() {
        let effects = Effects::default();
        let source = DamageSource::of(DamageType::DROWN);
        let hurt = hurt(&body(&effects), &source, &DROWN, 2.0);
        assert!(hurt.damage_event);
        assert!(!hurt.mark_hurt);
        assert_eq!(hurt.motion, [0.0; 3]);
        assert_eq!(hurt.hurt_direction, None);
        assert_eq!(hurt.hurt_sound, Some("minecraft:entity.player.hurt_drown"));
        assert_eq!(hurt.from_health, 2.0);
    }

    #[test]
    fn a_falling_anvil_takes_a_quarter_off_for_a_helmet_and_wears_it_twice() {
        let effects = Effects::default();
        let helmet = ItemStack::new(items::IRON_HELMET, 1);
        let mut player = body(&effects);
        player.equipment[usize::from(slots::HEAD)] = Some(&helmet);
        let anvil = DamageSource::of(kind("minecraft:falling_anvil"));
        let helmeted = hurt(&player, &anvil, &FALLING_ANVIL, 10.0);
        assert_eq!(helmeted.from_health, 7.5);
        // `hurtHelmet` by the 10 (2), then the armour's wear by the 7.5 (1).
        assert_eq!(helmeted.wear, vec![(slots::HEAD, 2), (slots::HEAD, 1)]);
        let bare = body(&effects);
        assert_eq!(hurt(&bare, &anvil, &FALLING_ANVIL, 10.0).from_health, 10.0);
    }

    #[test]
    fn freezing_hurts_a_strider_five_times_over_and_a_player_once() {
        assert_eq!(
            freeze_multiplier(entity_types::STRIDER, DamageType::FREEZE),
            5.0
        );
        assert_eq!(
            freeze_multiplier(entity_types::STRIDER, DamageType::FALL),
            1.0
        );
        assert_eq!(
            freeze_multiplier(entity_types::PLAYER, DamageType::FREEZE),
            1.0
        );
        let effects = Effects::default();
        let source = DamageSource::of(DamageType::FREEZE);
        let hurt = hurt(&body(&effects), &source, &FREEZE, 1.0);
        assert_eq!(hurt.from_health, 1.0);
        assert_eq!(hurt.hurt_sound, Some("minecraft:entity.player.hurt_freeze"));
    }

    #[test]
    fn a_shield_blocks_a_zombie_in_front_and_not_behind() {
        let effects = Effects::default();
        let player = shielded(&effects, 20);
        let in_front = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([0.0, 0.0, 2.0]));
        let blocked = hurt(&player, &in_front, &MOB_ATTACK, 3.0);
        assert!(!blocked.took);
        assert_eq!(blocked.health, 20.0);
        let shield = blocked.shield.clone().expect("the shield blocks");
        assert_eq!((shield.hand, shield.blocked), (slots::OFF_HAND, 3.0));
        // At the threshold of 3: `floor(1 + 1 · 3)`.
        assert_eq!(shield.wear, 4);
        assert_eq!(shield.block_sound, Some("minecraft:item.shield.block"));
        assert_eq!((shield.disabled_for, shield.disable_sound), (None, None));
        // `onBlocked` in place of the damage event; nothing marked or pushed.
        assert!(!blocked.damage_event);
        assert!(!blocked.mark_hurt);
        assert_eq!(blocked.motion, [0.0; 3]);
        assert_eq!(blocked.hurt_direction, None);
        // `@434-446` sounds the full hit even so, and the cooldown begins.
        assert_eq!(blocked.hurt_sound, Some("minecraft:entity.player.hurt"));
        assert_eq!((blocked.damage_cooldown, blocked.last_hurt), (20, 0.0));
        let behind = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([0.0, 0.0, -2.0]));
        let taken = hurt(&player, &behind, &MOB_ATTACK, 3.0);
        assert_eq!(taken.shield, None);
        assert_eq!(taken.from_health, 3.0);
        assert!(taken.damage_event);
    }

    #[test]
    fn a_shield_blocks_nothing_in_its_first_five_ticks() {
        let effects = Effects::default();
        let in_front = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([0.0, 0.0, 2.0]));
        let raised = hurt(&shielded(&effects, 4), &in_front, &MOB_ATTACK, 3.0);
        assert_eq!(raised.shield, None);
        assert_eq!(raised.from_health, 3.0);
        let ready = hurt(&shielded(&effects, 5), &in_front, &MOB_ATTACK, 3.0);
        assert!(ready.shield.is_some());
        assert_eq!(ready.from_health, 0.0);
    }

    #[test]
    fn a_shield_does_not_block_a_type_that_bypasses_shields_nor_a_piercing_arrow() {
        let effects = Effects::default();
        let player = shielded(&effects, 20);
        let cactus = DamageSource {
            position: Some([0.0, 0.0, 1.0]),
            ..DamageSource::of(kind("minecraft:cactus"))
        };
        let pricked = hurt(&player, &cactus, &CACTUS, 1.0);
        assert_eq!(pricked.shield, None);
        assert_eq!(pricked.from_health, 1.0);
        let arrow = Causer {
            living: false,
            kind: entity_types::ARROW as u16,
            ..zombie_at([0.0, 1.0, 2.0])
        };
        let shot = DamageSource {
            kind: kind("minecraft:arrow"),
            ..DamageSource::thrown(arrow, [0.0, 0.0, -1.0], None)
        };
        let piercing = Hurting {
            weapon: None,
            pierce_level: 1,
        };
        assert_eq!(
            hurt_with(&player, &shot, &ARROW, 4.0, piercing).from_health,
            4.0
        );
        // Without piercing it is blocked, and an arrow does not disable the shield.
        let stopped = hurt(&player, &shot, &ARROW, 4.0);
        assert_eq!(stopped.from_health, 0.0);
        assert_eq!(stopped.shield.map(|shield| shield.disabled_for), Some(None));
    }

    #[test]
    fn an_axe_disables_a_shield_it_meets_for_five_seconds() {
        assert_eq!(
            ItemComponents::of(items::IRON_AXE)
                .weapon
                .map(|weapon| weapon.disable_blocking_for_seconds),
            Some(5.0)
        );
        let effects = Effects::default();
        let player = shielded(&effects, 20);
        let axe = ItemStack::new(items::IRON_AXE, 1);
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([0.0, 0.0, 2.0]));
        let with_axe = Hurting {
            weapon: Some(&axe),
            pierce_level: 0,
        };
        let chopped = hurt_with(&player, &source, &MOB_ATTACK, 3.0, with_axe);
        let shield = chopped.shield.expect("the shield blocks");
        assert_eq!(shield.disabled_for, Some(100));
        assert_eq!(shield.disable_sound, Some("minecraft:item.shield.break"));
        // A sword's 0 seconds disable nothing.
        let sword = ItemStack::new(items::IRON_SWORD, 1);
        let with_sword = Hurting {
            weapon: Some(&sword),
            pierce_level: 0,
        };
        let parried = hurt_with(&player, &source, &MOB_ATTACK, 3.0, with_sword);
        assert_eq!(parried.shield.and_then(|shield| shield.disabled_for), None);
    }

    #[test]
    fn a_partial_block_pushes_the_player_and_lets_the_rest_through() {
        // A reduction of a fixed point, which leaves 2 of a hit of 3.
        const PARTIAL: BlocksAttacks = BlocksAttacks {
            damage_reductions: &[DamageReduction {
                horizontal_blocking_angle: 90.0,
                kinds: None,
                base: 1.0,
                factor: 0.0,
            }],
            ..SHIELD
        };
        let effects = Effects::default();
        let mut player = body(&effects);
        player.using = Some(ItemUse {
            hand: slots::MAIN_HAND,
            ticks: 20,
            blocks_attacks: Some(PARTIAL),
        });
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([0.0, 0.0, 2.0]));
        let hurt = hurt(&player, &source, &MOB_ATTACK, 3.0);
        assert!(hurt.took);
        assert_eq!(hurt.from_health, 2.0);
        let shield = hurt.shield.expect("the shield blocks some");
        assert_eq!((shield.blocked, shield.wear), (1.0, 0));
        assert_eq!(shield.block_sound, Some("minecraft:item.shield.block"));
        // `onBlocked` still in place of the event, but marked and pushed, and no hurt
        // direction told (`indicateDamage` is for an unblocked hit).
        assert!(!hurt.damage_event);
        assert!(hurt.mark_hurt);
        assert_eq!(hurt.hurt_direction, None);
        // `blockedByItem` pushes by 0.5 away from `(player − zombie)`, toward the zombie
        // at +z, and up 0.4; then the knockback's 0.4f away from the zombie.
        assert_eq!(hurt.motion, [0.0, 0.4, 0.5 / 2.0 - STRENGTH]);
    }

    #[test]
    fn a_totem_in_the_off_hand_saves_a_player_at_one_health() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.health = 1.0;
        player.death_protection = [None, Some(TOTEM)];
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([1.0, 0.0, 0.0]));
        let saved = hurt(&player, &source, &MOB_ATTACK, 5.0);
        let totem = saved.totem.clone().expect("the totem saves");
        assert_eq!(totem.hand, slots::OFF_HAND);
        assert_eq!(totem.effects, TOTEM);
        assert_eq!((saved.health, saved.from_health), (1.0, 1.0));
        assert!(!saved.died && !saved.death_sound);
        // `@405 ifne 449`: past the hurt sound too.
        assert_eq!(saved.hurt_sound, None);
        assert!(saved.damage_event);
        // The main hand's is used first.
        player.death_protection = [Some(TOTEM), Some(TOTEM)];
        let both = hurt(&player, &source, &MOB_ATTACK, 5.0);
        assert_eq!(both.totem.map(|totem| totem.hand), Some(slots::MAIN_HAND));
        // Nothing saves from the void.
        let void = DamageSource::of(DamageType::OUT_OF_WORLD);
        let fell = hurt(&player, &void, &OUT_OF_WORLD, 4.0);
        assert!(fell.died && fell.death_sound && fell.totem.is_none());
        assert_eq!(fell.health, 0.0);
    }

    #[test]
    fn a_hurt_that_takes_the_last_health_kills_with_the_death_sound() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.health = 2.0;
        let source = DamageSource::by(kind("minecraft:thorns"), zombie_at([1.0, 0.0, 0.0]));
        let killed = hurt(&player, &source, &THORNS, 5.0);
        assert!(killed.died && killed.death_sound && killed.thorns_sound);
        assert_eq!(killed.hurt_sound, None);
        assert_eq!(
            (killed.health, killed.from_health, killed.damage),
            (0.0, 2.0, 5.0)
        );
        // Thorns hurts with the plain hurt's sound.
        assert_eq!(
            hurt_sound(DamageEffects::Thorns),
            "minecraft:entity.player.hurt"
        );
        // A dead player is not hurt again.
        player.health = 0.0;
        assert_eq!(outcome(&player, &source, &THORNS, 5.0), PlayerHurt::Refused);
    }

    #[test]
    fn a_creative_player_is_refused_every_hurt_but_the_void() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.abilities.invulnerable = true;
        player.abilities.instabuild = true;
        let source = DamageSource::by(DamageType::MOB_ATTACK, zombie_at([1.0, 0.0, 0.0]));
        let refused = outcome(&player, &source, &MOB_ATTACK, 3.0);
        assert_eq!(refused, PlayerHurt::Refused);
        assert!(!refused.took());
        let void = DamageSource::of(DamageType::OUT_OF_WORLD);
        let fell = outcome(&player, &void, &OUT_OF_WORLD, 4.0);
        assert!(fell.took());
        let PlayerHurt::Hurt(fell) = fell else {
            panic!("the void hurts");
        };
        assert_eq!(fell.from_health, 4.0);
        // `causeFoodExhaustion` adds nothing while invulnerable.
        assert_eq!(fell.exhaustion, 0.0);
    }

    #[test]
    fn a_hit_by_a_player_is_refused_with_pvp_off() {
        let effects = Effects::default();
        let mut player = body(&effects);
        player.rules.pvp = false;
        let source = DamageSource::by(DamageType::PLAYER_ATTACK, player_at([1.0, 0.0, 0.0]));
        assert_eq!(
            outcome(&player, &source, &PLAYER_ATTACK, 3.0),
            PlayerHurt::Refused
        );
        player.rules.pvp = true;
        let hit = hurt(&player, &source, &PLAYER_ATTACK, 3.0);
        assert_eq!(hit.from_health, 3.0);
        assert_eq!(hit.blame_player, Some(0x1234));
        assert_eq!(hit.blame_mob, Some(EntityId(8)));
    }

    #[test]
    fn the_damage_rules_loading_and_changing_level_refuse_a_hurt() {
        let effects = Effects::default();
        let fall = DamageSource::of(DamageType::FALL);
        let mut player = body(&effects);
        player.rules.fall_damage = false;
        assert_eq!(outcome(&player, &fall, &FALL, 4.0), PlayerHurt::Refused);
        let mut player = body(&effects);
        player.rules.fire_damage = false;
        let burning = DamageSource::of(DamageType::ON_FIRE);
        assert_eq!(
            outcome(&player, &burning, &ON_FIRE, 1.0),
            PlayerHurt::Refused
        );
        let mut player = body(&effects);
        player.client_loaded = false;
        assert_eq!(outcome(&player, &fall, &FALL, 4.0), PlayerHurt::Refused);
        let mut player = body(&effects);
        player.changing_level = true;
        assert_eq!(outcome(&player, &fall, &FALL, 4.0), PlayerHurt::Refused);
        // An ender pearl's 5 still hurts a player changing level.
        let pearl = DamageSource::of(kind("minecraft:ender_pearl"));
        assert_eq!(hurt(&player, &pearl, &ENDER_PEARL, 5.0).from_health, 5.0);
    }

    #[test]
    fn fire_resistance_refuses_a_fire_s_hurt_and_a_sleeper_wakes() {
        let mut effects = Effects::default();
        effects.add(Effect::FIRE_RESISTANCE, Layer::new(0, 200, 0), 0);
        let player = body(&effects);
        let burning = DamageSource::of(DamageType::ON_FIRE);
        assert_eq!(
            outcome(&player, &burning, &ON_FIRE, 1.0),
            PlayerHurt::Refused
        );
        let none = Effects::default();
        let mut sleeper = body(&none);
        sleeper.sleeping = true;
        let woken = hurt(&sleeper, &burning, &ON_FIRE, 1.0);
        assert!(woken.woke);
        assert_eq!(
            woken.hurt_sound,
            Some("minecraft:entity.player.hurt_on_fire")
        );
    }

    #[test]
    fn a_type_without_data_reads_as_the_game_s_defaults() {
        let data = type_data(DamageType("minecraft:no_such_type"));
        assert_eq!(
            (data.exhaustion, data.scaling, data.effects),
            (0.0, DamageScaling::Never, DamageEffects::Hurt)
        );
        assert_eq!(data.message_id, "generic");
    }
}
