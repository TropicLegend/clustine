//! The holder's side of N9's entity effects (`docs/groundwork/n9-plan.md`, sections 8.2
//! and 8.3): a hurt run by the target's own rules and the extras by its outcome, drawn
//! from the hit's stream (`Random::of_hit`), the same whether the hit was made in the
//! region or across a line; effects given by a splash or a cloud; a pearl's landing; and
//! the path of a hurt, effects or a landing addressed to a player
//! (`Effected::OnPlayer`).
//!
//! **Addressed to a player**: the effect goes to the block the player's maker last saw
//! them at (their face, or the players' table), and the holder of that chunk looks the
//! player up by their entity among its own. That is the region that holds their stay
//! but while a hand-over is under way, when the player is in no region's hands or
//! another's: the effect then finds nobody and is dropped (named; the plan's store
//! delivery by the stay's holder would miss the same).
//!
//! Named differences: a hurt of an armour stand, a vehicle or a hanging thing across a
//! line is taken as a projectile's of its kind (`thrown.rs`) with the hurt's amount; the
//! zombie's and the husk's extras read the difficulty where the attacker stood as the
//! hurt carries it.

use clustine_data::{entity_types, sounds};
use clustine_world::{
    BlockPos, Effect, Effected, Entity, EntityData, EntityField, EntityId, EntityUse, GivenEffects,
    HitExtra, HurtBy, PlayerId,
};

use super::Region;
use super::random_ticks::Random;
use crate::api::{PlayerEvent, RegionEvent, TickOutput};
use crate::mobs::body::effects::{self, Effect as MobEffect};
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

/// The entity types `isSensitiveToWater` (water from a splash hurts them): the
/// enderman, the blaze, the snow golem and the strider.
const SENSITIVE_TO_WATER: [i32; 4] = [
    entity_types::ENDERMAN,
    entity_types::BLAZE,
    entity_types::SNOW_GOLEM,
    entity_types::STRIDER,
];

/// The cause or the deal of a hurt as the simulation's damage source names it.
fn causer(of: &clustine_world::Causer) -> Causer {
    Causer {
        key: of.key,
        kind: of.kind,
        position: of.position.map(|value| value.0),
        player: of.player.map(|uuid| uuid.as_u128()),
        living: of.living,
    }
}

/// The damage source a hurt names; `None` for a damage type the game does not have.
pub(crate) fn source_of(hurt: &HurtBy) -> Option<DamageSource> {
    let kind = DamageType::named(&hurt.damage_type)?;
    Some(DamageSource {
        kind,
        cause: hurt.cause.as_ref().map(causer),
        direct: hurt.direct.as_ref().map(causer),
        position: hurt.position.map(|at| at.map(|value| value.0)),
        projectile_motion: hurt
            .projectile_motion
            .map(|motion| motion.map(|value| value.0)),
        clean_water: false,
    })
}

impl Region {
    /// `send_on_entity`'s path for a player: the use `by` addressed to the player
    /// `player`, whose stay is the entity `entity` and whom the maker last saw at `at`.
    pub(crate) fn send_on_player(
        &mut self,
        player: PlayerId,
        entity: EntityId,
        at: BlockPos,
        by: EntityUse,
    ) {
        self.made.push(Effect {
            at,
            what: Effected::OnPlayer { player, entity, by },
        });
    }

    /// The use `by` addressed to the player whose stay is `entity`, where the region has
    /// them: a hurt (refused: every player is creative), effects, a pearl's landing.
    pub(crate) fn player_used(&mut self, entity: EntityId, by: EntityUse, output: &mut TickOutput) {
        let Some(id) = self.player_with_entity(entity) else {
            return;
        };
        match by {
            EntityUse::Effects(given) => {
                for instance in &given.instances {
                    let Some(effect) = MobEffect::named(&instance.effect) else {
                        continue;
                    };
                    // Instant ones heal a full player not at all and hurt them not at
                    // all: they are creative.
                    if effect == MobEffect::INSTANT_HEALTH || effect == MobEffect::INSTANT_DAMAGE {
                        continue;
                    }
                    let layer = effects::Layer {
                        ambient: instance.ambient,
                        visible: instance.visible,
                        show_icon: instance.show_icon,
                        ..effects::Layer::new(
                            u8::try_from(instance.amplifier.clamp(0, 255)).unwrap_or(0),
                            instance.duration,
                            self.tick,
                        )
                    };
                    self.add_player_effect(id, effect, layer, output);
                }
            }
            EntityUse::PearlLanded { at, endermite } => {
                self.pearl_landed(id, at.map(|value| value.0), endermite, output);
            }
            // A creative player is hurt by nothing.
            _ => {}
        }
    }

    /// A mob's melee hit on the entity `key` (`Local::Strike`, `docs/groundwork/
    /// n9-plan.md`, section 8.2): taken here where the region holds the entity, refused
    /// by a player (creative), and sent to the holder of one in a copy.
    pub(crate) fn strike(&mut self, key: EntityId, hurt: Box<HurtBy>, output: &mut TickOutput) {
        if let Some((_, entity)) = self.find_entity(key) {
            let entity = entity.clone();
            self.take_hurt(&entity, &hurt, output);
            return;
        }
        if self.player_with_entity(key).is_some() {
            return;
        }
        self.send_on_entity(key, EntityUse::Hurt(hurt));
    }

    /// `ThrownEnderpearl.onHit` for its owner, the player `id`: an endermite where the
    /// maker drew one (`teleports.rs`), off what they ride, at `at`, and the teleport's
    /// sound; the pearl's hurt of 5 refused (creative).
    fn pearl_landed(
        &mut self,
        id: PlayerId,
        at: [f64; 3],
        endermite: bool,
        output: &mut TickOutput,
    ) {
        if endermite {
            self.pearl_endermite(id, output);
        }
        self.dismount_player(id, output);
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let previous_chunk = player.chunk();
        player.pose.position = clustine_world::Vec3::new(at[0], at[1], at[2]);
        let (entity, pose) = (player.entity_id, player.pose);
        self.journal.players.insert(id);
        output.events.push(RegionEvent::EntityMoved {
            entity,
            pose,
            previous_chunk,
        });
        output
            .player_events
            .push((id, PlayerEvent::Teleported { pose }));
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_PLAYER_TELEPORT,
            source: crate::api::SOUND_SOURCE_PLAYERS,
            position: pose.position,
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// The hurt `hurt` of the region's entity `entity`, and what follows if it took
    /// (section 8.2): the lander's part of every hit of N9, from within and across a line
    /// alike. Whether it took.
    pub(crate) fn take_hurt(
        &mut self,
        entity: &Entity,
        hurt: &HurtBy,
        output: &mut TickOutput,
    ) -> bool {
        let Some(source) = source_of(hurt) else {
            return false;
        };
        let key = entity.key;
        let EntityData::Mob { .. } = entity.data else {
            // Another kind is hurt as a projectile of the hurt's amount hurts it.
            let hit = super::thrown::Hit {
                projectile: hurt.direct.as_ref().map_or(key, |direct| direct.key),
                kind: hurt.direct.as_ref().map_or(0, |direct| direct.kind),
                at: entity.position,
                motion: hurt
                    .projectile_motion
                    .map_or([0.0; 3], |motion| motion.map(|value| value.0)),
                damage: hurt.amount.0,
                owner: hurt.cause.as_ref().map(|cause| clustine_world::Thrower {
                    key: cause.key,
                    kind: cause.kind,
                    from: cause.position,
                    player: cause.player,
                }),
            };
            self.thrown_hit_here(key, hit, output);
            return true;
        };
        // A burning arrow sets the target alight before the hurt, and takes it back
        // where the hurt is refused.
        let fire_before = entity.fire_ticks;
        if hurt.fire_before > 0 {
            self.set_fire_ticks(key, fire_before.max(hurt.fire_before), output);
        }
        let took = self.hurt_mob(key, &source, hurt.amount.0, output);
        if !took {
            if hurt.fire_before > 0 {
                self.set_fire_ticks(key, fire_before, output);
            }
            return false;
        }
        if let Some([strength, x, z]) = hurt.knockback {
            self.knock_mob(key, strength.0, x.0, z.0, output);
        }
        if hurt.fire_after > 0 {
            let now = self
                .find_entity(key)
                .map_or(0, |(_, entity)| entity.fire_ticks);
            self.set_fire_ticks(key, now.max(hurt.fire_after), output);
        }
        for instance in &hurt.effects {
            self.give_mob_effect(key, instance, output);
        }
        if hurt.arrow {
            self.count_arrow(key, output);
        }
        self.hit_extras(key, hurt, output);
        if let Some((sound, volume, pitch)) = hurt.sound {
            let at = self
                .find_entity(key)
                .map_or(entity.position, |(_, entity)| entity.position);
            output.events.push(RegionEvent::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                volume: volume.0,
                pitch: pitch.0,
                except: None,
            });
        }
        true
    }

    /// What the attacker's type adds to its hit that took on the region's mob `key`,
    /// drawn from the hit's stream (section 8.2).
    fn hit_extras(&mut self, key: EntityId, hurt: &HurtBy, output: &mut TickOutput) {
        if hurt.extras.is_empty() {
            return;
        }
        let mut stream = hurt
            .hit
            .map(|(attacker, lived)| Random::of_hit(self.config.seed, attacker, lived, key));
        let from = hurt
            .cause
            .as_ref()
            .map_or([0.0; 3], |cause| cause.position.map(|value| value.0));
        let difficulty = self.difficulty_at(BlockPos::new(
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
                        let seconds = 2 * difficulty as i32;
                        let now = self
                            .find_entity(key)
                            .map_or(0, |(_, entity)| entity.fire_ticks);
                        self.set_fire_ticks(key, now.max(seconds * 20), output);
                    }
                }
                // `Husk.doHurtTarget`: hunger for `140 · (int) eff` ticks, given even for
                // none (finding 47).
                HitExtra::HuskHunger => {
                    let hunger = clustine_world::components::EffectInstance {
                        effect: "minecraft:hunger".to_owned(),
                        amplifier: 0,
                        duration: 140 * difficulty as i32,
                        ambient: false,
                        visible: true,
                        show_icon: true,
                    };
                    self.give_mob_effect(key, &hunger, output);
                }
                HitExtra::Fling(up) => self.fling_mob(key, up.0, output),
            }
        }
    }

    /// `addEffect(instance)` on the region's mob `key`, as its type takes it.
    fn give_mob_effect(
        &mut self,
        key: EntityId,
        instance: &clustine_world::components::EffectInstance,
        output: &mut TickOutput,
    ) {
        let Some(effect) = MobEffect::named(&instance.effect) else {
            return;
        };
        let amplifier = u8::try_from(instance.amplifier.clamp(0, 255)).unwrap_or(0);
        self.struck_between_ticks(key, output, |struck| {
            effects::add(
                struck.mob,
                struck.kind,
                effect,
                amplifier,
                instance.duration,
                struck.moment,
            );
        });
    }

    /// The effects `given` of a splash or a cloud on the region's entity `entity`: water
    /// hurts what is sensitive to it; each effect added, the instant ones applied with
    /// their factor (section 4.4).
    pub(crate) fn take_effects(
        &mut self,
        entity: &Entity,
        given: &GivenEffects,
        output: &mut TickOutput,
    ) {
        let key = entity.key;
        let EntityData::Mob { .. } = entity.data else {
            return;
        };
        let magic = |kind| DamageSource {
            kind,
            cause: given.owner.as_ref().map(causer),
            direct: given.source.as_ref().map(causer),
            position: None,
            projectile_motion: None,
            clean_water: false,
        };
        if given.water {
            if SENSITIVE_TO_WATER.contains(&i32::from(entity.kind)) {
                let water = DamageSource {
                    clean_water: true,
                    ..magic(DamageType::INDIRECT_MAGIC)
                };
                self.hurt_mob(key, &water, 1.0, output);
            }
            return;
        }
        for instance in &given.instances {
            let Some(effect) = MobEffect::named(&instance.effect) else {
                continue;
            };
            if effect == MobEffect::INSTANT_HEALTH || effect == MobEffect::INSTANT_DAMAGE {
                let amplifier = u8::try_from(instance.amplifier.clamp(0, 31)).unwrap_or(0);
                let factor = f64::from(given.instant_factor.0);
                self.struck_between_ticks(key, output, |struck| {
                    effects::instant_with(
                        struck,
                        effect == MobEffect::INSTANT_DAMAGE,
                        amplifier,
                        factor,
                        &magic(DamageType::INDIRECT_MAGIC),
                    );
                });
                continue;
            }
            self.give_mob_effect(key, instance, output);
        }
    }

    /// The region's entity `key` burns for `ticks` from now (`setRemainingFireTicks`).
    fn set_fire_ticks(&mut self, key: EntityId, ticks: i32, output: &mut TickOutput) {
        let Some((chunk, entity)) = self.find_entity(key) else {
            return;
        };
        if entity.fire_ticks == ticks {
            return;
        }
        self.commit_entity(chunk, key, vec![EntityField::FireTicks(ticks)], output);
    }

    /// `alertOthers`' word on the region's mob `key`: it targets `attacker` (of the type
    /// `kind`) if it is of the alerting mob's class `class`, of no class the alert
    /// ignores, and has no target (section 8.1; `mobs/ai/targets.rs`). `setTarget` keeps
    /// the attacker, a valid target where the alerting mob's goal took it; a ghast never
    /// is one. **Named**: a target held is taken as valid, which the holder cannot test
    /// of a player or entity it does not hold.
    pub(crate) fn alerted(
        &mut self,
        key: EntityId,
        attacker: EntityId,
        kind: u16,
        class: u16,
        output: &mut TickOutput,
    ) {
        let Some((_, entity)) = self.find_entity(key) else {
            return;
        };
        if !crate::mobs::ai::targets::comes_to_alert(class, entity.kind)
            || i32::from(kind) == entity_types::GHAST
        {
            return;
        }
        let free = crate::mobs::Mob::decode(&entity.data)
            .is_ok_and(|mob| mob.body.combat.target.is_none());
        if !free {
            return;
        }
        self.struck_between_ticks(key, output, |struck| {
            struck.mob.body.combat.target = Some(attacker);
        });
    }

    /// Another mob's `setTarget(target)` of the region's mob `key`, and where
    /// `aggressive` its `setAggressive(true)` (`Local::SetTarget`).
    pub(crate) fn target_set(
        &mut self,
        key: EntityId,
        target: Option<EntityId>,
        aggressive: bool,
        output: &mut TickOutput,
    ) {
        if self.find_entity(key).is_none() {
            return;
        }
        self.struck_between_ticks(key, output, |struck| {
            struck.mob.body.combat.target = target;
            if aggressive {
                struck.mob.body.combat.aggressive = true;
            }
        });
    }

    /// The holder's side of the uses of N9 on the region's entity `entity`: what
    /// `entities.rs` passes on.
    pub(crate) fn n9_entity_used(
        &mut self,
        entity: &Entity,
        by: &EntityUse,
        output: &mut TickOutput,
    ) -> bool {
        match by {
            EntityUse::Hurt(hurt) => {
                self.take_hurt(entity, hurt, output);
            }
            EntityUse::Effects(given) => self.take_effects(entity, given, output),
            EntityUse::Alert {
                attacker,
                kind,
                class,
            } => self.alerted(entity.key, *attacker, *kind, *class, output),
            EntityUse::PatrolTarget { pos } => self.patrol_target_given(entity.key, *pos, output),
            // Only ever addressed to a player.
            EntityUse::PearlLanded { .. } => {}
            _ => return false,
        }
        true
    }
}
