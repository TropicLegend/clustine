//! A player's hurt in the region (V4, `docs/groundwork/v-plan.md`, section 4.3; the build
//! design's sections 3 and 7): [`crate::player_hurt`] works out what a hurt does to the
//! player as they are, and this applies it: the health and absorption, the cooldown, the
//! shield's wear and disabling, the armour's wear, the totem, the damage event and the
//! sounds, the knock and the shake of the view their client is told, the food the hurt
//! costs, who is blamed, the combat tracker's entry, and the death.

use clustine_data::{ConsumeEffect, ItemComponents, attributes};
use clustine_world::{HitExtra, PlayerId};

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

/// The players' sound source (`SoundSource.PLAYERS`), and the neutral one a hit's own
/// sound plays in.
const SOUND_SOURCE_PLAYERS: i32 = 7;
const SOUND_SOURCE_NEUTRAL: i32 = 6;

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

    /// The block `on`'s `fallOn` of the region's player `id` landing after `distance`
    /// (`Block.fallOn` and the blocks that have their own): a bed halves the distance
    /// (`fallDistanceReduction` 0.5), hay and honey take a fifth of the hurt (honey with
    /// its slide's sound and event 54), slime none unless the player sneaks, powder snow
    /// none but its sound, a stalagmite's tip `2.5` more and twice over by `stalagmite`;
    /// then `causeFallDamage`. Named: farmland's trampling and the block's own fall sound
    /// (`playBlockFallSound`).
    pub(crate) fn player_fell_on(
        &mut self,
        id: PlayerId,
        distance: f64,
        on: clustine_data::BlockState,
        output: &mut TickOutput,
    ) {
        use clustine_data::BlockClass;
        let is = |class| clustine_blocks::data::is_class(on, class);
        let Some(sneaking) = self.players.get(&id).map(|player| player.keys.sneak) else {
            return;
        };
        let fall = crate::mobs::body::hurt::DamageType::FALL;
        let (distance, multiplier, kind) = if is(BlockClass::AbstractBedBlock) {
            (distance * f64::from(1.0_f32 - 0.5), 1.0, fall)
        } else if is(BlockClass::HayBlock) {
            (distance, 0.2, fall)
        } else if is(BlockClass::HoneyBlock) {
            self.sound_at_feet(id, clustine_data::sounds::BLOCK_HONEY_BLOCK_SLIDE, output);
            if let Some(player) = self.players.get(&id) {
                output.events.push(RegionEvent::EntityEvent {
                    entity: player.entity_id,
                    chunk: player.chunk(),
                    event: 54,
                });
            }
            (distance, 0.2, fall)
        } else if is(BlockClass::SlimeBlock) && !sneaking {
            (distance, 0.0, fall)
        } else if is(BlockClass::PowderSnowBlock) {
            if distance >= 4.0 {
                let sound = if distance < 7.0 {
                    clustine_data::sounds::ENTITY_PLAYER_SMALL_FALL
                } else {
                    clustine_data::sounds::ENTITY_PLAYER_BIG_FALL
                };
                self.sound_at_feet(id, sound, output);
            }
            return;
        } else if is(BlockClass::PointedDripstoneBlock)
            && on.property("vertical_direction") == Some("up")
            && on.property("thickness") == Some("tip")
        {
            (
                distance + 2.5,
                2.0,
                crate::mobs::body::hurt::DamageType::STALAGMITE,
            )
        } else {
            (distance, 1.0, fall)
        };
        self.cause_fall_damage(id, distance, multiplier, kind, output);
    }

    /// `Player.causeFallDamage` and `LivingEntity.causeFallDamage` of the region's player
    /// `id`: nothing for one who may fly; else `floor((distance + 1e-6 − safe fall
    /// distance) · multiplier · fall damage multiplier)`, and where that is above 0 the
    /// fall's sound (small to 4, big beyond) to everyone but the player, and the hurt.
    fn cause_fall_damage(
        &mut self,
        id: PlayerId,
        distance: f64,
        multiplier: f32,
        kind: crate::mobs::body::hurt::DamageType,
        output: &mut TickOutput,
    ) {
        let Some(may_fly) = self
            .players
            .get(&id)
            .map(|player| player.mode.current.abilities().may_fly)
        else {
            return;
        };
        if may_fly {
            return;
        }
        let safe = self.player_attribute(id, attributes::SAFE_FALL_DISTANCE);
        let scale = self.player_attribute(id, attributes::FALL_DAMAGE_MULTIPLIER);
        let power = distance + 1.0e-6 - safe;
        let damage = (power * f64::from(multiplier) * scale).floor() as i32;
        if damage <= 0 {
            return;
        }
        let sound = if damage > 4 {
            clustine_data::sounds::ENTITY_PLAYER_BIG_FALL
        } else {
            clustine_data::sounds::ENTITY_PLAYER_SMALL_FALL
        };
        if let Some(position) = self.players.get(&id).map(|player| player.pose.position) {
            self.sound_at(position, sound, Some(id), 1.0, output);
        }
        let source = DamageSource::of(kind);
        self.hurt_player(id, &source, damage as f32, Hurting::default(), output);
    }

    /// `Entity.playSound` at the player's feet: to everyone near but them.
    fn sound_at_feet(&mut self, id: PlayerId, sound: i32, output: &mut TickOutput) {
        if let Some(position) = self.players.get(&id).map(|player| player.pose.position) {
            self.sound_at(position, sound, Some(id), 1.0, output);
        }
    }

    /// A hurt that names the region's player `id` (`EntityUse::Hurt`: a mob's melee hit
    /// here or across a line, a projectile, a fang): taken as a mob takes it
    /// (`take_hurt`), with what comes with it if it took: a burning arrow's fire set
    /// before (taken back if refused) and after, the effects, the arrow counted, the
    /// attacker's extra knockback, its type's extras, and its sound.
    pub(crate) fn player_took_hurt(
        &mut self,
        id: PlayerId,
        hurt: &clustine_world::HurtBy,
        output: &mut TickOutput,
    ) -> bool {
        let Some(source) = super::entity_effects::source_of(hurt) else {
            return false;
        };
        let Some(fire_before) = self.players.get(&id).map(|player| player.vitals.fire) else {
            return false;
        };
        if hurt.fire_before > 0 {
            self.set_player_fire(id, fire_before.max(hurt.fire_before));
        }
        let hurting = Hurting {
            weapon: hurt.weapon.as_ref(),
            pierce_level: 0,
        };
        if !self.hurt_player(id, &source, hurt.amount.0, hurting, output) {
            if hurt.fire_before > 0 {
                self.set_player_fire(id, fire_before);
            }
            return false;
        }
        if let Some([strength, x, z]) = hurt.knockback {
            self.knock_player(id, strength.0, x.0, z.0, output);
        }
        if hurt.fire_after > 0 {
            let now = self.players.get(&id).map_or(0, |player| player.vitals.fire);
            self.set_player_fire(id, now.max(hurt.fire_after));
        }
        for instance in &hurt.effects {
            self.give_player(id, instance, output);
        }
        if hurt.arrow
            && let Some(player) = self.players.get_mut(&id)
        {
            player.vitals.arrows += 1;
            self.journal.players.insert(id);
        }
        self.player_hit_extras(id, hurt, output);
        if let Some((sound, volume, pitch)) = hurt.sound
            && let Some(player) = self.players.get(&id)
        {
            output.events.push(RegionEvent::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: player.pose.position,
                volume: volume.0,
                pitch: pitch.0,
                except: None,
            });
        }
        true
    }

    /// What the attacker's type adds to its hit that took on the region's player `id`,
    /// drawn from the hit's stream, as [`Region::hit_extras`] does for a mob.
    fn player_hit_extras(
        &mut self,
        id: PlayerId,
        hurt: &clustine_world::HurtBy,
        output: &mut TickOutput,
    ) {
        if hurt.extras.is_empty() {
            return;
        }
        let Some(key) = self.players.get(&id).map(|player| player.entity_id) else {
            return;
        };
        let mut stream = hurt
            .hit
            .map(|(attacker, lived)| super::Random::of_hit(self.seed(), attacker, lived, key));
        let from = hurt
            .cause
            .as_ref()
            .map_or([0.0; 3], |cause| cause.position.map(|value| value.0));
        let difficulty = self.difficulty_at(clustine_world::BlockPos::new(
            crate::physics::floor(from[0]),
            crate::physics::floor(from[1]),
            crate::physics::floor(from[2]),
        ));
        for extra in &hurt.extras {
            match *extra {
                // `Zombie.doHurtTarget`: the draw is made whether or not it sets fire.
                HitExtra::ZombieFire => {
                    let Some(stream) = stream.as_mut() else {
                        continue;
                    };
                    if stream.next_float() < difficulty * 0.3 {
                        let ticks = 2 * difficulty as i32 * 20;
                        let now = self.players.get(&id).map_or(0, |player| player.vitals.fire);
                        self.set_player_fire(id, now.max(ticks));
                    }
                }
                // `Husk.doHurtTarget`: hunger for `140 · (int) eff` ticks.
                HitExtra::HuskHunger => {
                    let hunger = clustine_world::components::EffectInstance {
                        effect: "minecraft:hunger".to_owned(),
                        amplifier: 0,
                        duration: 140 * difficulty as i32,
                        ambient: false,
                        visible: true,
                        show_icon: true,
                    };
                    self.give_player(id, &hunger, output);
                }
                // `IronGolem.doHurtTarget`'s fling: up by what knockback resistance leaves.
                HitExtra::Fling(up) => {
                    let resistance = self.player_attribute(id, attributes::KNOCKBACK_RESISTANCE);
                    self.push_player(id, output, |motion| {
                        motion[1] += f64::from(up.0) * (1.0 - resistance).max(0.0);
                    });
                }
                // `HoglinBase.throwTarget`, as [`Region::throw_mob`] reads it.
                HitExtra::Throw(knockback) => {
                    let Some(stream) = stream.as_mut() else {
                        continue;
                    };
                    let resistance = self.player_attribute(id, attributes::KNOCKBACK_RESISTANCE);
                    let f = f64::from(knockback.0) - resistance;
                    if f <= 0.0 {
                        continue;
                    }
                    let Some(feet) = self.players.get(&id).map(|player| player.pose.position)
                    else {
                        continue;
                    };
                    let (g, h) = (feet.x - from[0], feet.z - from[2]);
                    let turn = (stream.next_int(21) as i32 - 10) as f32;
                    let j = f * f64::from(stream.next_float() * 0.5 + 0.2);
                    let away = crate::physics::scale(crate::physics::normalize([g, 0.0, h]), j);
                    let away = super::riding::y_rot(away, turn);
                    let up = f * f64::from(stream.next_float()) * 0.5;
                    self.push_player(id, output, |motion| {
                        motion[0] += away[0];
                        motion[1] += up;
                        motion[2] += away[2];
                    });
                }
            }
        }
    }

    /// `LivingEntity.knockback(strength, x, z)` of the region's player `id`, from the
    /// motion their body keeps, and their client told it.
    pub(crate) fn knock_player(
        &mut self,
        id: PlayerId,
        strength: f64,
        x: f64,
        z: f64,
        output: &mut TickOutput,
    ) {
        let resistance = self.player_attribute(id, attributes::KNOCKBACK_RESISTANCE);
        let strength = strength * (1.0 - resistance);
        if strength <= 0.0 {
            return;
        }
        let Some(on_ground) = self.players.get(&id).map(|player| player.pose.on_ground) else {
            return;
        };
        let away = crate::physics::scale(crate::physics::normalize([x, 0.0, z]), strength);
        self.push_player(id, output, |motion| {
            *motion = [
                motion[0] / 2.0 - away[0],
                if on_ground {
                    (motion[1] / 2.0 + strength).min(0.4)
                } else {
                    motion[1]
                },
                motion[2] / 2.0 - away[2],
            ];
        });
    }

    /// Changes the motion the region's player `id`'s body keeps by `push`, and tells their
    /// client (`hurtMarked`).
    fn push_player(
        &mut self,
        id: PlayerId,
        output: &mut TickOutput,
        push: impl FnOnce(&mut [f64; 3]),
    ) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        push(&mut player.vitals.motion);
        let motion = player.vitals.motion;
        self.journal.players.insert(id);
        output
            .player_events
            .push((id, PlayerEvent::Knocked { motion }));
    }

    /// Sets the region's player `id`'s fire ticks (`setRemainingFireTicks`).
    pub(crate) fn set_player_fire(&mut self, id: PlayerId, ticks: i32) {
        if let Some(player) = self.players.get_mut(&id) {
            player.vitals.fire = ticks;
            self.journal.players.insert(id);
        }
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
