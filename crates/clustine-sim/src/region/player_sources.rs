//! What hurts a player in their own tick (V4a, `docs/groundwork/v-plan.md`, section 2.3;
//! the build design's sections 3 and 7, "The tick (M1)" and "Falling (M3)"): the body's
//! part of `ServerPlayer.doTick`, after the level's entities and block entities as the
//! game's connections tick after its levels, for each living player in the order of
//! their ids. Each hurt goes through [`Region::hurt_player`], which refuses what a
//! creative player, a player whose client has not loaded or the dead are spared.
//!
//! The game's order, which this keeps (26.3 jar, read for V4):
//!
//! 1. `Entity.baseTick @125-213`: burning hurts `on_fire` 1 every 20 fire ticks unless
//!    in lava, and the fire ticks count down; lava halves the fall distance. The void
//!    (`checkBelowWorld @214`) stays `void.rs`'s. The shared flag of burning is the fire
//!    ticks after the count (@227-240).
//! 2. `LivingEntity.baseTick @90-418`: `in_wall` 1 with the eyes in a block that
//!    suffocates (not while asleep, `LivingEntity.isInWall`); under water out of a bubble
//!    column one air less a tick, unless water breathing, conduit power or the
//!    abilities' invulnerability keep it, or a draw from the player's stream of at least
//!    `1 / (oxygen_bonus + 1)` (Respiration, `decreaseAirSupply @0-51`); at −20 the air
//!    is 0, entity event 67's bubbles go out and `drown` hurts 2; elsewhere four back a
//!    tick to 300. Then `tickEffects @603` ([`Region::player_effect_ticks`]).
//! 3. `aiStep`: flying ends a fall (`Player.aiStep @28-46`); a glide caps the fall at 1
//!    while the motion's y is above −0.5 (`updateFallFlying` →
//!    `checkFallDistanceAccumulation`); slow falling and levitation end it
//!    (`LivingEntity.aiStep @527-548`); then `applyEffectsFromBlocks @628` along the
//!    tick's move: a magma block's `stepOn` (`hot_floor` 1 unless sneaking), then the
//!    blocks met in `InsideBlockEffectType`'s order with their hurts (`cactus` 1 at once,
//!    `sweet_berry_bush` 1 of a grown bush to a player whose last move went 0.003 or more
//!    sideways, `campfire` 1 or a soul campfire's 2 at once, fire's `in_fire` 1 or soul
//!    fire's 2 after it lights them for 8 s, lava's `lava` 4 after 15 s of fire, water
//!    and powder snow putting the fire out); then powder snow's freezing (one more a
//!    tick in it to 140, two less out of it; `freeze` 1 every 40 ticks of their age
//!    when frozen through, `@687-761`), which leather worn keeps off
//!    (`#freeze_immune_wearables`); then cramming (`pushEntities @807`): with more than
//!    23 pushable entities touching them and a draw of one in four from their stream,
//!    `cramming` 6 if more than 23 of them ride nothing.
//! 4. Their air, fire, frozen ticks and fall distance kept in [`crate::state::Vitals`],
//!    with the kept absorption; what changed of what only the server sets of their own
//!    entity told to their client ([`PlayerEvent::OwnData`]), and their health where it
//!    changed ([`PlayerEvent::Health`], as `void.rs` tells it).
//!
//! The player's moves are their client's: the body is put where the client last put it,
//! and the walk through the blocks follows the tick's last move (`known_movements`).
//!
//! Named differences, where they live:
//! - Only the tick's last move is walked: a client that sent two moves in one tick has
//!   the blocks of the first passed over (the game walks each of `movementThisTick`).
//! - No weather reaches a player yet: rain puts out no fire (`isInRain`,
//!   `applyEffectsFromBlocks @76-84`), and the world border's hurt is not built.
//! - A burning player in powder snow does not melt it (`PowderSnowBlock.entityInside
//!   @176-189`'s `runBefore(EXTINGUISH)`); its snowflakes, as all particles, are not sent.
//! - `EnchantmentHelper.tickEffects` (`LivingEntity.baseTick @44`: Soul Speed, Frost
//!   Walker's ice) is not run here.
//! - Cramming counts the entities of the region's own chunks and its players, not those
//!   of copies of another region's chunks, and takes a mob that climbs for pushable.
//! - The first tick of a joined player counts lava (`firstTick`), which the game leaves
//!   to its second.
//! - The fall distance kept is [`crate::state::Vitals`]'s; the moves still count their
//!   own in `vibrations::Steps` until the falls are built on it.
//! - The burning flag that others see of the player (shared flag 0 of `poses.rs`) is not
//!   set here; only the player's own client is told ([`PlayerEvent::OwnData`]), every
//!   tick the fire ticks are above 0 at its start or its end, since the flag of the tick
//!   before is not kept; a fire put out in the blocks' effects is told out in that tick,
//!   a tick before the game's flag says so.

use clustine_data::{attributes, entity_types, items, sounds};
use clustine_world::{EntityData, PlayerId};

use super::Region;
use super::items::View;
use crate::api::{OwnData, PlayerEvent, RegionEvent, TickOutput};
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::body::effects::Effect;
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Movement};
use crate::physics::entities::Mover;
use crate::player_hurt::Hurting;
use crate::state::PlayerPose;

/// The players' sound source (`SoundSource.PLAYERS`).
const SOUND_SOURCE_PLAYERS: i32 = 7;

/// The entity event of drowning's bubbles (`LivingEntity.baseTick @310-314`).
const DROWNING_EVENT: u8 = 67;

/// `getFireImmuneTicks()` of a player (`Player.getFireImmuneTicks`).
const PLAYER_FIRE_IMMUNE_TICKS: i32 = 20;

/// The ticks of a player's age from one `freeze` hurt of the frozen through to the next
/// (`LivingEntity.aiStep @723-730`, `tickCount % 40`).
const FREEZE_EVERY: i64 = 40;

/// The game rule `max_entity_cramming`'s default, which no command changes yet.
const MAX_ENTITY_CRAMMING: usize = 24;

/// The width, height and eye height of a player in `pose` (`Avatar.POSES`;
/// `LivingEntity.SLEEPING_DIMENSIONS`), as `poses.rs` has them.
fn dimensions(pose: PlayerPose) -> (f32, f32, f32) {
    match pose {
        PlayerPose::Standing => (0.6, 1.8, 1.62),
        PlayerPose::Crouching => (0.6, 1.5, 1.27),
        PlayerPose::FallFlying | PlayerPose::Swimming | PlayerPose::SpinAttack => (0.6, 0.6, 0.4),
        PlayerPose::Sleeping => (0.2, 0.2, 0.2),
    }
}

/// What the client is told of the player's own entity, as it stood.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Own {
    air: i32,
    fire: i32,
    frozen: i32,
    absorption: f32,
    health: f32,
}

impl Region {
    /// Each living player's own body tick, in the order of their ids; not of one who
    /// stands in a chunk another region holds, who is let go at the end of the tick and
    /// whose body the region they go to ticks.
    pub(crate) fn players_body_tick(&mut self, output: &mut TickOutput) {
        let ids: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| {
                !player.dead && self.land.knowledge(player.chunk()).holder().is_none()
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.player_body_tick(id, output);
        }
    }

    /// The body's part of `ServerPlayer.doTick` of the player `id` (the module's list).
    fn player_body_tick(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let before = Own {
            air: player.vitals.air,
            fire: player.vitals.fire,
            frozen: player.vitals.frozen,
            absorption: player.vitals.absorption,
            health: player.health,
        };
        let sleeping = player.sleeping.is_some();
        let chunk = player.chunk();
        let Some(mut living) = self.living_of_player(id) else {
            return;
        };
        // `Entity.baseTick` and `LivingEntity.baseTick` up to the effects, the draws of
        // Respiration from the player's stream; then the hurts they met, in order.
        let mut random = self.player_stream(id);
        let start = random.clone();
        living.base_tick_drawing(
            &View {
                region: self,
                home: chunk,
            },
            random.as_mut(),
        );
        if let (Some(random), Some(start)) = (&random, &start) {
            self.player_drew(id, random, start);
        }
        let drowned = std::mem::take(&mut living.drowned);
        let on_fire = living.on_fire_flag;
        self.take_player_hurts(id, &mut living, drowned, sleeping, output);
        if self.player_dead(id) {
            self.keep_body(id, &living, before, on_fire, output);
            return;
        }
        // `tickEffects`.
        self.player_effect_ticks(id, output);
        if self.player_dead(id) {
            self.keep_body(id, &living, before, on_fire, output);
            return;
        }
        // `aiStep`.
        self.player_fall_resets(id, &mut living);
        let Some(mut random) = self.player_stream(id) else {
            return;
        };
        let start = random.clone();
        living.after_travel(
            &View {
                region: self,
                home: chunk,
            },
            &mut random,
        );
        self.player_drew(id, &random, &start);
        self.take_player_hurts(id, &mut living, false, sleeping, output);
        self.player_sounds(&mut living, output);
        self.freeze_player(id, &living, output);
        self.cram_player(id, &living, output);
        self.keep_body(id, &living, before, on_fire, output);
    }

    /// Whether the player `id` is dead (or gone).
    fn player_dead(&self, id: PlayerId) -> bool {
        self.players.get(&id).is_none_or(|player| player.dead)
    }

    /// The living body of the player `id` as their tick starts: where their client put
    /// them, in their pose's box, with their air, fire, frozen ticks and fall distance,
    /// and what their abilities, effects and equipment say of it.
    fn living_of_player(&self, id: PlayerId) -> Option<Living> {
        let oxygen_bonus = self.player_attribute(id, attributes::OXYGEN_BONUS);
        let player = self.players.get(&id)?;
        let feet = player.pose.position;
        let position = [feet.x, feet.y, feet.z];
        let (width, height, eye) = dimensions(player.posture.pose);
        let known = self.known_movements.get(&id).copied();
        let entity = player.entity_id;
        let body = Body {
            stands_on_lava: false,
            position,
            motion: player.vitals.motion,
            on_ground: player.pose.on_ground,
            fall_distance: player.vitals.fall_distance,
            width,
            height,
            old_position: position,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: false,
            supporting: None,
            on_ground_no_blocks: false,
            stuck_speed: [0.0; 3],
            // `movementThisTick`: the tick's last move, from where it began; none for a
            // player who made none, whose walk is where they stand.
            movements: known
                .map(|delta| Movement {
                    from: [
                        position[0] - delta[0],
                        position[1] - delta[1],
                        position[2] - delta[2],
                    ],
                    to: position,
                    asked: None,
                })
                .into_iter()
                .collect(),
            in_water: false,
            sync_position: false,
            falling_block: false,
            item: false,
            fell_on: None,
            noclip: None,
            mover: Mover {
                key: entity,
                root: entity,
                like_a_vehicle: false,
                happy_ghast: false,
            },
            steps: None,
        };
        let kind = Kind::Mob {
            kind: u16::try_from(entity_types::PLAYER).expect("a type id"),
            hurts: Vec::new(),
        };
        let mut living = Living::new(body, kind, eye);
        let abilities = player.mode.current.abilities();
        let spectator = player.mode.current == clustine_world::GameMode::Spectator;
        living.travels = false;
        // A spectator moves through everything and is touched by no block
        // (`Player.tick @0-5`, `isAffectedByBlocks`).
        living.no_physics = spectator;
        living.first_tick = false;
        living.pushed_by_fluid = false;
        living.fire_ticks = player.vitals.fire;
        living.air = player.vitals.air;
        living.max_air = MAX_AIR;
        living.frozen = player.vitals.frozen;
        // `LivingEntity.canFreeze`: not a spectator, and nothing of
        // `#freeze_immune_wearables` worn as armour.
        let worn = |slot: u8| {
            let at = clustine_world::ARMOUR_START + usize::from(slot - slots::FEET);
            player.inventory.0[at].as_ref()
        };
        living.can_freeze = !spectator
            && ![slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD]
                .into_iter()
                .filter_map(worn)
                .any(|stack| {
                    clustine_blocks::data::item_in_tag(
                        stack.item,
                        "minecraft:freeze_immune_wearables",
                    )
                });
        living.walks_on_powder_snow =
            worn(slots::FEET).is_some_and(|stack| stack.item == items::LEATHER_BOOTS);
        // `isSteppingCarefully`: a player's shift key.
        living.careful = player.keys.sneak;
        // `baseTick @243-279`: water breathing, conduit power and the abilities'
        // invulnerability keep the air under water; what the effects keep comes back.
        living.water_breathing = player.effects.water_breathing() || abilities.invulnerable;
        living.effects_refill_air = player.effects.refill_air();
        living.player_hurts = Some(Vec::new());
        living.fire_immune_ticks = PLAYER_FIRE_IMMUNE_TICKS;
        living.oxygen_bonus = oxygen_bonus;
        living.known_movement = known.unwrap_or([0.0; 3]);
        Some(living)
    }

    /// The hurts the body met so far in the tick, taken in the order met through the
    /// player's hurt: drowning's bubbles before its hurt, lava's burning sound after a
    /// hurt of lava that took (`Entity.lavaHurt @25-96`). The void's is `void.rs`'s; a
    /// sleeper is in no wall (`LivingEntity.isInWall`).
    fn take_player_hurts(
        &mut self,
        id: PlayerId,
        living: &mut Living,
        drowned: bool,
        sleeping: bool,
        output: &mut TickOutput,
    ) {
        let hurts = living
            .player_hurts
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        for (kind, amount) in hurts {
            if self.player_dead(id) {
                break;
            }
            if kind == DamageType::OUT_OF_WORLD || (kind == DamageType::IN_WALL && sleeping) {
                continue;
            }
            if kind == DamageType::DROWN && drowned {
                self.player_entity_event(id, DROWNING_EVENT, output);
            }
            let took = self.hurt_player(
                id,
                &DamageSource::of(kind),
                amount,
                Hurting::default(),
                output,
            );
            if kind == DamageType::LAVA && took {
                self.lava_burn_sound(id, output);
            }
        }
    }

    /// Entity event `event` of the player `id`, for whoever watches them and them.
    fn player_entity_event(&self, id: PlayerId, event: u8, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        output.events.push(RegionEvent::EntityEvent {
            entity: player.entity_id,
            chunk: player.chunk(),
            event,
        });
    }

    /// `lavaHurt`'s burning sound, heard by everyone: 0.4 loud, its pitch
    /// `2 + r · 0.4` from the player's stream.
    fn lava_burn_sound(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(mut random) = self.player_stream(id) else {
            return;
        };
        let start = random.clone();
        let pitch = 2.0 + random.next_float() * 0.4;
        self.player_drew(id, &random, &start);
        let Some(player) = self.players.get(&id) else {
            return;
        };
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_GENERIC_BURN,
            source: SOUND_SOURCE_PLAYERS,
            position: player.pose.position,
            volume: 0.4,
            pitch,
            except: None,
        });
    }

    /// What ends or caps a fall in `aiStep` before the blocks are walked: flying
    /// (`Player.aiStep @28-46`, not while riding), a glide's cap of 1 while the motion's
    /// y is above −0.5 (`checkFallDistanceAccumulation`), slow falling and levitation
    /// (`LivingEntity.aiStep @527-548`).
    fn player_fall_resets(&self, id: PlayerId, living: &mut Living) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let body = &mut living.body;
        if player.flying && player.riding.is_none() {
            body.fall_distance = 0.0;
        }
        if player.posture.fall_flying && body.motion[1] > -0.5 && body.fall_distance > 1.0 {
            body.fall_distance = 1.0;
        }
        if player.effects.has(Effect::SLOW_FALLING) || player.effects.has(Effect::LEVITATION) {
            body.fall_distance = 0.0;
        }
    }

    /// The body's sounds (fire put out), heard by everyone from the players' source
    /// (`playEntityOnFireExtinguishedSound`'s `playSound(null, …)`).
    fn player_sounds(&self, living: &mut Living, output: &mut TickOutput) {
        for sound in std::mem::take(&mut living.sounds) {
            let [x, y, z] = sound.position;
            output.events.push(RegionEvent::Sound {
                sound: sound.sound,
                source: SOUND_SOURCE_PLAYERS,
                position: clustine_world::Vec3 { x, y, z },
                volume: sound.volume,
                pitch: sound.pitch,
                except: None,
            });
        }
    }

    /// `freeze` of 1 to a player frozen through who can freeze, every 40 ticks of their
    /// age (`LivingEntity.aiStep @723-761`), after the tick's freezing.
    fn freeze_player(&mut self, id: PlayerId, living: &Living, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let age = i64::try_from(self.tick).unwrap_or(i64::MAX) - player.game_origin;
        if age.rem_euclid(FREEZE_EVERY) == 0
            && living.frozen >= crate::living::TICKS_TO_FREEZE
            && living.can_freeze
        {
            let source = DamageSource::of(DamageType::FREEZE);
            self.hurt_player(id, &source, 1.0, Hurting::default(), output);
        }
    }

    /// `LivingEntity.pushEntities @0-156`'s cramming of the player `id`: with more than
    /// 23 pushable entities in their box, a draw of one in four from their stream, and
    /// `cramming` of 6 if more than 23 of them ride nothing. The push itself is the
    /// carts' and boats' (`minecarts.rs`, `boats.rs`).
    fn cram_player(&mut self, id: PlayerId, living: &Living, output: &mut TickOutput) {
        if living.no_physics || self.player_dead(id) {
            return;
        }
        let bounds = living.body.bounds();
        let (touching, free) = self.pushables_in(id, &bounds);
        if touching < MAX_ENTITY_CRAMMING {
            return;
        }
        let Some(mut random) = self.player_stream(id) else {
            return;
        };
        let start = random.clone();
        let drawn = random.next_int(4);
        self.player_drew(id, &random, &start);
        if drawn == 0 && free > MAX_ENTITY_CRAMMING - 1 {
            let source = DamageSource::of(DamageType::CRAMMING);
            self.hurt_player(id, &source, 6.0, Hurting::default(), output);
        }
    }

    /// The pushable entities whose boxes meet `bounds` but the player `id`
    /// (`getPushableEntities`): the living mobs whose types are pushable, boats and
    /// minecarts, and the other living players who are not spectators; how many, and how
    /// many of them ride nothing.
    fn pushables_in(&self, id: PlayerId, bounds: &Aabb) -> (usize, usize) {
        let mut touching = 0;
        let mut free = 0;
        for (key, chunk) in self.entities_in(bounds) {
            let Some(entity) = self.entity_record(key, chunk) else {
                continue;
            };
            let pushable = match &entity.data {
                EntityData::Mob { body, .. } => {
                    crate::mobs::types::of(entity.kind).pushable()
                        && crate::mobs::part::decode::<crate::mobs::body::Vitals>(body)
                            .is_ok_and(|vitals| !vitals.dying())
                }
                _ => crate::physics::entities::is_pushable(entity.kind),
            };
            if pushable {
                touching += 1;
                if entity.riding.vehicle.is_none() {
                    free += 1;
                }
            }
        }
        for (other, player) in &self.players {
            if *other == id
                || player.dead
                || player.mode.current == clustine_world::GameMode::Spectator
            {
                continue;
            }
            let feet = player.pose.position;
            let (width, height, _) = dimensions(player.posture.pose);
            let box_of = Aabb::of_entity([feet.x, feet.y, feet.z], width, height);
            if box_of.intersects(*bounds) {
                touching += 1;
                if player.riding.is_none() {
                    free += 1;
                }
            }
        }
        (touching, free)
    }

    /// The body's air, fire, frozen ticks and fall distance kept in the player's vitals
    /// (a player whose abilities are invulnerable burns for a tick at most,
    /// `Player.setRemainingFireTicks @0-20`), with their absorption; what changed of
    /// their own entity told to their client, and their health where it changed.
    fn keep_body(
        &mut self,
        id: PlayerId,
        living: &Living,
        before: Own,
        on_fire: bool,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let invulnerable = player.mode.current.abilities().invulnerable;
        let fire = if invulnerable {
            living.fire_ticks.min(1)
        } else {
            living.fire_ticks
        };
        let vitals = &mut player.vitals;
        vitals.air = living.air;
        vitals.fire = fire;
        vitals.frozen = living.frozen;
        vitals.fall_distance = living.body.fall_distance;
        vitals.absorption = player.effects.absorption;
        let after = Own {
            air: vitals.air,
            fire: vitals.fire,
            frozen: vitals.frozen,
            absorption: vitals.absorption,
            health: player.health,
        };
        let (food, arrows) = (vitals.food, vitals.arrows);
        if after != before {
            self.journal.players.insert(id);
        }
        // The flag of the base tick, or none where the tick put the fire out. The fire
        // ticks above 0 at either end of the tick are where the flag may have changed.
        let on_fire = on_fire && after.fire > 0;
        let own_changed = after.air != before.air
            || after.frozen != before.frozen
            || after.absorption.to_bits() != before.absorption.to_bits()
            || before.fire > 0
            || after.fire > 0;
        if own_changed {
            output.player_events.push((
                id,
                PlayerEvent::OwnData(OwnData {
                    air: after.air,
                    absorption: after.absorption,
                    on_fire,
                    frozen: after.frozen,
                    arrows,
                }),
            ));
        }
        if after.health.to_bits() != before.health.to_bits() {
            output.player_events.push((
                id,
                PlayerEvent::Health {
                    health: after.health,
                    food: food.food,
                    saturation: food.saturation,
                },
            ));
        }
    }
}
