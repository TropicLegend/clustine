//! The custom spawners (`docs/groundwork/n9-plan.md`, section 9.7): the phantoms of
//! the night and the patrols of the day (n9 PhantomSpawner, n9 PatrolSpawner), and the
//! region's side of a patrol's target given across a line (section 8.6).
//!
//! **The schedule.** The game keeps each spawner's countdown and draws its next delay
//! from the level's random; a region keeps neither. Each spawner's firings form a chain
//! from game time 0, each next one drawn first from a stream of the world, the spawner
//! and the firing's own game time (`Random::of_schedule`): the phantoms' `(60 +
//! nextInt(60)) · 20`, the patrols' `12 000 + nextInt(1200)`. Every region computes the
//! same chain; one computes it once and keeps its end in memory, and fires at its first
//! tick at or after a due time each due time after its previous tick's game time and
//! at or before its own, so that the clock's jump at a publication skips none. A new
//! region (a restore, a merge's survivor, a split's halves) counts from its first tick:
//! a firing due in the hand-over's tick may be missed or made twice (named, rare).
//!
//! **Phantoms**, by night: each region rolls for its own non-spectator players, each from
//! their stream aside for the tick, as the game rolls for each player of the level from
//! its one random. **Patrols**, by day: the draws go on in the firing's stream, the same
//! in every region, which choose a player of the world's table by `nextInt(size)` in
//! the order of player ids; only the region that holds that player goes on. Where two
//! regions hold tables a second apart that differ, a firing may make no patrol or two
//! (named, rare).
//!
//! The sea level is the world's (`RegionConfig::sea_level`: 63 for the terrain, −63 for
//! a flat world, as the game's generators have it), as the phantom's own rules read it.
//! A patrol's member faces south (yaw 0), where the game leaves the yaw its
//! constructor drew. Where a block or its light is not read, a phantom or a member is
//! not made there.

use clustine_data::{BlockState, entity_types, spawning};
use clustine_world::{BlockPos, ChunkPos, Difficulty, EntityId, EntityUse, PlayerId};

use super::Region;
use crate::api::TickOutput;
use crate::mobs::Mob;
use crate::mobs::ai::patrol;
use crate::mobs::making::{finalize, moon_brightness};
use crate::mobs::types::{SpawnGroup, SpawnReason, Spawning, illagers};
use crate::region::Random;

#[cfg(test)]
#[path = "custom_spawners_n9_tests.rs"]
mod custom_spawners_n9_tests;

/// The phantom spawner's mark among the schedules.
const PHANTOMS: u64 = 1;
/// The patrol spawner's mark among the schedules.
const PATROLS: u64 = 2;

/// The ticks awake below which a player is never visited (`nextInt(timeSinceRest) >=
/// 72000`).
const AWAKE: u32 = 72_000;

/// The overworld clock's tick from which the `early_game` timeline lets patrols come
/// (`timeline/early_game.json`: false from 0, true from 120 000, no period).
const PATROLS_FROM_DAY: u64 = 120_000;

/// The spectator's game mode.
const SPECTATOR: u8 = 3;

/// What the region keeps of the schedules, in memory only: each spawner's next due game
/// time once computed, and the game time of the region's previous tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Schedules {
    phantoms: Option<u64>,
    patrols: Option<u64>,
    last: Option<u64>,
}

/// The phantom spawner's next delay: `(60 + nextInt(60)) · 20` ticks.
fn phantom_delay(stream: &mut Random) -> u64 {
    u64::from(60 + stream.next_int(60)) * 20
}

/// The patrol spawner's next delay: `12 000 + nextInt(1200)` ticks.
fn patrol_delay(stream: &mut Random) -> u64 {
    12_000 + u64::from(stream.next_int(1200))
}

/// The firings of the spawner `spawner` of the world `world` due after the game time
/// `after` and at or before `upto`, each with its stream on from its delay's draw: the
/// chain from game time 0, or from `next` where it was computed before, which is left
/// at the first due time after `upto`.
pub(crate) fn firings(
    world: u64,
    spawner: u64,
    delay: fn(&mut Random) -> u64,
    next: &mut Option<u64>,
    after: u64,
    upto: u64,
) -> Vec<(u64, Random)> {
    let mut due = next.unwrap_or(0);
    let mut fired = Vec::new();
    while due <= upto {
        let mut stream = Random::of_schedule(world, spawner, due);
        let step = delay(&mut stream);
        if due > after {
            fired.push((due, stream));
        }
        due += step;
    }
    *next = Some(due);
    fired
}

/// `DifficultyInstance.getSpecialMultiplier()` of the effective difficulty `effective`.
fn special_multiplier(effective: f32) -> f32 {
    if effective < 2.0 {
        0.0
    } else if effective > 4.0 {
        1.0
    } else {
        (effective - 2.0) / 2.0
    }
}

/// `NaturalSpawner.isValidEmptySpawnBlock(level, pos, state, fluid, kind)`: no full
/// block's collision, no signal, no fluid, nothing that prevents mobs spawning inside it
/// and nothing dangerous to the type (n7 NaturalSpawner 787–853).
fn is_valid_empty_spawn_block(state: BlockState, kind: u16) -> bool {
    !state.voxel_collision_shape().is_block()
        && !state.props().is_signal_source()
        && state.props().fluid().is_none()
        && !clustine_blocks::data::in_tag(state, "minecraft:prevent_mob_spawning_inside")
        && !spawning::is_block_dangerous(kind, u32::from(state.0))
}

/// A player the phantom spawner rolls for: their stay's entity, their block, their game
/// mode and their time since rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sleeper {
    pub(crate) entity: EntityId,
    pub(crate) block: BlockPos,
    pub(crate) game_mode: u8,
    pub(crate) time_since_rest: u32,
}

impl Region {
    /// The custom spawners' phase of the tick, after the random ticks (the game's
    /// `tickCustomSpawners`, n7 ServerLevel): the phantoms' firings due since the
    /// region's previous tick, then the patrols'.
    pub(super) fn run_custom_spawners(&mut self, output: &mut TickOutput) {
        let now = self.clock.game_time;
        let schedules = &mut self.custom_spawners;
        let after = schedules.last.unwrap_or(now.saturating_sub(1));
        schedules.last = Some(now);
        let world = self.config.seed;
        let phantoms = firings(
            world,
            PHANTOMS,
            phantom_delay,
            &mut schedules.phantoms,
            after,
            now,
        );
        let patrols = firings(
            world,
            PATROLS,
            patrol_delay,
            &mut schedules.patrols,
            after,
            now,
        );
        for _ in phantoms {
            let sleepers = self.sleepers();
            self.spawn_phantoms(&sleepers, output);
        }
        for (_, stream) in patrols {
            self.spawn_patrol(stream, output);
        }
    }

    /// The region's own players, by id, as the phantom spawner reads them.
    fn sleepers(&self) -> Vec<Sleeper> {
        self.players
            .values()
            .map(|player| Sleeper {
                entity: player.entity_id,
                block: crate::physics::containing([
                    player.pose.position.x,
                    player.pose.position.y,
                    player.pose.position.z,
                ]),
                game_mode: player.face().game_mode,
                time_since_rest: player.time_since_rest(self.tick),
            })
            .collect()
    }

    /// `Level.canSeeSky(pos)`: the sky's light full there; not where it is not known.
    fn sees_sky(&self, pos: BlockPos) -> bool {
        self.light_at(pos).is_some_and(|light| light.sky >= 15)
    }

    /// `PhantomSpawner.tick`'s firing (n9 PhantomSpawner 71–426): nothing by day (sky
    /// darken below 5); else for each of `sleepers` not a spectator, at or above sea
    /// level under the open sky, from their stream aside: the difficulty's
    /// `isHarderThan(nextFloat() · 3)`, `nextInt(timeSinceRest) >= 72000`, a position 20
    /// to 34 above and within 10 along x and z, empty for a phantom, and there `1 +
    /// nextInt(difficulty + 1)` phantoms, each finalised (`NATURAL`) and made by section
    /// 4.1's rule.
    pub(crate) fn spawn_phantoms(&mut self, sleepers: &[Sleeper], output: &mut TickOutput) {
        if crate::time::sky_darken(self.clock.day) < 5 {
            return;
        }
        for sleeper in sleepers {
            if sleeper.game_mode == SPECTATOR {
                continue;
            }
            let block = sleeper.block;
            if block.y < self.config.sea_level || !self.sees_sky(block) {
                continue;
            }
            let effective = self.difficulty_at(block);
            let mut level = self.entity_stream(sleeper.entity).random;
            let start = level.clone();
            self.phantoms_for(sleeper, effective, &mut level, output);
            self.entity_drew(sleeper.entity, level.draws_since(&start));
        }
    }

    /// One player's roll and phantoms, drawing from `level`.
    fn phantoms_for(
        &mut self,
        sleeper: &Sleeper,
        effective: f32,
        level: &mut Random,
        output: &mut TickOutput,
    ) {
        if effective <= level.next_float() * 3.0 {
            return;
        }
        let awake = sleeper.time_since_rest.clamp(1, i32::MAX as u32);
        if level.next_int(awake) < AWAKE {
            return;
        }
        let up = 20 + level.next_int(15) as i32;
        let east = -10 + level.next_int(21) as i32;
        let south = -10 + level.next_int(21) as i32;
        let block = sleeper.block;
        let pos = BlockPos::new(block.x + east, block.y + up, block.z + south);
        let kind = entity_types::PHANTOM as u16;
        if !self
            .block(pos)
            .is_some_and(|state| is_valid_empty_spawn_block(state, kind))
        {
            return;
        }
        let count = 1 + level.next_int(Difficulty::WORLD.id() as u32 + 1);
        let difficulty = (effective, special_multiplier(effective));
        let mut group: Option<SpawnGroup> = None;
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        for _ in 0..count {
            let mut prepare = |region: &mut Region, mob: &mut Mob, own: &mut Random| {
                region.finalize_made(
                    kind,
                    mob,
                    (&mut *level, own),
                    pos,
                    SpawnReason::Natural,
                    difficulty,
                    &mut group,
                );
            };
            self.make_mob(kind, at, (0.0, 0.0), &mut prepare, output);
        }
    }

    /// `finalizeSpawn(level, difficulty, reason, group)` of a mob of the type `kind` made
    /// at the block `at`, drawing the level's part from `level` and its own from `own`.
    #[expect(
        clippy::too_many_arguments,
        reason = "`finalizeSpawn`'s arguments and what the region reads for them"
    )]
    fn finalize_made(
        &self,
        kind: u16,
        mob: &mut Mob,
        (level, own): (&mut Random, &mut Random),
        at: BlockPos,
        reason: SpawnReason,
        difficulty: (f32, f32),
        group: &mut Option<SpawnGroup>,
    ) {
        let day = self.clock.day;
        finalize(
            kind,
            &mut Spawning {
                mob,
                level,
                own,
                biome: self.biome_at(at),
                at,
                day,
                game_time: i64::try_from(self.clock.game_time).unwrap_or(i64::MAX),
                reason,
                group,
                structure: None,
                moon_brightness: moon_brightness(day),
                difficulty,
                companions: None,
            },
        );
    }

    /// `PatrolSpawner.tick`'s firing (n9 PatrolSpawner 70–428), its draws on from the
    /// delay's in `stream`: only by day (sky darken below 4, `isBrightOutside`), one
    /// firing in five, a player of the world's table chosen by `nextInt(size)` in the
    /// order of ids, whom the region must hold and who is no spectator, not near a
    /// village; 24 to 47 blocks off along x and z each, every chunk within 10 blocks of
    /// there the region's own, the `early_game` timeline past and no mushroom fields;
    /// then `ceil(difficulty) + 1` members, each on the surface, the first the leader,
    /// whose failure ends the patrol, each next one stepped by `nextInt(5) − nextInt(5)`
    /// along x and z.
    pub(crate) fn spawn_patrol(&mut self, mut stream: Random, output: &mut TickOutput) {
        if crate::time::sky_darken(self.clock.day) >= 4 {
            return;
        }
        if stream.next_int(5) != 0 {
            return;
        }
        let Some(table) = self.faces.world.as_ref() else {
            return;
        };
        let mut ids: Vec<PlayerId> = table.iter().map(|player| player.player).collect();
        ids.sort();
        let Ok(size) = u32::try_from(ids.len()) else {
            return;
        };
        if size < 1 {
            return;
        }
        let chosen = ids[stream.next_int(size) as usize];
        let Some(player) = self.players.get(&chosen) else {
            return;
        };
        if player.face().game_mode == SPECTATOR {
            return;
        }
        let block = crate::physics::containing([
            player.pose.position.x,
            player.pose.position.y,
            player.pose.position.z,
        ]);
        if self.village_near(block) {
            return;
        }
        let mut offset = || {
            let distance = 24 + stream.next_int(24) as i32;
            if stream.next_bool() {
                -distance
            } else {
                distance
            }
        };
        let dx = offset();
        let dz = offset();
        let mut pos = BlockPos::new(block.x + dx, block.y, block.z + dz);
        if !self.has_chunks_about(pos, 10) || !self.can_pillager_patrol_spawn(pos) {
            return;
        }
        let effective = self.difficulty_at(pos);
        let members = f64::from(effective).ceil() as i32 + 1;
        for member in 0..members {
            let Some(y) = self.motion_blocking_no_leaves_height(pos.x, pos.z) else {
                // The column is not read: no member there, the steps drawn all the same.
                if member == 0 {
                    break;
                }
                pos = self.patrol_step(pos, &mut stream);
                continue;
            };
            pos = BlockPos::new(pos.x, y, pos.z);
            let made = self.spawn_patrol_member(pos, member == 0, &mut stream, output);
            if member == 0 && !made {
                break;
            }
            pos = self.patrol_step(pos, &mut stream);
        }
    }

    /// The members' step: x and z each by `nextInt(5) − nextInt(5)`.
    #[expect(clippy::unused_self, reason = "beside the loop it steps")]
    fn patrol_step(&self, pos: BlockPos, stream: &mut Random) -> BlockPos {
        let x = pos.x + stream.next_int(5) as i32 - stream.next_int(5) as i32;
        let z = pos.z + stream.next_int(5) as i32 - stream.next_int(5) as i32;
        BlockPos::new(x, pos.y, z)
    }

    /// `ServerLevel.hasChunksAt(x − r, z − r, x + r, z + r)`: every chunk the square
    /// reaches the region's own and loaded (section 4.1's rule for a patrol's start).
    fn has_chunks_about(&self, pos: BlockPos, reach: i32) -> bool {
        let (low, high) = (
            BlockPos::new(pos.x - reach, 0, pos.z - reach).chunk(),
            BlockPos::new(pos.x + reach, 0, pos.z + reach).chunk(),
        );
        (low.x..=high.x)
            .all(|x| (low.z..=high.z).all(|z| self.chunks.contains_key(&ChunkPos::new(x, z))))
    }

    /// `gameplay/can_pillager_patrol_spawn` at `pos`: true by default, false in the
    /// mushroom fields (the biome's attribute, read at the biome's cell), and-ed with the
    /// `early_game` timeline, false before the overworld clock's tick 120 000.
    fn can_pillager_patrol_spawn(&self, pos: BlockPos) -> bool {
        self.clock.day >= PATROLS_FROM_DAY
            && self.biome_at(pos) != Some("minecraft:mushroom_fields")
    }

    /// `Level.getHeight(MOTION_BLOCKING_NO_LEAVES, x, z)`: the y above the highest block
    /// that blocks motion but leaves in the column, where the region reads its chunk.
    pub(crate) fn motion_blocking_no_leaves_height(&self, x: i32, z: i32) -> Option<i32> {
        let pos = BlockPos::new(x, 0, z);
        let chunk = self.readable_chunk(pos.chunk())?;
        let (x, z) = pos.in_chunk();
        Some(chunk.min_y() + i32::from(chunk.heights().motion_blocking_no_leaves[z << 4 | x]))
    }

    /// `PatrolSpawner.spawnPatrolMember(level, pos, random, leader)` (n9 PatrolSpawner
    /// 0–125): the block empty for a pillager, its block light 8 or less and the block
    /// below one a pillager may stand on (`checkPatrollingMonsterSpawnRules`, `PATROL`
    /// being no spawner's reason); then a pillager made at the block's corner by section
    /// 4.1's rule, a leader with a patrol target found before it is placed (from the
    /// origin, where `create` leaves it, as the game's), finalised for `PATROL` with the
    /// level's draws from `stream`. Whether it was made.
    fn spawn_patrol_member(
        &mut self,
        pos: BlockPos,
        leader: bool,
        stream: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let kind = entity_types::PILLAGER as u16;
        let Some(state) = self.block(pos) else {
            return false;
        };
        if !is_valid_empty_spawn_block(state, kind) {
            return false;
        }
        if self.light_at(pos).is_none_or(|light| light.block > 8) {
            return false;
        }
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        if !self
            .block(below)
            .is_some_and(|state| spawning::is_valid_spawn(u32::from(state.0), kind))
        {
            return false;
        }
        let effective = self.difficulty_at(pos);
        let difficulty = (effective, special_multiplier(effective));
        let mut group: Option<SpawnGroup> = None;
        let mut prepare = |region: &mut Region, mob: &mut Mob, own: &mut Random| {
            if leader {
                let target = patrol::find_patrol_target(BlockPos::new(0, 0, 0), own);
                illagers::update(mob, |raider| {
                    raider.patrol_leader = true;
                    raider.patrol_target = Some(target);
                    raider.patrolling = true;
                });
            }
            region.finalize_made(
                kind,
                mob,
                (&mut *stream, own),
                pos,
                SpawnReason::Patrol,
                difficulty,
                &mut group,
            );
        };
        let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        self.make_mob(kind, at, (0.0, 0.0), &mut prepare, output)
            .is_some()
    }

    /// `LongDistancePatrolGoal`'s word on the region's mob `key`: a mob that can join the
    /// patrol takes `pos` as its patrol target and patrols (`setPatrolTarget`).
    pub(crate) fn patrol_target_given(
        &mut self,
        key: EntityId,
        pos: BlockPos,
        output: &mut TickOutput,
    ) {
        let Some((_, entity)) = self.find_entity(key) else {
            return;
        };
        if !patrol::can_join_patrol(entity.kind) {
            return;
        }
        self.struck_between_ticks(key, output, |struck| {
            illagers::update(&mut *struck.mob, |raider| {
                raider.patrol_target = Some(pos);
                raider.patrolling = true;
            });
        });
    }

    /// A leader's `setPatrolTarget(pos)` of its companion `key` (`Local::PatrolTarget`):
    /// the region's own at once, one in a copy through its holder.
    pub(crate) fn patrol_target_set(
        &mut self,
        key: EntityId,
        pos: BlockPos,
        output: &mut TickOutput,
    ) {
        if self.find_entity(key).is_some() {
            self.patrol_target_given(key, pos, output);
        } else {
            self.send_on_entity(key, EntityUse::PatrolTarget { pos });
        }
    }
}
