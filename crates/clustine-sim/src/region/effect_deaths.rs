//! What a mob's effects do as it is removed killed and as it is hurt
//! (`MobEffect.onMobRemoved`: oozing, weaving and wind charged; `onMobHurt`: infested;
//! `docs/groundwork/n9-plan.md`, section 5.2; part S1's), each as its n9 dump has it.
//!
//! - **Oozing** (`OozingMobEffect`): two slimes of size 2, fewer where the slimes about
//!   the dying mob (its box inflated by 2, itself left out) leave less than two below
//!   `max_entity_cramming` (24); each at its feet half a block up, turned by the level's
//!   `nextFloat() · 360`, added `TRIGGERED`.
//! - **Weaving** (`WeavingMobEffect`): `randomBetweenInclusive(2, 3)` cobwebs (the mob's
//!   own random, as is everything it draws here), at positions of
//!   `randomInCube(random, 15, its block, 1)` taken in the order drawn, each where the
//!   block can be replaced and the one below is sturdy on top and not taken already,
//!   until there are enough; then each set with every update in the order of the game's
//!   `HashSet` of them, with level event 3018. `mob_griefing` is on, so a mob of any type
//!   weaves. A cobweb in another region's land is a `SetIf` expecting what the copy shows
//!   (`set_block_updated`).
//! - **Wind charged** (`WindChargedMobEffect`): a burst of radius `3 + nextFloat() · 2`
//!   at its feet and half its height (`wind_bursts.rs`).
//! - **Infested** (`InfestedMobEffect`), at each hurt that took: where its
//!   `nextFloat() <= 0.1`, `randomBetweenInclusive(1, 2)` silverfish at its feet and half
//!   its height, each with its look times 0.3 (y 1.5 times that) turned about the up axis
//!   by `randomBetween(−π/2, π/2)` as its motion, turned by the level's
//!   `nextFloat() · 360`, added `TRIGGERED` with the silverfish's hurt sound.
//!
//! The mob's own random is its stream as the region gives it now (its own in its tick,
//! aside between them); the level's is its stream aside (`Random::of_entity_aside`),
//! which between its ticks is the same stream. A type not built yet
//! (`types::is_built`) makes nothing, as the game's `EntityType.create` gives null for
//! a type whose features are off: none of the made mob's draws are made, but the counts
//! and the chance are drawn as the game's.
//!
//! Named differences: a slime is made at its type's size until S9's slime gives a size
//! to set (`Slime.setSize(2, true)`); a hurt the mob takes in its own tick (poison, a fall,
//! fire) reaches no `onMobHurt` here, only those between its ticks (`mob_bodies.rs`'s
//! hook); a made mob's head is turned with its body.

use clustine_data::{Face, SupportType, blocks, entity_types, sounds};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId};

use super::Region;
use super::random_ticks::Random;
use super::updates::{UPDATE_ALL, UPDATE_LIMIT};
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::body::effects::Effect;
use crate::mobs::making::{construct, fresh_record};
use crate::mobs::types;
use crate::physics::Vec3;
use crate::physics::aabb::Aabb;

/// `GameRules.MAX_ENTITY_CRAMMING`'s default, which Clustine keeps.
const MAX_ENTITY_CRAMMING: i32 = 24;

/// `OozingMobEffect.RADIUS_TO_CHECK_SLIMES`.
const SLIME_RADIUS: f64 = 2.0;

/// The level event of a cobweb woven (`LevelEvent.PARTICLES_AND_SOUND_..._COBWEB`, 3018).
const COBWEB_WOVEN: i32 = 3018;

/// `InfestedMobEffect`'s chance (`0.1f`).
const INFESTED_CHANCE: f32 = 0.1;

/// `Math.PI / 2` as a `float` (`1.5707964f`).
const HALF_PI: f32 = 1.570_796_4;

/// The hostile mobs' sound source, which a silverfish's sounds are of.
const SOUND_SOURCE_HOSTILE: i32 = 5;

/// The streams of a mob's effects at its removal or its hurt: its own random, and the
/// level's where that is apart from it (in its own tick); between its ticks both are its
/// stream aside.
struct Streams {
    key: EntityId,
    own: Random,
    own_start: Random,
    level: Option<(Random, Random)>,
}

impl Streams {
    fn own(&mut self) -> &mut Random {
        &mut self.own
    }

    fn level(&mut self) -> &mut Random {
        match &mut self.level {
            Some((level, _)) => level,
            None => &mut self.own,
        }
    }
}

/// `Vector3f.rotateY(angle)` of `(x, z)` in JOML's `float`s: `sin`, and `cosFromSin`'s
/// square root with the sign of its quarter.
fn rotate_y(x: f32, z: f32, angle: f32) -> (f32, f32) {
    let sin = f64::from(angle).sin() as f32;
    let cos = {
        let cos = (1.0 - sin * sin).sqrt();
        let a = angle + HALF_PI;
        let two_pi = std::f32::consts::TAU;
        let mut b = a - (a / two_pi).trunc() * two_pi;
        if b < 0.0 {
            b += two_pi;
        }
        if b >= std::f32::consts::PI { -cos } else { cos }
    };
    (x * cos + z * sin, -x * sin + z * cos)
}

/// The order a Java `HashSet` of up to twelve block positions (sixteen buckets) gives
/// them: by bucket (`Vec3i.hashCode()`, spread by `h ^ (h >>> 16)`), and within one in
/// the order added.
pub(crate) fn hash_set_order(positions: &[BlockPos]) -> Vec<BlockPos> {
    let bucket = |pos: &BlockPos| {
        let hash = pos
            .y
            .wrapping_add(pos.z.wrapping_mul(31))
            .wrapping_mul(31)
            .wrapping_add(pos.x);
        let spread = hash ^ ((hash as u32) >> 16) as i32;
        spread & 15
    };
    let mut ordered = positions.to_vec();
    ordered.sort_by_key(bucket);
    ordered
}

impl Region {
    /// `triggerOnDeathMobEffects`: `onMobRemoved` of each effect, with its amplifier,
    /// that the region's mob `key` held as it was removed killed at `at`
    /// (`LivingEntity.remove(KILLED)` at the end of its death, after its drops and orbs,
    /// before it leaves the world), in the order of the effects.
    pub(crate) fn effects_at_removal(
        &mut self,
        key: EntityId,
        at: Vec3,
        effects: &[(Effect, u8)],
        output: &mut TickOutput,
    ) {
        let acts = |effect: &Effect| {
            [Effect::OOZING, Effect::WEAVING, Effect::WIND_CHARGED].contains(effect)
        };
        if !effects.iter().any(|(effect, _)| acts(effect)) {
            return;
        }
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let mut streams = self.streams_of(&record);
        for (effect, _) in effects {
            match *effect {
                Effect::OOZING => self.ooze(&record, at, &mut streams, output),
                Effect::WEAVING => self.weave(at, &mut streams, output),
                Effect::WIND_CHARGED => self.wind_charged(&record, at, &mut streams, output),
                _ => {}
            }
        }
        self.streams_drew(streams);
    }

    /// `onMobHurt` of each effect, with its amplifier, that the region's mob `key` at
    /// `at` holds after a hurt that took (the end of `LivingEntity.hurtServer`).
    pub(crate) fn effects_at_hurt(
        &mut self,
        key: EntityId,
        at: Vec3,
        effects: &[(Effect, u8)],
        output: &mut TickOutput,
    ) {
        if !effects
            .iter()
            .any(|(effect, _)| *effect == Effect::INFESTED)
        {
            return;
        }
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let mut streams = self.streams_of(&record);
        for (effect, _) in effects {
            if *effect == Effect::INFESTED {
                self.infest(&record, at, &mut streams, output);
            }
        }
        self.streams_drew(streams);
    }

    /// The streams of the mob of `record` as they stand now.
    fn streams_of(&self, record: &Entity) -> Streams {
        let key = record.key;
        let own = self.entity_stream(key).random;
        let ticking = matches!(self.ticking, Some((ticking, _)) if ticking == key);
        let level = ticking.then(|| {
            let mut level = Random::of_entity_aside(self.config.seed, key, record.lived, self.tick);
            for _ in 0..self.aside_draws.get(&key).copied().unwrap_or(0) {
                level.next_u64();
            }
            (level.clone(), level)
        });
        Streams {
            key,
            own_start: own.clone(),
            own,
            level,
        }
    }

    /// Notes what `streams` drew.
    fn streams_drew(&mut self, streams: Streams) {
        let Streams {
            key,
            own,
            own_start,
            level,
        } = streams;
        self.entity_drew(key, own.draws_since(&own_start));
        if let Some((level, start)) = level {
            let used = level.draws_since(&start);
            if used > 0 {
                *self.aside_draws.entry(key).or_default() += used;
            }
        }
    }

    /// The box of the mob of `record` at `at`.
    fn box_at(&self, record: &Entity, at: Vec3) -> Aabb {
        let size = self.entity_box(record).size();
        Aabb::of_entity(at, size[0] as f32, size[1] as f32)
    }

    /// `OozingMobEffect.onMobRemoved`: its slimes, as many as cramming leaves room for.
    fn ooze(&mut self, record: &Entity, at: Vec3, streams: &mut Streams, output: &mut TickOutput) {
        // `spawnedCount`, a constant 2: nothing drawn.
        let count = 2;
        let nearby = self.nearby_slimes(record, at, MAX_ENTITY_CRAMMING);
        let room = (MAX_ENTITY_CRAMMING - nearby).max(0).min(count);
        let made = if MAX_ENTITY_CRAMMING < 1 { count } else { room };
        let slime = entity_types::SLIME as u16;
        for _ in 0..made {
            if !types::is_built(slime) {
                // `EntityType.create` gives null: nothing made, nothing drawn.
                continue;
            }
            let yaw = streams.level().next_float() * 360.0;
            let feet = [at[0], at[1] + 0.5, at[2]];
            self.make_triggered(slime, feet, yaw, [0.0; 3], output);
        }
    }

    /// `NearbySlimes.closeTo(mob).count(most)`: the slimes (of that type alone) whose
    /// boxes meet the mob's inflated by 2, but itself, of the region's chunks and its
    /// copies, no more than `most`.
    pub(super) fn nearby_slimes(&self, record: &Entity, at: Vec3, most: i32) -> i32 {
        let bounds = self
            .box_at(record, at)
            .inflate([SLIME_RADIUS, SLIME_RADIUS, SLIME_RADIUS]);
        let slime = entity_types::SLIME as u16;
        let found = self
            .blast_targets(&bounds, record.key)
            .into_iter()
            .filter(|found| found.record.kind == slime)
            .count();
        i32::try_from(found).unwrap_or(i32::MAX).min(most.max(0))
    }

    /// `WeavingMobEffect.onMobRemoved`'s `spawnCobwebsRandomlyAround`.
    fn weave(&mut self, at: Vec3, streams: &mut Streams, output: &mut TickOutput) {
        let random = streams.own();
        let most = random.next_int_between(2, 3);
        let centre = crate::physics::containing(at);
        let mut taken: Vec<BlockPos> = Vec::new();
        for _ in 0..15 {
            let pos = BlockPos::new(
                centre.x - 1 + random.next_int(3) as i32,
                centre.y - 1 + random.next_int(3) as i32,
                centre.z - 1 + random.next_int(3) as i32,
            );
            if taken.contains(&pos) {
                continue;
            }
            let replaceable = self
                .block(pos)
                .is_some_and(|state| state.props().can_be_replaced());
            let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
            let sturdy = self
                .block(below)
                .is_some_and(|state| state.props().is_face_sturdy(Face::Up, SupportType::Full));
            if replaceable && sturdy {
                taken.push(pos);
                if i32::try_from(taken.len()).unwrap_or(i32::MAX) >= most {
                    break;
                }
            }
        }
        for pos in hash_set_order(&taken) {
            self.set_block_updated(pos, blocks::COBWEB, UPDATE_ALL, UPDATE_LIMIT, output);
            output.events.push(RegionEvent::LevelEvent {
                kind: COBWEB_WOVEN,
                position: pos,
                data: 0,
            });
        }
    }

    /// `WindChargedMobEffect.onMobRemoved`: the burst at its feet and half its height.
    fn wind_charged(
        &mut self,
        record: &Entity,
        at: Vec3,
        streams: &mut Streams,
        output: &mut TickOutput,
    ) {
        let height = self.entity_box(record).size()[1] as f32;
        let centre = [at[0], at[1] + f64::from(height / 2.0), at[2]];
        let radius = 3.0 + streams.own().next_float() * 2.0;
        // The burst's draws are the level's.
        match &mut streams.level {
            Some((level, _)) => self.wind_burst(record.key, centre, radius, level, output),
            None => {
                let own = &mut streams.own;
                self.wind_burst(record.key, centre, radius, own, output);
            }
        }
    }

    /// `InfestedMobEffect.onMobHurt`: its chance, then its silverfish.
    fn infest(
        &mut self,
        record: &Entity,
        at: Vec3,
        streams: &mut Streams,
        output: &mut TickOutput,
    ) {
        if streams.own().next_float() > INFESTED_CHANCE {
            return;
        }
        let count = streams.own().next_int_between(1, 2);
        let height = self.entity_box(record).size()[1];
        let feet = [at[0], at[1] + height / 2.0, at[2]];
        let silverfish = entity_types::SILVERFISH as u16;
        let look = crate::mobs::ai::goals::view_vector(record.pitch, record.yaw);
        for _ in 0..count {
            if !types::is_built(silverfish) {
                // `EntityType.create` gives null: nothing made, nothing drawn.
                continue;
            }
            let angle = streams.own().next_float() * (HALF_PI - -HALF_PI) + -HALF_PI;
            // `getLookAngle().toVector3f().mul(0.3f).mul(1, 1.5f, 1).rotateY(angle)`.
            let x = look[0] as f32 * 0.3;
            let y = look[1] as f32 * 0.3 * 1.5;
            let z = look[2] as f32 * 0.3;
            let (x, z) = rotate_y(x, z, angle);
            let motion = [f64::from(x), f64::from(y), f64::from(z)];
            let yaw = streams.level().next_float() * 360.0;
            if self
                .make_triggered(silverfish, feet, yaw, motion, output)
                .is_some()
            {
                // `playSound(SILVERFISH_HURT)`: at it, of its source, to everyone near.
                output.events.push(RegionEvent::Sound {
                    sound: sounds::ENTITY_SILVERFISH_HURT,
                    source: SOUND_SOURCE_HOSTILE,
                    position: clustine_world::Vec3::new(feet[0], feet[1], feet[2]),
                    volume: 1.0,
                    pitch: 1.0,
                    except: None,
                });
            }
        }
    }

    /// `EntityType.create(level, TRIGGERED)` of a built type `kind` (its constructor's
    /// draws from its own new stream), `snapTo(at, yaw, 0)`, its motion, and
    /// `addFreshEntity`: into the region's ticking chunk, or sent where its chunk is
    /// another's. Its key, where it made one.
    fn make_triggered(
        &mut self,
        kind: u16,
        at: Vec3,
        yaw: f32,
        motion: Vec3,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let chunk = ChunkPos::containing(at[0], at[2]);
        if !self.readable(chunk) {
            return None;
        }
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let (uuid, mob) = construct(kind, &mut own, yaw);
        self.entity_drew(key, own.draws_since(&start));
        let mut record = fresh_record(key, kind, uuid, at, yaw, &mob);
        record.motion = motion;
        if here {
            self.add_fresh_entity(record, output);
        } else {
            self.send_made(record, output);
        }
        Some(key)
    }
}
