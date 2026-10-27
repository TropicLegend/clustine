//! The custom spawners (`docs/groundwork/n9-plan.md`, section 9.7): the phantoms of
//! the night and the patrols of the day (n9 PhantomSpawner, n9 PatrolSpawner), and the
//! region's side of a patrol's target given across a line (section 8.6); the cats, the
//! siege and the wandering trader (`docs/groundwork/n10-plan.md`, section 10 and
//! decisions 11 and 12; n7 CatSpawner, n9 VillageSiege, n7 WanderingTraderSpawner).
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
//! **Cats** fire every 1 200 ticks as a chain of their own (the game's from its first
//! tick, not saved), choosing their player as the patrols do. **The siege** is one a
//! world: at the overworld clock's `roll_village_siege` marker the world's stream of
//! that tick rolls for it and chooses its player from the world's table, and the region
//! that holds that player runs it, its draws from a world stream of each tick. **The
//! trader** fires every 24 000 ticks as a chain (its delay depends on nothing else), its
//! own random the firing's stream; its chance depends on how the firings went, and is
//! the world's: each firing's is told the store, which keeps the latest firing's (of one
//! firing, the lowest) and publishes it with the clock ([`Region::trader_chance`], plan
//! 9.6).
//!
//! Named differences of these: a region that did not tick at a siege's roll (it began
//! that night) knows of no siege that night; a player who crosses a line in the night of
//! their siege may meet a second one set up by the region they come to; the regions
//! that do not hold a trader firing's player keep the raised chance where the game's,
//! where the trader was made, is back at 25, until the store's next publication (a
//! second at most); a llama made in
//! another region's land than its trader's is not leashed to it; the players chosen
//! from are the world's table's, alive or not; a siege's zombie and a cat make no
//! jockey; a siege's zombie faces its drawn yaw with its head and body to the south.
//!
//! The sea level is the world's (`RegionConfig::sea_level`: 63 for the terrain, −63 for
//! a flat world, as the game's generators have it), as the phantom's own rules read it.
//! A patrol's member faces south (yaw 0), where the game leaves the yaw its
//! constructor drew. Where a block or its light is not read, a phantom or a member is
//! not made there.

use clustine_data::spawning::Heightmap;
use clustine_data::{BlockState, entity_types, spawning};
use clustine_world::poi::PoiType;
use clustine_world::{
    BlockPos, ChunkPos, Difficulty, EntityId, EntityUse, KeptKind, LeashHolder, PlayerId,
};

use super::Region;
use super::pois::Occupancy;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::ai::{maths, patrol};
use crate::mobs::body::Home;
use crate::mobs::body::hurt::mob_voice_pitch;
use crate::mobs::making::{construct, finalize, fresh_record, moon_brightness};
use crate::mobs::types::{
    self, SpawnGroup, SpawnReason, Spawning, animal, illagers, wandering_trader,
};
use crate::physics::aabb::Aabb;
use crate::region::Random;
use crate::spawning::rules;

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
    pub(super) last: Option<u64>,
    pub(super) cats: Option<u64>,
    pub(super) traders: Option<u64>,
    /// The trader spawner's chance and the firing that left it, once one did
    /// ([`Region::trader_chance`]).
    pub(super) trader_chance: Option<(u64, i32)>,
    pub(super) siege: Siege,
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
        // `spawn_mobs` off: no spawner fires, and none catches up when it is on again.
        if !self.config.spawn_mobs {
            self.custom_spawners.last = None;
            // `VillageSiege.tick` without enemies: the siege is over.
            self.custom_spawners.siege = Siege::default();
            return;
        }
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
        self.run_village_spawners(after, now, output);
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
        // `isCloseToVillage(pos, 2)`, through the switch of `pois.rs`.
        if self.close_to_village(block, 2) {
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
        let joins = crate::mobs::Mob::decode(&entity.data).is_ok_and(|mob| {
            patrol::can_join_patrol(entity.kind, &mob, |raid, wave| {
                self.raid_known(raid, wave)
                    .is_some_and(|known| known.is_active())
            })
        });
        if !joins {
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

// ---------------------------------------------------------------------------------
// Cats, the siege and the wandering trader (`docs/groundwork/n10-plan.md`, section 10).

/// The cat spawner's mark among the schedules.
pub(super) const CATS: u64 = 3;
/// The trader spawner's mark among the schedules.
pub(super) const TRADERS: u64 = 4;
/// The mark of the stream of a night's roll for a siege.
pub(super) const SIEGE_NIGHTS: u64 = 5;
/// The mark of the stream of a tick's draws of a siege.
pub(super) const SIEGE_TICKS: u64 = 6;

/// The cat spawner's cadence (`TICK_DELAY`), which it fires at from its first tick.
const CAT_DELAY: u64 = 1_200;
/// The trader spawner's delay (`DEFAULT_SPAWN_DELAY`): its `spawnDelay` less 1 200 every
/// 1 200 ticks, firing at nothing, and back to 24 000.
const TRADER_DELAY: u64 = 24_000;
/// The trader's chance at its least, and at its most (`MIN_SPAWN_CHANCE`,
/// `MAX_SPAWN_CHANCE`).
pub(super) const MIN_TRADER_CHANCE: i32 = 25;
const MAX_TRADER_CHANCE: i32 = 75;
/// How long a trader the spawner makes stays, and how far from its spot it keeps.
pub(super) const TRADER_STAYS: i32 = 48_000;
const TRADER_HOME_RADIUS: i32 = 16;

/// The overworld clock's `roll_village_siege` marker in its day (`timeline/day.json`).
const ROLL_VILLAGE_SIEGE: u64 = 18_000;
/// How many zombies a siege makes, and every how many ticks.
pub(super) const SIEGE_ZOMBIES: i32 = 20;
const SIEGE_SPAWN_EVERY: i32 = 2;

/// The cat spawner's delay: its cadence, no draw.
fn cat_delay(_: &mut Random) -> u64 {
    CAT_DELAY
}

/// The trader spawner's delay: no draw.
fn trader_delay(_: &mut Random) -> u64 {
    TRADER_DELAY
}

/// The siege of the night (`VillageSiege`, in memory only, as the game keeps it): the
/// night's roll and its player, from the world's stream of the roll's tick and the
/// world's table of players, the same in every region; and, in the region that holds
/// that player, how far the siege has come.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Siege {
    /// The player of the night's siege (`SIEGE_TONIGHT`); none when it is done.
    pub(super) player: Option<PlayerId>,
    /// `hasSetupSiege`.
    pub(super) set_up: bool,
    /// `zombiesToSpawn`, `nextSpawnTime`, and where they come (`spawnX`, `spawnY`,
    /// `spawnZ`).
    pub(super) zombies: i32,
    pub(super) next_spawn: i32,
    pub(super) at: Option<BlockPos>,
}

/// `Entity.snapTo(BlockPos, ...)` and `Vec3.atBottomCenterOf`: the middle of a block's
/// bottom.
fn bottom_centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// A mob made by the spawners here, and what its `playAmbientSound` gave
/// (`EntityType.spawn`'s last step).
type Sound = Option<(i32, f32)>;

impl Region {
    /// The cat spawner's, the siege's and the trader spawner's part of the custom
    /// spawners' phase, after the patrols' (the brief's 8.5: phantoms, patrols, cats,
    /// the siege, the trader).
    fn run_village_spawners(&mut self, after: u64, now: u64, output: &mut TickOutput) {
        let world = self.config.seed;
        let schedules = &mut self.custom_spawners;
        let cats = firings(world, CATS, cat_delay, &mut schedules.cats, after, now);
        let traders = firings(
            world,
            TRADERS,
            trader_delay,
            &mut schedules.traders,
            after,
            now,
        );
        for (_, stream) in cats {
            self.spawn_cats(stream, output);
        }
        self.tick_siege(output);
        // The chain starts at game time 0, where the game's delay starts at 24 000.
        for (due, stream) in traders.into_iter().filter(|(due, _)| *due > 0) {
            self.trader_fired(due, stream, output);
        }
    }

    /// The player of the world's table that a firing's `nextInt(size)` chooses, in the
    /// order of ids (`getRandomPlayer`, as the patrol spawner chooses): none drawn where
    /// the table is empty, and none known where the region has not heard of it.
    fn random_world_player(&self, stream: &mut Random) -> Option<Option<PlayerId>> {
        let table = self.faces.world.as_ref()?;
        let mut ids: Vec<PlayerId> = table.iter().map(|player| player.player).collect();
        ids.sort();
        let size = u32::try_from(ids.len()).ok()?;
        if size == 0 {
            return Some(None);
        }
        Some(Some(ids[stream.next_int(size) as usize]))
    }

    /// The block a player of the region's own stands in, with their game mode.
    fn own_player_block(&self, player: PlayerId) -> Option<(BlockPos, u8)> {
        let held = self.players.get(&player)?;
        let block = crate::physics::containing([
            held.pose.position.x,
            held.pose.position.y,
            held.pose.position.z,
        ]);
        Some((block, held.face().game_mode))
    }

    /// The entities of the type `kind` of the region's land and copies whose boxes meet
    /// `bounds` (`getEntitiesOfClass(class, box).size()`).
    fn count_in(&self, kind: i32, bounds: &Aabb) -> usize {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut count = 0;
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let position = ChunkPos::new(x, z);
                for chunk in [self.chunks.get(&position), self.copies.get(&position)]
                    .into_iter()
                    .flatten()
                {
                    count += chunk
                        .entities()
                        .iter()
                        .filter(|entity| {
                            i32::from(entity.kind) == kind
                                && self.entity_box(entity).intersects(*bounds)
                        })
                        .count();
                }
            }
        }
        count
    }

    /// `Level.getHeight(map, x, z)` where the region reads the column.
    fn height_of(&self, map: Heightmap, x: i32, z: i32) -> Option<i32> {
        let pos = BlockPos::new(x, 0, z);
        let chunk = self.readable_chunk(pos.chunk())?;
        let (x, z) = pos.in_chunk();
        let heights = chunk.heights();
        let column = z << 4 | x;
        let height = match map {
            Heightmap::MotionBlocking => heights.motion_blocking[column],
            Heightmap::MotionBlockingNoLeaves => heights.motion_blocking_no_leaves[column],
            Heightmap::WorldSurface => heights.world_surface[column],
            Heightmap::OceanFloor => heights.ocean_floor[column],
        };
        Some(chunk.min_y() + i32::from(height))
    }

    /// `playAmbientSound()` of a mob just made: the type's sound and its voice's pitch,
    /// drawn of its own stream.
    fn ambient_of_made(&self, kind: u16, mob: &Mob, own: &mut Random) -> Sound {
        let mob_type = types::of(kind);
        let now = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let sound = mob_type.ambient_sound_drawn(mob, own, now, 0, None)?;
        Some((sound, mob_voice_pitch(mob_type, mob, own)))
    }

    /// The ambient sound `sound` of the mob of the type `kind` made at `at`.
    fn play_made(&self, kind: u16, mob: &Mob, at: [f64; 3], sound: Sound, output: &mut TickOutput) {
        let Some((sound, pitch)) = sound else {
            return;
        };
        output.events.push(RegionEvent::Sound {
            sound,
            source: crate::mobs::body::ladder::sound_source(kind),
            position: clustine_world::Vec3::new(at[0], at[1], at[2]),
            volume: types::of(kind).sound_volume_of(mob),
            pitch,
            except: None,
        });
    }

    /// [`Region::make_mob`] of a mob whose yaw `prepare` decides, after what it draws of
    /// the level's random (`snapTo` after `finalizeSpawn`, as the siege makes its
    /// zombies): constructed facing south, `prepare` done to it, and made facing what it
    /// gave.
    fn make_mob_facing(
        &mut self,
        kind: u16,
        at: [f64; 3],
        prepare: &mut dyn FnMut(&mut Region, &mut Mob, &mut Random) -> f32,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return None;
        }
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let (uuid, mut mob) = construct(kind, &mut own, 0.0);
        let yaw = prepare(self, &mut mob, &mut own);
        let record = fresh_record(key, kind, uuid, at, yaw, &mob);
        if here {
            self.entity_drew(key, own.draws_since(&start));
            self.add_fresh_entity(record, output);
        } else {
            self.send_made(record, output);
        }
        Some(key)
    }

    // -----------------------------------------------------------------------------
    // Cats.

    /// `CatSpawner.tick`'s firing (n7 CatSpawner 13–216), its draws in the firing's
    /// stream: a player of the world's table (only their region goes on); 8 to 31 blocks
    /// off along x and z each, each way by a `nextBoolean`, at the player's height;
    /// every chunk within 10 blocks the region's; a place a cat may stand; then in a
    /// village (`isCloseToVillage(pos, 2)`) with more than four occupied homes within 48
    /// and fewer than five cats in its block grown by 48, 8 and 48, a stray; else in a
    /// swamp hut with no cat in its block grown by 16, 8 and 16, a persistent one.
    pub(crate) fn spawn_cats(&mut self, mut stream: Random, output: &mut TickOutput) {
        let Some(Some(chosen)) = self.random_world_player(&mut stream) else {
            return;
        };
        let Some((block, _)) = self.own_player_block(chosen) else {
            return;
        };
        let mut offset = || {
            let distance = 8 + stream.next_int(24) as i32;
            if stream.next_bool() {
                -distance
            } else {
                distance
            }
        };
        let dx = offset();
        let dz = offset();
        let pos = BlockPos::new(block.x + dx, block.y, block.z + dz);
        if !self.has_chunks_about(pos, 10) {
            return;
        }
        let kind = entity_types::CAT as u16;
        if !rules::is_spawn_position_ok(kind, &self.spawn_world(), pos) {
            return;
        }
        let around = |x: f64, y: f64, z: f64| Aabb::of_block(pos).inflate([x, y, z]);
        if self.close_to_village(pos, 2) {
            let homes = self
                .pois_in_range(pos, 48, |kind| kind == PoiType::Home, Occupancy::IsOccupied)
                .len();
            if homes > 4 && self.count_in(entity_types::CAT, &around(48.0, 8.0, 48.0)) < 5 {
                self.spawn_cat(pos, false, &mut stream, output);
            }
        } else if self
            .structures_at(pos)
            .iter()
            .any(|kept| kept.kind == KeptKind::SwampHut)
            && self.count_in(entity_types::CAT, &around(16.0, 8.0, 16.0)) == 0
        {
            self.spawn_cat(pos, true, &mut stream, output);
        }
    }

    /// `CatSpawner.spawnCat(pos, level, persistent)`: a cat made for `NATURAL`, finalised
    /// where it was made (the origin, before it is placed: its variant is the origin's),
    /// persistent in a hut, and placed at the middle of the block's bottom facing south.
    fn spawn_cat(
        &mut self,
        pos: BlockPos,
        persistent: bool,
        stream: &mut Random,
        output: &mut TickOutput,
    ) {
        let kind = entity_types::CAT as u16;
        let effective = self.difficulty_at(pos);
        let difficulty = (effective, special_multiplier(effective));
        let mut group: Option<SpawnGroup> = None;
        let mut prepare = |region: &mut Region, mob: &mut Mob, own: &mut Random| {
            region.finalize_made(
                kind,
                mob,
                (&mut *stream, own),
                BlockPos::new(0, 0, 0),
                SpawnReason::Natural,
                difficulty,
                &mut group,
            );
            if persistent {
                mob.body.flags.persistent = true;
            }
        };
        self.make_mob(kind, bottom_centre(pos), (0.0, 0.0), &mut prepare, output);
    }

    // -----------------------------------------------------------------------------
    // The siege.

    /// `VillageSiege.tick` (n9 VillageSiege 28–330), every tick while mobs spawn: by day
    /// the siege is over; at the overworld clock's `roll_village_siege` marker the world's
    /// stream of that tick rolls `nextInt(10) == 0` for a siege tonight and chooses its
    /// player from the world's table; the region holding that player sets it up as soon
    /// as the player stands in a village, and then makes a zombie every third tick, twenty
    /// in all.
    pub(crate) fn tick_siege(&mut self, output: &mut TickOutput) {
        if crate::time::sky_darken(self.clock.day) < 4 {
            self.custom_spawners.siege = Siege::default();
            return;
        }
        if self.clock.day % crate::time::DAY == ROLL_VILLAGE_SIEGE {
            let mut stream = Random::of_schedule(self.config.seed, SIEGE_NIGHTS, self.clock.day);
            let player = if stream.next_int(10) == 0 {
                self.random_world_player(&mut stream).flatten()
            } else {
                None
            };
            self.custom_spawners.siege.player = player;
        }
        let Some(player) = self.custom_spawners.siege.player else {
            return;
        };
        let mut stream = Random::of_schedule(self.config.seed, SIEGE_TICKS, self.clock.game_time);
        if !self.custom_spawners.siege.set_up {
            if !self.set_up_siege(player, &mut stream) {
                return;
            }
            self.custom_spawners.siege.set_up = true;
        }
        let siege = &mut self.custom_spawners.siege;
        if siege.next_spawn > 0 {
            siege.next_spawn -= 1;
            return;
        }
        siege.next_spawn = SIEGE_SPAWN_EVERY;
        if siege.zombies > 0 {
            siege.zombies -= 1;
            if let Some(at) = siege.at {
                self.siege_zombie(at, &mut stream, output);
            }
        } else {
            siege.player = None;
        }
    }

    /// `tryToSetupSiege(level)` for the night's player: where the region holds them, they
    /// are no spectator and stand in a village of a biome that has sieges, up to ten
    /// angles (`nextFloat() · 2π`) 32 blocks off, the first from which a zombie's place
    /// is found giving where the zombies come; set up whether one was found or not.
    fn set_up_siege(&mut self, player: PlayerId, stream: &mut Random) -> bool {
        let Some((block, game_mode)) = self.own_player_block(player) else {
            return false;
        };
        if game_mode == SPECTATOR || !self.close_to_village(block, 1) {
            return false;
        }
        if animal::biome_in_tag(
            self.zoomed_biome_at(block),
            "minecraft:without_zombie_sieges",
        ) {
            return false;
        }
        for _ in 0..10 {
            let angle = stream.next_float() * 6.283_185_5;
            let dx = maths::floor_f32(clustine_noise::trig::cos(f64::from(angle)) * 32.0);
            let dz = maths::floor_f32(clustine_noise::trig::sin(f64::from(angle)) * 32.0);
            let at = BlockPos::new(block.x + dx, block.y, block.z + dz);
            self.custom_spawners.siege.at = Some(at);
            if self.siege_spawn_pos(at, stream).is_some() {
                self.custom_spawners.siege.next_spawn = 0;
                self.custom_spawners.siege.zombies = SIEGE_ZOMBIES;
                break;
            }
        }
        true
    }

    /// `findRandomSpawnPos(level, pos)`: ten tries of `nextInt(16) − 8` along x and z, on
    /// the surface (`WORLD_SURFACE`), in a village, passing the monsters' rules for a
    /// zombie with their draws; the middle of its bottom.
    fn siege_spawn_pos(&self, around: BlockPos, stream: &mut Random) -> Option<[f64; 3]> {
        let kind = entity_types::ZOMBIE as u16;
        for _ in 0..10 {
            let x = around.x + stream.next_int(16) as i32 - 8;
            let z = around.z + stream.next_int(16) as i32 - 8;
            let Some(y) = self.height_of(Heightmap::WorldSurface, x, z) else {
                continue;
            };
            let pos = BlockPos::new(x, y, z);
            if !self.close_to_village(pos, 1) {
                continue;
            }
            if rules::check_spawn_rules(kind, &self.spawn_world(), pos, stream) {
                return Some(bottom_centre(pos));
            }
        }
        None
    }

    /// `trySpawn(level)`: where a place is found about `around`, a zombie constructed,
    /// finalised for `EVENT` where it was made (the origin), and placed there facing a
    /// `nextFloat() · 360` drawn after.
    fn siege_zombie(&mut self, around: BlockPos, stream: &mut Random, output: &mut TickOutput) {
        let Some(at) = self.siege_spawn_pos(around, stream) else {
            return;
        };
        let kind = entity_types::ZOMBIE as u16;
        let origin = BlockPos::new(0, 0, 0);
        let effective = self.difficulty_at(origin);
        let difficulty = (effective, special_multiplier(effective));
        let mut group: Option<SpawnGroup> = None;
        let mut prepare = |region: &mut Region, mob: &mut Mob, own: &mut Random| {
            region.finalize_made(
                kind,
                mob,
                (&mut *stream, own),
                origin,
                SpawnReason::Event,
                difficulty,
                &mut group,
            );
            stream.next_float() * 360.0
        };
        self.make_mob_facing(kind, at, &mut prepare, output);
    }

    // -----------------------------------------------------------------------------
    // The wandering trader.

    /// The trader spawner's chance (`WanderingTraderData.spawnChance`, 25 to 75): the
    /// world's, kept by the store with the firing that left it and published with the
    /// clock (`docs/groundwork/n10-plan.md`, decision 12 and section 9.6); 25 before any
    /// firing.
    pub(crate) fn trader_chance(&self) -> i32 {
        self.custom_spawners
            .trader_chance
            .map_or(MIN_TRADER_CHANCE, |(_, chance)| chance)
    }

    /// The store's word of the chance: taken unless the region's own is of a later
    /// firing, which the store has not published yet. Of one firing the lower stands, as
    /// in the store: only the region that made the trader set it back to 25.
    pub(crate) fn take_trader_chance(&mut self, firing: u64, chance: i32) {
        let stands = match self.custom_spawners.trader_chance {
            Some(kept) if kept.0 > firing || (kept.0 == firing && kept.1 <= chance) => kept,
            _ => (firing, chance),
        };
        self.custom_spawners.trader_chance = Some(stands);
    }

    /// The trader spawner's chance set at its firing due at `due`: kept, and told the
    /// store for its record.
    fn set_trader_chance(&mut self, due: u64, chance: i32, output: &mut TickOutput) {
        self.custom_spawners.trader_chance = Some((due, chance));
        output.trader_chance = Some((due, chance));
    }

    /// `WanderingTraderSpawner.tick`'s firing (n7 WanderingTraderSpawner 1–200), its own
    /// random the firing's stream: the chance raised by 25 (to 75 at most), `nextInt(100)`
    /// above the old chance ends it; a trader made, or no player in the world, sets the
    /// chance back to 25.
    pub(crate) fn trader_fired(&mut self, due: u64, mut stream: Random, output: &mut TickOutput) {
        let old = self.trader_chance();
        let raised = (old + 25).clamp(MIN_TRADER_CHANCE, MAX_TRADER_CHANCE);
        self.set_trader_chance(due, raised, output);
        if stream.next_int(100) as i32 > old {
            return;
        }
        if self.spawn_trader(&mut stream, output) {
            self.set_trader_chance(due, MIN_TRADER_CHANCE, output);
        }
    }

    /// `WanderingTraderSpawner.spawn(level)`: true with no player in the world; a player
    /// of the world's table, and only their region goes on (another region, not knowing
    /// how it went, keeps its raised chance); one firing in ten; a meeting point within
    /// 48 of them (the first found) or their block; a spot within 48 of that on the
    /// trader's surface where it may stand, with room for it, not in a biome without
    /// traders; there the trader (`EVENT`), two llamas within 4 of it leashed to it, its
    /// despawn delay 48 000, the spot its wander target and its home within 16.
    fn spawn_trader(&mut self, stream: &mut Random, output: &mut TickOutput) -> bool {
        let Some(chosen) = self.random_world_player(stream) else {
            return false;
        };
        let Some(chosen) = chosen else {
            return true;
        };
        let Some((block, _)) = self.own_player_block(chosen) else {
            return false;
        };
        if stream.next_int(10) != 0 {
            return false;
        }
        let centre = self
            .pois_in_range(block, 48, |kind| kind == PoiType::Meeting, Occupancy::Any)
            .first()
            .map_or(block, |record| record.pos);
        let Some(spot) = self.trader_spawn_position_near(centre, 48, stream) else {
            return false;
        };
        if !self.has_enough_space(spot) {
            return false;
        }
        if animal::biome_in_tag(
            self.zoomed_biome_at(spot),
            "minecraft:without_wandering_trader_spawns",
        ) {
            return false;
        }
        let trader_kind = entity_types::WANDERING_TRADER as u16;
        let Some(trader) = self.spawn_for_event(trader_kind, spot, stream, output, |mob| {
            wandering_trader::update(mob, |own| {
                own.despawn_delay = TRADER_STAYS;
                own.wander_target = Some(spot);
            });
            mob.body.home = Home {
                centre: Some(spot),
                radius: TRADER_HOME_RADIUS,
            };
        }) else {
            return false;
        };
        for _ in 0..2 {
            self.spawn_trader_llama(trader, spot, stream, output);
        }
        true
    }

    /// `tryToSpawnLlamaFor(level, trader, 4)`: a spot within 4 of the trader's block, a
    /// trader llama there (`EVENT`), leashed to the trader where the region holds both.
    fn spawn_trader_llama(
        &mut self,
        trader: EntityId,
        spot: BlockPos,
        stream: &mut Random,
        output: &mut TickOutput,
    ) {
        let Some(at) = self.trader_spawn_position_near(spot, 4, stream) else {
            return;
        };
        let kind = entity_types::TRADER_LLAMA as u16;
        let Some(llama) = self.spawn_for_event(kind, at, stream, output, |_| {}) else {
            return;
        };
        if self.find_entity(llama).is_some() && self.find_entity(trader).is_some() {
            self.set_leashed_to(llama, LeashHolder::Entity(trader), trader, output);
        }
    }

    /// `findSpawnPositionNear(level, pos, r)`: up to ten tries of `nextInt(2r) − r` along
    /// x and z, on the trader's heightmap, the first where the trader's placement holds.
    fn trader_spawn_position_near(
        &self,
        centre: BlockPos,
        r: i32,
        stream: &mut Random,
    ) -> Option<BlockPos> {
        let kind = entity_types::WANDERING_TRADER as u16;
        let (_, heightmap) = spawning::placement(kind);
        let world = self.spawn_world();
        for _ in 0..10 {
            let x = centre.x + stream.next_int((r * 2) as u32) as i32 - r;
            let z = centre.z + stream.next_int((r * 2) as u32) as i32 - r;
            let Some(y) = self.height_of(heightmap, x, z) else {
                continue;
            };
            let pos = BlockPos::new(x, y, z);
            if rules::is_spawn_position_ok(kind, &world, pos) {
                return Some(pos);
            }
        }
        None
    }

    /// `hasEnoughSpace(level, pos)`: every block from `pos` to `pos` + (1, 2, 1) without a
    /// collision shape (and read).
    fn has_enough_space(&self, pos: BlockPos) -> bool {
        (0..=1).all(|dx| {
            (0..=2).all(|dy| {
                (0..=1).all(|dz| {
                    self.block(pos.offset(dx, dy, dz))
                        .is_some_and(|state| state.voxel_collision_shape().is_empty())
                })
            })
        })
    }

    /// `EntityType.spawn(level, pos, EVENT)` (n7 EntityType 182–219): at the middle of
    /// the block's bottom, turned by the level's `nextFloat() · 360` (here the firing's
    /// stream), its type's `finalizeSpawn` for `EVENT` with the level's draws from
    /// `stream`, `made` done to it, added (or sent where its chunk is another region's),
    /// and its ambient sound played. Its key.
    fn spawn_for_event(
        &mut self,
        kind: u16,
        pos: BlockPos,
        stream: &mut Random,
        output: &mut TickOutput,
        made: impl FnOnce(&mut Mob),
    ) -> Option<EntityId> {
        let yaw = maths::wrap_degrees(stream.next_float() * 360.0);
        let effective = self.difficulty_at(pos);
        let difficulty = (effective, special_multiplier(effective));
        let mut group: Option<SpawnGroup> = None;
        let mut made = Some(made);
        let mut sound: Sound = None;
        let mut shown: Option<Mob> = None;
        let mut prepare = |region: &mut Region, mob: &mut Mob, own: &mut Random| {
            region.finalize_made(
                kind,
                mob,
                (&mut *stream, own),
                pos,
                SpawnReason::Event,
                difficulty,
                &mut group,
            );
            if let Some(made) = made.take() {
                made(mob);
            }
            sound = region.ambient_of_made(kind, mob, own);
            shown = Some(mob.clone());
        };
        let at = bottom_centre(pos);
        let key = self.make_mob(kind, at, (yaw, 0.0), &mut prepare, output)?;
        if let Some(mob) = shown {
            self.play_made(kind, &mob, at, sound, output);
        }
        Some(key)
    }
}
