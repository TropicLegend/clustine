//! A player's hurt in the region (V4, `docs/groundwork/v-plan.md`, section 4.3; the build
//! design's sections 3 and 7): [`crate::player_hurt`] works out what a hurt does to the
//! player as they are, and this applies it: the health and absorption, the cooldown, the
//! shield's wear and disabling, the armour's wear, the totem, the damage event and the
//! sounds, the knock and the shake of the view their client is told, the food the hurt
//! costs, who is blamed, the combat tracker's entry, and the death.

use clustine_data::{ConsumeEffect, ItemComponents, attributes};
use clustine_world::PlayerId;

use super::Region;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{Killer, PlayerEvent, RegionEvent, TickOutput};
use crate::items::consumables;
use crate::mobs::body::hurt::DamageSource;
use crate::player_hurt::{self, Hurt, Hurting, ItemUse, PlayerBody, PlayerHurt};
use crate::state::{CombatEntry, LastHurtBy};

/// How long a mob or a player that hurt a player is remembered
/// (`LivingEntity.setLastHurtByMob`, `setLastHurtByPlayer`: 100 ticks).
const BLAME_TICKS: i64 = 100;

/// The entity event of a totem's save (`LivingEntity.checkTotemDeathProtection`).
const TOTEM_EVENT: u8 = 35;

/// The players' sound source (`SoundSource.PLAYERS`).
const SOUND_SOURCE_PLAYERS: i32 = 7;

/// The inventory slot of each `EquipmentSlot` ordinal a hurt names, but the main hand's,
/// which is the selected hotbar slot: the off hand, the feet, the legs, the chest and the
/// head.
fn inventory_slot(slot: u8, selected: u8) -> usize {
    match slot {
        0 => usize::from(selected),
        1 => clustine_world::OFF_HAND,
        armour => clustine_world::ARMOUR_START + usize::from(armour - 2),
    }
}

impl Region {
    /// `hurtServer` of the region's player `id` by `amount` from `source`, with what
    /// `hurting` adds: worked out by [`player_hurt::hurt_player`] and applied. Whether it
    /// took.
    pub(crate) fn hurt_player(
        &mut self,
        id: PlayerId,
        source: &DamageSource,
        amount: f32,
        hurting: Hurting<'_>,
        output: &mut TickOutput,
    ) -> bool {
        let Some(outcome) = self.work_out_hurt(id, source, amount, hurting) else {
            return false;
        };
        let PlayerHurt::Hurt(hurt) = outcome else {
            return false;
        };
        self.apply_hurt(id, source, &hurt, hurting, output);
        hurt.took
    }

    /// What the hurt does to the player as they are now, the knockback's draws taken from
    /// their stream.
    fn work_out_hurt(
        &mut self,
        id: PlayerId,
        source: &DamageSource,
        amount: f32,
        hurting: Hurting<'_>,
    ) -> Option<PlayerHurt> {
        let armor = self.player_attribute(id, attributes::ARMOR);
        let toughness = self.player_attribute(id, attributes::ARMOR_TOUGHNESS);
        let knockback_resistance = self.player_attribute(id, attributes::KNOCKBACK_RESISTANCE);
        let max_absorption = self.player_attribute(id, attributes::MAX_ABSORPTION) as f32;
        let max_health = self.player_attribute(id, attributes::MAX_HEALTH) as f32;
        let client_loaded = self.client_loaded(id);
        let tick = self.tick_signed();
        let mut random = self.player_stream(id)?;
        let start = random.clone();
        let player = self.players.get(&id)?;
        let held: [Option<crate::api::ItemStack>; 6] = std::array::from_fn(|slot| {
            let slot = u8::try_from(slot).expect("six slots");
            player.inventory.0[inventory_slot(slot, player.selected_slot)].clone()
        });
        let equipment: [Option<&crate::api::ItemStack>; 6] =
            std::array::from_fn(|slot| held[slot].as_ref());
        let protection = |slot: usize| {
            held[slot]
                .as_ref()
                .and_then(|stack| ItemComponents::of(stack.item).death_protection)
        };
        let using = player.using.as_ref().and_then(|using| {
            let hand = match using.hand {
                crate::api::Hand::Main => 0,
                crate::api::Hand::Off => 1,
            };
            let stack = held[usize::from(hand)].as_ref()?;
            let duration = super::using::use_duration(stack)?;
            Some(ItemUse {
                hand,
                ticks: duration - using.remaining,
                blocks_attacks: ItemComponents::of(stack.item).blocks_attacks,
            })
        });
        let feet = player.pose.position;
        let body = PlayerBody {
            health: player.health,
            max_health,
            effects: &player.effects,
            max_absorption,
            equipment,
            death_protection: [protection(0), protection(1)],
            armor,
            toughness,
            knockback_resistance,
            damage_cooldown: player
                .vitals
                .cooldown_until
                .map_or(0, |until| i32::try_from((until - tick).max(0)).unwrap_or(0)),
            last_hurt: player.vitals.last_hurt,
            position: [feet.x, feet.y, feet.z],
            motion: player.vitals.motion,
            on_ground: player.pose.on_ground,
            yaw: player.pose.yaw,
            // A player's head turns with their look (`ServerPlayer`'s `yHeadRot` is set
            // from the move's yaw).
            head_yaw: player.pose.yaw,
            using,
            abilities: player.mode.current.abilities(),
            client_loaded,
            // A change of level hands the player on at once: no hurt meets them in it.
            changing_level: false,
            sleeping: player.sleeping.is_some(),
            rules: clustine_world::GameRules::WORLD,
            difficulty: clustine_world::Difficulty::WORLD,
        };
        let data = player_hurt::type_data(source.kind);
        let outcome = player_hurt::hurt_player(&body, source, &data, amount, hurting, &mut random);
        self.player_drew(id, &random, &start);
        Some(outcome)
    }

    /// What `hurt` says done to the player `id`.
    fn apply_hurt(
        &mut self,
        id: PlayerId,
        source: &DamageSource,
        hurt: &Hurt,
        hurting: Hurting<'_>,
        output: &mut TickOutput,
    ) {
        let tick = self.tick_signed();
        if hurt.woke {
            self.stop_sleep(id, true, true, output);
        }
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let (entity, chunk, selected) = (player.entity_id, player.chunk(), player.selected_slot);
        let position = player.pose.position;
        // The shield: worn, and disabled by the attacker's weapon, its use ended.
        if let Some(shield) = &hurt.shield {
            let slot = inventory_slot(shield.hand, selected);
            if let Some(sound) = shield.block_sound.and_then(consumables::sound_named) {
                self.sound_at(position, sound, None, 1.0, output);
            }
            if let Some(ticks) = shield.disabled_for {
                let held = self
                    .players
                    .get(&id)
                    .and_then(|player| player.inventory.0[slot].clone());
                if let Some(held) = held {
                    self.add_cooldown(id, &held, ticks, output);
                }
                self.stop_using(id, output);
                if let Some(sound) = shield.disable_sound.and_then(consumables::sound_named) {
                    self.sound_at(position, sound, None, 1.0, output);
                }
            }
            if shield.wear > 0 {
                self.hurt_and_break_slot(id, slot, shield.wear, output);
            }
        }
        for &(slot, wear) in &hurt.wear {
            self.hurt_and_break_slot(id, inventory_slot(slot, selected), wear, output);
        }
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.health = hurt.health;
        player.effects.absorption = hurt.absorption;
        player.vitals.cooldown_until =
            (hurt.damage_cooldown > 0).then(|| tick + i64::from(hurt.damage_cooldown));
        player.vitals.last_hurt = hurt.last_hurt;
        player.vitals.motion = hurt.motion;
        if hurt.exhaustion > 0.0 {
            let invulnerable = player.mode.current.abilities().invulnerable;
            crate::food::cause_exhaustion(&mut player.vitals.food, hurt.exhaustion, invulnerable);
        }
        if let Some(mob) = hurt.blame_mob {
            player.vitals.last_hurt_by = Some(LastHurtBy {
                entity: mob,
                player: hurt.blame_player.map(uuid::Uuid::from_u128),
                until: tick + BLAME_TICKS,
            });
        }
        if hurt.damage > 0.0 {
            let fall_distance = player.vitals.fall_distance as f32;
            player.vitals.combat.push(CombatEntry {
                damage_type: source.kind.0.to_owned(),
                amount: hurt.damage,
                killer: source.cause.as_ref().map(|cause| Killer {
                    entity: cause.key,
                    kind: cause.kind,
                    name: cause.name.clone(),
                    player: cause.player.is_some(),
                    weapon: hurting.weapon.cloned(),
                }),
                // Where they fell from: the combat tracker's fall location (V4c).
                fall_location: None,
                fall_distance,
                at: tick,
            });
        }
        self.journal.players.insert(id);
        if hurt.damage_event {
            output.events.push(RegionEvent::DamageEvent {
                entity,
                chunk,
                damage_type: source.kind.id().unwrap_or(0),
                cause: source.cause.as_ref().map(|cause| cause.key),
                direct: source.direct.as_ref().map(|direct| direct.key),
                position: source.position,
            });
        }
        if hurt.game_event
            && let Some(vibration) = Vibration::named("minecraft:entity_damage")
        {
            let context = Context::of(Some(Source::of(entity)), None);
            self.vibrate(vibration, [position.x, position.y, position.z], context);
        }
        if hurt.mark_hurt {
            output.player_events.push((
                id,
                PlayerEvent::Knocked {
                    motion: hurt.motion,
                },
            ));
        }
        if let Some(yaw) = hurt.hurt_direction {
            output
                .player_events
                .push((id, PlayerEvent::HurtAnimation { yaw }));
        }
        if let Some(totem) = &hurt.totem {
            self.used_totem(id, totem.hand, totem.effects, output);
            output.events.push(RegionEvent::EntityEvent {
                entity,
                chunk,
                event: TOTEM_EVENT,
            });
        }
        // The hurt's sounds, to everyone but the player, whose client plays its own from
        // the damage event (`Player.playSound`), at the voice's pitch drawn from their
        // stream (`getVoicePitch`, after the hurt's draws); thorns' to everyone.
        if let Some(sound) = hurt.hurt_sound.and_then(consumables::sound_named) {
            let pitch = self.voice_pitch(id);
            self.sound_at(position, sound, Some(id), pitch, output);
        }
        if hurt.thorns_sound {
            let sound = clustine_data::sounds::ENCHANT_THORNS_HIT;
            self.sound_at(position, sound, None, 1.0, output);
        }
        if hurt.death_sound {
            let pitch = self.voice_pitch(id);
            let sound = clustine_data::sounds::ENTITY_PLAYER_DEATH;
            self.sound_at(position, sound, Some(id), pitch, output);
        }
        if hurt.died {
            self.die(id, output);
        }
    }

    /// A totem's save: one of the hand's stack used up, then its effects
    /// (`DeathProtection.applyEffects`), in their order.
    fn used_totem(
        &mut self,
        id: PlayerId,
        hand: u8,
        effects: &'static [ConsumeEffect],
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let slot = inventory_slot(hand, player.selected_slot);
        let left = player.inventory.0[slot]
            .clone()
            .and_then(|stack| (stack.count > 1).then(|| stack.with_count(stack.count - 1)));
        self.set_inventory_slot(id, slot, left, output);
        let Some(mut random) = self.player_stream(id) else {
            return;
        };
        let start = random.clone();
        for effect in effects {
            match *effect {
                ConsumeEffect::ApplyEffects {
                    effects: given,
                    probability,
                } => {
                    if random.next_float() >= probability {
                        continue;
                    }
                    for instance in given {
                        let instance = consumables::instance_of(instance);
                        self.give_player(id, &instance, output);
                    }
                }
                ConsumeEffect::ClearAllEffects => {
                    let held: Vec<_> = self.players.get(&id).map_or_else(Vec::new, |player| {
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
                ConsumeEffect::RemoveEffects(set) => {
                    for effect in consumables::effects_of(set) {
                        self.remove_player_effect(id, effect, output);
                    }
                }
                // The generator gives no totem a teleport or a sound.
                ConsumeEffect::TeleportRandomly { .. } | ConsumeEffect::PlaySound(_) => {}
            }
        }
        self.player_drew(id, &random, &start);
    }

    /// A sound at `position` of the players' source, at `pitch`, heard by everyone near
    /// but `except`.
    fn sound_at(
        &mut self,
        position: clustine_world::Vec3,
        sound: i32,
        except: Option<PlayerId>,
        pitch: f32,
        output: &mut TickOutput,
    ) {
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_PLAYERS,
            position,
            volume: 1.0,
            pitch,
            except,
        });
    }

    /// `LivingEntity.getVoicePitch`: `(r1 − r2)·0.2 + 1`, from the player's stream.
    fn voice_pitch(&mut self, id: PlayerId) -> f32 {
        let Some(mut random) = self.player_stream(id) else {
            return 1.0;
        };
        let start = random.clone();
        let pitch = (random.next_float() - random.next_float()) * 0.2 + 1.0;
        self.player_drew(id, &random, &start);
        pitch
    }

    /// The region's tick, signed, as a body's ticks are kept.
    fn tick_signed(&self) -> i64 {
        i64::try_from(self.tick).unwrap_or(i64::MAX)
    }
}
