//! A player's mob effects (`docs/groundwork/n9-plan.md`, section 5.4): kept as a mob's
//! are (`crate::mobs::body::effects`), each level's end in the region's ticks, carried as
//! the ticks each has left wherever the player goes; ticked in the player's body tick
//! (`LivingEntity.baseTick`'s `tickEffects`, `player_sources.rs`), after their air and
//! before their `aiStep`, a dead player's only counted down.
//!
//! Their ticks are the game's (V4, [`Region::player_effect_ticks`]): poison, wither and
//! harming hurt through the player's hurt, which refuses what a creative player is
//! spared; regeneration and instant health heal; hunger costs exhaustion and saturation
//! feeds; absorption's start gives its absorption and its end takes it away
//! (`AbsorptionMobEffect`, `refreshDirtyAttributes`). The movement effects, night
//! vision, invisibility, glowing, water breathing and fire resistance are the client's
//! to show and to move by, and the region reads them where they count (the air, the
//! fall, the hurt). The region's part beyond the ticks is to say what changes: the effects
//! themselves to the player's client (`ServerPlayer.onEffectAdded`, `onEffectUpdated`,
//! `onEffectsRemoved`, every one again when they enter the world), the attributes their
//! modifiers change to that client too (the client moves by its own attributes), and the
//! shared flags of being invisible or glowing to the others.
//!
//! Named differences: an effect's particles (data 10 and 11) are part S12's to report
//! with the edge's writing of them; the attributes are those of the player's entity as
//! the game makes it, its defaults, the effects' modifiers and the equipment's
//! (`worn.rs`).

use clustine_data::{ATTRIBUTES, DEFAULT_ATTRIBUTES, attributes as ids, entity_types};
use clustine_world::PlayerId;

use super::worn::{self, Worn, WornModifier};
use super::{Player, Region};
use crate::api::{AttributeShown, PlayerEvent, RegionEvent, TickOutput};
use crate::mobs::body::effects::{self, Added, Counted, Effect, Effects, Layer};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::player_hurt::Hurting;
use crate::shown::DataShown;

/// `Mth.clamp(float, float, float)`: below the least is the least, else at most the most.
fn clamp(value: f32, least: f32, most: f32) -> f32 {
    if value < least {
        least
    } else {
        value.min(most)
    }
}

/// `LivingEntity.DATA_EFFECT_PARTICLES` and `DATA_EFFECT_AMBIENCE_ID`.
pub(super) const EFFECT_PARTICLES: u8 = 10;
pub(super) const EFFECT_AMBIENCE: u8 = 11;

/// The shared flags' data index (`Entity.DATA_SHARED_FLAGS_ID`).
pub(super) const SHARED_FLAGS: u8 = 0;

/// `AttributeModifier.Operation`'s ids.
const ADD_VALUE: u8 = 0;
const ADD_MULTIPLIED_TOTAL: u8 = 2;

/// The attribute modifiers an effect adds, as `MobEffects` registers them: the
/// attribute, the modifier's id, its amount a level and its operation; the amount of a
/// level `a` is `amount · (a + 1)` (`AttributeTemplate.create`).
const MODIFIERS: [(Effect, i32, &str, f64, u8); 12] = [
    (
        Effect(0),
        ids::MOVEMENT_SPEED,
        "minecraft:effect.speed",
        0.200_000_002_980_232_24,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect(1),
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
        Effect(4),
        ids::ATTACK_DAMAGE,
        "minecraft:effect.strength",
        3.0,
        ADD_VALUE,
    ),
    (
        Effect(7),
        ids::SAFE_FALL_DISTANCE,
        "minecraft:effect.jump_boost",
        1.0,
        ADD_VALUE,
    ),
    (
        Effect(13),
        ids::WAYPOINT_TRANSMIT_RANGE,
        "minecraft:effect.waypoint_transmit_range_hide",
        -1.0,
        ADD_MULTIPLIED_TOTAL,
    ),
    (
        Effect(17),
        ids::ATTACK_DAMAGE,
        "minecraft:effect.weakness",
        -4.0,
        ADD_VALUE,
    ),
    (
        Effect(20),
        ids::MAX_HEALTH,
        "minecraft:effect.health_boost",
        4.0,
        ADD_VALUE,
    ),
    (
        Effect(21),
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

/// A player's base value of the attribute `attribute`: the player's default, else the
/// attribute's own.
fn base(attribute: i32) -> f64 {
    let kind = usize::try_from(entity_types::PLAYER).expect("a type id");
    DEFAULT_ATTRIBUTES[kind]
        .and_then(|values| values.iter().find(|(id, _)| i32::from(*id) == attribute))
        .map(|(_, value)| *value)
        .or_else(|| {
            usize::try_from(attribute)
                .ok()
                .and_then(|at| ATTRIBUTES.get(at))
                .map(|attribute| attribute.default)
        })
        .unwrap_or(0.0)
}

/// The attributes `effect`'s modifiers touch.
fn touched(effect: Effect) -> impl Iterator<Item = i32> {
    MODIFIERS
        .iter()
        .filter(move |(of, ..)| *of == effect)
        .map(|(_, attribute, ..)| *attribute)
}

/// The attribute `attribute` of a player with `effects` and the equipment's modifiers
/// `worn` (`worn.rs`), as their client is sent it.
fn attribute_of(effects: &Effects, worn: &[WornModifier], attribute: i32) -> AttributeShown {
    let mut modifiers: Vec<(String, f64, u8)> = MODIFIERS
        .iter()
        .filter(|(_, of, ..)| *of == attribute)
        .filter_map(|(effect, _, id, amount, operation)| {
            let level = effects
                .active
                .iter()
                .find(|active| active.effect == *effect)?;
            let amplifier = f64::from(level.top().amplifier) + 1.0;
            Some(((*id).to_owned(), amount * amplifier, *operation))
        })
        .collect();
    modifiers.extend(
        worn.iter()
            .filter(|modifier| i32::from(modifier.attribute) == attribute)
            .map(|modifier| (modifier.id.clone(), modifier.amount, modifier.operation)),
    );
    AttributeShown {
        attribute: u16::try_from(attribute).expect("attribute ids fit 16 bits"),
        base: base(attribute),
        modifiers,
    }
}

impl Player {
    /// Their attribute `attribute` as their client is sent it: their base, their
    /// effects' modifiers and their equipment's at the last look.
    pub(super) fn attribute_shown(&self, attribute: i32) -> AttributeShown {
        attribute_of(&self.effects, &self.worn.0, attribute)
    }
}

/// The instance of `effect` in `effects` as the client is sent it, at the moment `now`.
fn instance(
    effects: &Effects,
    effect: Effect,
    now: u64,
) -> Option<clustine_world::components::EffectInstance> {
    effects::instances(effects, now)
        .into_iter()
        .find(|instance| Some(instance.effect.as_str()) == effect.name())
}

impl Region {
    /// `addEffect` on the player `id` of `effect` at the level `layer` (its end in the
    /// region's ticks): whether anything changed. Their client is told, and the others
    /// where it changes what they see.
    pub(crate) fn add_player_effect(
        &mut self,
        id: PlayerId,
        effect: Effect,
        layer: Layer,
        output: &mut TickOutput,
    ) -> bool {
        let now = self.tick;
        let Some(player) = self.players.get_mut(&id) else {
            return false;
        };
        let before = effects::particles(&player.effects);
        let added = player.effects.add(effect, layer, now);
        if added == Added::Nothing {
            // `addEffect @84-86`: the level's `onEffectStarted` whatever came of it.
            self.player_effect_started(id, effect, layer.amplifier);
            return false;
        }
        self.journal.players.insert(id);
        self.effect_changed(id, effect, added == Added::New, output);
        self.show_particles(id, before, output);
        if added == Added::Updated {
            self.refresh_player_attributes(id);
        }
        self.player_effect_started(id, effect, layer.amplifier);
        true
    }

    /// `MobEffect.onEffectStarted(player, amplifier)`: absorption's gives the player at
    /// least `4 · (amplifier + 1)` of absorption (`AbsorptionMobEffect.onEffectStarted
    /// @6-20`), within their most (`setAbsorptionAmount`'s clamp); every other effect's
    /// does nothing.
    fn player_effect_started(&mut self, id: PlayerId, effect: Effect, amplifier: u8) {
        if effect != Effect::ABSORPTION {
            return;
        }
        let most = self.player_attribute(id, ids::MAX_ABSORPTION) as f32;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let given = (4 * (i32::from(amplifier) + 1)) as f32;
        player.effects.absorption = clamp(player.effects.absorption.max(given), 0.0, most);
        self.journal.players.insert(id);
    }

    /// `refreshDirtyAttributes()` after an effect's modifiers changed (`onEffectUpdated`,
    /// `onEffectsRemoved`): health above a lowered most (health boost's end) is lowered
    /// to it, and so is the absorption (absorption's end, `onAttributeUpdated`).
    fn refresh_player_attributes(&mut self, id: PlayerId) {
        let most_health = self.player_attribute(id, ids::MAX_HEALTH) as f32;
        let most_absorption = self.player_attribute(id, ids::MAX_ABSORPTION) as f32;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.health > most_health {
            player.health = most_health;
        }
        if player.effects.absorption > most_absorption {
            player.effects.absorption = clamp(most_absorption, 0.0, most_absorption);
        }
    }

    /// `removeEffect` of `effect` from the player `id`: whether they had it.
    pub(crate) fn remove_player_effect(
        &mut self,
        id: PlayerId,
        effect: Effect,
        output: &mut TickOutput,
    ) -> bool {
        let Some(player) = self.players.get_mut(&id) else {
            return false;
        };
        let before = effects::particles(&player.effects);
        if !player.effects.remove(effect) {
            return false;
        }
        self.journal.players.insert(id);
        self.effect_changed(id, effect, false, output);
        self.show_particles(id, before, output);
        self.refresh_player_attributes(id);
        true
    }

    /// The omens' ticks of every player, and the count down of a dead player's effects,
    /// in the order of the players. A living player's effects do their ticks and count
    /// down in their body's tick, between their air and their `aiStep`
    /// ([`Region::player_effect_ticks`], `player_sources.rs`).
    pub(crate) fn tick_player_effects(&mut self, output: &mut TickOutput) {
        self.tick_omens(output);
        let ids: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| player.dead && !player.effects.active.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.count_down_player_effects(id, output);
        }
    }

    /// `LivingEntity.tickEffects @9-122` of the player `id`, in their body's tick: each
    /// effect that had time left does its tick where it applies this tick
    /// ([`effects::applies_this_tick`], with the duration it had before the count down,
    /// and the player's age for an endless one), and is taken off where its tick refuses
    /// it (absorption spent); then they count down, each that gives way to the level it
    /// hid or ends told as an addition does it. The ticks (`applyEffectTick` of each):
    ///
    /// - poison: `magic` of 1 while the health is above 1 (`PoisonMobEffect @0-22`);
    /// - wither: `wither` of 1 (`WitherMobEffect @0-13`);
    /// - regeneration: 1 healed below the most (`RegenerationMobEffect @0-14`);
    /// - instant health and harming, where one was given as an effect: `4 << amplifier`
    ///   healed, or `magic` of `6 << amplifier` (`HealOrHarmMobEffect @0-44`; a player is
    ///   not undead);
    /// - hunger: exhaustion of `0.005 · (amplifier + 1)` (`HungerMobEffect @0-22`);
    /// - saturation: `FoodData.eat(amplifier + 1, 1.0)` (`SaturationMobEffect @0-22`);
    /// - absorption: stays while there is absorption left (`AbsorptionMobEffect @0-14`).
    ///
    /// Named difference: an endless effect's period counts from the player's age as the
    /// region has it (`game_origin`), which a hand-over keeps but a rejoin starts afresh.
    pub(crate) fn player_effect_ticks(&mut self, id: PlayerId, output: &mut TickOutput) {
        let now = self.tick;
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.effects.active.is_empty() {
            return;
        }
        let age = i64::try_from(now).unwrap_or(i64::MAX) - player.game_origin;
        // The game's `tickCount` in the tick, counted at its start.
        let tick_count = i32::try_from(age.wrapping_add(1).max(0)).unwrap_or(i32::MAX);
        let mut refused = Vec::new();
        for (effect, top) in player.effects.due(now) {
            let duration = if top.end.is_none() {
                tick_count
            } else {
                i32::try_from(top.duration(now.saturating_sub(1))).unwrap_or(i32::MAX)
            };
            if effects::applies_this_tick(effect, duration, top.amplifier)
                && !self.apply_player_effect_tick(id, effect, top.amplifier, output)
            {
                refused.push(effect);
            }
        }
        for effect in refused {
            self.remove_player_effect(id, effect, output);
        }
        self.count_down_player_effects(id, output);
    }

    /// `applyEffectTick` of `effect` at `amplifier` on the player `id`: whether it stays.
    fn apply_player_effect_tick(
        &mut self,
        id: PlayerId,
        effect: Effect,
        amplifier: u8,
        output: &mut TickOutput,
    ) -> bool {
        // Java's shift, of the amplifier's low five bits.
        let shifted = |by: i32| by.wrapping_shl(u32::from(amplifier));
        let Some(player) = self.players.get(&id) else {
            return true;
        };
        let health = player.health;
        match effect {
            Effect::POISON => {
                if health > 1.0 {
                    let source = DamageSource::of(DamageType::MAGIC);
                    self.hurt_player(id, &source, 1.0, Hurting::default(), output);
                }
            }
            Effect::WITHER => {
                let source = DamageSource::of(DamageType("minecraft:wither"));
                self.hurt_player(id, &source, 1.0, Hurting::default(), output);
            }
            Effect::REGENERATION => {
                if health < self.player_attribute(id, ids::MAX_HEALTH) as f32 {
                    self.heal_player(id, 1.0);
                }
            }
            Effect::INSTANT_HEALTH => self.heal_player(id, shifted(4).max(0) as f32),
            Effect::INSTANT_DAMAGE => {
                let source = DamageSource::of(DamageType::MAGIC);
                self.hurt_player(id, &source, shifted(6) as f32, Hurting::default(), output);
            }
            Effect::HUNGER => {
                if let Some(player) = self.players.get_mut(&id) {
                    let invulnerable = player.mode.current.abilities().invulnerable;
                    let amount = 0.005 * (f32::from(amplifier) + 1.0);
                    crate::food::cause_exhaustion(&mut player.vitals.food, amount, invulnerable);
                    self.journal.players.insert(id);
                }
            }
            Effect::SATURATION => {
                if let Some(player) = self.players.get_mut(&id) {
                    let nutrition = i32::from(amplifier) + 1;
                    crate::food::eat_scaled(&mut player.vitals.food, nutrition, 1.0);
                    self.journal.players.insert(id);
                }
            }
            Effect::ABSORPTION => {
                return self
                    .players
                    .get(&id)
                    .is_some_and(|player| player.effects.absorption > 0.0);
            }
            _ => {}
        }
        true
    }

    /// `LivingEntity.heal(amount)` of the player `id`: nothing for the dead (health 0),
    /// else `setHealth` of their health and `amount`, within 0 and their most.
    pub(crate) fn heal_player(&mut self, id: PlayerId, amount: f32) {
        let most = self.player_attribute(id, ids::MAX_HEALTH) as f32;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.dead || player.health <= 0.0 {
            return;
        }
        player.health = clamp(player.health + amount, 0.0, most);
        self.journal.players.insert(id);
    }

    /// The count down of the player `id`'s effects at the region's tick: each that gives
    /// way to the level it hid or ends told as an addition does it, and the attributes
    /// refreshed.
    fn count_down_player_effects(&mut self, id: PlayerId, output: &mut TickOutput) {
        let now = self.tick;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let before = effects::particles(&player.effects);
        let counted = player.effects.count_down(now);
        if counted.is_empty() {
            return;
        }
        self.journal.players.insert(id);
        for counted in counted {
            let (Counted::Downgraded(effect) | Counted::Ended(effect)) = counted;
            self.effect_changed(id, effect, false, output);
        }
        self.show_particles(id, before, output);
        self.refresh_player_attributes(id);
    }

    /// The omens' ticks before the effects count down (`BadOmenMobEffect`,
    /// `RaidOmenMobEffect`; `docs/groundwork/n10-dumps.md`, part C, section 2 and
    /// correction 1; `docs/groundwork/n10-plan.md`, section 9.5), in the order of the
    /// effects' ids, bad omen before raid omen:
    ///
    /// - bad omen, where the player stands in a village with no raid at its most omen
    ///   there: raid omen for 600 ticks at its level, and the place kept; and bad omen
    ///   taken off only where raid omen was there already, since the game's removal of it
    ///   in the tick raid omen comes fails (its map changed under its loop), so that it
    ///   goes a tick later, the place kept again then;
    /// - raid omen, in its last tick, with a place kept: the raid starts or grows there,
    ///   the place is forgotten, and it ends. One added in this tick is not ticked in it.
    fn tick_omens(&mut self, output: &mut TickOutput) {
        let now = self.tick;
        let ids: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| {
                player.effects.has(Effect::BAD_OMEN) || player.effects.has(Effect::RAID_OMEN)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let Some(player) = self.players.get(&id) else {
                continue;
            };
            let at = player.pose.position;
            let pos = crate::physics::containing([at.x, at.y, at.z]);
            let bad = player.effects.amplifier(Effect::BAD_OMEN);
            let had_raid_omen = player.effects.has(Effect::RAID_OMEN);
            let mut raid_omen_new = false;
            if let Some(amplifier) = bad {
                let peaceful =
                    clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Peaceful;
                let at_most = matches!(
                    self.get_raid_at(pos),
                    Some(super::raids::RaidAt::Own { id: raid, .. })
                        if self.find_raid(raid).is_some_and(|(_, raid)| raid.omen_level >= 5)
                ) || matches!(
                    self.get_raid_at(pos),
                    Some(super::raids::RaidAt::Seen(raid)) if raid.omen >= 5
                );
                if !peaceful && self.close_to_village(pos, 1) && !at_most {
                    // The raid omen added in the tick before was not counted down in it
                    // (the game's loop abandoned), so a second of the same level finds
                    // it unchanged: nothing is added or sent (`raids_oracle.rs`).
                    let unchanged = had_raid_omen
                        && player.effects.amplifier(Effect::RAID_OMEN) == Some(amplifier)
                        && instance(&player.effects, Effect::RAID_OMEN, now)
                            .is_some_and(|raid_omen| raid_omen.duration == 599);
                    if !unchanged {
                        let layer = crate::mobs::body::effects::Layer::new(amplifier, 600, now);
                        self.add_player_effect(id, Effect::RAID_OMEN, layer, output);
                    }
                    if let Some(player) = self.players.get_mut(&id) {
                        player.raid_omen_position = Some(pos);
                    }
                    if had_raid_omen {
                        self.remove_player_effect(id, Effect::BAD_OMEN, output);
                    } else {
                        raid_omen_new = true;
                    }
                }
            }
            if raid_omen_new {
                continue;
            }
            let Some(player) = self.players.get(&id) else {
                continue;
            };
            let last = instance(&player.effects, Effect::RAID_OMEN, now)
                .is_some_and(|raid_omen| raid_omen.duration == 0);
            if let (true, Some(place), Some(amplifier)) = (
                last,
                player.raid_omen_position,
                player.effects.amplifier(Effect::RAID_OMEN),
            ) {
                self.omen_ended(place, i32::from(amplifier) + 1, output);
                if let Some(player) = self.players.get_mut(&id) {
                    player.raid_omen_position = None;
                }
                self.remove_player_effect(id, Effect::RAID_OMEN, output);
            }
        }
    }

    /// `Raids.createOrExtendRaid(player, place)` of a raid omen of `omen` that ended
    /// (section 9.5): the centre is the mean of the occupied points of `#village` within
    /// 64 of the place, or the place, and the work is the centre's chunk's holder's.
    fn omen_ended(&mut self, place: clustine_world::BlockPos, omen: i32, output: &mut TickOutput) {
        // `can_start_raid` false at the place (the Nether's): no raid, and the omen ends
        // all the same, as `RaidOmenMobEffect` ends it whatever came of it.
        if !self.can_start_raid() {
            return;
        }
        let points = self.pois_in_range(
            place,
            64,
            |kind| kind.is_village(),
            super::pois::Occupancy::IsOccupied,
        );
        let centre = if points.is_empty() {
            place
        } else {
            let count = points.len() as f64;
            let sum = points.iter().fold([0.0f64; 3], |sum, record| {
                [
                    sum[0] + f64::from(record.pos.x),
                    sum[1] + f64::from(record.pos.y),
                    sum[2] + f64::from(record.pos.z),
                ]
            });
            let scale = 1.0 / count;
            crate::physics::containing([sum[0] * scale, sum[1] * scale, sum[2] * scale])
        };
        self.create_or_extend_raid(centre, omen, output);
    }

    /// What a change of the player `id`'s `effect` tells: the effect as it is now (blended
    /// in where `new`) or its end to their client, the attributes it touches, and the
    /// shared flags.
    fn effect_changed(&mut self, id: PlayerId, effect: Effect, new: bool, output: &mut TickOutput) {
        let now = self.tick;
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(name) = effect.name() else {
            return;
        };
        let event = match instance(&player.effects, effect, now) {
            Some(instance) => PlayerEvent::EffectAdded {
                instance,
                blend: new,
            },
            None => PlayerEvent::EffectRemoved {
                effect: name.to_owned(),
            },
        };
        output.player_events.push((id, event));
        let attributes: Vec<AttributeShown> = touched(effect)
            .map(|attribute| player.attribute_shown(attribute))
            .collect();
        if !attributes.is_empty() {
            output
                .player_events
                .push((id, PlayerEvent::Attributes(attributes)));
        }
        if effect == Effect::INVISIBILITY || effect == Effect::GLOWING {
            self.show_shared_flags(id, output);
        }
    }

    /// The player `id`'s effect particles as the others are shown them
    /// (`updateSynchronizedMobEffectParticles`: `DATA_EFFECT_PARTICLES`, 10, and
    /// `DATA_EFFECT_AMBIENCE_ID`, 11) after their effects changed from showing `before`.
    /// Only a value that changed is sent, as `SynchedEntityData.set` marks a value dirty
    /// only where it differs: the official server never sends a player's ambience while
    /// it stays false, its default.
    fn show_particles(
        &self,
        id: PlayerId,
        before: (Vec<crate::shown::ShownParticle>, bool),
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let (particles, ambient) = effects::particles(&player.effects);
        let mut values = Vec::new();
        if particles != before.0 {
            values.push((EFFECT_PARTICLES, DataShown::Particles(particles)));
        }
        if ambient != before.1 {
            values.push((EFFECT_AMBIENCE, DataShown::Boolean(ambient)));
        }
        if values.is_empty() {
            return;
        }
        output.events.push(RegionEvent::PlayerData {
            entity: player.entity_id,
            chunk: player.chunk(),
            values,
        });
    }

    /// Everything a client entering the world is told of the player's effects: each of
    /// them, and the attributes they and the player's equipment touch (`worn.rs`), the
    /// equipment looked at afresh.
    pub(crate) fn tell_effects(&mut self, id: PlayerId, output: &mut TickOutput) {
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.worn = Worn(worn::worn_modifiers(&player.equipment()));
        for instance in effects::instances(&player.effects, tick) {
            output.player_events.push((
                id,
                PlayerEvent::EffectAdded {
                    instance,
                    blend: false,
                },
            ));
        }
        let mut attributes: Vec<i32> = player
            .effects
            .active
            .iter()
            .flat_map(|active| touched(active.effect))
            .chain(
                player
                    .worn
                    .0
                    .iter()
                    .filter(|modifier| worn::syncable(modifier.attribute))
                    .map(|modifier| i32::from(modifier.attribute)),
            )
            .collect();
        attributes.sort_unstable();
        attributes.dedup();
        if !attributes.is_empty() {
            let attributes = attributes
                .into_iter()
                .map(|attribute| player.attribute_shown(attribute))
                .collect();
            output
                .player_events
                .push((id, PlayerEvent::Attributes(attributes)));
        }
    }

    /// The player `id`'s shared flags as the others are shown them, whole
    /// (`docs/groundwork/n9-plan.md`, section 18, decision 6; [`super::Player::shared_flags`]).
    pub(crate) fn show_shared_flags(&self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        output.events.push(RegionEvent::PlayerData {
            entity: player.entity_id,
            chunk: player.chunk(),
            values: vec![(SHARED_FLAGS, DataShown::Byte(player.shared_flags()))],
        });
    }
}
