//! A mob's effects (`LivingEntity.activeEffects`, `MobEffectInstance`; the plan's
//! section 10, "Mob effects, the few N7k needs"): the effects it has, each with its
//! amplifier and when it ends, and the few whose effect is one rule.
//!
//! An effect's duration is kept as the moment it ends ([`super::moment`]), not as a
//! count written every tick (ADR-0028, section 3): an effect added at the moment `m` for
//! `d` ticks has `end - now` left at the moment `now`, and is gone at the `tickEffects`
//! that comes at its end. An effect added in `aiStep` comes at the next moment, one
//! between ticks or in `baseTick` at the tick's: either way the game's count of `d`
//! calls of `tickEffects` ends it at that moment. Infinite effects have no end.
//!
//! The effects whose rule is built: fire resistance (no hurt of `#is_fire`, `hurt.rs`),
//! water breathing (no air lost, `Living::base_tick`), swiftness (its speed modifier,
//! added and taken off with it) and invisibility (the shared flags' invisible bit,
//! [`crate::shown::mob_shared_flags`]); instant health and harming heal or hurt at each
//! `tickEffects` while they last ([`instant`]). Every other effect is held, and
//! `hasEffect` answers of it, but it does nothing: slowness, the spider's effects on
//! hard, the husk's hunger, weakness and the rest are named.
//!
//! Named differences: what clients see of effects beyond the invisible flag is not sent
//! (`DATA_EFFECT_PARTICLES`, a list of particle options the edge has no serializer for,
//! and `DATA_EFFECT_AMBIENCE_ID`), so a mob with an effect shows no swirls; an
//! instance's `ambient`, `visible` and `showIcon` are not kept, as only those two values
//! read them; the shared flag is derived from the effects where the game sets it in
//! `updateDataBeforeSync`, so a mob reading another's `isInvisible()` later in the same
//! tick sees the effect a tick sooner; `onEffectUpdated` every 600 ticks marks nothing a
//! mob shows; a type's own `canBeAffected` (a spider's poison, a wither skeleton's
//! wither) is not asked; and the active effects tick in the order of their ids, where
//! the game's is a hash map's.

use clustine_blocks::data::entity_in_tag;
use clustine_data::attributes as ids;
use serde::{Deserialize, Serialize};

use super::attributes::{ADD_MULTIPLIED_TOTAL, SPEED_EFFECT};
use super::hurt::{DamageSource, DamageType};
use super::{Attributes, Modifier, attribute};
use crate::mobs::Mob;
use crate::mobs::part::{Part, body_ids};
use crate::mobs::tick::MobTick;

/// A mob effect, by its id in the game's registry of them (`MobEffects`, in the order
/// it registers them, which is the order of [`NAMES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Effect(pub(crate) u8);

/// The effects' names, by id.
const NAMES: [&str; 40] = [
    "minecraft:speed",
    "minecraft:slowness",
    "minecraft:haste",
    "minecraft:mining_fatigue",
    "minecraft:strength",
    "minecraft:instant_health",
    "minecraft:instant_damage",
    "minecraft:jump_boost",
    "minecraft:nausea",
    "minecraft:regeneration",
    "minecraft:resistance",
    "minecraft:fire_resistance",
    "minecraft:water_breathing",
    "minecraft:invisibility",
    "minecraft:blindness",
    "minecraft:night_vision",
    "minecraft:hunger",
    "minecraft:weakness",
    "minecraft:poison",
    "minecraft:wither",
    "minecraft:health_boost",
    "minecraft:absorption",
    "minecraft:saturation",
    "minecraft:glowing",
    "minecraft:levitation",
    "minecraft:luck",
    "minecraft:unluck",
    "minecraft:slow_falling",
    "minecraft:conduit_power",
    "minecraft:dolphins_grace",
    "minecraft:bad_omen",
    "minecraft:hero_of_the_village",
    "minecraft:darkness",
    "minecraft:trial_omen",
    "minecraft:raid_omen",
    "minecraft:wind_charged",
    "minecraft:weaving",
    "minecraft:oozing",
    "minecraft:infested",
    "minecraft:breath_of_the_nautilus",
];

impl Effect {
    /// Swiftness.
    pub(crate) const SPEED: Self = Self(0);
    pub(crate) const INSTANT_HEALTH: Self = Self(5);
    pub(crate) const INSTANT_DAMAGE: Self = Self(6);
    pub(crate) const REGENERATION: Self = Self(9);
    pub(crate) const FIRE_RESISTANCE: Self = Self(11);
    pub(crate) const WATER_BREATHING: Self = Self(12);
    pub(crate) const INVISIBILITY: Self = Self(13);
    pub(crate) const POISON: Self = Self(18);
    pub(crate) const CONDUIT_POWER: Self = Self(28);
    pub(crate) const OOZING: Self = Self(37);
    pub(crate) const INFESTED: Self = Self(38);
    pub(crate) const BREATH_OF_THE_NAUTILUS: Self = Self(39);

    /// The effect of the name `name` (`minecraft:` and all).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the witch's potions name their effects (N7k)")
    )]
    pub(crate) fn named(name: &str) -> Option<Self> {
        let at = NAMES.iter().position(|known| *known == name)?;
        Some(Self(u8::try_from(at).expect("forty names fit a byte")))
    }

    /// Its name, or none for an id this build does not know.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the witch's potions name their effects (N7k)")
    )]
    pub(crate) fn name(self) -> Option<&'static str> {
        NAMES.get(usize::from(self.0)).copied()
    }
}

/// One level of an effect: its amplifier, and the moment it ends (none for an infinite
/// one, `duration` -1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Layer {
    pub(crate) amplifier: u8,
    pub(crate) end: Option<u32>,
}

impl Layer {
    /// A level of `amplifier` for `duration` ticks from the moment `now`; -1 is for
    /// ever, and a duration below that is none.
    fn new(amplifier: u8, duration: i32, now: u32) -> Self {
        let end = (duration != -1)
            .then(|| now.saturating_add(u32::try_from(duration.max(0)).unwrap_or(0)));
        Self { amplifier, end }
    }

    /// `duration` at the moment `now`: what is left, never below 0 (`mapDuration` stops
    /// a count at 0, a hidden level's too), or -1 for ever.
    fn duration(self, now: u32) -> i64 {
        self.end
            .map_or(-1, |end| (i64::from(end) - i64::from(now)).max(0))
    }

    /// `hasRemainingDuration()` at the moment `now`.
    fn remains(self, now: u32) -> bool {
        self.end.is_none() || self.duration(now) > 0
    }

    /// `isShorterDurationThan(other)` at the moment `now`.
    fn shorter_than(self, other: Self, now: u32) -> bool {
        self.end.is_some() && (self.duration(now) < other.duration(now) || other.end.is_none())
    }
}

/// An effect the mob has (`MobEffectInstance`): the level in force first, then the
/// levels it hides (`hiddenEffect`, and theirs), which come back as it ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Active {
    pub(crate) effect: Effect,
    pub(crate) layers: Vec<Layer>,
}

impl Active {
    /// The level in force.
    fn top(&self) -> Layer {
        self.layers[0]
    }
}

/// The effects a mob has, ascending by effect. A part of its own, so that a record
/// written before it reads as one without effects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Effects {
    pub(crate) active: Vec<Active>,
}

impl Part for Effects {
    const ID: u16 = body_ids::EFFECTS;
    const VERSION: u8 = 1;
}

impl Effects {
    fn at(&self, effect: Effect) -> Result<usize, usize> {
        self.active
            .binary_search_by_key(&effect, |active| active.effect)
    }

    /// `hasEffect(effect)`. An effect is taken off in the `tickEffects` at its end, so
    /// that it is had until then whatever the moment of the asking.
    pub(crate) fn has(&self, effect: Effect) -> bool {
        self.at(effect).is_ok()
    }

    /// The amplifier of `effect` in force, if the mob has it (`getEffect`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the witch and the trader read their effects (N7k)"
        )
    )]
    pub(crate) fn amplifier(&self, effect: Effect) -> Option<u8> {
        self.at(effect)
            .ok()
            .map(|at| self.active[at].top().amplifier)
    }

    /// `MobEffectUtil.hasWaterBreathing`: under water it loses no air.
    pub(crate) fn water_breathing(&self) -> bool {
        self.has(Effect::WATER_BREATHING)
            || self.has(Effect::CONDUIT_POWER)
            || self.has(Effect::BREATH_OF_THE_NAUTILUS)
    }

    /// `MobEffectUtil.shouldEffectsRefillAirsupply`: what does not lose air under water
    /// gets it back there, unless the nautilus's breath alone keeps it.
    pub(crate) fn refill_air(&self) -> bool {
        !self.has(Effect::BREATH_OF_THE_NAUTILUS)
            || self.has(Effect::WATER_BREATHING)
            || self.has(Effect::CONDUIT_POWER)
    }

    /// The shared flag of being invisible, as `updateInvisibilityStatus` leaves it: set
    /// while it has invisibility, clear otherwise (no effects at all clears it too).
    pub(crate) fn invisible(&self) -> bool {
        self.has(Effect::INVISIBILITY)
    }
}

/// `canBeAffected(instance)`: the types of a tag are spared its effect.
fn can_be_affected(kind: u16, effect: Effect) -> bool {
    let kind = i32::from(kind);
    if entity_in_tag(kind, "minecraft:immune_to_infested") {
        return effect != Effect::INFESTED;
    }
    if entity_in_tag(kind, "minecraft:immune_to_oozing") {
        return effect != Effect::OOZING;
    }
    if entity_in_tag(kind, "minecraft:ignores_poison_and_regen") {
        return effect != Effect::REGENERATION && effect != Effect::POISON;
    }
    true
}

/// `MobEffect.addAttributeModifiers(attributes, amplifier)`: swiftness's speed, its
/// own taken off first. `AttributeTemplate.create(level)` is not dumped; its amount
/// times one more than the level is the game's (**Inferred**, and the plan's "+20 % a
/// level").
fn add_modifiers(attributes: &mut Attributes, kind: u16, effect: Effect, amplifier: u8) {
    if effect != Effect::SPEED {
        return;
    }
    if let Some(instance) = attributes.instance(kind, attribute(ids::MOVEMENT_SPEED)) {
        instance.remove_modifier(SPEED_EFFECT);
        instance.add_modifier(Modifier {
            id: SPEED_EFFECT.to_owned(),
            amount: f64::from(0.2_f32) * f64::from(i32::from(amplifier) + 1),
            operation: ADD_MULTIPLIED_TOTAL,
        });
    }
}

/// `MobEffect.removeAttributeModifiers(attributes)`.
fn remove_modifiers(attributes: &mut Attributes, kind: u16, effect: Effect) {
    if effect != Effect::SPEED {
        return;
    }
    if let Some(instance) = attributes.instance(kind, attribute(ids::MOVEMENT_SPEED)) {
        instance.remove_modifier(SPEED_EFFECT);
    }
}

/// `MobEffectInstance.update(other)` of the level `at` of `layers` and below it: a
/// stronger effect takes over, and hides the one it took over from if it is the shorter;
/// a longer one of the same level lengthens it; a longer weaker one waits hidden below
/// (or updates the one hidden there). Whether the level in force or its length changed.
fn update(layers: &mut Vec<Layer>, at: usize, new: Layer, now: u32) -> bool {
    let this = layers[at];
    if new.amplifier > this.amplifier {
        if new.shorter_than(this, now) {
            layers.insert(at + 1, this);
        }
        layers[at] = new;
        true
    } else if this.shorter_than(new, now) {
        if new.amplifier == this.amplifier {
            layers[at].end = new.end;
            true
        } else {
            if layers.len() == at + 1 {
                layers.push(new);
            } else {
                update(layers, at + 1, new, now);
            }
            false
        }
    } else {
        false
    }
}

/// `addEffect(new MobEffectInstance(effect, duration, amplifier))` on the mob `mob` of
/// the type `kind` at the moment `now`: a new effect is added with its modifiers
/// (`onEffectAdded`), one it has is updated as [`update`] says and its modifiers made
/// again where that changed it (`onEffectUpdated`). Whether anything changed; nothing
/// for an effect its type is spared.
pub(crate) fn add(
    mob: &mut Mob,
    kind: u16,
    effect: Effect,
    amplifier: u8,
    duration: i32,
    now: u32,
) -> bool {
    if !can_be_affected(kind, effect) {
        return false;
    }
    let new = Layer::new(amplifier, duration, now);
    let body = &mut mob.body;
    match body.effects.at(effect) {
        Err(at) => {
            body.effects.active.insert(
                at,
                Active {
                    effect,
                    layers: vec![new],
                },
            );
            add_modifiers(&mut body.attributes, kind, effect, amplifier);
            true
        }
        Ok(at) => {
            let active = &mut body.effects.active[at];
            if !update(&mut active.layers, 0, new, now) {
                return false;
            }
            let amplifier = active.top().amplifier;
            remove_modifiers(&mut body.attributes, kind, effect);
            add_modifiers(&mut body.attributes, kind, effect, amplifier);
            true
        }
    }
}

/// `removeAllEffects()`: every effect taken off with its modifiers
/// (`onEffectsRemoved`), as milk does. Whether it had any.
pub(crate) fn remove_all(mob: &mut Mob, kind: u16) -> bool {
    let body = &mut mob.body;
    let gone = std::mem::take(&mut body.effects.active);
    for active in &gone {
        remove_modifiers(&mut body.attributes, kind, active.effect);
    }
    !gone.is_empty()
}

/// `HealOrHarmMobEffect.applyEffectTick(level, mob, amplifier)` of instant health
/// (`harm` false) or harming (`harm` true), which is not dumped (**Inferred**): the one
/// that is not the mob's own, as an undead's (`#inverted_healing_and_harm`) is the other
/// way round, heals `4 << amplifier`; the other hurts `6 << amplifier` of `magic`.
pub(crate) fn instant(tick: &mut MobTick<'_>, harm: bool, amplifier: u8) {
    let inverted = entity_in_tag(i32::from(tick.kind), "minecraft:inverted_healing_and_harm");
    // Java's shift, of the amplifier's low five bits.
    let shifted = |by: i32| by.wrapping_shl(u32::from(amplifier));
    if harm == inverted {
        let kind = tick.kind;
        crate::mobs::types::tamable::heal(&mut tick.mob, kind, shifted(4).max(0) as f32);
    } else {
        let source = DamageSource::of(DamageType::MAGIC);
        super::hurt_by(tick, &source, shifted(6) as f32);
    }
}

/// Instant health of `amplifier` on the mob of `tick` at once: `4 << amplifier` back,
/// or as much hurt to an undead ([`instant`]). A potion a mob drinks is added as an
/// effect of one tick instead ([`add`]), healing at the next `tickEffects`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "a splash of healing on a mob (N7k and after)")
)]
pub(crate) fn instant_health(tick: &mut MobTick<'_>, amplifier: u8) {
    instant(tick, false, amplifier);
}

/// `tickEffects()` at the end of `LivingEntity.baseTick`, after the counters (the
/// moment of the tick's `aiStep`): each effect that lasts does its tick (instant health
/// and harming theirs) and counts down; one at its end gives way to the level it hid
/// (`downgradeToHiddenEffect`, its modifiers made again) or is taken off with its
/// modifiers (`onEffectsRemoved`).
pub(crate) fn tick_effects(tick: &mut MobTick<'_>) {
    if tick.mob.body.effects.active.is_empty() {
        return;
    }
    let now = super::moment(tick);
    let kind = tick.kind;
    let mut at = 0;
    while at < tick.mob.body.effects.active.len() {
        let active = &tick.mob.body.effects.active[at];
        let (effect, top) = (active.effect, active.top());
        // `tickServer`: nothing left before the tick (added for no time) ends it.
        if top.remains(now.saturating_sub(1)) {
            // `shouldApplyEffectTickThisTick`: an instantaneous effect's every tick it
            // lasts; the others built do nothing in their tick.
            if effect == Effect::INSTANT_HEALTH || effect == Effect::INSTANT_DAMAGE {
                instant(tick, effect == Effect::INSTANT_DAMAGE, top.amplifier);
            }
            let body = &mut tick.mob.body;
            let active = &mut body.effects.active[at];
            if top.end.is_some() && top.duration(now) == 0 && active.layers.len() > 1 {
                active.layers.remove(0);
                let amplifier = active.top().amplifier;
                remove_modifiers(&mut body.attributes, kind, effect);
                add_modifiers(&mut body.attributes, kind, effect, amplifier);
            }
            if body.effects.active[at].top().remains(now) {
                at += 1;
                continue;
            }
        }
        let body = &mut tick.mob.body;
        body.effects.active.remove(at);
        remove_modifiers(&mut body.attributes, kind, effect);
    }
}
