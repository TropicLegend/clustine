//! The region's side of a mob's body (`crate::mobs::body`; the brief's sections 1.4 to
//! 1.10): what its tick showed and dropped laid down and told, its hurts between ticks (a
//! player's hit), a player's use of it, and its leash.
//!
//! A hurt between ticks comes at the mob's `lived` (`crate::mobs::body`'s moments) and
//! draws from the mob's stream aside (`Random::of_entity_aside`); it is committed at
//! once, and its death's drops are laid down at once, as the game lays them in the hit.
//!
//! Named differences: a hit across a line (another region's player) is a player's hit
//! of the damage the attacker's region worked out, pushed only by default, from the
//! player's feet, and remembered without the player's UUID (the effect does not carry
//! it); a use across a line ties and lets go no lead (as N5's boats); a name tag names
//! nothing (Clustine keeps no names); a leashed mob's slack path towards its
//! holder and the move flag a snapped leash takes off are the navigation's and the
//! goals' (N7c), and its leash is ticked before the rest of its tick, where the game
//! ticks it at the end of `Entity.baseTick`.

use clustine_data::{EntityTypeInfo, entity_types, items, sounds};
use clustine_world::{ChunkPos, Entity, EntityData, EntityField, EntityId, EntityUse};

use super::Region;
use super::items::{View, tracked};
use super::leashes;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{AttributeShown, RegionEvent, TickOutput, Tracked};
use crate::mobs::Mob;
use crate::mobs::body::hurt::{self, Causer, DamageSource, DamageType, Struck};
use crate::mobs::body::{Attributes, Death, Equipment, Event, Home};
use crate::mobs::part;
use crate::mobs::tick::{Local, MobOutput};
use crate::mobs::types::{self, Interaction};
use crate::physics::Vec3;
use crate::physics::World;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

/// What a leash's pull left of a mob's motion, fall and yaw, for its tick to start from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LeashPull {
    pub(crate) motion: Vec3,
    pub(crate) fall_distance: f64,
    pub(crate) yaw: f32,
}

impl Region {
    /// `Leashable.tickLeash(level, mob)` of the mob `entity`, in the chunk `chunk`: a
    /// saved leash looked for; a leash of a dying mob, or whose holder is gone, dropped
    /// with a lead; beyond the snap distance broken with its sound; otherwise
    /// `whenLeashedTo`'s home (the holder's block, within 5) and the pull, whose result
    /// the mob's tick starts from. The swing is kept in the record in place.
    pub(crate) fn mob_tick_leash(
        &mut self,
        entity: &Entity,
        mob: &mut Mob,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) -> Option<LeashPull> {
        let key = entity.key;
        let leash = entity.riding.leash.as_deref().copied()?;
        if !self.leash_found(key) {
            let memory = self.entity_memory.entry(key).or_default();
            memory.boat.leash.ticks = memory.boat.leash.ticks.saturating_add(1);
            let ticks = memory.boat.leash.ticks;
            if !self.restore_leash(key, leash.holder, ticks, output) {
                return None;
            }
            self.note_leash_found(key);
        }
        // `!isAlive()`: a dying mob lets go a tick after it died.
        if mob.body.vitals.dying() {
            self.drop_leash(key, true, output);
            return None;
        }
        let Some(held) = self.leash_holder(leash.holder) else {
            self.drop_leash(key, true, output);
            return None;
        };
        // `PathfinderMob.whenLeashedTo`: home at the holder's block, as far as the
        // elastic distance less one.
        let [x, y, z] = held.end.position;
        mob.body.home = Home {
            centre: Some(crate::physics::containing([x, y, z])),
            radius: 5,
        };
        let (_, record) = self.find_entity(key)?;
        let record = record.clone();
        let info = EntityTypeInfo::of(i32::from(record.kind));
        let (width, height) = types::own_size(&record)
            .map_or((info.width, info.height), |size| (size.width, size.height));
        let leashee = leashes::End {
            position: record.position,
            yaw: record.yaw,
            width,
            height,
            movement: record.motion,
        };
        let mut pull = LeashPull {
            motion: record.motion,
            fall_distance: record.fall_distance,
            yaw: record.yaw,
        };
        let mut momentum = leash.angular_momentum.0;
        let friction = self.mob_angular_friction(&record, chunk);
        let snapped = leashes::leash_tick(
            &leashee,
            &held.end,
            &mut pull.motion,
            &mut pull.fall_distance,
            &mut pull.yaw,
            &mut momentum,
            friction,
        );
        if snapped {
            output.events.push(RegionEvent::Sound {
                sound: sounds::ITEM_LEAD_BREAK,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(x, y, z),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
            // `leashTooFarBehaviour`: the lead drops; the goals' move flag is N7c's.
            self.drop_leash(key, true, output);
            mob.body.home = Home::default();
            return Some(pull);
        }
        let mut riding = record.riding.clone();
        if let Some(kept) = riding.leash.as_deref_mut() {
            kept.angular_momentum = clustine_world::Double(momentum);
        }
        self.age_entity(chunk, key, vec![EntityField::Riding(riding)], output);
        Some(pull)
    }

    /// `Leashable.angularFriction(mob)`: on the ground the friction of the block below
    /// that moves it, times 0.91; in a liquid 0.8; in the air 0.91.
    fn mob_angular_friction(&self, record: &Entity, chunk: ChunkPos) -> f32 {
        let view = View {
            region: self,
            home: chunk,
        };
        let info = EntityTypeInfo::of(i32::from(record.kind));
        let body = self.living_body(record, info.width, info.height, chunk);
        if record.on_ground {
            let below = view.state(body.below_for_movement(&view));
            return below.physics().map_or(0.6, |physics| physics.friction) * 0.91;
        }
        let at = view.state(body.block_position());
        if clustine_blocks::fluid::FluidState::of(at).kind().is_some() {
            0.8
        } else {
            0.91
        }
    }

    /// What a mob's tick, or a hurt between its ticks, left beyond its record: its
    /// writes to others, what it showed, what it dropped as it died, and its leaving
    /// the world. `before` is its record as the tick began, `at` where it is now.
    pub(crate) fn mob_ticked(
        &mut self,
        before: &Entity,
        at: Vec3,
        out: MobOutput,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        self.apply_locals(key, out.local, output);
        for event in out.events {
            self.show_body_event(key, chunk, at, event, output);
        }
        if let Some(death) = out.died {
            self.lay_death(death, output);
        }
        if out.removed.is_some()
            && let Some((chunk, record)) = self.find_entity(key)
        {
            let hops = record.hops;
            self.unride_removed(key, true, output);
            self.remove_entity(key, hops, chunk, output);
        }
    }

    /// What the mob `key`'s tick wrote to others, in the order made: pushes, a request
    /// to breed, blocks it changed, items it dropped.
    pub(crate) fn apply_locals(
        &mut self,
        key: EntityId,
        locals: Vec<Local>,
        output: &mut TickOutput,
    ) {
        for local in locals {
            match local {
                Local::Push { key: pushed, by } => self.push_mob(pushed, by, output),
                Local::BreedAsk { partner, number } => {
                    self.ask_to_breed(key, partner, number, output);
                }
                Local::SetBlock { pos, state, flags } => {
                    self.set_block_updated(pos, state, flags, super::updates::UPDATE_LIMIT, output);
                }
                Local::DestroyBlock { pos } => {
                    self.destroy_block(pos, false, super::updates::UPDATE_LIMIT, output);
                }
                Local::LevelEvent { kind, pos, data } => {
                    output.events.push(RegionEvent::LevelEvent {
                        kind,
                        position: pos,
                        data,
                    });
                }
                Local::GameEvent { name, pos } => {
                    if let Some(vibration) = Vibration::named(name) {
                        let context = Context::of(Some(Source::of(key)), None);
                        self.vibrate_at(vibration, pos, context);
                    }
                }
                Local::GameEventAt { name, at, affected } => {
                    if let Some(vibration) = Vibration::named(name) {
                        let context = Context::of(Some(Source::of(key)), affected);
                        self.vibrate(vibration, at, context);
                    }
                }
                Local::Drop {
                    stack,
                    at,
                    pick_up_delay,
                } => {
                    self.spawn_item_as(at, stack, pick_up_delay, None, output);
                }
                Local::Sound {
                    sound,
                    source,
                    at,
                    volume,
                    pitch,
                } => output.events.push(RegionEvent::Sound {
                    sound,
                    source,
                    position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                    volume,
                    pitch,
                    except: None,
                }),
                Local::TakeItem { item, count } => self.item_taken_by_mob(key, item, count, output),
                Local::JoinSchool {
                    leader,
                    followers,
                    leader_gains,
                } => self.join_school(leader, &followers, leader_gains, output),
                Local::LeaveSchool { leader } => self.leave_school(leader, output),
                Local::Hurt {
                    key: hurt,
                    kind,
                    from,
                    amount,
                    sound,
                } => self.hurt_by_mob(key, kind, from, hurt, amount, sound, output),
                Local::Throw {
                    stack,
                    at,
                    pick_up_delay,
                    motion,
                } => {
                    self.spawn_item_as(at, stack, pick_up_delay, Some(motion), output);
                }
                Local::EjectPassengers => self.eject_passengers(key, output),
                Local::Pop { pos, stack } => self.pop_resource_at(pos, stack, output),
                Local::EnterHive { hive } => self.enter_hive(key, hive, output),
                Local::Sting {
                    key: stung,
                    kind,
                    from,
                    amount,
                } => self.stung_by_bee(key, kind, from, stung, amount, output),
                Local::GrowUp => self.tadpole_grown_up(key, output),
            }
        }
    }

    /// `Entity.push(x, y, z)` of the region's mob `key` by another's `pushEntities`:
    /// its motion, sent at the next update.
    fn push_mob(&mut self, key: EntityId, by: Vec3, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        if !matches!(record.data, EntityData::Mob { .. }) || !by.iter().all(|v| v.is_finite()) {
            return;
        }
        let mut after = record.clone();
        after.motion = crate::physics::add(record.motion, by);
        self.commit_entity(chunk, key, vec![EntityField::Motion(after.motion)], output);
        let mut report = tracked(&after, Tracked::NEEDS_SYNC, false);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// One of a mob's body's events, as the region's.
    fn show_body_event(
        &mut self,
        key: EntityId,
        chunk: ChunkPos,
        at: Vec3,
        event: Event,
        output: &mut TickOutput,
    ) {
        match event {
            Event::Sound {
                sound,
                volume,
                pitch,
            } => output.events.push(RegionEvent::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                volume,
                pitch,
                except: None,
            }),
            Event::Entity(event) => output.events.push(RegionEvent::EntityEvent {
                entity: key,
                chunk,
                event,
            }),
            Event::Damaged {
                kind,
                cause,
                direct,
                position,
            } => output.events.push(RegionEvent::DamageEvent {
                entity: key,
                chunk,
                damage_type: kind.id().unwrap_or(0),
                cause,
                direct,
                position,
            }),
            Event::GameEvent(event) => {
                if let Some(vibration) = Vibration::named(event.name()) {
                    let context = Context::of(Some(Source::of(key)), None);
                    self.vibrate(vibration, at, context);
                }
            }
        }
    }

    /// What a mob dropped as it died, laid at its feet: each stack (`spawnAtLocation`,
    /// nothing for an empty one), then its experience (`ExperienceOrb.award`).
    fn lay_death(&mut self, death: Death, output: &mut TickOutput) {
        for stack in death.drops {
            if stack.count > 0 && stack.item != items::AIR {
                self.spawn_item(death.position, stack, output);
            }
        }
        for (value, draw) in death.orbs {
            self.award_orb(death.position, value, draw, output);
        }
    }

    /// What clients are told of a mob whose record went from `before` to `after` beyond
    /// its report: its attributes (`UpdateAttributes`), where the synced ones changed or
    /// it made its first in its first tick, and its equipment (`SetEquipment`), where it
    /// changed.
    pub(crate) fn mob_shown_changes(
        &mut self,
        before: &Entity,
        after: &Entity,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let (EntityData::Mob { body: was, .. }, EntityData::Mob { body: is, .. }) =
            (&before.data, &after.data)
        else {
            return;
        };
        let kind = after.kind;
        let attributes = |entries, ticked| {
            part::decode::<Attributes>(entries)
                .map(|attributes| attributes.syncable(kind, ticked))
                .unwrap_or_default()
        };
        let shown = attributes(is, after.lived > 0);
        if shown != attributes(was, before.lived > 0) {
            output.events.push(RegionEvent::AttributesChanged {
                entity: after.key,
                chunk,
                attributes: shown
                    .into_iter()
                    .map(|instance| AttributeShown {
                        attribute: instance.attribute,
                        base: instance.base,
                        modifiers: instance
                            .modifiers
                            .into_iter()
                            .map(|modifier| (modifier.id, modifier.amount, modifier.operation))
                            .collect(),
                    })
                    .collect(),
            });
        }
        let worn = |entries| {
            part::decode::<Equipment>(entries)
                .map(|equipment| equipment.slots)
                .unwrap_or_default()
        };
        let now = worn(is);
        if now != worn(was) {
            output.events.push(RegionEvent::EquipmentChanged {
                entity: after.key,
                chunk,
                slots: (0u8..).zip(now).collect(),
            });
        }
    }

    /// A player's hit on, or use of, the mob `entity` of the chunk `chunk`: made here,
    /// or by another region's player as an effect.
    pub(crate) fn mob_used(
        &mut self,
        entity: &Entity,
        chunk: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        match by {
            EntityUse::Attack {
                attacker,
                damage,
                from,
                ..
            } => match self.player_with_entity(attacker) {
                Some(player) => {
                    self.player_attack(player, entity.key, output);
                }
                None => {
                    let source = DamageSource::by(
                        DamageType::PLAYER_ATTACK,
                        Causer {
                            key: attacker,
                            kind: player_kind(),
                            position: from.map(|value| value.0),
                            player: Some(0),
                            living: true,
                        },
                    );
                    self.hurt_mob(entity.key, &source, damage.0, output);
                }
            },
            EntityUse::Interact { .. } => self.mob_interacted(entity, chunk, by, output),
            // An explosion's hit is the same from within and across a line
            // (`explosions.rs`).
            EntityUse::Blasted { .. } => {}
            EntityUse::BreedAsk { asker, number } => {
                self.breed_asked(entity.key, asker, number, output);
            }
            EntityUse::BreedAnswer {
                partner,
                number,
                accepted,
            } => self.bred(entity.key, partner, number, accepted, output),
        }
    }

    /// `Mob.interact(player, hand, location)`: nothing for a dying mob; a spawn egg of
    /// its own type makes a baby of it (`checkAndHandleImportantInteractions`); then the
    /// leash's part of `Entity.interact` (a lead ties a mob that can be leashed, the
    /// holder lets go, a sneaking player ties their leashees to it, shears cut) and
    /// shears take what may be shorn of what it wears; then its type's own
    /// (`mobInteract`), and where that passed, the item's own use on it
    /// (`ItemStack.interactLivingEntity`), each of which posts `ENTITY_INTERACT` where it
    /// did anything.
    fn mob_interacted(
        &mut self,
        entity: &Entity,
        chunk: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let EntityUse::Interact {
            player,
            creative,
            held,
            sneaking,
            from,
            ..
        } = by
        else {
            return;
        };
        let Ok(mob) = Mob::decode(&entity.data) else {
            return;
        };
        if mob.body.vitals.dying() {
            return;
        }
        if self.offspring_from_egg(entity, held, player, output) {
            self.interacted(entity, player, Interaction::SuccessServer, output);
            return;
        }
        let mob_type = types::of(entity.kind);
        let lead = held.is_some_and(|stack| stack.item == items::LEAD);
        let leashable = mob_type.can_be_leashed(entity.kind, &mob);
        if (!lead || leashable) && self.leash_used(entity, by, output) {
            return;
        }
        let shears = held.is_some_and(|stack| stack.count > 0 && stack.item == items::SHEARS);
        if shears
            && !sneaking
            && entity.riding.passengers.is_empty()
            && self.shear_equipment(entity, chunk, player, output)
        {
            return;
        }
        let from = from.map(|value| value.0);
        self.mob_used_by(
            entity, chunk, player, held, creative, sneaking, from, output,
        );
    }

    /// `hurtServer(source, amount)` of the region's mob `key` between its ticks (a
    /// player's hit, an explosion's): committed at once, shown, and its death's drops
    /// laid down. Whether it took the hurt.
    pub(crate) fn hurt_mob(
        &mut self,
        key: EntityId,
        source: &DamageSource,
        amount: f32,
        output: &mut TickOutput,
    ) -> bool {
        self.struck_between_ticks(key, output, |struck| {
            hurt::hurt_server(struck, source, amount)
        })
        .unwrap_or(false)
    }

    /// `LivingEntity.knockback(strength, x, z)` of the region's mob `key` between its
    /// ticks (a player's attack's own push, a sweep's).
    pub(crate) fn knock_mob(
        &mut self,
        key: EntityId,
        strength: f64,
        x: f64,
        z: f64,
        output: &mut TickOutput,
    ) {
        self.struck_between_ticks(key, output, |struck| {
            hurt::knockback(struck, strength, x, z);
        });
    }

    /// Does `what` to the region's mob `key` between its ticks, at its `lived`, and
    /// commits, shows and lays down what it left; `None` where there is no such mob.
    fn struck_between_ticks<T>(
        &mut self,
        key: EntityId,
        output: &mut TickOutput,
        what: impl FnOnce(&mut Struck<'_>) -> T,
    ) -> Option<T> {
        let (chunk, record) = self.find_entity(key)?;
        let record = record.clone();
        let mut mob = Mob::decode(&record.data).ok()?;
        let vehicle = record
            .riding
            .vehicle
            .and_then(|vehicle| self.find_entity(vehicle))
            .map(|(_, vehicle)| vehicle.kind);
        let mut motion = record.motion;
        let mut stream = self.entity_stream(key).random;
        let start = stream.clone();
        let mut out = MobOutput::default();
        let answer = {
            let mut struck = Struck {
                kind: record.kind,
                mob: &mut mob,
                mob_type: types::of(record.kind),
                position: record.position,
                motion: &mut motion,
                on_ground: record.on_ground,
                on_fire: record.fire_ticks > 0,
                vehicle,
                moment: record.lived,
                draws: &mut stream,
                out: &mut out,
            };
            what(&mut struck)
        };
        self.entity_drew(key, stream.draws_since(&start));
        let mut fields = mob.changes_from(&record.data);
        let bits = |values: Vec3| values.map(f64::to_bits);
        if bits(motion) != bits(record.motion) {
            fields.push(EntityField::Motion(motion));
        }
        let mut flags = 0;
        if out.needs_sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        if out.sync_velocity {
            flags |= Tracked::SYNC_VELOCITY;
        }
        let shows = !fields.is_empty() || flags != 0;
        let data_changed = fields.iter().any(|field| {
            matches!(
                field,
                EntityField::BodyEntry(_) | EntityField::BodyGone(_) | EntityField::TypeState(_)
            )
        });
        if !fields.is_empty() {
            self.commit_entity(chunk, key, fields, output);
        }
        if shows && let Some((_, after)) = self.find_entity(key) {
            let after = after.clone();
            let mut report = tracked(&after, flags, data_changed);
            report.chunk = chunk;
            output.events.push(RegionEvent::EntityTracked(report));
            self.mob_shown_changes(&record, &after, chunk, output);
        }
        self.mob_ticked(&record, record.position, out, chunk, output);
        Some(answer)
    }
}

/// The entity type of a player, by id.
pub(crate) fn player_kind() -> u16 {
    u16::try_from(entity_types::PLAYER).expect("entity type ids fit 16 bits")
}
