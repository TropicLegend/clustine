//! A player's mob effects (`docs/groundwork/n9-plan.md`, section 5.4): kept as a mob's
//! are (`crate::mobs::body::effects`), each level's end in the region's ticks, carried as
//! the ticks each has left wherever the player goes; ticked at the players' part of the
//! region's tick (`LivingEntity.baseTick`'s `tickEffects`), before their use of an item.
//!
//! What a creative player feels of them: every hurt is refused, so poison, wither and
//! harming do nothing; regeneration and instant health heal a full player not at all;
//! hunger's exhaustion is skipped; the movement effects, night vision, invisibility,
//! glowing, water breathing and fire resistance are the client's to show and to move
//! by. So the region's part is to keep them and to say what changes: the effects
//! themselves to the player's client (`ServerPlayer.onEffectAdded`, `onEffectUpdated`,
//! `onEffectsRemoved`, every one again when they enter the world), the attributes their
//! modifiers change to that client too (the client moves by its own attributes), and the
//! shared flags of being invisible or glowing to the others.
//!
//! Named differences: an effect's particles (data 10 and 11) are part S12's to report
//! with the edge's writing of them; the attributes are those of the player's entity as
//! the game makes it, its defaults and the effects' modifiers, since nothing else changes
//! a player's.

use clustine_data::{ATTRIBUTES, DEFAULT_ATTRIBUTES, attributes as ids, entity_types};
use clustine_world::PlayerId;

use super::Region;
use crate::api::{AttributeShown, PlayerEvent, RegionEvent, TickOutput};
use crate::mobs::body::effects::{self, Added, Counted, Effect, Effects, Layer};
use crate::shown::DataShown;

/// `LivingEntity.DATA_EFFECT_PARTICLES` and `DATA_EFFECT_AMBIENCE_ID`.
const EFFECT_PARTICLES: u8 = 10;
const EFFECT_AMBIENCE: u8 = 11;

/// The shared flags' data index, and the bits the effects decide.
const SHARED_FLAGS: u8 = 0;
const CROUCHING: u8 = 1 << 1;
const SPRINTING: u8 = 1 << 3;
const INVISIBLE: u8 = 1 << 5;
const GLOWING: u8 = 1 << 6;

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

/// The attribute `attribute` of a player with `effects`, as their client is sent it.
fn attribute_of(effects: &Effects, attribute: i32) -> AttributeShown {
    let modifiers = MODIFIERS
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
    AttributeShown {
        attribute: u16::try_from(attribute).expect("attribute ids fit 16 bits"),
        base: base(attribute),
        modifiers,
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
            return false;
        }
        self.journal.players.insert(id);
        self.effect_changed(id, effect, added == Added::New, output);
        self.show_particles(id, before, output);
        true
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
        true
    }

    /// `tickEffects` of every player, in the order of the players: their effects do
    /// their ticks (nothing a creative player feels) and count down, each that gives way
    /// to the level it hid or ends told as an addition does it.
    pub(crate) fn tick_player_effects(&mut self, output: &mut TickOutput) {
        self.tick_omens(output);
        let now = self.tick;
        let ids: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| !player.effects.active.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            let before = effects::particles(&player.effects);
            let counted = player.effects.count_down(now);
            if counted.is_empty() {
                continue;
            }
            self.journal.players.insert(id);
            for counted in counted {
                let (Counted::Downgraded(effect) | Counted::Ended(effect)) = counted;
                self.effect_changed(id, effect, false, output);
            }
            self.show_particles(id, before, output);
        }
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
            .map(|attribute| attribute_of(&player.effects, attribute))
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
    /// them, and the attributes they touch.
    pub(crate) fn tell_effects(&self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        for instance in effects::instances(&player.effects, self.tick) {
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
            .collect();
        attributes.sort_unstable();
        attributes.dedup();
        if !attributes.is_empty() {
            let attributes = attributes
                .into_iter()
                .map(|attribute| attribute_of(&player.effects, attribute))
                .collect();
            output
                .player_events
                .push((id, PlayerEvent::Attributes(attributes)));
        }
    }

    /// The player `id`'s shared flags as the others are shown them, whole
    /// (`docs/groundwork/n9-plan.md`, section 18, decision 6): crouching and sprinting by
    /// their keys, invisible and glowing by their effects.
    pub(crate) fn show_shared_flags(&self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let mut flags = 0;
        if player.keys.sneak && !player.flying {
            flags |= CROUCHING;
        }
        if player.keys.sprint {
            flags |= SPRINTING;
        }
        if player.effects.has(Effect::INVISIBILITY) {
            flags |= INVISIBLE;
        }
        if player.effects.has(Effect::GLOWING) {
            flags |= GLOWING;
        }
        output.events.push(RegionEvent::PlayerData {
            entity: player.entity_id,
            chunk: player.chunk(),
            values: vec![(SHARED_FLAGS, DataShown::Byte(flags))],
        });
    }
}
