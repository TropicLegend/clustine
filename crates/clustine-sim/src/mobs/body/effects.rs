//! A mob's effects (`LivingEntity.activeEffects`, `MobEffectInstance`; the plan's
//! section 10 of N7k and section 5.2 of `docs/groundwork/n9-plan.md`): the effects it
//! has, each with its amplifier and when it ends, the rule of each, which types are
//! spared which, and what clients are shown of them.
//!
//! An effect's duration is kept as the moment it ends ([`super::moment`]), not as a
//! count written every tick (ADR-0028, section 3): an effect added at the moment `m` for
//! `d` ticks has `end - now` left at the moment `now`, and is gone at the `tickEffects`
//! that comes at its end. An effect added in `aiStep` comes at the next moment, one
//! between ticks or in `baseTick` at the tick's: either way the game's count of `d`
//! calls of `tickEffects` ends it at that moment. Infinite effects have no end.
//!
//! The rules (`tickEffects`, each effect's `applyEffectTick` where its
//! `shouldApplyEffectTickThisTick` holds of the duration left before the count down):
//! poison hurts 1 of `magic` every `25 >> amplifier` ticks while the mob has more than 1
//! health, wither 1 of `wither` every `40 >> amplifier` ticks, regeneration heals 1
//! every `50 >> amplifier` ticks below the most; instant health and harming heal or hurt
//! at once, the other way round for the undead ([`instant`]); absorption gives its
//! amount as it starts and ends when that is spent. The attribute modifiers
//! ([`MODIFIERS`]: swiftness, slowness, haste, mining fatigue, strength, weakness, jump
//! boost's safe fall, health boost, absorption's most, luck and its opposite,
//! invisibility's waypoint range) are made as an effect starts or changes and taken off
//! as it ends. Resistance and absorption take their share of a hurt
//! ([`magic_absorbed`], [`absorbed`], from `hurt.rs`); jump boost, slow falling and
//! levitation are read by the jump and the travel ([`jump_boost_power`],
//! [`slow_falling`], [`levitation`]); fire resistance, water breathing, invisibility and
//! glowing as before; hunger and saturation are a player's alone. What a mob's effects
//! do at its death and its hurt (oozing, weaving, wind charged, infested) is the
//! region's (`region/effect_deaths.rs`).
//!
//! Clients are shown the effects' particles (`DATA_EFFECT_PARTICLES`, index 10) and
//! whether they are all ambient (`DATA_EFFECT_AMBIENCE_ID`, index 11) through
//! [`crate::shown::mob_values`] ([`particles`]), and glowing and invisibility in the
//! shared flags ([`crate::shown::mob_shared_flags`]).
//!
//! Named differences: the shared flags and the particles are derived from the effects
//! where the game sets them in `updateDirtyEffects`, so a mob reading another's
//! `isInvisible()` later in the same tick sees the effect a tick sooner; when the last
//! effect goes, index 11 is shown as false where the game leaves it as it was (only an
//! ambient effect, a beacon's, would make it true, and no mob is given one);
//! `onEffectUpdated` every 600 ticks marks nothing a mob shows; an effect's sound as it
//! is added (the omens') is not played for a mob, as nothing gives a mob one; the active
//! effects tick in the order of their ids, where the game's is a hash map's.
//! `AbsorptionMobEffect` is not dumped: its rules (the amount on start, the end when it
//! is spent, every tick) are **Inferred** from the game's source as known.

use clustine_blocks::data::entity_in_tag;
use clustine_data::{attributes as ids, entity_types};
use clustine_world::components::EffectInstance;
use serde::{Deserialize, Serialize};

use super::attributes::{ADD_MULTIPLIED_TOTAL, ADD_VALUE, SPEED_EFFECT};
use super::hurt::{DamageSource, DamageType};
use super::{Attributes, Modifier, attribute};
use crate::mobs::Mob;
use crate::mobs::part::{Part, PartError, body_ids};
use crate::mobs::tick::MobTick;
use crate::shown::ShownParticle;

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
    pub(crate) const SLOWNESS: Self = Self(1);
    pub(crate) const STRENGTH: Self = Self(4);
    pub(crate) const INSTANT_HEALTH: Self = Self(5);
    pub(crate) const INSTANT_DAMAGE: Self = Self(6);
    pub(crate) const JUMP_BOOST: Self = Self(7);
    pub(crate) const REGENERATION: Self = Self(9);
    pub(crate) const RESISTANCE: Self = Self(10);
    pub(crate) const FIRE_RESISTANCE: Self = Self(11);
    pub(crate) const WATER_BREATHING: Self = Self(12);
    pub(crate) const INVISIBILITY: Self = Self(13);
    pub(crate) const HUNGER: Self = Self(16);
    pub(crate) const WEAKNESS: Self = Self(17);
    pub(crate) const POISON: Self = Self(18);
    pub(crate) const WITHER: Self = Self(19);
    pub(crate) const HEALTH_BOOST: Self = Self(20);
    pub(crate) const ABSORPTION: Self = Self(21);
    pub(crate) const SATURATION: Self = Self(22);
    /// Glowing, the 24th the game registers (23 from 0; N9d had it as levitation's 24).
    pub(crate) const GLOWING: Self = Self(23);
    pub(crate) const LEVITATION: Self = Self(24);
    pub(crate) const SLOW_FALLING: Self = Self(27);
    pub(crate) const CONDUIT_POWER: Self = Self(28);
    pub(crate) const BAD_OMEN: Self = Self(30);
    pub(crate) const TRIAL_OMEN: Self = Self(33);
    pub(crate) const RAID_OMEN: Self = Self(34);
    pub(crate) const WIND_CHARGED: Self = Self(35);
    pub(crate) const WEAVING: Self = Self(36);
    pub(crate) const OOZING: Self = Self(37);
    pub(crate) const INFESTED: Self = Self(38);
    pub(crate) const BREATH_OF_THE_NAUTILUS: Self = Self(39);

    /// The effect of the name `name` (`minecraft:` and all).
    pub(crate) fn named(name: &str) -> Option<Self> {
        let at = NAMES.iter().position(|known| *known == name)?;
        Some(Self(u8::try_from(at).expect("forty names fit a byte")))
    }

    /// Its name, or none for an id this build does not know.
    pub(crate) fn name(self) -> Option<&'static str> {
        NAMES.get(usize::from(self.0)).copied()
    }

    /// Its colour (`MobEffect.color`, as `MobEffects` registers it), as RGB.
    pub(crate) fn colour(self) -> Option<i32> {
        COLOURS.get(usize::from(self.0)).copied()
    }

    /// `MobEffect.createParticleOptions(instance)` of a level with `ambient`: the
    /// particle of the four effects of N9 and the two omens that have one of their own,
    /// else `entity_effect` of its colour, fainter where it is ambient (alpha
    /// `floor(38.25f)`, else 255: `ARGB.color(alpha, colour)`).
    pub(crate) fn particle(self, ambient: bool) -> Option<ShownParticle> {
        let own = match self {
            Self::OOZING => Some(particle::ITEM_SLIME),
            Self::WEAVING => Some(particle::ITEM_COBWEB),
            Self::WIND_CHARGED => Some(particle::SMALL_GUST),
            Self::INFESTED => Some(particle::INFESTED),
            Self::TRIAL_OMEN => Some(particle::TRIAL_OMEN),
            Self::RAID_OMEN => Some(particle::RAID_OMEN),
            _ => None,
        };
        if let Some(own) = own {
            return Some(ShownParticle::Simple(own));
        }
        let alpha: i32 = if ambient { AMBIENT_ALPHA } else { 255 };
        let colour = self.colour()?;
        Some(ShownParticle::EntityEffect {
            colour: (alpha << 24) | (colour & 0x00FF_FFFF),
        })
    }
}

/// `MobEffect.AMBIENT_ALPHA`: `Mth.floor(38.25f)`.
const AMBIENT_ALPHA: i32 = 38;

/// The particles the effects show by type, by their ids in the
/// `minecraft:particle_type` registry (the 26.3 report's `registries.json`).
pub(crate) mod particle {
    pub(crate) const SMALL_GUST: i32 = 32;
    pub(crate) const INFESTED: i32 = 40;
    pub(crate) const ITEM_SLIME: i32 = 62;
    pub(crate) const ITEM_COBWEB: i32 = 63;
    pub(crate) const RAID_OMEN: i32 = 123;
    pub(crate) const TRIAL_OMEN: i32 = 124;
}

/// The effects' colours by id, as `MobEffects` registers them (n7k `MobEffects`).
const COLOURS: [i32; 40] = [
    3_402_751, 9_154_528, 14_270_531, 4_866_583, 16_762_624, 16_262_179, 11_101_546, 16_646_020,
    5_578_058, 13_458_603, 9_520_880, 16_750_848, 10_017_472, 16_185_078, 2_039_587, 12_779_366,
    5_797_459, 4_738_376, 8_889_187, 7_561_558, 16_284_963, 2_445_989, 16_262_179, 9_740_385,
    13_565_951, 5_882_118, 12_624_973, 15_978_425, 1_950_417, 8_954_814, 745_784, 4_521_796,
    2_696_993, 1_484_454, 14_565_464, 12_438_015, 7_891_290, 10_092_451, 9_214_860, 65_518,
];

/// The attribute modifiers an effect adds, as `MobEffects` registers them (n7k
/// `MobEffects`): the effect, the attribute, the modifier's id, its amount a level and
/// its operation. The amount of a level `a` is `amount · (a + 1)`
/// (`AttributeTemplate.create`). A player's are told to their client from the same
/// effects (`region/player_effects.rs`, which keeps a table of its own).
pub(crate) const MODIFIERS: [(Effect, i32, &str, f64, u8); 12] = [
    (
        Effect::SPEED,
        ids::MOVEMENT_SPEED,
        SPEED_EFFECT,
        0.200_000_002_980_232_24,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect::SLOWNESS,
        ids::MOVEMENT_SPEED,
        "minecraft:effect.slowness",
        -0.150_000_005_960_464_48,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect(2),
        ids::ATTACK_SPEED,
        "minecraft:effect.haste",
        0.100_000_001_490_116_12,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect(3),
        ids::ATTACK_SPEED,
        "minecraft:effect.mining_fatigue",
        -0.100_000_001_490_116_12,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect::STRENGTH,
        ids::ATTACK_DAMAGE,
        "minecraft:effect.strength",
        3.0,
        ADD_VALUE,
    ),
    (
        Effect::JUMP_BOOST,
        ids::SAFE_FALL_DISTANCE,
        "minecraft:effect.jump_boost",
        1.0,
        ADD_VALUE,
    ),
    (
        Effect::INVISIBILITY,
        ids::WAYPOINT_TRANSMIT_RANGE,
        "minecraft:effect.waypoint_transmit_range_hide",
        -1.0,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect::WEAKNESS,
        ids::ATTACK_DAMAGE,
        "minecraft:effect.weakness",
        -4.0,
        ADD_VALUE,
    ),
    (
        Effect::HEALTH_BOOST,
        ids::MAX_HEALTH,
        "minecraft:effect.health_boost",
        4.0,
        ADD_VALUE,
    ),
    (
        Effect::ABSORPTION,
        ids::MAX_ABSORPTION,
        "minecraft:effect.absorption",
        4.0,
        ADD_VALUE,
    ),
    (
        Effect(25),
        ids::LUCK,
        "minecraft:effect.luck",
        1.0,
        ADD_VALUE,
    ),
    (
        Effect(26),
        ids::LUCK,
        "minecraft:effect.unluck",
        -1.0,
        ADD_VALUE,
    ),
];

/// One level of an effect: its amplifier, the moment it ends (none for an infinite one,
/// `duration` -1), and the flags the game keeps of an instance (`ambient`, from a
/// beacon or a conduit; `visible`, its particles; `showIcon`). A mob's moments are its
/// own (its `lived`), a player's the region's ticks (`docs/groundwork/n9-plan.md`,
/// 5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Layer {
    pub(crate) amplifier: u8,
    pub(crate) end: Option<u64>,
    pub(crate) ambient: bool,
    pub(crate) visible: bool,
    pub(crate) show_icon: bool,
}

/// A level as version 1 of the part kept it, before the flags, which an effect then had
/// as the game's defaults: not ambient, visible, with its icon.
#[derive(Debug, Clone, Copy, Deserialize)]
struct LayerV1 {
    amplifier: u8,
    end: Option<u32>,
}

impl Layer {
    /// A level of `amplifier` for `duration` ticks from the moment `now`, not ambient,
    /// visible and with its icon (`new MobEffectInstance(effect, duration, amplifier)`);
    /// -1 is for ever, and a duration below that is none.
    pub(crate) fn new(amplifier: u8, duration: i32, now: u64) -> Self {
        let end = (duration != -1)
            .then(|| now.saturating_add(u64::try_from(duration.max(0)).unwrap_or(0)));
        Self {
            amplifier,
            end,
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }

    /// `duration` at the moment `now`: what is left, never below 0 (`mapDuration` stops
    /// a count at 0, a hidden level's too), or -1 for ever.
    pub(crate) fn duration(self, now: u64) -> i64 {
        self.end.map_or(-1, |end| {
            i64::try_from(end.saturating_sub(now)).unwrap_or(i64::MAX)
        })
    }

    /// `hasRemainingDuration()` at the moment `now`.
    fn remains(self, now: u64) -> bool {
        self.end.is_none() || self.duration(now) > 0
    }

    /// `isShorterDurationThan(other)` at the moment `now`.
    fn shorter_than(self, other: Self, now: u64) -> bool {
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
    pub(crate) fn top(&self) -> Layer {
        self.layers[0]
    }
}

/// What an addition did to a store of effects (`addEffect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Added {
    /// Spared, or no stronger nor longer than what it has: nothing changed.
    Nothing,
    /// A new effect (`onEffectAdded`).
    New,
    /// The level in force or its length changed (`onEffectUpdated`).
    Updated,
}

/// What the count down of a store did to one of its effects (`tickEffects`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Counted {
    /// Its level in force ended and gave way to the one it hid
    /// (`downgradeToHiddenEffect`, `onEffectUpdated`).
    Downgraded(Effect),
    /// It ended (`onEffectsRemoved`).
    Ended(Effect),
}

/// The effects a mob has, ascending by effect, and the absorption it has
/// (`absorptionAmount`, which absorption's effect gives and a hurt spends first). A part
/// of its own, so that a record written before it reads as one without effects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Effects {
    pub(crate) active: Vec<Active>,
    pub(crate) absorption: f32,
}

impl Part for Effects {
    const ID: u16 = body_ids::EFFECTS;
    const VERSION: u8 = 3;
}

/// The part as its version 1 kept it.
#[derive(Debug, Clone, Deserialize)]
struct EffectsV1 {
    active: Vec<(Effect, Vec<LayerV1>)>,
}

/// The part as its version 2 kept it, before the absorption, which a mob then never had.
#[derive(Debug, Clone, Deserialize)]
struct EffectsV2 {
    active: Vec<Active>,
}

/// The effects of `entries`, from the part of any of its versions.
pub(crate) fn decode(entries: &[clustine_world::Entry]) -> Result<Effects, PartError> {
    let Ok(at) = entries.binary_search_by_key(&Effects::ID, |entry| entry.id) else {
        return Ok(Effects::default());
    };
    let entry = &entries[at];
    let unreadable = || PartError::Unreadable {
        id: entry.id,
        version: entry.version,
    };
    match entry.version {
        1 => {
            let old: EffectsV1 = postcard::from_bytes(&entry.bytes).map_err(|_| unreadable())?;
            Ok(Effects {
                active: old
                    .active
                    .into_iter()
                    .map(|(effect, layers)| Active {
                        effect,
                        layers: layers
                            .into_iter()
                            .map(|layer| Layer {
                                amplifier: layer.amplifier,
                                end: layer.end.map(u64::from),
                                ambient: false,
                                visible: true,
                                show_icon: true,
                            })
                            .collect(),
                    })
                    .collect(),
                absorption: 0.0,
            })
        }
        2 => {
            let old: EffectsV2 = postcard::from_bytes(&entry.bytes).map_err(|_| unreadable())?;
            Ok(Effects {
                active: old.active,
                absorption: 0.0,
            })
        }
        _ => crate::mobs::part::decode(entries),
    }
}

impl Effects {
    fn at(&self, effect: Effect) -> Result<usize, usize> {
        self.active
            .binary_search_by_key(&effect, |active| active.effect)
    }

    /// `addEffect` of `effect` at the level `new` at the moment `now`, as
    /// `MobEffectInstance.update` keeps the levels: what it did. Whoever has the store
    /// makes the effect's modifiers again where it is new or updated.
    pub(crate) fn add(&mut self, effect: Effect, new: Layer, now: u64) -> Added {
        match self.at(effect) {
            Err(at) => {
                self.active.insert(
                    at,
                    Active {
                        effect,
                        layers: vec![new],
                    },
                );
                Added::New
            }
            Ok(at) => {
                if update(&mut self.active[at].layers, 0, new, now) {
                    Added::Updated
                } else {
                    Added::Nothing
                }
            }
        }
    }

    /// `removeEffect(effect)`: whether it had it.
    pub(crate) fn remove(&mut self, effect: Effect) -> bool {
        match self.at(effect) {
            Ok(at) => {
                self.active.remove(at);
                true
            }
            Err(_) => false,
        }
    }

    /// The effects that do their tick in the `tickEffects` at the moment `now`, with
    /// their levels in force: those that had time left before it.
    pub(crate) fn due(&self, now: u64) -> Vec<(Effect, Layer)> {
        self.active
            .iter()
            .map(|active| (active.effect, active.top()))
            .filter(|(_, top)| top.remains(now.saturating_sub(1)))
            .collect()
    }

    /// The count down of `tickEffects` at the moment `now`, after the effects due did
    /// their tick: each level in force at its end gives way to the one it hid, or the
    /// effect ends. What it did, in the order of the effects.
    pub(crate) fn count_down(&mut self, now: u64) -> Vec<Counted> {
        let mut counted = Vec::new();
        let mut at = 0;
        while at < self.active.len() {
            let active = &mut self.active[at];
            let (effect, top) = (active.effect, active.top());
            if top.remains(now.saturating_sub(1)) {
                if top.end.is_some() && top.duration(now) == 0 && active.layers.len() > 1 {
                    active.layers.remove(0);
                    counted.push(Counted::Downgraded(effect));
                }
                if self.active[at].top().remains(now) {
                    at += 1;
                    continue;
                }
            }
            self.active.remove(at);
            counted.push(Counted::Ended(effect));
        }
        counted
    }

    /// `hasEffect(effect)`. An effect is taken off in the `tickEffects` at its end, so
    /// that it is had until then whatever the moment of the asking.
    pub(crate) fn has(&self, effect: Effect) -> bool {
        self.at(effect).is_ok()
    }

    /// The amplifier of `effect` in force, if the mob has it (`getEffect`).
    pub(crate) fn amplifier(&self, effect: Effect) -> Option<u8> {
        self.at(effect)
            .ok()
            .map(|at| self.active[at].top().amplifier)
    }

    /// The amplifier in force of an effect it has.
    fn amplifier_in_force(&self, effect: Effect) -> u8 {
        self.at(effect)
            .map_or(0, |at| self.active[at].top().amplifier)
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

    /// The shared flag of glowing, as `updateGlowingStatus` leaves it
    /// (`isCurrentlyGlowing`: the effect, as no mob is given the glowing tag).
    pub(crate) fn glowing(&self) -> bool {
        self.has(Effect::GLOWING)
    }
}

/// What clients are shown of `effects` (`updateSynchronizedMobEffectParticles`): the
/// particle of each visible effect's level in force, in the order of the effects
/// (`DATA_EFFECT_PARTICLES`), and whether every visible one is ambient
/// (`areAllEffectsAmbient`, `DATA_EFFECT_AMBIENCE_ID`); without effects no particles
/// (`removeEffectParticles`) and false, the value's default.
pub(crate) fn particles(effects: &Effects) -> (Vec<ShownParticle>, bool) {
    if effects.active.is_empty() {
        return (Vec::new(), false);
    }
    let shown = effects
        .active
        .iter()
        .map(|active| (active.effect, active.top()))
        .filter(|(_, top)| top.visible)
        .filter_map(|(effect, top)| effect.particle(top.ambient))
        .collect();
    let all_ambient = effects
        .active
        .iter()
        .map(Active::top)
        .all(|top| !top.visible || top.ambient);
    (shown, all_ambient)
}

/// `getJumpBoostPower()`: a tenth of a block's power a level of jump boost.
pub(crate) fn jump_boost_power(effects: &Effects) -> f32 {
    effects
        .amplifier(Effect::JUMP_BOOST)
        .map_or(0.0, |amplifier| 0.1 * (f32::from(amplifier) + 1.0))
}

/// Whether the mob falls slowly (`hasEffect(SLOW_FALLING)`), which the travel's
/// `getEffectiveGravity` and `aiStep`'s fall read.
pub(crate) fn slow_falling(effects: &Effects) -> bool {
    effects.has(Effect::SLOW_FALLING)
}

/// The amplifier of the levitation the mob has, which `travelInAir` lifts it by in place
/// of gravity.
pub(crate) fn levitation(effects: &Effects) -> Option<u8> {
    effects.amplifier(Effect::LEVITATION)
}

/// `getDamageAfterMagicAbsorb(source, amount)`'s part for effects: nothing taken where
/// the source bypasses effects; resistance's fifth a level
/// (`max(amount · (25 − 5 · (a + 1)) / 25, 0)`) unless it bypasses resistance; then 0
/// where nothing is left. No mob of Clustine wears an enchantment that protects it, so
/// the rest is the amount. A type's own share is taken after this
/// (`MobType::magic_absorbed`, the witch's).
pub(crate) fn magic_absorbed(effects: &Effects, source: &DamageSource, amount: f32) -> f32 {
    if source.kind.is("minecraft:bypasses_effects") {
        return amount;
    }
    let mut amount = amount;
    if let Some(amplifier) = effects.amplifier(Effect::RESISTANCE)
        && !source.kind.is("minecraft:bypasses_resistance")
    {
        let resisted = (i32::from(amplifier) + 1) * 5;
        let left = 25 - resisted;
        let product = amount * left as f32;
        amount = (product / 25.0).max(0.0);
    }
    if amount <= 0.0 {
        return 0.0;
    }
    amount
}

/// `actuallyHurt`'s spending of the absorption of `effects` on a hurt of `amount` that
/// the armour, the effects and the type left: what is left for the health
/// (`max(amount − absorption, 0)`), the absorption lowered by what it took and kept
/// within `0` and `most` (`setAbsorptionAmount`).
pub(crate) fn absorbed(effects: &mut Effects, amount: f32, most: f32) -> f32 {
    let held = effects.absorption;
    let left = (amount - held).max(0.0);
    effects.absorption = clamp(held - (amount - left), 0.0, most);
    left
}

/// `actuallyHurt`'s second `setAbsorptionAmount`, after the health is lowered by
/// `amount`: what is left of it less the amount, which is nothing where the hurt reached
/// the health.
pub(crate) fn absorbed_after(effects: &mut Effects, amount: f32, most: f32) {
    effects.absorption = clamp(effects.absorption - amount, 0.0, most);
}

/// `Mth.clamp(float, float, float)`.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min { min } else { value.min(max) }
}

/// The effects of `effects` as a place or a hand-over carries them at the moment `now`:
/// each level by its effect's name with the ticks it has left (-1 for ever), the
/// levels of an effect one after another from the one in force down.
pub(crate) fn instances(effects: &Effects, now: u64) -> Vec<EffectInstance> {
    effects
        .active
        .iter()
        .filter_map(|active| Some((active.effect.name()?, active)))
        .flat_map(|(name, active)| {
            active.layers.iter().map(move |layer| EffectInstance {
                effect: name.to_owned(),
                amplifier: i32::from(layer.amplifier),
                duration: i32::try_from(layer.duration(now)).unwrap_or(i32::MAX),
                ambient: layer.ambient,
                visible: layer.visible,
                show_icon: layer.show_icon,
            })
        })
        .collect()
}

/// The effects `instances` of a place or a hand-over, counted on from the moment `now`
/// (the inverse of [`instances`]); an effect this build does not know is dropped.
pub(crate) fn from_instances(instances: &[EffectInstance], now: u64) -> Effects {
    let mut effects = Effects::default();
    for instance in instances {
        let Some(effect) = Effect::named(&instance.effect) else {
            continue;
        };
        let layer = Layer {
            ambient: instance.ambient,
            visible: instance.visible,
            show_icon: instance.show_icon,
            ..Layer::new(
                u8::try_from(instance.amplifier.clamp(0, 255)).unwrap_or(0),
                instance.duration,
                now,
            )
        };
        match effects.at(effect) {
            Ok(at) => effects.active[at].layers.push(layer),
            Err(at) => effects.active.insert(
                at,
                Active {
                    effect,
                    layers: vec![layer],
                },
            ),
        }
    }
    effects
}

/// The effects of `effects` as a region's state keeps them, their ends as they are.
pub(crate) fn kept(effects: &Effects) -> Vec<crate::state::KeptEffect> {
    effects
        .active
        .iter()
        .filter_map(|active| Some((active.effect.name()?, active)))
        .flat_map(|(name, active)| {
            active
                .layers
                .iter()
                .map(move |layer| crate::state::KeptEffect {
                    effect: name.to_owned(),
                    amplifier: layer.amplifier,
                    end: layer.end,
                    ambient: layer.ambient,
                    visible: layer.visible,
                    show_icon: layer.show_icon,
                })
        })
        .collect()
}

/// The effects a region's state kept (the inverse of [`kept`]); an effect this build does
/// not know is dropped.
pub(crate) fn from_kept(kept: &[crate::state::KeptEffect]) -> Effects {
    let mut effects = Effects::default();
    for level in kept {
        let Some(effect) = Effect::named(&level.effect) else {
            continue;
        };
        let layer = Layer {
            amplifier: level.amplifier,
            end: level.end,
            ambient: level.ambient,
            visible: level.visible,
            show_icon: level.show_icon,
        };
        match effects.at(effect) {
            Ok(at) => effects.active[at].layers.push(layer),
            Err(at) => effects.active.insert(
                at,
                Active {
                    effect,
                    layers: vec![layer],
                },
            ),
        }
    }
    effects
}

/// `canBeAffected(instance)` of a mob or a player of the type `kind`: the types' own
/// refusals (`Spider` and its cave spider poison, `Parched` weakness, `WitherSkeleton`
/// wither, `AbstractNautilus` and so the nautilus and the zombie nautilus poison: n7k
/// and n7 dumps), then `LivingEntity`'s tags (n9 `LivingEntity` 2372–2449): the
/// silverfish spared infested, the slime oozing, the undead poison and regeneration.
/// A table by type here rather than a hook in each type's file.
pub(crate) fn can_be_affected(kind: u16, effect: Effect) -> bool {
    let kind = i32::from(kind);
    let own = match kind {
        entity_types::SPIDER
        | entity_types::CAVE_SPIDER
        | entity_types::NAUTILUS
        | entity_types::ZOMBIE_NAUTILUS => Some(Effect::POISON),
        entity_types::PARCHED => Some(Effect::WEAKNESS),
        entity_types::WITHER_SKELETON => Some(Effect::WITHER),
        _ => None,
    };
    if own == Some(effect) {
        return false;
    }
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

/// `MobEffect.addAttributeModifiers(attributes, amplifier)`: each of the effect's
/// modifiers ([`MODIFIERS`]) of `amount · (amplifier + 1)` on the instance of its
/// attribute where the type has one, its own of the same id taken off first.
fn add_modifiers(attributes: &mut Attributes, kind: u16, effect: Effect, amplifier: u8) {
    for (of, id, name, amount, operation) in MODIFIERS {
        if of != effect {
            continue;
        }
        if let Some(instance) = attributes.instance(kind, attribute(id)) {
            instance.remove_modifier(name);
            instance.add_modifier(Modifier {
                id: name.to_owned(),
                amount: amount * f64::from(i32::from(amplifier) + 1),
                operation,
            });
        }
    }
}

/// `MobEffect.removeAttributeModifiers(attributes)`.
fn remove_modifiers(attributes: &mut Attributes, kind: u16, effect: Effect) {
    for (of, id, name, ..) in MODIFIERS {
        if of != effect {
            continue;
        }
        if let Some(instance) = attributes.instance(kind, attribute(id)) {
            instance.remove_modifier(name);
        }
    }
}

/// `refreshDirtyAttributes()` after an effect's modifiers changed (`onEffectUpdated`,
/// `onEffectsRemoved`; `onEffectAdded` does not): health above a lowered most is
/// lowered to it, and so is the absorption (`onAttributeUpdated`).
fn refresh_attributes(mob: &mut Mob, kind: u16) {
    let body = &mut mob.body;
    let most = body.attributes.value(kind, attribute(ids::MAX_HEALTH)) as f32;
    if body.vitals.health > most {
        body.vitals.health = most;
    }
    let most = max_absorption(mob, kind);
    if mob.body.effects.absorption > most {
        mob.body.effects.absorption = clamp(most, 0.0, most);
    }
}

/// `getMaxAbsorption()`.
pub(crate) fn max_absorption(mob: &Mob, kind: u16) -> f32 {
    mob.body
        .attributes
        .value(kind, attribute(ids::MAX_ABSORPTION)) as f32
}

/// `MobEffectInstance.update(other)` of the level `at` of `layers` and below it: a
/// stronger effect takes over, and hides the one it took over from if it is the shorter;
/// a longer one of the same level lengthens it; a longer weaker one waits hidden below
/// (or updates the one hidden there). Whether the level in force or its length changed.
fn update(layers: &mut Vec<Layer>, at: usize, new: Layer, now: u64) -> bool {
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
/// the type `kind` at the moment `now`: [`add_layer`] of a level that is not ambient,
/// visible and with its icon.
pub(crate) fn add(
    mob: &mut Mob,
    kind: u16,
    effect: Effect,
    amplifier: u8,
    duration: i32,
    now: u32,
) -> bool {
    let layer = Layer::new(amplifier, duration, u64::from(now));
    add_layer(mob, kind, effect, layer, now)
}

/// `addEffect(instance)` of `effect` at the level `new` on the mob `mob` of the type
/// `kind` at the moment `now`: refused for an effect its type is spared
/// (`canBeAffected`); a new effect is added with its modifiers (`onEffectAdded`), one it
/// has is updated as [`update`] says and its modifiers made again where that changed it
/// (`onEffectUpdated`); then, whatever came of it, the level's `onEffectStarted`
/// (absorption's amount). Whether anything changed.
pub(crate) fn add_layer(mob: &mut Mob, kind: u16, effect: Effect, new: Layer, now: u32) -> bool {
    if !can_be_affected(kind, effect) {
        return false;
    }
    let now = u64::from(now);
    let body = &mut mob.body;
    let changed = match body.effects.add(effect, new, now) {
        Added::Nothing => false,
        Added::New => {
            add_modifiers(&mut body.attributes, kind, effect, new.amplifier);
            true
        }
        Added::Updated => {
            let amplifier = body.effects.amplifier_in_force(effect);
            remove_modifiers(&mut body.attributes, kind, effect);
            add_modifiers(&mut body.attributes, kind, effect, amplifier);
            refresh_attributes(mob, kind);
            true
        }
    };
    on_effect_started(mob, kind, effect, new.amplifier);
    changed
}

/// `MobEffect.onEffectStarted(mob, amplifier)`: absorption's gives the mob at least
/// `4 · (amplifier + 1)` of absorption, within its most (**Inferred**:
/// `AbsorptionMobEffect` is not dumped); every other effect's does nothing.
fn on_effect_started(mob: &mut Mob, kind: u16, effect: Effect, amplifier: u8) {
    if effect != Effect::ABSORPTION {
        return;
    }
    let given = (4 * (i32::from(amplifier) + 1)) as f32;
    let most = max_absorption(mob, kind);
    let held = mob.body.effects.absorption;
    mob.body.effects.absorption = clamp(held.max(given), 0.0, most);
}

/// `removeEffect(effect)` on the mob `mob` of the type `kind`: the effect taken off with
/// its modifiers (`onEffectsRemoved`). Whether it had it.
pub(crate) fn remove(mob: &mut Mob, kind: u16, effect: Effect) -> bool {
    if !mob.body.effects.remove(effect) {
        return false;
    }
    remove_modifiers(&mut mob.body.attributes, kind, effect);
    refresh_attributes(mob, kind);
    true
}

/// `removeAllEffects()`: every effect taken off with its modifiers
/// (`onEffectsRemoved`), as milk does. Whether it had any.
pub(crate) fn remove_all(mob: &mut Mob, kind: u16) -> bool {
    let body = &mut mob.body;
    let gone = std::mem::take(&mut body.effects.active);
    for active in &gone {
        remove_modifiers(&mut body.attributes, kind, active.effect);
    }
    if !gone.is_empty() {
        refresh_attributes(mob, kind);
    }
    !gone.is_empty()
}

/// `HealOrHarmMobEffect.applyEffectTick(level, mob, amplifier)` of instant health
/// (`harm` false) or harming (`harm` true) (n9 `HealOrHarmMobEffect`): the one that is
/// not the mob's own, as an undead's (`isInvertedHealAndHarm`,
/// `#inverted_healing_and_harm`) is the other way round, heals `max(4 << amplifier, 0)`;
/// the other hurts `6 << amplifier` of `magic`.
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

/// `HealOrHarmMobEffect.applyInstantenousEffect` of a splash or a cloud on the mob of
/// `struck`, between its ticks: `(int) (factor · (4 << amplifier) + 0.5)` healed, or as
/// much of `6 << amplifier` hurt by `source` (`indirectMagic(potion, owner)`), the other
/// way round for an undead.
pub(crate) fn instant_with(
    struck: &mut super::hurt::Struck<'_>,
    harm: bool,
    amplifier: u8,
    factor: f64,
    source: &DamageSource,
) {
    let inverted = entity_in_tag(
        i32::from(struck.kind),
        "minecraft:inverted_healing_and_harm",
    );
    let shifted = |by: i32| f64::from(by.wrapping_shl(u32::from(amplifier)));
    if harm == inverted {
        let amount = (factor * shifted(4) + 0.5) as i32;
        let kind = struck.kind;
        crate::mobs::types::tamable::heal(struck.mob, kind, amount.max(0) as f32);
    } else {
        let amount = (factor * shifted(6) + 0.5) as i32;
        super::hurt::hurt_server(struck, source, amount as f32);
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

/// `shouldApplyEffectTickThisTick(duration, amplifier)` of `effect`, with the duration
/// left before the count down (the mob's `tickCount` for an endless one): poison every
/// `25 >> amplifier` ticks, wither every `40 >> amplifier`, regeneration every
/// `50 >> amplifier` (each every tick where the shift leaves nothing), hunger and
/// absorption every tick, the instantaneous ones (instant health, harming, saturation)
/// every tick they last; the others never.
pub(crate) fn applies_this_tick(effect: Effect, duration: i32, amplifier: u8) -> bool {
    let every = |interval: i32| {
        // Java's shift, of the amplifier's low five bits.
        let period = interval.wrapping_shr(u32::from(amplifier));
        period <= 0 || duration % period == 0
    };
    match effect {
        Effect::POISON => every(25),
        Effect::WITHER => every(40),
        Effect::REGENERATION => every(50),
        Effect::HUNGER | Effect::ABSORPTION => true,
        Effect::INSTANT_HEALTH | Effect::INSTANT_DAMAGE | Effect::SATURATION => duration >= 1,
        _ => false,
    }
}

/// `MobEffect.applyEffectTick(level, mob, amplifier)` of `effect` on the mob of `tick`:
/// whether the effect stays (absorption's ends once it is spent, every other's stays).
/// Poison hurts 1 of `magic` while the mob has more than 1 health (n9
/// `PoisonMobEffect`), wither 1 of `wither` at any health (`WitherMobEffect`),
/// regeneration heals 1 below the most (`RegenerationMobEffect`), instant health and
/// harming as [`instant`]; hunger's exhaustion and saturation's food are a player's
/// (`HungerMobEffect`, `SaturationMobEffect`), and nothing else does anything here.
fn apply_effect_tick(tick: &mut MobTick<'_>, effect: Effect, amplifier: u8) -> bool {
    let kind = tick.kind;
    match effect {
        Effect::POISON => {
            if tick.mob.body.vitals.health > 1.0 {
                let source = DamageSource::of(DamageType::MAGIC);
                super::hurt_by(tick, &source, 1.0);
            }
        }
        Effect::WITHER => {
            let source = DamageSource::of(DamageType("minecraft:wither"));
            super::hurt_by(tick, &source, 1.0);
        }
        Effect::REGENERATION => {
            if crate::mobs::types::tamable::hurt(&tick.mob, kind) {
                crate::mobs::types::tamable::heal(&mut tick.mob, kind, 1.0);
            }
        }
        Effect::INSTANT_HEALTH | Effect::INSTANT_DAMAGE => {
            instant(tick, effect == Effect::INSTANT_DAMAGE, amplifier);
        }
        Effect::ABSORPTION => return tick.mob.body.effects.absorption > 0.0,
        _ => {}
    }
    true
}

/// `tickEffects()` at the end of `LivingEntity.baseTick`, after the counters (the
/// moment of the tick's `aiStep`): each effect that lasts (`tickServer`) does its tick
/// where it applies this tick, with the duration it had left before the count down, and
/// is taken off with its modifiers where that refuses it (`onEffectsRemoved`); the rest
/// count down, and one at its end gives way to the level it hid
/// (`downgradeToHiddenEffect`, its modifiers made again) or is taken off.
pub(crate) fn tick_effects(tick: &mut MobTick<'_>) {
    if tick.mob.body.effects.active.is_empty() {
        return;
    }
    let now = u64::from(super::moment(tick));
    let kind = tick.kind;
    // The game's `tickCount` in the tick, counted at its start: an endless effect's
    // duration for its period.
    let tick_count = i32::try_from(tick.tick_count.wrapping_add(1)).unwrap_or(i32::MAX);
    let mut refused = Vec::new();
    for (effect, top) in tick.mob.body.effects.due(now) {
        let duration = if top.end.is_none() {
            tick_count
        } else {
            i32::try_from(top.duration(now.saturating_sub(1))).unwrap_or(i32::MAX)
        };
        if applies_this_tick(effect, duration, top.amplifier)
            && !apply_effect_tick(tick, effect, top.amplifier)
        {
            refused.push(effect);
        }
    }
    for effect in refused {
        remove(&mut tick.mob, kind, effect);
    }
    let counted = tick.mob.body.effects.count_down(now);
    if counted.is_empty() {
        return;
    }
    let body = &mut tick.mob.body;
    for counted in counted {
        match counted {
            Counted::Downgraded(effect) => {
                let amplifier = body.effects.amplifier_in_force(effect);
                remove_modifiers(&mut body.attributes, kind, effect);
                add_modifiers(&mut body.attributes, kind, effect, amplifier);
            }
            Counted::Ended(effect) => remove_modifiers(&mut body.attributes, kind, effect),
        }
    }
    refresh_attributes(&mut tick.mob, kind);
}

#[cfg(test)]
#[path = "effects_n9_tests.rs"]
mod n9_tests;
