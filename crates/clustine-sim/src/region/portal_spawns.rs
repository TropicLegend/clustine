//! Zombified piglins from Nether portal blocks (`NetherPortalBlock.randomTick`, d
//! NetherPortalBlock.txt 36–101 of the brief's section 4.2; family E1 of P12,
//! `docs/groundwork/p-plan.md`, sections 18.3, 18.5 and 18.6).
//!
//! On a portal block's random tick, where mobs spawn (`isSpawningMonsters`, the game
//! rule `spawn_mobs`; `spawn_monsters` is on, as Clustine keeps it), the difficulty is
//! not peaceful, the level's `nether_portal_spawns_piglin` holds (the overworld's; not
//! the Nether's nor the End's) and `nextInt(2000)` of the tick's random is under the
//! difficulty's id (one in 2000 at easy), with a player close enough to spawn by
//! (`anyPlayerCloseEnoughForSpawning`): down through the portal's blocks to the one under
//! them; where a zombified piglin may spawn on that (`isValidSpawn`), one is made on it
//! (`EntityType.spawn(level, pos, STRUCTURE)`) and set on its portal cooldown, and its
//! vehicle too.
//!
//! The making is `EntityType.spawn`'s: at the middle of the block's bottom, its yaw the
//! level's draw (the tick's random) and its `finalizeSpawn` for `STRUCTURE` of the same,
//! the difficulty at its block; added with what that made beside it (a baby's chicken,
//! `addFreshEntityWithPassengers`); then its ambient sound (`playAmbientSound`). In a
//! chunk the region copies it is made and sent to the chunk's holder with nothing made
//! beside it, as an egg's mob is (named, `n7k-plan.md`, section 10); where the region can
//! read nothing it is not made.

use clustine_blocks::data::is_block;
use clustine_data::{BlockState, blocks, entity_types, spawning};
use clustine_world::{BlockPos, ChunkPos, Difficulty, Entity, EntityId};

use super::portals::DIMENSION_CHANGING_DELAY;
use super::riding::wrap_degrees;
use super::spawn_caps::SpawnState;
use super::{Random, Region};
use crate::api::{PublishedCounts, RegionEvent, TickOutput};
use crate::mobs::body::hurt::mob_voice_pitch;
use crate::mobs::making::{construct, finalize, fresh_record, moon_brightness};
use crate::mobs::types::{self, SpawnReason, Spawning};
use crate::physics::aabb::Aabb;

#[cfg(test)]
#[path = "portal_spawns_tests.rs"]
mod portal_spawns_tests;

impl Region {
    /// A random tick of `state` at `pos`, where it is a portal block: whether it was
    /// one, which ends the random tick's search for a rule.
    pub(super) fn portal_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        if !is_block(state, blocks::NETHER_PORTAL) {
            return false;
        }
        let difficulty = Difficulty::WORLD;
        if !self.config.spawn_mobs
            || difficulty == Difficulty::Peaceful
            || !self.config.dimension.rules().nether_portal_spawns_piglin
        {
            return true;
        }
        if (random.next_int(2000) as i32) >= difficulty.id() {
            return true;
        }
        if !self.any_player_close_for_spawning(pos.chunk()) {
            return true;
        }
        // Down through the portal's blocks; a block the region cannot read ends it, as
        // the game's unloaded block is air.
        let mut below = pos;
        while self
            .block(below)
            .is_some_and(|state| is_block(state, blocks::NETHER_PORTAL))
        {
            below = below.offset(0, -1, 0);
        }
        let kind = entity_types::ZOMBIFIED_PIGLIN as u16;
        let valid = self
            .block(below)
            .is_some_and(|ground| spawning::is_valid_spawn(u32::from(ground.0), kind));
        if valid {
            self.spawn_portal_piglin(below.offset(0, 1, 0), random, output);
        }
        true
    }

    /// `ServerLevel.anyPlayerCloseEnoughForSpawning(pos)` of the chunk `chunk`: some
    /// player not a spectator within 128 blocks of its centre, and some player's chunk
    /// within 8 of it (the distance manager's), of the players spawning reads.
    fn any_player_close_for_spawning(&self, chunk: ChunkPos) -> bool {
        let counts = std::collections::BTreeMap::new();
        SpawnState::new(
            self.spawning_players(),
            &counts,
            &PublishedCounts::default(),
            false,
        )
        .may_spawn_in(chunk)
    }

    /// `ZOMBIFIED_PIGLIN.spawn(level, pos, STRUCTURE)` and `setPortalCooldown()` on it
    /// and its vehicle (NetherPortalBlock.txt 106–146), the tick's random `level` as the
    /// level's.
    fn spawn_portal_piglin(&mut self, pos: BlockPos, level: &mut Random, output: &mut TickOutput) {
        let kind = entity_types::ZOMBIFIED_PIGLIN as u16;
        let chunk = pos.chunk();
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return;
        }
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return;
        };
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        // `EntityType.create`: the mob's own draws (its UUID, its constructor's), then
        // the level's yaw, then `finalizeSpawn` with the level's draws.
        let mut own = self.entity_stream(key).random;
        let own_start = own.clone();
        let (uuid, mut mob) = construct(kind, &mut own, 0.0);
        let yaw = wrap_degrees(level.next_float() * 360.0);
        mob.body.head.head_yaw = yaw;
        mob.body.head.body_yaw = yaw;
        let biome = self.biome_at(pos);
        let day = self.clock.day;
        let game_time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let effective = self.difficulty_at(pos);
        let mut companions = PortalCompanions {
            region: self,
            chunk,
            at,
            made: Vec::new(),
            rides: None,
        };
        finalize(
            kind,
            &mut Spawning {
                mob: &mut mob,
                level,
                own: &mut own,
                biome,
                at: pos,
                day,
                game_time,
                reason: SpawnReason::Structure,
                group: &mut None,
                structure: None,
                moon_brightness: moon_brightness(day),
                difficulty: (effective, special_multiplier(effective)),
                companions: here.then_some(&mut companions as &mut dyn types::Companions),
            },
        );
        let rides = companions.rides;
        let made = companions.made;
        let mut record: Entity = fresh_record(key, kind, uuid, at, yaw, &mob);
        // `setPortalCooldown()`: a mob carrying no player waits 300 ticks.
        record.portal.cooldown = DIMENSION_CHANGING_DELAY;
        // `playAmbientSound()`, of its own stream, after it is added.
        let now = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let mob_type = types::of(kind);
        let sound = mob_type
            .ambient_sound_drawn(&mob, &mut own, now, 0, None)
            .map(|sound| (sound, mob_voice_pitch(mob_type, &mob, &mut own)));
        if here {
            self.entity_drew(key, own.draws_since(&own_start));
            let (vehicles, riders): (Vec<_>, Vec<_>) = made
                .into_iter()
                .partition(|made| made.seat == types::CompanionSeat::Vehicle);
            let vehicle = vehicles.first().map(|made| made.entity.key).or(rides);
            for mut made in vehicles {
                // Its vehicle's cooldown too: one that carries a mob waits 300.
                made.entity.portal.cooldown = DIMENSION_CHANGING_DELAY;
                self.add_portal_companion(made, output);
            }
            self.add_fresh_entity(record, output);
            if let Some(vehicle) = vehicle {
                if rides.is_some() {
                    self.cool_down(vehicle, output);
                }
                self.start_riding(key, vehicle, true, output);
            }
            for made in riders {
                let rider = made.entity.key;
                self.add_portal_companion(made, output);
                self.start_riding(rider, key, true, output);
            }
        } else {
            self.send_made(record, output);
        }
        if let Some((sound, pitch)) = sound {
            output.events.push(RegionEvent::Sound {
                sound,
                source: crate::mobs::body::ladder::sound_source(kind),
                position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                volume: mob_type.sound_volume_of(&mob),
                pitch,
                except: None,
            });
        }
    }

    /// Adds a mob made beside the portal's, its own draws counted.
    fn add_portal_companion(&mut self, made: types::Companion, output: &mut TickOutput) {
        let start = self.entity_stream(made.entity.key).random;
        self.entity_drew(made.entity.key, made.own.draws_since(&start));
        self.add_fresh_entity(made.entity, output);
    }

    /// `setPortalCooldown()` of the region's entity `key` already there (a chicken a
    /// baby rides): 300 ticks, as it carries a mob.
    fn cool_down(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((chunk, entity)) = self.find_entity(key) else {
            return;
        };
        let mut portal = entity.portal;
        portal.cooldown = DIMENSION_CHANGING_DELAY;
        self.commit_entity(
            chunk,
            key,
            vec![clustine_world::EntityField::Portal(portal)],
            output,
        );
    }
}

/// `DifficultyInstance.getSpecialMultiplier()`: 0 below an effective difficulty of 2, 1
/// above 4, a straight line between.
fn special_multiplier(effective: f32) -> f32 {
    if effective < 2.0 {
        0.0
    } else if effective > 4.0 {
        1.0
    } else {
        (effective - 2.0) / 2.0
    }
}

/// The portal's side of a mob made beside its zombified piglin (a baby's chicken): its
/// key from the region's block for the chunk, and its stream.
struct PortalCompanions<'r> {
    region: &'r mut Region,
    chunk: ChunkPos,
    at: crate::physics::Vec3,
    made: Vec<types::Companion>,
    rides: Option<EntityId>,
}

impl types::Companions for PortalCompanions<'_> {
    fn next(&mut self) -> Option<(EntityId, Random)> {
        let key = self.region.take_entity_key(self.chunk)?;
        Some((key, self.region.entity_stream(key).random))
    }

    fn add(&mut self, companion: types::Companion) {
        self.made.push(companion);
    }

    fn nearby(&self, bounds: &Aabb) -> Vec<(EntityId, u16, bool)> {
        self.region
            .entities_in(bounds)
            .into_iter()
            .filter_map(|(key, _)| {
                let (_, entity) = self.region.find_entity(key)?;
                let ridden =
                    !entity.riding.passengers.is_empty() || entity.riding.vehicle.is_some();
                Some((key, entity.kind, ridden))
            })
            .collect()
    }

    fn ride(&mut self, vehicle: EntityId) {
        self.rides = Some(vehicle);
    }

    fn position(&self) -> Option<crate::physics::Vec3> {
        Some(self.at)
    }
}
