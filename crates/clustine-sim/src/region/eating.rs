//! A player's eating and drinking (`docs/groundwork/n9-plan.md`, section 5.5 and
//! decision 5 of section 18; part S1's): the mouthfuls while the use lasts
//! (`Consumable.emitParticlesAndSounds`), and what the meal or the drink does at its end
//! (`Consumable.onConsume`: the last mouthful, the stack's listeners, then every
//! `on_consume_effects` entry of the data in order, the `eat` or `drink` game event),
//! with the cooldown it leaves (`applyAfterUseComponentSideEffects`). The use itself is
//! `using.rs`', the data's reading `crate::items::consumables`.
//!
//! Every draw is the player's (`getRandom()`), from their stream aside
//! (`Random::of_entity_aside`), in the game's order: a mouthful's three for its sound and
//! four a crumb, the food's sound and burp, one for each `apply_effects` entry whether or
//! not it gives anything, and the chorus fruit's teleport's (`teleports.rs`).
//!
//! What a creative player, as every player is, feels of it: nothing is used up and no
//! remainder is given (`hasInfiniteMaterials`); the food fills nothing their client
//! shows; instant health, harming and saturation from a potion heal, hurt and feed them
//! not at all; every other effect is theirs as anyone's (`player_effects.rs`).
//!
//! Named differences: the crumbs are not shown, as the game's server shows none
//! (`addParticle` on a server level), but the eater's own client throws them as it
//! predicts the meal; the entity event that tells the eater's client the use is complete
//! (`ServerPlayer.completeUsingItem`'s event 9) is `using.rs`' to send and is not sent;
//! the food's hunger and saturation are not kept, as nothing of a creative player's food
//! is; the order of the stack's listeners is the potion's, the stew's, the ominous
//! bottle's, then the food's, where the game's is its component map's (no item has more
//! than the food and one other, and the other draws nothing).

use clustine_data::{ConsumeEffect, UseAnimation, sounds};
use clustine_world::PlayerId;
use clustine_world::components::EffectInstance;

use super::thrown::{all_effects, duration_scale_of, is_instantaneous, potion_of};
use super::vibrations::{Context, Source, Vibration};
use super::{Random, Region};
use crate::api::{RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput, Using};
use crate::items::consumables::{self, Mouthful};
use crate::mobs::body::effects::{self, Effect, Layer};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

/// The crumbs of a mouthful while the use lasts, and of the last.
const CRUMBS_A_MOUTHFUL: u32 = 5;
const CRUMBS_AT_THE_END: u32 = 16;

/// `MobEffectInstance.withScaledDuration(scale)`: an endless or ended effect as it is,
/// any other at `max(floor(duration · scale), 1)`.
fn scaled(mut instance: EffectInstance, scale: f32) -> EffectInstance {
    if instance.duration != -1 && instance.duration != 0 {
        instance.duration = ((instance.duration as f32 * scale).floor() as i32).max(1);
    }
    instance
}

impl Region {
    /// `Consumable`'s part of `onUseTick` of the player `id`'s meal: crumbs and a
    /// mouthful's sound where `shouldEmitParticlesAndSounds(remaining)` holds, the
    /// remaining ticks as the use had them before this tick's count down.
    pub(crate) fn consume_tick(&mut self, id: PlayerId, using: Using, output: &mut TickOutput) {
        let Some(stack) = self.in_hand(id, using.hand) else {
            return;
        };
        let Some(consumable) = consumables::consumable_of(&stack) else {
            return;
        };
        if !consumables::should_emit(&consumable, using.remaining) {
            return;
        }
        let Some(mut random) = self.player_stream(id) else {
            return;
        };
        let start = random.clone();
        let mouthful = consumables::mouthful(&mut random, &consumable, CRUMBS_A_MOUTHFUL);
        self.player_drew(id, &random, &start);
        self.play_mouthful(id, mouthful, output);
    }

    /// `finishUsingItem` of the player `id`'s meal: `Consumable.onConsume` (the last
    /// mouthful, the listeners, the `on_consume_effects`, the game event, nothing used up
    /// by a creative player), then the stack's cooldown.
    pub(crate) fn finish_consuming(&mut self, id: PlayerId, using: Using, output: &mut TickOutput) {
        let Some(stack) = self.in_hand(id, using.hand) else {
            return;
        };
        let Some(consumable) = consumables::consumable_of(&stack) else {
            return;
        };
        let Some(mut random) = self.player_stream(id) else {
            return;
        };
        let mut start = random.clone();
        let mouthful = consumables::mouthful(&mut random, &consumable, CRUMBS_AT_THE_END);
        self.play_mouthful(id, mouthful, output);
        // The listeners: the potion's contents (`PotionContents.applyToLivingEntity` of
        // the stack's duration scale), the stew's and the ominous bottle's effects.
        let contents = potion_of(&stack);
        let scale = duration_scale_of(&stack);
        for instance in all_effects(&contents) {
            // Instant health, harming and saturation heal, hurt and feed a creative
            // player not at all (`applyInstantaneousEffect`).
            if is_instantaneous(&instance.effect) {
                continue;
            }
            self.give_player(id, &scaled(instance, scale), output);
        }
        for instance in consumables::listener_effects(&stack) {
            self.give_player(id, &instance, output);
        }
        // `FoodProperties.onConsume`: the meal heard by everyone near, and the burp.
        if consumables::food_of(&stack).is_some() {
            let (eaten, burp) = consumables::food_pitches(&mut random, true);
            if let Some(sound) = consumables::sound_named(consumable.sound) {
                self.sound_at_player(id, sound, SOUND_SOURCE_NEUTRAL, 1.0, eaten, output);
            }
            if let Some(burp) = burp {
                let sound = sounds::ENTITY_PLAYER_BURP;
                self.sound_at_player(id, sound, SOUND_SOURCE_PLAYERS, 0.5, burp, output);
            }
        }
        for effect in consumables::on_consume_effects(&consumable) {
            match *effect {
                ConsumeEffect::ApplyEffects {
                    effects: given,
                    probability,
                } => {
                    // The draw is made whether or not anything is given.
                    if random.next_float() >= probability {
                        continue;
                    }
                    for instance in given {
                        self.give_player(id, &consumables::instance_of(instance), output);
                    }
                }
                ConsumeEffect::RemoveEffects(set) => {
                    for effect in consumables::effects_of(set) {
                        self.remove_player_effect(id, effect, output);
                    }
                }
                ConsumeEffect::ClearAllEffects => {
                    let held: Vec<Effect> = self.players.get(&id).map_or_else(Vec::new, |player| {
                        player
                            .effects
                            .active
                            .iter()
                            .map(|active| active.effect)
                            .collect()
                    });
                    for effect in held {
                        self.remove_player_effect(id, effect, output);
                    }
                }
                ConsumeEffect::TeleportRandomly { diameter, .. } => {
                    // The teleport draws from the same stream, on from here, and what
                    // follows from it on from the teleport's.
                    self.player_drew(id, &random, &start);
                    if let Some(mut teleport) = self.player_stream(id) {
                        let from = teleport.clone();
                        self.chorus_teleport(id, f64::from(diameter), &mut teleport, output);
                        self.player_drew(id, &teleport, &from);
                    }
                    let Some(again) = self.player_stream(id) else {
                        return;
                    };
                    random = again;
                    start = random.clone();
                }
                ConsumeEffect::PlaySound(name) => {
                    // `playSound(null, blockPosition, …)`: at the middle of their block.
                    if let Some(sound) = consumables::sound_named(name) {
                        self.sound_at_block(id, sound, output);
                    }
                }
            }
        }
        self.player_drew(id, &random, &start);
        self.after_consumed(id, &stack, &consumable, output);
    }

    /// The end of `onConsume` and of `finishUsingItem`: `drink` or `eat` heard where the
    /// player is now, one of the stack used up (nothing for a creative player, and so no
    /// remainder: `consumables::eaten`), and the stack's cooldown started.
    fn after_consumed(
        &mut self,
        id: PlayerId,
        stack: &clustine_world::ItemStack,
        consumable: &clustine_data::Consumable,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let feet = player.pose.position;
        let entity = player.entity_id;
        let event = if consumable.animation == UseAnimation::Drink {
            "minecraft:drink"
        } else {
            "minecraft:eat"
        };
        if let Some(vibration) = Vibration::named(event) {
            let context = Context::of(Some(Source::of(entity)), None);
            self.vibrate(vibration, [feet.x, feet.y, feet.z], context);
        }
        // `ItemStack.consume` and the `use_remainder`: every player is creative
        // (`hasInfiniteMaterials`), so the stack is left as it was and nothing is given
        // (`consumables::eaten` of infinite materials).
        if let Some(ticks) = consumables::cooldown_ticks(stack) {
            self.add_cooldown(id, stack, ticks, output);
        }
    }

    /// `addEffect` of `instance` on the player `id` (`player_effects.rs`), refused where
    /// the player is spared it, with bad omen's sound where it is new
    /// (`MobEffect.onEffectAdded`'s `soundOnAdded`, heard by everyone near).
    fn give_player(&mut self, id: PlayerId, instance: &EffectInstance, output: &mut TickOutput) {
        let Some(effect) = Effect::named(&instance.effect) else {
            return;
        };
        let player_kind = super::mob_bodies::player_kind();
        if !effects::can_be_affected(player_kind, effect) {
            return;
        }
        let had = self
            .players
            .get(&id)
            .is_some_and(|player| player.effects.has(effect));
        let layer = Layer {
            ambient: instance.ambient,
            visible: instance.visible,
            show_icon: instance.show_icon,
            ..Layer::new(
                u8::try_from(instance.amplifier.clamp(0, 255)).unwrap_or(0),
                instance.duration,
                self.tick,
            )
        };
        let added = self.add_player_effect(id, effect, layer, output);
        if added && !had && effect == Effect::BAD_OMEN {
            let sound = sounds::EVENT_MOB_EFFECT_BAD_OMEN;
            self.sound_at_player(id, sound, SOUND_SOURCE_PLAYERS, 1.0, 1.0, output);
        }
    }

    /// The player `id`'s stream aside, on from what it has drawn in this tick.
    fn player_stream(&self, id: PlayerId) -> Option<Random> {
        let key = self.players.get(&id)?.entity_id;
        Some(self.entity_stream(key).random)
    }

    /// Notes what `random`, taken from the player `id`'s stream as `start`, has drawn.
    fn player_drew(&mut self, id: PlayerId, random: &Random, start: &Random) {
        if let Some(key) = self.players.get(&id).map(|player| player.entity_id) {
            self.entity_drew(key, random.draws_since(start));
        }
    }

    /// A mouthful's sound (`Player.playSound`: at their feet, of the players' source,
    /// heard by everyone near but them, whose client plays its own).
    fn play_mouthful(&mut self, id: PlayerId, mouthful: Mouthful, output: &mut TickOutput) {
        let Some(sound) = mouthful.sound else {
            return;
        };
        let Some(player) = self.players.get(&id) else {
            return;
        };
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_PLAYERS,
            position: player.pose.position,
            volume: mouthful.volume,
            pitch: mouthful.pitch,
            except: Some(id),
        });
    }

    /// `level.playSound(null, x, y, z, …)` at the player `id`'s feet, heard by everyone
    /// near, them too.
    fn sound_at_player(
        &mut self,
        id: PlayerId,
        sound: i32,
        source: i32,
        volume: f32,
        pitch: f32,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        output.events.push(RegionEvent::Sound {
            sound,
            source,
            position: player.pose.position,
            volume,
            pitch,
            except: None,
        });
    }

    /// `PlaySoundConsumeEffect.apply`: `playSound(null, blockPosition, sound, PLAYERS, 1,
    /// 1)`, at the middle of the block the player `id` stands in.
    fn sound_at_block(&mut self, id: PlayerId, sound: i32, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let feet = player.pose.position;
        let block = crate::physics::containing([feet.x, feet.y, feet.z]);
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_PLAYERS,
            position: clustine_world::Vec3::new(
                f64::from(block.x) + 0.5,
                f64::from(block.y) + 0.5,
                f64::from(block.z) + 0.5,
            ),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }
}

#[cfg(test)]
#[path = "eating_n9_tests.rs"]
mod n9_tests;
